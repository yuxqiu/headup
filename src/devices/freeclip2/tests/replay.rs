//! Replays recorded 43/89 samples (tests/fixtures/freeclip2/*.csv) through the real protocol
//! ([`Link`]), as the socket would deliver them; and, through [`ReplayTracker`], through the real
//! posture session.

use crate::core::quat::Quat;
use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use crate::devices::freeclip2::FreeClip2;
use crate::devices::freeclip2::link::Link;
use crate::devices::freeclip2::protocol::sample_frame;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

/// One recorded 43/89 sample; `phase` is the guided-test step ("" if the recording has none).
pub struct Row {
    pub phase: String,
    pub t: f64,
    pub seq: u32,
    pub raw: Quat,
}

pub fn read_csv(name: &str) -> Vec<Row> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        phase: String,
        t: f64,
        seq: u32,
        w: f64,
        x: f64,
        y: f64,
        z: f64,
    }
    let path = format!(
        "{}/tests/fixtures/freeclip2/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    csv::Reader::from_path(&path)
        .unwrap()
        .deserialize::<Line>()
        .map(|line| {
            let l = line.unwrap_or_else(|e| panic!("{path}: {e}"));
            Row {
                phase: l.phase,
                t: l.t,
                seq: l.seq,
                raw: Quat {
                    w: l.w,
                    x: l.x,
                    y: l.y,
                    z: l.z,
                },
            }
        })
        .collect()
}

/// Feeds a recording to a [`Link`] as the socket would: each sample as a 43/89 frame at its
/// recorded time, and the link's timer whenever that falls due first. Times are seconds since
/// `start`.
pub struct Replay {
    link: Link,
    start: Instant,
    rows: VecDeque<(f64, Vec<u8>)>,
    now: f64,
}

impl Replay {
    pub fn new(rows: &[Row], start: Instant) -> Self {
        let adapter = "00:00:00:00:00:01".parse().unwrap();
        let mut link = Link::new(adapter, DeviceLog::new(None), start);
        link.take_outbox();
        let rows = rows
            .iter()
            .map(|r| (r.t, sample_frame(r.seq, r.raw).encode()))
            .collect();
        Replay {
            link,
            start,
            rows,
            now: 0.0,
        }
    }

    /// The next event and when it is handed out; `None` once the recording and everything the
    /// link held are out.
    pub fn next(&mut self) -> Option<(f64, Event)> {
        loop {
            if let Some(event) = self.link.next_event() {
                return Some((self.now, event));
            }
            let wake = self.link.next_wake().map(|w| self.seconds(w));
            let row = self.rows.front().map(|&(t, _)| t);
            let feed_row = match (row, wake) {
                (Some(t), Some(w)) => t <= w,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => return None,
            };
            if feed_row {
                let (t, bytes) = self.rows.pop_front().unwrap();
                self.now = t;
                self.link.on_bytes(self.instant(t), &bytes).unwrap();
            } else {
                let w = wake.unwrap();
                self.now = w;
                self.link.on_timer(self.instant(w));
            }
        }
    }

    fn instant(&self, t: f64) -> Instant {
        self.start + Duration::from_secs_f64(t)
    }

    fn seconds(&self, at: Instant) -> f64 {
        at.duration_since(self.start).as_secs_f64()
    }
}

/// A released pose, measured at `t` (seconds since the recording began).
pub struct Out {
    pub t: f64,
    pub abs: Quat,
}

/// Every pose the protocol hands out for a recording.
pub fn replay(rows: &[Row]) -> Vec<Out> {
    let start = Instant::now();
    let mut replay = Replay::new(rows, start);
    std::iter::from_fn(|| replay.next())
        .map(|(_, event)| match event {
            Event::Pose { at, orientation } => Out {
                t: at.duration_since(start).as_secs_f64(),
                abs: orientation,
            },
            other => panic!("a recording of samples produced {other:?}"),
        })
        .collect()
}

/// A [`HeadTracker`] that replays a recording through the protocol on tokio's paused clock, for
/// running the real session on it. Time zero is when it is created; it fails with "script
/// finished" when the recording is over.
pub struct ReplayTracker {
    replay: Replay,
    start: Instant,
    pending: Option<(f64, Event)>,
}

impl ReplayTracker {
    pub fn new(rows: &[Row]) -> Self {
        let start = Instant::now();
        ReplayTracker {
            replay: Replay::new(rows, start),
            start,
            pending: None,
        }
    }
}

impl HeadTracker for ReplayTracker {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        if self.pending.is_none() {
            self.pending = self.replay.next();
        }
        let Some((t, _)) = self.pending else {
            bail!("script finished");
        };
        let when = self.start + Duration::from_secs_f64(t);
        if when > deadline {
            tokio::time::sleep_until(deadline).await;
            return Ok(None);
        }
        tokio::time::sleep_until(when).await;
        Ok(self.pending.take().map(|(_, event)| event))
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn traits(&self) -> TrackerTraits {
        FreeClip2::TRAITS
    }
}

/// The guided phases of a recording, in order, with their start times.
pub fn phases(rows: &[Row]) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = Vec::new();
    for r in rows {
        if v.last().is_none_or(|p| p.0 != r.phase) {
            v.push((r.phase.clone(), r.t));
        }
    }
    v
}

/// Index into `phases` of the phase a sample measured at `t` belongs to.
pub fn phase_at(phases: &[(String, f64)], t: f64) -> usize {
    phases.partition_point(|p| p.1 <= t).saturating_sub(1)
}
