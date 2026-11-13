//! Replays recorded 43/89 samples (tests/fixtures/freeclip2/*.csv) through the real protocol
//! ([`Link`]), as the socket would deliver them. [`ReplayTracker`] also runs them through the real
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

/// One recorded 43/89 sample, received at `t` since the recording began. `phase` is the
/// guided-test step, or "" if the recording has none.
pub struct Row {
    pub phase: String,
    pub t: Duration,
    pub seq: u32,
    pub raw: Quat,
}

/// `x` seconds, for recorded and hand-written test times.
pub fn secs(x: f64) -> Duration {
    Duration::from_secs_f64(x)
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
                t: secs(l.t), // the recording's float seconds, converted once
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
/// recorded time. All times are exact `Instant`s.
pub struct Replay {
    link: Link,
    rows: VecDeque<(Instant, Vec<u8>)>,
    now: Instant,
}

impl Replay {
    /// Replay `rows`, with the recording's time zero at `start`.
    pub fn new(rows: &[Row], start: Instant) -> Self {
        let adapter = "00:00:00:00:00:01".parse().unwrap();
        let mut link = Link::new(adapter, DeviceLog::new(None), start);
        link.take_outbox();
        let rows = rows
            .iter()
            .map(|r| (start + r.t, sample_frame(r.seq, r.raw).encode()))
            .collect();
        Replay {
            link,
            rows,
            now: start,
        }
    }

    /// The next event and when it is handed out. `None` once the recording is used up. Samples
    /// held for a glide that has not ended stay held.
    pub fn next(&mut self) -> Option<(Instant, Event)> {
        loop {
            if let Some(event) = self.link.next_event() {
                return Some((self.now, event));
            }
            let (at, bytes) = self.rows.pop_front()?;
            self.now = at;
            self.link.on_bytes(at, &bytes).unwrap();
        }
    }
}

/// A released pose, received at `t` since the recording began.
pub struct Out {
    pub t: Duration,
    pub abs: Quat,
}

/// Every pose the protocol hands out for a recording.
pub fn replay(rows: &[Row]) -> Vec<Out> {
    let start = Instant::now();
    let mut replay = Replay::new(rows, start);
    std::iter::from_fn(|| replay.next())
        .map(|(_, event)| match event {
            Event::Pose { at, orientation } => Out {
                t: at - start,
                abs: orientation,
            },
            other => panic!("a recording of samples produced {other:?}"),
        })
        .collect()
}

/// A [`HeadTracker`] that replays a recording through the protocol on tokio's paused clock, so the
/// real session can run on it. Time zero is when it is created. It fails with "script finished"
/// when the recording is over.
pub struct ReplayTracker {
    replay: Replay,
    pending: Option<(Instant, Event)>,
}

impl ReplayTracker {
    pub fn new(rows: &[Row]) -> Self {
        ReplayTracker {
            replay: Replay::new(rows, Instant::now()),
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
        let Some((when, _)) = self.pending else {
            bail!("script finished");
        };
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
pub fn phases(rows: &[Row]) -> Vec<(String, Duration)> {
    let mut v: Vec<(String, Duration)> = Vec::new();
    for r in rows {
        if v.last().is_none_or(|p| p.0 != r.phase) {
            v.push((r.phase.clone(), r.t));
        }
    }
    v
}

/// Index into `phases` of the phase a sample measured at `t` belongs to.
pub fn phase_at(phases: &[(String, Duration)], t: Duration) -> usize {
    phases.partition_point(|p| p.1 <= t).saturating_sub(1)
}
