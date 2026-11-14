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
    /// A link to earbuds just connected at `now`. It starts by asking for the wear state and
    /// switching the pose stream on.
    pub fn new(adapter: Address, now: Instant) -> Self {
        let mut out = Outputs::default();
        for f in [wear_query(), capability_query(), stream_switch(true)] {
            out.send(f);
        }
        Link {
            decoder: Decoder::default(),
            adapter,
            earbuds: Earbuds::default(),
            frame: PoseFrame::new(now),
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
    fn on_reading(&mut self, now: Instant, seq: u32, raw: Quat) {
        self.earbuds.on_reading(raw);
        if let Some(Availability::Unavailable { .. }) = self.earbuds.availability() {
            return;
        }
        for out in self.frame.push(now, seq, raw) {
            match out {
                FrameOut::Pose(pose) => self.out.event(pose),
                FrameOut::HoldStarted { seq } => {
                    self.out.log(LogRecord::Hold(Hold { hold: seq }));
                    self.earbuds.on_hold_started();
                }
            }
        }
    }

    /// Tell the session what changed. When tracking stops, the pose frame ends (see
    /// [`PoseFrame`]).
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
/// the stretch's first reading and ended when tracking stops or the earbuds start a new stream.
///
/// While tracking is unavailable the earbuds keep tracking and re-centring unseen (they also hold
/// while the user walks), so their reference may end up anywhere. A frame kept across that would
/// rotate the axes later movements are measured about (a reference off by ~180° reads looking down
/// as looking up), which recalibrating does not undo. So each stretch gets a new frame, and the
/// session recalibrates in it.
struct PoseFrame {
    /// Time zero for the rebuilder, which works in durations since it.
    epoch: Instant,
    rebuilder: Option<Rebuilder>,
}

/// What a reading gives, in the link's terms.
enum FrameOut {
    /// An `Event::Pose`.
    Pose(Event),
    /// See [`Rebuilt::HoldStarted`].
    HoldStarted { seq: u32 },
}

impl PoseFrame {
    fn new(epoch: Instant) -> Self {
        PoseFrame {
            epoch,
            rebuilder: None,
        }
    }

    /// Rebuild a reading received at `now`, starting a frame if there is none.
    fn push(&mut self, now: Instant, seq: u32, raw: Quat) -> Vec<FrameOut> {
        let t = now
            .checked_duration_since(self.epoch)
            .expect("time went backwards past the link's start");
        let rebuilt = self
            .rebuilder
            .get_or_insert_with(Rebuilder::default)
            .push(t, seq, raw);
        rebuilt
            .into_iter()
            .map(|out| match out {
                Rebuilt::Pose { at, orientation } => FrameOut::Pose(Event::Pose {
                    at: self.epoch + at,
                    orientation,
                }),
                Rebuilt::HoldStarted { seq } => FrameOut::HoldStarted { seq },
            })
            .collect()
    }

    /// End the frame: the next reading starts a new one.
    fn end(&mut self) {
        self.rebuilder = None;
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
pub(super) const NOT_STREAMING: &str =
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
            self.send(Request::Restart)
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
        resend.then(|| self.send(Request::SwitchOn))
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

    fn send(&mut self, request: Request) -> Request {
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
mod tests {
    use super::super::protocol::sample_frame;
    use super::*;
    use crate::core::quat::Quat;
    use std::time::Duration;

    fn pc() -> Address {
        "AA:BB:CC:DD:EE:02".parse().unwrap()
    }

    fn phone() -> Address {
        "12:34:56:78:9A:BC".parse().unwrap()
    }

    fn link(now: Instant) -> Link {
        let mut link = Link::new(pc(), now);
        link.take_outbox();
        link
    }

    fn wear(left: u8, right: u8) -> Vec<u8> {
        Frame::new(43, 37, &[(1, &[left]), (2, &[right])]).encode()
    }

    /// A 43/54 push for `address` with audio state byte `state`.
    fn audio(address: Address, state: u8) -> Vec<u8> {
        let mut v = address.0;
        v.reverse();
        Frame::new(43, 54, &[(5, &[&v[..], &[state]].concat())]).encode()
    }

    /// A 43/88 reply with result `v`.
    fn reply(v: u8) -> Vec<u8> {
        Frame::new(43, 88, &[(2, &[v])]).encode()
    }

    fn events(link: &mut Link) -> Vec<Event> {
        std::iter::from_fn(|| link.next_event()).collect()
    }

    /// A head turned about 50° to the left, as a raw reading.
    fn turned() -> Quat {
        Quat {
            w: 0.9,
            x: 0.0,
            y: 0.0,
            z: 0.19_f64.sqrt(),
        }
    }

    /// Feed reading `raw` with sequence number `seq`, and return the events other than poses.
    fn reading(link: &mut Link, seq: u32, raw: Quat) -> Vec<Event> {
        let now = Instant::now();
        link.on_bytes(now, &sample_frame(seq, raw).encode())
            .unwrap();
        events(link)
            .into_iter()
            .filter(|e| !matches!(e, Event::Pose { .. }))
            .collect()
    }

    #[test]
    fn starts_by_querying_and_switching_the_stream_on() {
        let mut link = Link::new(pc(), Instant::now());
        let sent = link.take_outbox();
        assert_eq!(
            sent,
            [wear_query(), capability_query(), stream_switch(true)]
        );
        assert_eq!(link.take_outbox(), []);
        link.stop();
        assert_eq!(link.take_outbox(), [stream_switch(false)]);
    }

    // Putting the earbuds back on starts a new orientation frame and switches the stream on
    // again. The first wear report and repeated reports send nothing.
    #[test]
    fn rewearing_restarts_the_stream() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &wear(1, 1)).unwrap();
        link.on_bytes(now, &wear(1, 1)).unwrap();
        assert_eq!(events(&mut link), [Event::Worn(true)]);
        assert_eq!(link.take_outbox(), []);
        link.on_bytes(now, &wear(0, 1)).unwrap();
        assert_eq!(events(&mut link), [Event::Worn(false)]);
        link.on_bytes(now, &wear(1, 1)).unwrap();
        assert_eq!(events(&mut link), [Event::Worn(true)]);
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
    }

    // When this computer starts streaming, switch the stream on again.
    #[test]
    fn this_computer_starting_audio_rearms_the_stream() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &audio(phone(), 0x09)).unwrap();
        assert_eq!(events(&mut link), []);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), [Event::Available]);
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), []);
        link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
        assert_eq!(
            events(&mut link),
            [Event::Unavailable {
                reason: NOT_STREAMING
            }]
        );
        assert_eq!(link.take_outbox(), []);
    }

    // headup connected while the phone was the earbuds' current device. The 43/88 is refused with
    // `02`, and tracking is unavailable. Once this computer streams, the stream is switched on
    // again (data/live2_raw.jsonl: `2=02`, 43/54 `…09`, re-sent 43/88 `2=00`, then samples).
    #[test]
    fn refused_stream_switch_waits_for_this_computer_to_stream() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &reply(2)).unwrap();
        assert_eq!(
            events(&mut link),
            [Event::Unavailable {
                reason: NOT_STREAMING
            }]
        );
        link.on_bytes(now, &reply(2)).unwrap();
        assert_eq!(events(&mut link), []);
        assert_eq!(link.take_outbox(), []);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), [Event::Available]);
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
        link.on_bytes(now, &reply(0)).unwrap();
        assert_eq!(events(&mut link), []);
    }

    // An accepted switch survives an audio gap. When this computer streams again, the earbuds
    // resume the stream by themselves (data/grounding_test.log, "audio_gap": samples 0.3 s after
    // 43/54 `…09` with no 43/88). Switching on again would restart the stream, and each new stream
    // adds a re-centre, so the switch is not sent.
    #[test]
    fn audio_resuming_after_an_accepted_switch_keeps_the_stream() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &reply(0)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(
            events(&mut link),
            [
                Event::Available,
                Event::Unavailable {
                    reason: NOT_STREAMING
                },
                Event::Available
            ]
        );
        assert_eq!(link.take_outbox(), []);
    }

    // Until the earbuds accept a switch, this computer starting to stream switches the stream on
    // again (the reply may not have arrived yet).
    #[test]
    fn audio_starting_before_any_reply_switches_on_again() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
    }

    // Another device (the phone) started streaming after the switch was accepted. The switch does
    // not survive the earbuds changing their current device (data/grounding_test3.log
    // "phone_takeover"), so it is sent again when this computer streams again.
    #[test]
    fn switch_is_resent_after_another_device_streamed() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &reply(0)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
        link.on_bytes(now, &audio(phone(), 0x09)).unwrap();
        assert_eq!(link.take_outbox(), []);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
    }

    // The switch is refused while this computer streams, because the earbud-to-earbud link was
    // down. Putting the earbuds back on sends the switch again, so tracking is available again.
    #[test]
    fn rewearing_after_a_refusal_makes_tracking_available_again() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &wear(1, 1)).unwrap();
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        link.on_bytes(now, &reply(1)).unwrap();
        assert_eq!(
            events(&mut link),
            [
                Event::Worn(true),
                Event::Available,
                Event::Unavailable {
                    reason: NOT_STREAMING
                }
            ]
        );
        link.take_outbox();
        link.on_bytes(now, &wear(0, 1)).unwrap();
        link.on_bytes(now, &wear(1, 1)).unwrap();
        assert_eq!(
            events(&mut link),
            [Event::Worn(false), Event::Worn(true), Event::Available]
        );
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
        link.on_bytes(now, &reply(0)).unwrap();
        assert_eq!(events(&mut link), []);
    }

    // While the user walks around, the earbuds hold: the reading jumps to identity with no glide
    // and stays there while their reference follows the head. Head tracking is unavailable until
    // the reading moves again (FINDINGS "Instant resets while walking").
    #[test]
    fn a_hold_makes_tracking_unavailable_until_the_reading_moves() {
        let now = Instant::now();
        let mut link = link(now);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), [Event::Available]);
        assert_eq!(reading(&mut link, 1, turned()), []);
        let walking = Event::Unavailable { reason: WALKING };
        assert_eq!(reading(&mut link, 2, Quat::IDENTITY), [walking]);
        assert_eq!(reading(&mut link, 3, Quat::IDENTITY), []);
        assert_eq!(reading(&mut link, 400, turned()), [Event::Available]);
    }

    // The earbuds send no 43/54 on connect while audio already plays (13 of 22 raw logs in
    // data/). Readings arrive only at the current device, so the first one makes tracking
    // available, and a hold's end makes it available again.
    #[test]
    fn readings_show_this_computer_streams_when_no_audio_push_came() {
        let mut link = link(Instant::now());
        assert_eq!(reading(&mut link, 1, turned()), [Event::Available]);
        let walking = Event::Unavailable { reason: WALKING };
        assert_eq!(reading(&mut link, 2, Quat::IDENTITY), [walking]);
        assert_eq!(reading(&mut link, 400, turned()), [Event::Available]);
    }

    // While tracking was unavailable the earbuds kept re-centring where headup could not see it, so
    // when it resumes a new orientation frame starts: the first reading afterwards is the frame's
    // origin, whatever frame came before. Here the frame before had moved its reference to a turned
    // pose (a new stream starts from the last pose).
    #[test]
    fn tracking_resuming_starts_a_new_frame() {
        let now = Instant::now();
        let mut link = link(now);
        let orientations = |link: &mut Link, seq, raw| -> Vec<Quat> {
            link.on_bytes(now, &sample_frame(seq, raw).encode())
                .unwrap();
            events(link)
                .into_iter()
                .filter_map(|e| match e {
                    Event::Pose { orientation, .. } => Some(orientation),
                    _ => None,
                })
                .collect()
        };
        orientations(&mut link, 1, turned());
        orientations(&mut link, 2, turned());
        orientations(&mut link, 1, Quat::IDENTITY); // a new stream, from the turned pose
        link.on_bytes(now, &audio(pc(), 0x03)).unwrap(); // audio paused: unavailable
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap(); // playing again: available
        orientations(&mut link, 50, Quat::IDENTITY);
        let after = orientations(&mut link, 51, Quat::IDENTITY);
        assert_eq!(
            after.first().map(|q| q.angle_to(Quat::IDENTITY) < 1e-9),
            Some(true)
        );
    }

    #[test]
    fn malformed_frames_are_errors() {
        let now = Instant::now();
        let mut link = link(now);
        let short = Frame::new(43, 54, &[(5, &[1, 2])]).encode();
        assert!(link.on_bytes(now, &short).is_err());
    }

    // Poses carry the time their sample was read, not when they are handed out.
    #[test]
    fn poses_carry_their_read_time() {
        let start = Instant::now();
        let mut link = link(start);
        let at = |i: u32| start + Duration::from_millis(20 * u64::from(i));
        for seq in 1..=80 {
            let raw = Quat::IDENTITY;
            link.on_bytes(at(seq), &sample_frame(seq, raw).encode())
                .unwrap();
        }
        let Some(Event::Pose { at: first, .. }) = link.next_event() else {
            panic!("no pose");
        };
        assert_eq!(first, at(1));
    }
}
