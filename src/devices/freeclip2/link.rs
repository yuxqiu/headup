//! The FreeClip 2 protocol without I/O: bytes and the time come in; events for the session, frames
//! to send and `--raw-log` records go out. [`super::FreeClip2`] only moves bytes between this and the socket, so
//! every protocol decision here can be tested without earbuds.

use super::mbb::{Decoder, Frame};
use super::protocol::{
    Message, RawFrame, RawSample, SwitchReply, capability_query, stream_switch, wear_query,
};
use super::rebuild::{Rebuilder, Rebuilt};
use crate::core::quat::Quat;
use crate::core::tracker::Event;
use crate::devices::bluetooth::Address;
use anyhow::Result;
use serde::Serialize;
use std::collections::VecDeque;
use tokio::time::Instant;

pub struct Link {
    decoder: Decoder,
    /// This computer's adapter address, to pick its entry out of 43/54 pushes.
    adapter: Address,
    earbuds: Earbuds,
    frame: PoseFrame,
    /// The earbuds' view as last reported to the session.
    reported: View,
    out: Outputs,
}

impl Link {
    /// A link to earbuds just connected. It starts by asking for the wear state and switching the
    /// pose stream on.
    pub fn new(adapter: Address) -> Self {
        let mut out = Outputs::default();
        for f in [wear_query(), capability_query(), stream_switch(true)] {
            out.send(f);
        }
        Link {
            decoder: Decoder::default(),
            adapter,
            earbuds: Earbuds::default(),
            frame: PoseFrame(None),
            reported: View::default(),
            out,
        }
    }

    /// Bytes read from the earbuds at `now`. Desync, CRC errors and malformed frames are errors.
    pub fn on_bytes(&mut self, now: Instant, data: &[u8]) -> Result<()> {
        self.decoder.push(data);
        while let Some(f) = self.decoder.next_frame()? {
            self.on_frame(now, &f)?;
        }
        Ok(())
    }

    /// The next event for the session, in order.
    pub fn next_event(&mut self) -> Option<Event> {
        self.out.events.pop_front()
    }

    /// Frames waiting to be sent, oldest first.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.out.outbox)
    }

    /// `--raw-log` records waiting to be written, oldest first.
    pub fn take_log(&mut self) -> Vec<LogRecord> {
        std::mem::take(&mut self.out.log)
    }

    /// Switch the pose stream off, leaving the earbuds as they were before headup.
    pub fn stop(&mut self) {
        self.out.send(stream_switch(false));
    }

    /// Log the frame, update the earbuds' facts with it (a reading also goes into the pose
    /// frame), send what the facts ask for, then tell the session what changed.
    fn on_frame(&mut self, now: Instant, f: &Frame) -> Result<()> {
        let msg = Message::parse(f);
        self.out.log(match msg {
            Ok(Message::Sample { seq, raw }) => LogRecord::Sample(RawSample::new(seq, raw)),
            _ => LogRecord::Frame(RawFrame::from(f)), // even if malformed, for debugging
        });
        let request = match msg? {
            Message::Wear { both_worn } => self.earbuds.on_wear(both_worn),
            Message::DeviceAudio { address, streaming } => {
                self.earbuds.on_audio(address == self.adapter, streaming)
            }
            Message::StreamSwitch(reply) => self.earbuds.on_reply(reply),
            Message::Sample { seq, raw } => {
                self.on_reading(now, seq, raw);
                None
            }
            Message::Other => None,
        };
        match request {
            Some(Request::SwitchOn) => self.out.send(stream_switch(true)),
            Some(Request::Restart) => {
                self.frame.end();
                self.out.send(stream_switch(true));
            }
            None => {}
        }
        self.report();
        Ok(())
    }

    /// A 43/89 reading: a fact about the earbuds, and, while tracking is available, a pose in the
    /// current frame. While it is unavailable, readings say nothing usable about the head (held
    /// identity, or trailing data the session ignores) and are not rebuilt.
    ///
    /// The reading that ends a hold is the new frame's first; its pose goes out just before the
    /// `Available` it causes (reported at the end of the frame). The session ignores poses while
    /// tracking is unavailable, so that pose only goes unused.
    fn on_reading(&mut self, now: Instant, seq: u32, raw: Quat) {
        self.earbuds.on_reading(raw);
        if let Some(Availability::Unavailable { .. }) = self.earbuds.availability() {
            return;
        }
        for out in self.frame.push(now, seq, raw) {
            match out {
                Rebuilt::Pose { at, orientation } => {
                    self.out.event(Event::Pose { at, orientation })
                }
                Rebuilt::HoldStarted { seq } => {
                    self.out.log(LogRecord::Hold(Hold { hold: seq }));
                    self.earbuds.on_hold_started();
                }
            }
        }
    }

    /// Tell the session what changed. When tracking stops, the pose frame ends; since readings are
    /// not rebuilt while it is unavailable, the next stretch starts a new one (see [`PoseFrame`]).
    fn report(&mut self) {
        for event in self.reported.update_to(self.earbuds.view()) {
            if let Event::Unavailable { .. } = event {
                self.frame.end();
            }
            self.out.event(event);
        }
    }
}

