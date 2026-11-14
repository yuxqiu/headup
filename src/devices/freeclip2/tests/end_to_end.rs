//! Recorded FreeClip 2 sessions replayed through the protocol into the real posture session. The
//! re-anchoring test feeds the poses straight into the detector instead.

use super::replay::{ReplayTracker, phase_at, phases, read_csv, replay, secs};
use crate::core::detector::Detector;
use crate::core::session::detector_config;
use crate::devices::freeclip2::FreeClip2;
use crate::testing::run_session;
use std::time::Duration;

// Ground truth (guided.py --slouch): five cycles of upright, then look down, then back to the
// same screen point. The earbuds' re-centres add ~10° towards "looking up" per cycle. Without the
// detector's re-anchoring, upright drifts to about -40° and the last look-down never counts as a
// slouch.
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
/// (since the recording began).
async fn session_cues(name: &str) -> Vec<(String, Duration)> {
    let mut tracker = ReplayTracker::new(&read_csv(name));
    let run = run_session(&mut tracker, Duration::ZERO).await;
    run.cues
        .into_iter()
        .filter(|(name, _)| !name.starts_with("say "))
        .collect()
}

/// The cue names, without the spoken reminder (its timing depends on exactly how long a hold
/// lasted).
fn kinds(cues: &[(String, Duration)]) -> Vec<&str> {
    cues.iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| *name != "Speak")
        .collect()
}

// The real long-hold recording. Each 30 s look-down hold must cue, and sitting back up must
// register as recovery.
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

// The stream-restart recording has one slouch, so exactly one tone and one recovery. The recovery
// comes only once the user actually sat up (~25.6 s), not at the restart.
#[tokio::test(start_paused = true)]
async fn stream_restart_end_to_end() {
    let cues = session_cues("stream_restart.csv").await;
    assert_eq!(kinds(&cues), ["Tone", "Recovered"], "cues: {cues:?}");
    let recovered_at = cues.iter().find(|(n, _)| n == "Recovered").unwrap().1;
    assert!(
        recovered_at >= secs(25.6),
        "recovered at {recovered_at:?}, before sitting up"
    );
}

// data/live_check_raw.jsonl (2026-11-12) is the one recording at 35 Hz instead of 52.5 Hz. It has
// a quick glance down, a held slouch, and sitting back up. At this rate each 68-sample glide takes
// ~1.95 s. All four glides are dropped whole, the glance is ignored, and the slouch cues once and
// recovers.
#[tokio::test(start_paused = true)]
async fn live_35hz_session_end_to_end() {
    let rows = read_csv("live_35hz.csv");
    // Exactly each glide's 67 samples.
    assert_eq!(rows.len() - replay(&rows).len(), 4 * 67, "glides dropped");
    let cues = session_cues("live_35hz.csv").await;
    assert_eq!(kinds(&cues), ["Tone", "Recovered"], "cues: {cues:?}");
}

// data/live8_raw.jsonl (2026-11-13): headup reading its socket in its own task, two held
// slouches, then several short glances. The first slouch cues a tone; the second is held past
// 30 s, mostly with the head still and the earbuds silent, so the spoken reminder comes 20 s
// after its tone. Each sit-up plays the recovery cue, and no glance cues.
#[tokio::test(start_paused = true)]
async fn live_session_with_two_slouches_end_to_end() {
    let cues = session_cues("live8.csv").await;
    let names: Vec<&str> = cues.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Tone", "Recovered", "Tone", "Speak", "Recovered"]);
    let reminder = cues[3].1 - cues[2].1;
    assert!(
        (secs(20.0)..secs(20.1)).contains(&reminder),
        "spoken reminder {reminder:?} after the tone"
    );
}

// Four recorded walks (2026-11-13; FINDINGS "Instant resets while walking"). While the user walks,
// the earbuds hold their reading and lose the head's movement, so the reference from before the
// walk no longer applies. No reminder may come from it: each hold makes tracking unavailable, and
// headup calibrates again once the user has stopped and held still, so no attempt fails ("you
// moved") mid-walk. Before this, the first walk ended with a tone, a spoken reminder and a
// notification while the user sat upright.
#[tokio::test(start_paused = true)]
async fn walking_around_gives_no_reminder_end_to_end() {
    for name in ["walk_reconnect.csv", "turn_walk.csv", "walk_near_far.csv"] {
        let mut tracker = ReplayTracker::new(&read_csv(name));
        let run = run_session(&mut tracker, Duration::ZERO).await;
        let reminders: Vec<_> = run
            .cues
            .iter()
            .filter(|(n, _)| !n.starts_with("say "))
            .collect();
        assert!(reminders.is_empty(), "{name}: {reminders:?}");
        assert_eq!(run.count("say Moved"), 0, "{name}: {:?}", run.names());
        assert!(
            run.events.contains(&"TrackingUnavailable".to_string()),
            "{name}: {:?}",
            run.events
        );
    }
}

// data/walk3_raw.jsonl (2026-11-13): a live walk with the hold handling, then looking down at the
// desk. In the raw readings the look-downs (from about 126 s and 158 s) are +46° like every other
// look-down. They must read as looking down after the walk, so each one cues a tone and sitting up
// plays the recovery cue. With the reference from before the hold kept, the user's turn during the
// walk made them read as -47° (looking up), and nothing played.
#[tokio::test(start_paused = true)]
async fn looking_down_after_a_walk_still_cues_end_to_end() {
    let cues = session_cues("walk_slouch.csv").await;
    assert_eq!(
        kinds(&cues),
        ["Tone", "Recovered", "Tone", "Recovered"],
        "cues: {cues:?}"
    );
}
