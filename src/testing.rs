//! Test support shared across modules: run the real posture session against any tracker and
//! record what it did.

use crate::core::record::Recorder;
use crate::core::session;
use crate::core::sound::Cue;
use crate::core::sound::{Player, Sound};
use crate::core::tracker::HeadTracker;
use anyhow::{Result, ensure};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// Records each prompt and cue with when it started, measured from the session start. Speaking a
/// prompt takes `prompt`. The reminder `failing`, if any, cannot be played.
struct FakePlayer {
    start: Instant,
    prompt: Duration,
    failing: Option<Cue>,
    log: RefCell<Vec<(Sound, Duration)>>,
}

impl FakePlayer {
    fn record(&self, sound: Sound) {
        let at = self.start.elapsed();
        self.log.borrow_mut().push((sound, at));
    }
}

impl Player for FakePlayer {
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

/// A test tracker's script or recording is over: the error that ends [`run_session`].
#[derive(Debug)]
pub struct ScriptFinished;

impl std::fmt::Display for ScriptFinished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "script finished")
    }
}

impl std::error::Error for ScriptFinished {}

/// What a session did.
pub struct SessionRun {
    /// Prompts and reminders, with when each started (since the session began).
    pub sounds: Vec<(Sound, Duration)>,
    /// The `"event"` names in the session log, in order.
    pub events: Vec<String>,
    /// The uprights re-anchored to (relative to calibration), in order.
    pub uprights: Vec<f64>,
}

impl SessionRun {
    /// What played, in order.
    pub fn played(&self) -> Vec<Sound> {
        self.sounds.iter().map(|&(p, _)| p).collect()
    }

    pub fn count(&self, played: Sound) -> usize {
        self.sounds.iter().filter(|(p, _)| *p == played).count()
    }

    /// When `played` first started.
    pub fn first(&self, played: Sound) -> Duration {
        self.sounds
            .iter()
            .find(|(p, _)| *p == played)
            .unwrap_or_else(|| panic!("no {played:?} in {:?}", self.played()))
            .1
    }

    /// When each `played` started.
    pub fn times(&self, played: Sound) -> Vec<Duration> {
        self.sounds
            .iter()
            .filter(|(p, _)| *p == played)
            .map(|&(_, t)| t)
            .collect()
    }

    /// The reminder cues, with when each started.
    pub fn reminders(&self) -> Vec<(Cue, Duration)> {
        self.sounds
            .iter()
            .filter_map(|&(p, t)| match p {
                Sound::Reminder(cue) => Some((cue, t)),
                Sound::Prompt(_) => None,
            })
            .collect()
    }
}

/// How the fake player behaves.
#[derive(Default)]
pub struct PlayerSim {
    /// Each spoken prompt takes this long.
    pub prompt: Duration,
    /// This reminder cannot be played.
    pub failing: Option<Cue>,
}

/// Run the real session on `tracker` (on tokio's paused clock) until the tracker fails with
/// [`ScriptFinished`], and report what it did, with a fake player that behaves as `sim` says.
pub async fn run_session(tracker: &mut impl HeadTracker, sim: PlayerSim) -> SessionRun {
    let log = SharedBuf::default();
    let mut rec = Recorder::new(Some(Box::new(log.clone())));
    let player = FakePlayer {
        start: Instant::now(),
        prompt: sim.prompt,
        failing: sim.failing,
        log: RefCell::default(),
    };
    let Err(err) = session::run(tracker, &player, &mut rec).await;
    assert!(err.downcast_ref::<ScriptFinished>().is_some(), "{err:#}");
    let lines: Vec<serde_json::Value> = log
        .text()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let events = lines
        .iter()
        .filter_map(|v| v["event"].as_str().map(String::from))
        .collect();
    let uprights = lines
        .iter()
        .filter(|v| v["event"] == "Reanchored")
        .map(|v| v["upright"].as_f64().expect("a re-anchor logs its upright"))
        .collect();
    SessionRun {
        sounds: player.log.into_inner(),
        events,
        uprights,
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