/// The orientation frame of the current stretch of available tracking: a [`Rebuilder`], created at
/// the stretch's first reading and ended when tracking stops or the earbuds start a new stream
/// (re-wearing). Readings are not rebuilt while tracking is unavailable, so no frame lives across
/// an interruption.
///
/// While tracking is unavailable the earbuds keep tracking and re-centring unseen (they also hold
/// while the user walks), so their reference may end up anywhere. A frame kept across that would
/// rotate the axes later movements are measured about (a reference off by ~180° reads looking down
/// as looking up), which recalibrating does not undo. So each stretch gets a new frame, and the
/// session recalibrates in it.
struct PoseFrame(Option<Rebuilder>);

impl PoseFrame {
    /// Rebuild a reading received at `now`, starting a frame if there is none.
    fn push(&mut self, now: Instant, seq: u32, raw: Quat) -> Vec<Rebuilt> {
        self.0
            .get_or_insert_with(Rebuilder::default)
            .push(now, seq, raw)
    }

    /// End the frame: the next reading starts a new one.
    fn end(&mut self) {
        self.0 = None;
    }
}

/// What the link hands out, each in order: events for the session, frames to send to the
/// earbuds, and `--raw-log` records.
#[derive(Default)]
struct Outputs {
    events: VecDeque<Event>,
    outbox: Vec<Frame>,
    log: Vec<LogRecord>,
}

impl Outputs {
    fn event(&mut self, event: Event) {
        self.events.push_back(event);
    }

    fn send(&mut self, frame: Frame) {
        self.outbox.push(frame);
    }

    fn log(&mut self, record: LogRecord) {
        self.log.push(record);
    }
}

/// A `--raw-log` record. Each kind serialises as its own fields.
#[derive(Serialize)]
#[serde(untagged)]
pub enum LogRecord {
    Sample(RawSample),
    Frame(RawFrame),
    Hold(Hold),
}

/// `--raw-log` record of the start of a hold (see [`super::rebuild`]). It reads `{"hold":1234}`,
/// where 1234 is the sequence number of the identity sample that started it.
#[derive(Serialize)]
pub struct Hold {
    hold: u32,
}

/// Returns `current` if it is known and differs from `reported`, and stores it in `reported`.
/// Returns `None` otherwise.
fn changed<T: Copy + PartialEq>(reported: &mut Option<T>, current: Option<T>) -> Option<T> {
    let current = current?;
    (reported.replace(current) != Some(current)).then_some(current)
}

/// What the session sees of the earbuds. Each field is `None` while unknown.
#[derive(Clone, Copy, Default)]
struct View {
    /// Both earbuds are in the ears.
    worn: Option<bool>,
    /// Whether poses that follow the head can flow to this computer.
    availability: Option<Availability>,
}

