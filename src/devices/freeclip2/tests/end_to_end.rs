//! Recorded FreeClip 2 sessions replayed through the protocol into the real posture session (and,
//! for re-anchoring, straight into the detector).

use super::replay::{ReplayTracker, phase_at, phases, read_csv, replay};
use crate::core::detector::Detector;
use crate::core::session::detector_config;
use crate::devices::freeclip2::FreeClip2;
use crate::testing::run_session;

// Ground truth (guided.py --slouch): five upright → look-down cycles back to the same screen
// point. The earbuds' re-centres add ~10° towards "looking up" per cycle; without the detector's
// re-anchoring, upright drifts to about -40° and the last look-down never counts as a slouch.
#[test]
fn repeated_slouches_stay_detectable_end_to_end() {
    let rows = read_csv("slouch_guided.csv");
    let ph = phases(&rows);
    let mut det = Detector::new(detector_config(FreeClip2::TRAITS));
    let mut state = vec![(false, 0.0); ph.len()]; // (slouched, last smoothed) per phase
    for o in replay(&rows) {
        det.update(o.t, o.abs.euler().pitch); // baseline = stream start (upright)
        let s = &mut state[phase_at(&ph, o.t)];
        s.0 |= det.slouching();
        s.1 = det.smoothed().unwrap_or(0.0);
    }
    for ((name, _), (slouched, end)) in ph.iter().zip(&state) {
        if name.starts_with("down") {
            assert!(slouched, "{name}: look-down not detected; {state:?}");
        } else {
            assert!(
                end.abs() < 6.0,
                "{name}: upright ends at {end:.1}; {state:?}"
            );
        }
    }
}

/// The cues (not prompts) the real session plays on a recording, each with when it played
/// (seconds since the recording began).
async fn session_cues(name: &str) -> Vec<(String, f64)> {
    let mut tracker = ReplayTracker::new(&read_csv(name));
    let run = run_session(&mut tracker, 0.0).await;
    run.cues
        .into_iter()
        .filter(|(name, _)| !name.starts_with("say "))
        .collect()
}

/// The cue names, without the spoken reminder (its timing depends on exactly how long a hold
/// lasted).
fn kinds(cues: &[(String, f64)]) -> Vec<&str> {
    cues.iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| *name != "Speak")
        .collect()
}

// The real long-hold recording: each 30 s look-down hold must cue, and sitting back up must
// register as recovery (session 1 failed this before glides were suppressed).
#[tokio::test(start_paused = true)]
async fn long_holds_trigger_cues_end_to_end() {
    let rows = read_csv("hold_test.csv");
    let ph = phases(&rows);
    let cues = session_cues("hold_test.csv").await;
    let key: Vec<(&str, &str)> = cues
        .iter()
        .filter(|(name, _)| name != "Speak")
        .map(|(name, t)| (name.as_str(), ph[phase_at(&ph, *t)].0.as_str()))
        .collect();
    let want = [
        ("Tone", "down"),
        ("Recovered", "center"),
        ("Tone", "down"),
        ("Recovered", "center"),
    ];
    assert_eq!(key, want, "cues: {cues:?}");
}

// The stream-restart recording: one slouch, so exactly one tone and one recovery, and the
// recovery only once the user actually sat up (~25.6 s), not at the restart.
#[tokio::test(start_paused = true)]
async fn stream_restart_end_to_end() {
    let cues = session_cues("stream_restart.csv").await;
    assert_eq!(kinds(&cues), ["Tone", "Recovered"], "cues: {cues:?}");
    let recovered_at = cues.iter().find(|(n, _)| n == "Recovered").unwrap().1;
    assert!(
        recovered_at >= 25.6,
        "recovered at {recovered_at:.2}, before sitting up"
    );
}

// data/live_check_raw.jsonl (2026-11-12), the one recording at 35 Hz instead of 52.5 Hz: a quick
// glance down, a held slouch, sitting back up. At this rate each 68-sample glide takes ~1.95 s.
// All four glides are dropped whole, the glance is ignored, and the slouch cues once and recovers.
#[tokio::test(start_paused = true)]
async fn live_35hz_session_end_to_end() {
    let rows = read_csv("live_35hz.csv");
    // Each glide's 67 samples, plus the old stream's last sample at the restart near the start
    // (within the fit tolerance of a glide from the pose before it, so dropped as one).
    assert_eq!(
        rows.len() - replay(&rows).len(),
        4 * 67 + 1,
        "glides dropped"
    );
    let cues = session_cues("live_35hz.csv").await;
    assert_eq!(kinds(&cues), ["Tone", "Recovered"], "cues: {cues:?}");
}
