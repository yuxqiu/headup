//! Test support shared across modules: run the real posture session against any tracker and
//! record what it did.

use crate::core::cues::{CueSink, Sound};
use crate::core::detector::Cue;
use crate::core::record::Recorder;
use crate::core::session;
use crate::core::tracker::HeadTracker;
use anyhow::{Result, ensure};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// Records each prompt and cue with when it started, measured from the session start. Speaking a
/// prompt takes `prompt`. The reminder `failing`, if any, cannot be played.
struct FakeCues {
    start: Instant,
    prompt: Duration,
    failing: Option<Cue>,
    log: RefCell<Vec<(Sound, Duration)>>,
}

impl FakeCues {
    fn record(&self, sound: Sound) {
        let at = self.start.elapsed();
        self.log.borrow_mut().push((sound, at));
    }
}

impl CueSink for FakeCues {
    fn play(&self, sound: Sound) -> impl Future<Output = Result<()>> + 'static {
        self.record(sound);
        // A prompt takes `prompt` from now; a reminder is done at once.
        let done = Instant::now()
            + match sound {
                Sound::Prompt(_) => self.prompt,
                Sound::Reminder(_) => Duration::ZERO,
            };
        let fails = matches!(sound, Sound::Reminder(cue) if Some(cue) == self.failing);
        async move {
            tokio::time::sleep_until(done).await;
            ensure!(!fails, "the sound server did not answer");
            Ok(())
        }
    }
}

/// `x` seconds, rounded to the microsecond, for test times written by hand or read from a
/// recording. Recordings store times to 0.1 ms, so they stay exact.
pub fn secs(x: f64) -> Duration {
    Duration::from_micros((x * 1e6).round() as u64)
}

/// What a session did.
pub struct SessionRun {
    /// Prompts and cues, with when each started (since the session began).
    pub cues: Vec<(Sound, Duration)>,
    /// The `"event"` names in the session log, in order.
    pub events: Vec<String>,
}

impl SessionRun {
    /// What played, in order.
    pub fn played(&self) -> Vec<Sound> {
        self.cues.iter().map(|&(p, _)| p).collect()
    }

    pub fn count(&self, played: Sound) -> usize {
        self.cues.iter().filter(|(p, _)| *p == played).count()
    }

    /// When `played` first started.
    pub fn first(&self, played: Sound) -> Duration {
        self.cues
            .iter()
            .find(|(p, _)| *p == played)
            .unwrap_or_else(|| panic!("no {played:?} in {:?}", self.played()))
            .1
    }

    /// When each `played` started.
    pub fn times(&self, played: Sound) -> Vec<Duration> {
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
                Sound::Reminder(cue) => Some((cue, t)),
                Sound::Prompt(_) => None,
            })
            .collect()
    }
}

/// Run the real session on `tracker` (on tokio's paused clock) until the tracker fails with
/// `"script finished"`, and report what it did. Each spoken prompt takes `prompt`.
pub async fn run_session(tracker: &mut impl HeadTracker, prompt: Duration) -> SessionRun {
    run_session_failing(tracker, prompt, None).await
}

/// [`run_session`], where the reminder `failing` cannot be played.
pub async fn run_session_failing(
    tracker: &mut impl HeadTracker,
    prompt: Duration,
    failing: Option<Cue>,
) -> SessionRun {
    let log = SharedBuf::default();
    let mut rec = Recorder::new(Some(Box::new(log.clone())));
    let cues = FakeCues {
        start: Instant::now(),
        prompt,
        failing,
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
    SessionRun {
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