impl View {
    /// Update this view (the one last reported) to `current`, and return the events that report
    /// the change: one for each part of `current` that is known and differs.
    fn update_to(&mut self, current: View) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(worn) = changed(&mut self.worn, current.worn) {
            events.push(Event::Worn(worn));
        }
        if let Some(availability) = changed(&mut self.availability, current.availability) {
            events.push(match availability {
                Availability::Available => Event::Available,
                Availability::Unavailable { reason } => Event::Unavailable { reason },
            });
        }
        events
    }
}

/// Whether head tracking flows, as reported to the session.
#[derive(Clone, Copy, PartialEq)]
enum Availability {
    Available,
    Unavailable { reason: &'static str },
}

/// Why head tracking is unavailable, for the console: this computer is not the earbuds' current
/// device, or the switch was refused.
const NOT_STREAMING: &str =
    "play audio on this computer; FreeClip 2 only tracks your head while it is playing";
/// Why head tracking is unavailable, for the console: the earbuds hold (see [`Earbuds::holding`]).
const WALKING: &str = "FreeClip 2 does not track your head while you walk around; \
    it resumes when you move your head after you stop";

/// What the earbuds' state asks the link to send.
#[derive(Debug, PartialEq)]
enum Request {
    /// Switch the pose stream on.
    SwitchOn,
    /// A new pose stream: start a fresh orientation frame (drop the [`Rebuilder`], the stream's
    /// state) and switch the stream on.
    Restart,
}

/// What the earbuds have said about themselves, and what became of the last "stream on" sent.
/// Each message updates it and may ask the link to send a frame ([`Request`]). The link always
/// sends what is asked, so a switch recorded here as sent really was sent.
#[derive(Default)]
struct Earbuds {
    /// 43/37: both earbuds are in the ears (`None` until reported).
    worn: Option<bool>,
    /// 43/54: this computer streams audio to the earbuds, which makes it their current device
    /// (`None` until known). A 43/89 reading also tells it, if no 43/54 has (see
    /// [`Earbuds::on_reading`]).
    streaming: Option<bool>,
    switch: Switch,
    /// 43/89: the earbuds hold. While the user walks around, their activity detector locks the
    /// reference to the head on every sample, so the readings stay at identity, the stream pauses,
    /// and it resumes relative to wherever the head is then (FINDINGS "Instant resets while
    /// walking"). The readings do not follow the head meanwhile. The rebuilder recognises the start
    /// ([`Rebuilt::HoldStarted`]): telling it from the end of a glide takes the glide model.
    holding: bool,
}

/// What became of the last "stream on" (43/88 TLV 1 = 1).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Switch {
    /// The switch is not known to stand. Either it was sent and has no answer yet, or the earbuds
    /// accepted it and then cancelled it.
    ///
    /// Two things cancel an accepted switch:
    /// - Taking an earbud off. After re-wearing, no samples come until a new 43/88
    ///   (data/grounding_test.log "rewear").
    /// - Another device becoming current. After this computer streams again, no samples come until
    ///   a new 43/88 (data/grounding_test3.log "phone_takeover").
    ///
    /// One rule covers both: send the switch again when this computer starts streaming.
    ///
    /// The same rule covers a switch with no answer yet whose refusal is still in transit. The
    /// earbuds refuse a switch sent while another device is current. The 43/54 push saying this
    /// computer streams can arrive before that refusal.
    ///
    /// The earbuds answer every well-formed 43/88 (firmware `0x7039c018`; 13 of 13 live, within
    /// 80 ms), so no retry timer is needed.
    #[default]
    Unconfirmed,
    /// Accepted, and nothing has cancelled it since. It survives audio gaps. When this computer
    /// streams again, the earbuds resume the stream by themselves (FINDINGS "Grounding of headup's
    /// device logic"), so the switch is not sent again.
    Accepted,
    /// Refused (43/88 reply 1 or 2). The stream stays off, so silence does not mean the head is
    /// still.
    Refused,
}

