//! The FreeClip 2 protocol without I/O: bytes and the time come in, events for the session and
//! frames to send go out. [`super::FreeClip2`] only moves bytes between this and the socket, so
//! every protocol decision here can be tested without earbuds.

use super::mbb::{Decoder, Frame};
use super::protocol::{
    Message, RawFrame, RawSample, SwitchReply, capability_query, stream_switch, wear_query,
};
use super::rebuild::Rebuilder;
use crate::core::quat::Quat;
use crate::core::record::DeviceLog;
use crate::core::tracker::Event;
use anyhow::Result;
use bluer::Address;
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
    log: DeviceLog,
    /// Events for the session, in order.
    events: VecDeque<Event>,
    /// Frames to send to the earbuds.
    outbox: Vec<Frame>,
}

impl Link {
    /// A link to earbuds just connected at `now`. It starts by asking for the wear state and
    /// switching the pose stream on.
    pub fn new(adapter: Address, log: DeviceLog, now: Instant) -> Self {
        Link {
            decoder: Decoder::default(),
            rebuild: Rebuilder::default(),
            epoch: now,
            adapter,
            earbuds: Earbuds::default(),
            told: View::default(),
            log,
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

    /// Nothing arrived until `now`: release the held poses the silence allows.
    pub fn on_timer(&mut self, now: Instant) {
        let released = self.rebuild.release(self.since_epoch(now));
        self.queue_poses(released);
    }

    /// When to call [`Link::on_timer`] if nothing arrives first. Calling it then always releases
    /// what was due: times are exact (integer nanoseconds) all the way through.
    pub fn next_wake(&self) -> Option<Instant> {
        Some(self.epoch + self.rebuild.next_release()?)
    }

    /// The next event for the session, in order.
    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Frames waiting to be sent, oldest first.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// Switch the pose stream off, leaving the earbuds as they were before headup.
    pub fn stop(&mut self) {
        self.outbox.push(stream_switch(false));
    }

    /// Log the frame, let the earbuds' state take it in, do what that asks, then tell the session
    /// what changed.
    fn on_frame(&mut self, now: Instant, f: &Frame) -> Result<()> {
        let msg = Message::parse(f);
        match msg {
            Ok(Message::Sample { seq, raw }) => self.log.record(&RawSample::new(seq, raw))?,
            _ => self.log.record(&RawFrame::from(f))?, // logged even if malformed, for debugging
        }
        let request = match msg? {
            Message::Wear { both_worn } => self.earbuds.on_wear(both_worn),
            Message::DeviceAudio { address, streaming } => {
                self.earbuds.on_audio(address == self.adapter, streaming)
            }
            Message::StreamSwitch(reply) => self.earbuds.on_reply(reply),
            Message::Sample { seq, raw } => {
                let released = self.rebuild.push(self.since_epoch(now), seq, raw);
                self.queue_poses(released);
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

    fn queue_poses(&mut self, released: Vec<(Duration, Quat)>) {
        for (t, orientation) in released {
            let at = self.epoch + t;
            self.events.push_back(Event::Pose { at, orientation });
        }
    }

    fn since_epoch(&self, now: Instant) -> Duration {
        now.checked_duration_since(self.epoch)
            .expect("time went backwards past the link's start")
    }
}

/// `now` if it is known and differs from what `told` holds, which then becomes `now`.
fn changed(told: &mut Option<bool>, now: Option<bool>) -> Option<bool> {
    let now = now?;
    (told.replace(now) != Some(now)).then_some(now)
}

/// What the session sees of the earbuds; `None` while unknown.
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
/// Each message updates it and may ask for a frame to be sent ([`Request`]); a request it makes
/// is sent, so the switch it records as sent always was.
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
    /// Not known to stand: sent and not answered yet, or accepted and then cancelled by the earbuds.
    /// Taking an earbud off cancels it (no samples after re-wearing until a new 43/88;
    /// data/grounding_test.log "rewear"), and so does another device becoming current (no samples
    /// after this computer streams again until a new 43/88; data/grounding_test3.log
    /// "phone_takeover"). Both cases get the same rule: send again when this computer starts streaming. For an
    /// unanswered switch that covers a refusal still in transit: one sent while another device was
    /// current is refused, and the 43/54 push saying this computer streams can arrive before that
    /// refusal does. The earbuds answer every well-formed 43/88 (firmware `0x7039c018`; 13 of 13
    /// live, within 80 ms), so no retry timer is needed.
    #[default]
    Unconfirmed,
    /// Accepted, and nothing has cancelled it since. It survives audio gaps: the earbuds resume
    /// the stream by themselves when this computer streams again (FINDINGS "Grounding of headup's
    /// device logic"), so it is not sent again then.
    Accepted,
    /// Refused (43/88 reply 1 or 2): the stream stays off, so silence no longer means a still head.
    Refused,
}

impl Earbuds {
    /// 43/37. Taking an earbud off cancels the switch; putting the earbuds back on needs a fresh
    /// orientation frame and the switch again.
    fn on_wear(&mut self, worn: bool) -> Option<Request> {
        let was = self.worn.replace(worn);
        if !worn {
            self.cancel_switch();
            return None;
        }
        (was == Some(false)).then(|| self.send(Request::Restart))
    }

    /// 43/54 for one device. Another device streaming may cancel the switch. For this computer:
    /// switch on again unless the switch stands. An accepted switch is not re-sent: it would restart
    /// the stream, and every new stream adds a re-centre. An unconfirmed one is re-sent when
    /// streaming starts (see [`Switch::Unconfirmed`]), not on every push; a refused one on any push
    /// that says this computer streams.
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

    fn pc() -> Address {
        "D4:94:A9:22:86:6F".parse().unwrap()
    }

    fn phone() -> Address {
        "EC:81:50:6E:86:B5".parse().unwrap()
    }

    fn link(now: Instant) -> Link {
        let mut link = Link::new(pc(), DeviceLog::new(None), now);
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
        let mut link = Link::new(pc(), DeviceLog::new(None), Instant::now());
        let sent = link.take_outbox();
        assert_eq!(
            sent,
            [wear_query(), capability_query(), stream_switch(true)]
        );
        assert_eq!(link.take_outbox(), []);
        link.stop();
        assert_eq!(link.take_outbox(), [stream_switch(false)]);
    }

    // Putting the earbuds back on starts a new orientation frame and needs the stream switched
    // on again; the first report and repeated reports change nothing.
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

    // headup connected while the phone was the earbuds' current device: the 43/88 is refused with
    // `02`, tracking is unavailable, and the stream is switched on again once this computer
    // streams (data/live2_raw.jsonl: `2=02`, 43/54 `…09`, re-sent 43/88 `2=00`, then samples).
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

    // An accepted switch survives an audio gap: the earbuds resume the stream by themselves when this
    // computer streams again (data/grounding_test.log, "audio_gap": samples 0.3 s after 43/54 `…09`
    // with no 43/88). Switching on again would restart the stream, and each new stream adds a
    // re-centre, so it is not sent.
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

    // Another device (the phone) started streaming after the switch was accepted: whether the switch
    // survives the earbuds changing their current device is not observed, so it is sent again when
    // this computer streams again.
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

    // Refused while this computer streams (the earbud-to-earbud link was down): putting the earbuds
    // back on sends the switch again, so tracking is available again.
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

    // The timer fired exactly when `next_wake` asked: it must release something. When the
    // rebuilder kept time in f64 seconds, converting to an Instant and back could land a
    // nanosecond early; then nothing was released, `next_wake` stayed the same, and a caller
    // looping on it spun forever.
    #[test]
    fn timer_at_next_wake_makes_progress() {
        let start = Instant::now();
        for k in 0..200u32 {
            let mut link = link(start);
            // Arbitrary offsets and a 35 Hz spacing, so due times land on many different values.
            let base = Duration::from_micros(13_700) * k;
            for seq in 1..=3u32 {
                let at = start + base + Duration::from_nanos(28_571_429) * seq;
                let raw = Quat::IDENTITY;
                link.on_bytes(at, &sample_frame(seq, raw).encode()).unwrap();
            }
            let wake = link.next_wake().expect("samples are held");
            link.on_timer(wake);
            assert_ne!(
                link.next_wake(),
                Some(wake),
                "timing {k}: no progress at the wake time"
            );
        }
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
