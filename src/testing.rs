//! Test support shared across modules: run the real posture session against any tracker and
//! record what it did.

use crate::core::cues::{CueSink, Phrase};
use crate::core::detector::Cue;
use crate::core::record::Recorder;
use crate::core::session;
use crate::core::tracker::HeadTracker;
use anyhow::Result;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// Something the session played: a spoken prompt, or a reminder cue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Played {
    Prompt(Phrase),
    Reminder(Cue),
}

/// Records each prompt and cue with when it started, measured from the session start. Speaking a
/// prompt takes `prompt`.
struct FakeCues {
    start: Instant,
    prompt: Duration,
    log: RefCell<Vec<(Played, Duration)>>,
}

impl FakeCues {
    fn record(&self, played: Played) {
        let at = self.start.elapsed();
        self.log.borrow_mut().push((played, at));
    }
}

impl CueSink for FakeCues {
    fn play(&self, cue: Cue) {
        self.record(Played::Reminder(cue));
    }
    async fn say(&self, p: Phrase) -> Result<()> {
        self.record(Played::Prompt(p));
        tokio::time::sleep(self.prompt).await;
        Ok(())
    }
}

/// What a session did.
pub struct Outcome {
    /// Prompts and cues, with when each started (since the session began).
    pub cues: Vec<(Played, Duration)>,
    /// The `"event"` names in the session log, in order.
    pub events: Vec<String>,
}

impl Outcome {
    /// What played, in order.
    pub fn played(&self) -> Vec<Played> {
        self.cues.iter().map(|&(p, _)| p).collect()
    }

    pub fn count(&self, played: Played) -> usize {
        self.cues.iter().filter(|(p, _)| *p == played).count()
    }

    /// When `played` first started.
    pub fn first(&self, played: Played) -> Duration {
        self.cues
            .iter()
            .find(|(p, _)| *p == played)
            .unwrap_or_else(|| panic!("no {played:?} in {:?}", self.played()))
            .1
    }

    /// When each `played` started.
    pub fn times(&self, played: Played) -> Vec<Duration> {
        self.cues
            .iter()
            .filter(|(p, _)| *p == played)
            .map(|&(_, t)| t)
            .collect()
    }

    /// The reminder cues, with when each started.
    pub fn reminders(&self) -> Vec<(Cue, Duration)> {
        self.cues
            .iter()
            .filter_map(|&(p, t)| match p {
                Played::Reminder(cue) => Some((cue, t)),
                Played::Prompt(_) => None,
            })
            .collect()
    }
}

/// Run the real session on `tracker` (on tokio's paused clock) until the tracker fails with
/// `"script finished"`, and report what it did. Each spoken prompt takes `prompt`.
pub async fn run_session(tracker: &mut impl HeadTracker, prompt: Duration) -> Outcome {
    let log = SharedBuf::default();
    let mut rec = Recorder::new(Some(Box::new(log.clone())));
    let cues = FakeCues {
        start: Instant::now(),
        prompt,
        log: RefCell::default(),
    };
    let Err(err) = session::run(tracker, &cues, &mut rec).await;
    assert_eq!(err.to_string(), "script finished");
    let events = log
        .text()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter_map(|v| v["event"].as_str().map(String::from))
        .collect();
    Outcome {
        cues: cues.log.into_inner(),
        events,
    }
}

/// An in-memory log that can be written through one clone and read through another.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    /// Everything written so far.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for SharedBuf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Wait as a tracker does for an event due at `due`: until it is due, or until `timeout` has passed
/// if that comes first. Returns whether it is due.
pub async fn wait_until_due(due: Instant, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    if due > deadline {
        tokio::time::sleep_until(deadline).await;
        return false;
    }
    tokio::time::sleep_until(due).await;
    true
}