impl Earbuds {
    /// 43/37. Taking an earbud off cancels the switch. Putting the earbuds back on needs a fresh
    /// orientation frame and a new switch, and starts a new stream, so no hold carries over.
    fn on_wear(&mut self, worn: bool) -> Option<Request> {
        let was = self.worn.replace(worn);
        if !worn {
            self.cancel_switch();
            return None;
        }
        (was == Some(false)).then(|| {
            self.holding = false;
            self.request_switch(Request::Restart)
        })
    }

    /// 43/89: a hold started (see [`Earbuds::holding`]).
    fn on_hold_started(&mut self) {
        self.holding = true;
    }

    /// 43/89. The earbuds send readings only to their current device, so a reading also tells
    /// that this computer streams, if no 43/54 has said so yet (the earbuds send none on connect
    /// while audio is already playing). A reading that is not identity follows the head again, so
    /// it ends a hold.
    fn on_reading(&mut self, raw: Quat) {
        self.streaming.get_or_insert(true);
        if raw != Quat::IDENTITY {
            self.holding = false;
        }
    }

    /// 43/54 for one device.
    ///
    /// When another device streams, an accepted switch counts as cancelled.
    ///
    /// When this computer streams, whether the switch is sent again depends on its state:
    /// - Accepted: not sent. Sending it would restart the stream, and every new stream adds a
    ///   re-centre.
    /// - Unconfirmed: sent when this computer starts streaming (see [`Switch::Unconfirmed`]), not
    ///   on every push.
    /// - Refused: sent on every push that says this computer streams.
    fn on_audio(&mut self, this_computer: bool, streaming: bool) -> Option<Request> {
        if !this_computer {
            if streaming {
                self.cancel_switch();
            }
            return None;
        }
        let started = streaming && self.streaming != Some(true);
        self.streaming = Some(streaming);
        let resend = match self.switch {
            Switch::Accepted => false,
            Switch::Refused => streaming,
            Switch::Unconfirmed => started,
        };
        resend.then(|| self.request_switch(Request::SwitchOn))
    }

    /// 43/88 reply. A refusal is undone by [`Earbuds::on_audio`] when this computer streams, or by
    /// [`Earbuds::on_wear`] when the earbuds go back on.
    fn on_reply(&mut self, reply: SwitchReply) -> Option<Request> {
        self.switch = match reply {
            SwitchReply::Accepted => Switch::Accepted,
            SwitchReply::PeerLinkDown | SwitchReply::NotCurrentDevice => Switch::Refused,
        };
        None
    }

    fn view(&self) -> View {
        View {
            worn: self.worn,
            availability: self.availability(),
        }
    }

    /// Poses flow to this computer while it streams, unless the switch was refused. They follow
    /// the head unless the earbuds hold. `None` while it is unknown whether this computer streams.
    fn availability(&self) -> Option<Availability> {
        let flowing = match self.switch {
            Switch::Refused => false,
            Switch::Unconfirmed | Switch::Accepted => self.streaming?,
        };
        Some(if !flowing {
            Availability::Unavailable {
                reason: NOT_STREAMING,
            }
        } else if self.holding {
            Availability::Unavailable { reason: WALKING }
        } else {
            Availability::Available
        })
    }

    /// Ask the link to send the stream switch (`request`): the switch is unconfirmed until the
    /// earbuds answer.
    fn request_switch(&mut self, request: Request) -> Request {
        self.switch = Switch::Unconfirmed;
        request
    }

    /// The earbuds cancelled an accepted switch.
    fn cancel_switch(&mut self) {
        if self.switch == Switch::Accepted {
            self.switch = Switch::Unconfirmed;
        }
    }
}

#[cfg(test)]
#[path = "tests/link.rs"]
mod tests;
