//! The FreeClip 2 protocol without I/O: bytes and the time come in, events for the session and
//! frames to send go out. [`super::FreeClip2`] only moves bytes between this and the socket, so
//! every protocol decision here can be tested without earbuds.

use super::mbb::{Decoder, Frame};
use super::protocol::{Message, RawFrame, RawSample, capability_query, stream_switch, wear_query};
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
    /// Time zero for `rebuild`, which works in seconds.
    epoch: Instant,
    /// This computer's adapter address, to pick its entry out of 43/54 pushes.
    adapter: Address,
    /// The last reported wear and audio state (`None` until first reported).
    worn: Option<bool>,
    streaming: Option<bool>,
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
            worn: None,
            streaming: None,
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
        let released = self.rebuild.release(self.seconds(now));
        self.queue_poses(released);
    }

    /// When to call [`Link::on_timer`] if nothing arrives first.
    pub fn next_wake(&self) -> Option<Instant> {
        let t = self.rebuild.next_release()?;
        Some(self.epoch + Duration::from_secs_f64(t))
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

    fn on_frame(&mut self, now: Instant, f: &Frame) -> Result<()> {
        let msg = Message::parse(f);
        match msg {
            Ok(Message::Sample { seq, raw }) => self.log.record(&RawSample::new(seq, raw))?,
            _ => self.log.record(&RawFrame::from(f))?, // logged even if malformed, for debugging
        }
        match msg? {
            Message::Wear { both_worn } => self.on_wear(both_worn),
            Message::DeviceAudio { address, streaming } => self.on_audio(address, streaming),
            Message::Sample { seq, raw } => {
                let released = self.rebuild.push(self.seconds(now), seq, raw);
                self.queue_poses(released);
            }
            Message::Other => {}
        }
        Ok(())
    }

    fn on_wear(&mut self, worn: bool) {
        let was = self.worn.replace(worn);
        if was == Some(worn) {
            return;
        }
        if worn && was == Some(false) {
            // Back on the ear: start a fresh orientation frame and switch the stream on again.
            self.rebuild = Rebuilder::default();
            self.outbox.push(stream_switch(true));
        }
        self.events.push_back(Event::Worn(worn));
    }

    fn on_audio(&mut self, address: Address, streaming: bool) {
        if address != self.adapter || self.streaming.replace(streaming) == Some(streaming) {
            return;
        }
        if streaming {
            // A 43/88 sent while this computer was idle once left tracking off after audio began
            // (FINDINGS "Stream switch sent while idle"), so switch the stream on again now.
            self.outbox.push(stream_switch(true));
        }
        self.events.push_back(Event::Available(streaming));
    }

    fn queue_poses(&mut self, released: Vec<(f64, Quat)>) {
        for (t, orientation) in released {
            let at = self.epoch + Duration::from_secs_f64(t);
            self.events.push_back(Event::Pose { at, orientation });
        }
    }

    fn seconds(&self, now: Instant) -> f64 {
        now.duration_since(self.epoch).as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::sample_frame;
    use super::*;

    fn pc() -> Address {
        "D4:94:A9:22:86:6F".parse().unwrap()
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

    // The v0.1.2 fix (c7e0bda): when this computer starts streaming, switch the stream on again.
    #[test]
    fn this_computer_starting_audio_rearms_the_stream() {
        let now = Instant::now();
        let mut link = link(now);
        let phone: Address = "EC:81:50:6E:86:B5".parse().unwrap();
        link.on_bytes(now, &audio(phone, 0x09)).unwrap();
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
        let off = first.max(at(1)) - first.min(at(1));
        assert!(
            off < Duration::from_micros(1),
            "first pose stamped {off:?} off its read time"
        );
    }
}
