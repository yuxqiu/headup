//! Test support shared across modules: run the real posture session against any tracker and
//! record what it did.

use crate::core::cues::{CueSink, Phrase};
use crate::core::detector::Cue;
use crate::core::record::Recorder;
use crate::core::session;
use crate::core::tracker::HeadTracker;
use anyhow::Result;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// Records each prompt and cue with when it started (since the session began). A prompt takes
/// `prompt` to speak.
struct FakeCues {
    start: Instant,
    prompt: Duration,
    log: RefCell<Vec<(String, Duration)>>,
}

impl FakeCues {
    fn record(&self, name: String) {
        let at = self.start.elapsed();
        self.log.borrow_mut().push((name, at));
    }
}

impl CueSink for FakeCues {
    fn play(&self, cue: Cue) {
        self.record(format!("{cue:?}"));
    }
    async fn say(&self, p: Phrase) -> Result<()> {
        self.record(format!("say {p:?}"));
        tokio::time::sleep(self.prompt).await;
        Ok(())
    }
}

/// What a session did.
pub struct Outcome {
    /// Prompts ("say SitUpright") and cues ("Tone"), with when each started (since the session
    /// began).
    pub cues: Vec<(String, Duration)>,
    /// The `"event"` names in the session log, in order.
    pub events: Vec<String>,
}

impl Outcome {
    pub fn names(&self) -> Vec<&str> {
        self.cues.iter().map(|(name, _)| name.as_str()).collect()
    }

    pub fn count(&self, name: &str) -> usize {
        self.names().iter().filter(|n| **n == name).count()
    }

    /// When `name` first started.
    pub fn first(&self, name: &str) -> Duration {
        self.cues
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no {name} in {:?}", self.names()))
            .1
    }
}

/// Run the real session on `tracker` (on tokio's paused clock) until the tracker fails with
/// `"script finished"`, and report what it did. Each spoken prompt takes `prompt`.
pub async fn run_session(tracker: &mut impl HeadTracker, prompt: Duration) -> Outcome {
    static RUN: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "headup-session-{}-{}.jsonl",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let mut rec = Recorder::new(Some(std::fs::File::create(&path).unwrap()));
    let cues = FakeCues {
        start: Instant::now(),
        prompt,
        log: RefCell::default(),
    };
    let Err(err) = session::run(tracker, &cues, &mut rec).await;
    assert_eq!(err.to_string(), "script finished");
    let log = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let events = log
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter_map(|v| v["event"].as_str().map(String::from))
        .collect();
    Outcome {
        cues: cues.log.into_inner(),
        events,
    }
}
