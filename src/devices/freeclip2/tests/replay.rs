//! Replays recorded 43/89 samples (tests/fixtures/freeclip2/*.csv) through the real protocol:
//! [`Replay`] feeds them to a [`Link`] directly, and [`ReplayTracker`] writes them to the real
//! backend's socket, so the real posture session can run on them.

use crate::core::quat::Quat;
use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use crate::devices::freeclip2::ConnectionClosed;
use crate::devices::freeclip2::FreeClip2;
use crate::devices::freeclip2::link::Link;
use crate::devices::freeclip2::protocol::sample_frame;
use crate::testing::{ScriptFinished, secs};
use anyhow::Result;
use serde::Deserialize;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

/// One recorded 43/89 sample, received at `t` since the recording began. `phase` is the
/// guided-test step, or "" if the recording has none.
pub struct Row {
    pub phase: String,
    pub t: Duration,
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

/// The recording's samples as 43/89 frames, each with when it arrives (the recording's time zero at
/// `start`).
fn frames(rows: &[Row], start: Instant) -> impl Iterator<Item = (Instant, Vec<u8>)> + '_ {
    rows.iter()
        .map(move |r| (start + r.t, sample_frame(r.seq, r.raw).encode()))
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
        let mut link = Link::new(adapter);
        link.take_outbox();
        Replay {
            link,
            rows: frames(rows, start).collect(),
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
            self.link.take_log();
        }
    }
}

/// A released pose, received at `t` since the recording began.
pub struct Out {
    pub t: Duration,
    pub abs: Quat,
}

/// Every pose the protocol hands out for a recording. Availability changes are skipped.
pub fn replay(rows: &[Row]) -> Vec<Out> {
    let start = Instant::now();
    let mut replay = Replay::new(rows, start);
    std::iter::from_fn(|| replay.next())
        .filter_map(|(_, event)| match event {
            Event::Pose { at, orientation } => Some(Out {
                t: at - start,
                abs: orientation,
            }),
            Event::Available | Event::Unavailable { .. } => None,
        })
        .collect()
}

/// A [`HeadTracker`] that replays a recording through the real backend on tokio's paused clock, so
/// the real session can run on it: [`FreeClip2`] reads an in-memory socket, and a task writes each
/// sample to it as a 43/89 frame at its recorded time. Time zero is when it is created. When the
/// recording is over, the task closes the socket; once everything before that has been delivered,
/// the tracker fails with [`ScriptFinished`].
pub struct ReplayTracker(FreeClip2);

impl ReplayTracker {
    pub fn new(rows: &[Row]) -> Self {
        let frames: Vec<_> = frames(rows, Instant::now()).collect();
        let (ours, mut earbuds) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            for (at, bytes) in frames {
                tokio::time::sleep_until(at).await;
                earbuds.write_all(&bytes).await.unwrap();
            }
            // Dropping `earbuds` closes the socket: the backend reports the end of the recording.
        });
        let adapter = "00:00:00:00:00:01".parse().unwrap();
        ReplayTracker(FreeClip2::start(ours, adapter, DeviceLog::new(None)))
    }
}

impl HeadTracker for ReplayTracker {
    async fn next_event(&mut self) -> Result<Event> {
        self.0.next_event().await.map_err(|e| {
            if e.downcast_ref::<ConnectionClosed>().is_some() {
                ScriptFinished.into()
            } else {
                e
            }
        })
    }

    async fn stop(&mut self) -> Result<()> {
        self.0.stop().await
    }

    fn traits(&self) -> TrackerTraits {
        self.0.traits()
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
