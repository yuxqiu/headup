//! The FreeClip 2 protocol without I/O: bytes and the time come in; events for the session, frames
//! to send and `--raw-log` records go out. [`super::FreeClip2`] only moves bytes between this and the socket, so
//! every protocol decision here can be tested without earbuds.

use super::mbb::{Decoder, Frame};
use super::protocol::{
    Message, RawFrame, RawSample, SwitchReply, capability_query, stream_switch, wear_query,
};
use super::rebuild::{Rebuilder, Rebuilt};
use crate::core::tracker::Event;
use crate::devices::bluetooth::Address;
use anyhow::Result;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

pub struct Link {
    decoder: Decoder,
    rebuild: Rebuilder,
    /// Time zero for `rebuild`, which works in durations since it.
    epoch: Instant,
    /// This computer's adapter address, to pick its entry out of 43/54 pushes.
    adapter: Address,
    earbuds: Earbuds,
    /// What the session has been told so far.
    told: View,
    /// `--raw-log` records, oldest first.
    log: Vec<LogRecord>,
    /// Events for the session, in order.
    events: VecDeque<Event>,
    /// Frames to send to the earbuds.
    outbox: Vec<Frame>,
}

impl Link {
    /// A link to earbuds just connected at `now`. It starts by asking for the wear state and
    /// switching the pose stream on.
    pub fn new(adapter: Address, now: Instant) -> Self {
        Link {
            decoder: Decoder::default(),
            rebuild: Rebuilder::default(),
            epoch: now,
            adapter,
            earbuds: Earbuds::default(),
            told: View::default(),
            log: Vec::new(),
            events: VecDeque::new(),
            outbox: vec![wear_query(), capability_query(), stream_switch(true)],
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
        self.events.pop_front()
    }

    /// Frames waiting to be sent, oldest first.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// `--raw-log` records waiting to be written, oldest first.
    pub fn take_log(&mut self) -> Vec<LogRecord> {
        std::mem::take(&mut self.log)
    }

    /// Switch the pose stream off, leaving the earbuds as they were before headup.
    pub fn stop(&mut self) {
        self.outbox.push(stream_switch(false));
    }

    /// Log the frame and update the earbuds' state with it. Send what the state asks for, then
    /// tell the session what changed.
    fn on_frame(&mut self, now: Instant, f: &Frame) -> Result<()> {
        let msg = Message::parse(f);
        match msg {
            Ok(Message::Sample { seq, raw }) => {
                self.log.push(LogRecord::Sample(RawSample::new(seq, raw)))
            }
            _ => self.log.push(LogRecord::Frame(RawFrame::from(f))), // even if malformed, for debugging
        }
        let request = match msg? {
            Message::Wear { both_worn } => self.earbuds.on_wear(both_worn),
            Message::DeviceAudio { address, streaming } => {
                self.earbuds.on_audio(address == self.adapter, streaming)
            }
            Message::StreamSwitch(reply) => self.earbuds.on_reply(reply),
            Message::Sample { seq, raw } => {
                for out in self.rebuild.push(self.since_epoch(now), seq, raw) {
                    match out {
                        Rebuilt::Pose { at, orientation } => {
                            let at = self.epoch + at;
                            self.events.push_back(Event::Pose { at, orientation });
                        }
                        Rebuilt::MissedGlide { seq } => {
                            self.log
                                .push(LogRecord::MissedGlide(MissedGlide { missed_glide: seq }));
                        }
                    }
                }
                None
            }
            Message::Other => None,
        };
        match request {
            Some(Request::SwitchOn) => self.outbox.push(stream_switch(true)),
            Some(Request::Restart) => {
                self.rebuild = Rebuilder::default();
                self.outbox.push(stream_switch(true));
            }
            None => {}
        }
        self.report();
        Ok(())
    }

    /// Tell the session each part of the earbuds' view that changed since it was last told.
    fn report(&mut self) {
        let view = self.earbuds.view();
        if let Some(worn) = changed(&mut self.told.worn, view.worn) {
            self.events.push_back(Event::Worn(worn));
        }
        if let Some(available) = changed(&mut self.told.available, view.available) {
            self.events.push_back(Event::Available(available));
        }
    }

    fn since_epoch(&self, now: Instant) -> Duration {
        now.checked_duration_since(self.epoch)
            .expect("time went backwards past the link's start")
    }
}

/// A `--raw-log` record. Each kind serialises as its own fields.
#[derive(Serialize)]
#[serde(untagged)]
pub enum LogRecord {
    Sample(RawSample),
    Frame(RawFrame),
    MissedGlide(MissedGlide),
}

/// `--raw-log` record of a re-centre that the rebuilder did not recognise as a glide. It reads
/// `{"missed_glide":1234}`, where 1234 is the sequence number of the identity sample that ended
/// the glide.
#[derive(Serialize)]
pub struct MissedGlide {
    missed_glide: u32,
}

/// Returns `now` if it is known and differs from `told`, and stores it in `told`. Returns `None`
/// otherwise.
fn changed(told: &mut Option<bool>, now: Option<bool>) -> Option<bool> {
    let now = now?;
    (told.replace(now) != Some(now)).then_some(now)
}

/// What the session sees of the earbuds. Each field is `None` while unknown.
#[derive(Default)]
struct View {
    /// Both earbuds are in the ears.
    worn: Option<bool>,
    /// Poses can flow to this computer.
    available: Option<bool>,
}

/// What the earbuds' state asks the link to send.
#[derive(Debug, PartialEq)]
enum Request {
    /// Switch the pose stream on.
    SwitchOn,
    /// Start a fresh orientation frame and switch the pose stream on.
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
    /// (`None` until reported).
    streaming: Option<bool>,
    switch: Switch,
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
    /// orientation frame and a new switch.
    fn on_wear(&mut self, worn: bool) -> Option<Request> {
        let was = self.worn.replace(worn);
        if !worn {
            self.cancel_switch();
            return None;
        }
        (was == Some(false)).then(|| self.send(Request::Restart))
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
            // Poses flow to this computer while it streams, unless the switch was refused.
            available: match self.switch {
                Switch::Refused => Some(false),
                Switch::Unconfirmed | Switch::Accepted => self.streaming,
            },
        }
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

    fn pc() -> Address {
        "D4:94:A9:22:86:6F".parse().unwrap()
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
        assert_eq!(events(&mut link), [Event::Available(true)]);
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), []);
        link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
        assert_eq!(events(&mut link), [Event::Available(false)]);
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
        assert_eq!(events(&mut link), [Event::Available(false)]);
        link.on_bytes(now, &reply(2)).unwrap();
        assert_eq!(events(&mut link), []);
        assert_eq!(link.take_outbox(), []);
        link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
        assert_eq!(events(&mut link), [Event::Available(true)]);
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
                Event::Available(true),
                Event::Available(false),
                Event::Available(true)
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
                Event::Available(true),
                Event::Available(false)
            ]
        );
        link.take_outbox();
        link.on_bytes(now, &wear(0, 1)).unwrap();
        link.on_bytes(now, &wear(1, 1)).unwrap();
        assert_eq!(
            events(&mut link),
            [
                Event::Worn(false),
                Event::Worn(true),
                Event::Available(true)
            ]
        );
        assert_eq!(link.take_outbox(), [stream_switch(true)]);
        link.on_bytes(now, &reply(0)).unwrap();
        assert_eq!(events(&mut link), []);
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
