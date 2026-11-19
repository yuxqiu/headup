//! The FreeClip 2 protocol without I/O: bytes and the time come in; events for the session, frames
//! to send and `--raw-log` records go out. [`super::FreeClip2`] only moves bytes between this and
//! the socket, so every protocol decision here can be tested without earbuds.

use super::earbuds::{Availability, Earbuds, SwitchOn};
use super::mbb::{Decoder, Frame};
use super::protocol::{
    Audio, Message, RawFrame, RawSample, capability_query, device_list_query, stream_switch,
    wear_query,
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
    /// The availability last reported to the session; `None` before the first report.
    reported: Option<Availability>,
    out: Outputs,
}

impl Link {
    /// A link to earbuds just connected. It starts by asking for the wear state, sending the
    /// spatial-audio capability query (see [`capability_query`]) and asking for the device list,
    /// which says whether this computer streams; the pose stream is switched on once it does.
    pub fn new(adapter: Address) -> Self {
        let mut out = Outputs::default();
        for f in [wear_query(), capability_query(), device_list_query()] {
            out.outbox.push(f);
        }
        Link {
            decoder: Decoder::default(),
            adapter,
            earbuds: Earbuds::default(),
            frame: PoseFrame(None),
            reported: None,
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
        self.out.outbox.push(stream_switch(false));
    }

    /// Log the frame, update the earbuds' facts with it (a reading also goes into the pose
    /// frame), send the stream switch if the facts now call for it, then tell the session what
    /// changed.
    fn on_frame(&mut self, now: Instant, f: &Frame) -> Result<()> {
        let msg = Message::parse(f);
        self.out.log.push(match msg {
            Ok(Message::Sample { seq, raw }) => LogRecord::Sample(RawSample::new(seq, raw)),
            _ => LogRecord::Frame(RawFrame::from(f)), // even if malformed, for debugging
        });
        match msg? {
            Message::Wear { both_worn } => self.earbuds.on_wear(both_worn),
            Message::DeviceAudio { address, audio } if address == self.adapter => self
                .earbuds
                .on_audio(audio == Audio::Current { streaming: true }),
            Message::DeviceAudio {
                audio: Audio::Current { .. },
                ..
            } => self.earbuds.stream_lost(),
            Message::DeviceAudio {
                audio: Audio::Quiet,
                ..
            } => {}
            Message::StreamSwitch(reply) => self.earbuds.on_reply(reply),
            Message::Sample { seq, raw } => self.on_reading(now, seq, raw),
            Message::Other => {}
        }
        if let Some(SwitchOn) = self.earbuds.ask() {
            self.out.outbox.push(stream_switch(true));
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
        if let Some(Availability::Unavailable(_)) = self.earbuds.availability() {
            return;
        }
        for out in self.frame.push(now, seq, raw) {
            match out {
                Rebuilt::Pose { at, orientation } => {
                    self.out.events.push_back(Event::Pose { at, orientation })
                }
                Rebuilt::HoldStarted { seq } => {
                    self.out.log.push(LogRecord::Hold(Hold { hold: seq }));
                    self.earbuds.on_hold_started();
                }
            }
        }
    }

    /// Tell the session if the availability is known and changed. When tracking stops, the pose
    /// frame ends; since readings are not rebuilt while it is unavailable, the next stretch starts a
    /// new one (see [`PoseFrame`]).
    fn report(&mut self) {
        let Some(current) = self.earbuds.availability() else {
            return;
        };
        if self.reported.replace(current) == Some(current) {
            return;
        }
        self.out.events.push_back(match current {
            Availability::Available => Event::Available,
            Availability::Unavailable(blocker) => {
                self.frame.end();
                Event::Unavailable {
                    reason: blocker.reason(),
                }
            }
        });
    }
}

/// The orientation frame of the current stretch of available tracking: a [`Rebuilder`], created at
/// the stretch's first reading and ended when tracking stops (taking an earbud off included, so a
/// stream restarted after re-wearing starts a new frame too). Readings are not rebuilt while
/// tracking is unavailable, so no frame lives across an interruption.
///
/// While tracking is unavailable the earbuds keep tracking and re-centring unseen (they also hold
/// while they sense movement), so their reference may end up anywhere. A frame kept across that
/// would rotate the axes later movements are measured about (a reference off by ~180° reads looking
/// down as looking up), which recalibrating does not undo. So each stretch gets a new frame, and the
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

#[cfg(test)]
#[path = "tests/link.rs"]
mod tests;
