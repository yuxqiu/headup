//! Recorded FreeClip 2 sessions replayed through the rebuilder and the detector, the way the
//! session drives them.

use super::replay::{Row, phase_at, phases, read_csv, replay};
use crate::core::detector::{Config, Cue, Detector};
use crate::devices::freeclip2::FreeClip2;

// Ground truth (guided.py --slouch): five upright → look-down cycles back to the same screen
// point. The earbuds' re-centres add ~10° towards "looking up" per cycle; without the detector's
// re-anchoring, upright drifts to about -40° and the last look-down never counts as a slouch.
#[test]
fn repeated_slouches_stay_detectable_end_to_end() {
    let rows = read_csv("slouch_guided.csv");
    let ph = phases(&rows);
    let mut det = Detector::new(Config::for_traits(FreeClip2::TRAITS));
    let mut state = vec![(false, 0.0); ph.len()]; // (slouched, last smoothed) per phase
    for o in replay(&rows) {
        det.update(o.now, o.abs.euler().pitch); // baseline = stream start (upright)
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

/// Replay through a detector the way the session drives it: poses at their release time and
/// 1 s ticks in between. Returns (cue, measurement time of the last pose before it).
fn cues_end_to_end(rows: &[Row]) -> Vec<(Cue, f64)> {
    let mut det = Detector::new(Config::default());
    let (mut cues, mut tick, mut cur) = (Vec::new(), 0.0, 0.0);
    for o in replay(rows) {
        while tick < o.now {
            cues.extend(det.tick(tick).into_iter().map(|c| (c, cur))); // silence: the last pose holds
            tick += 1.0;
        }
        cur = o.t;
        // baseline = stream start (upright)
        cues.extend(
            det.update(o.now, o.abs.euler().pitch)
                .cues
                .into_iter()
                .map(|c| (c, cur)),
        );
    }
    cues
}

// End to end on the real long-hold recording: each 30 s look-down hold must cue, and sitting
// back up must register as recovery (session 1 failed this before glides were suppressed).
#[test]
fn long_holds_trigger_cues_end_to_end() {
    let rows = read_csv("hold_test.csv");
    let ph = phases(&rows);
    let cues = cues_end_to_end(&rows);
    // Speech (at 30 s) depends on exactly how long each ~30 s hold lasted, so it is not asserted.
    let key: Vec<(Cue, &str)> = cues
        .iter()
        .filter(|c| c.0 != Cue::Speak)
        .map(|&(c, t)| (c, ph[phase_at(&ph, t)].0.as_str()))
        .collect();
    let want = [
        (Cue::Tone, "down"),
        (Cue::Recovered, "center"),
        (Cue::Tone, "down"),
        (Cue::Recovered, "center"),
    ];
    assert_eq!(key, want, "cues: {cues:?}");
}

// End to end on the stream-restart recording: one slouch, so exactly one tone and one recovery,
// and the recovery only once the user actually sat up (~25.6 s), not at the restart.
#[test]
fn stream_restart_end_to_end() {
    let cues = cues_end_to_end(&read_csv("stream_restart.csv"));
    let kinds: Vec<Cue> = cues
        .iter()
        .map(|c| c.0)
        .filter(|c| *c != Cue::Speak)
        .collect();
    assert_eq!(kinds, [Cue::Tone, Cue::Recovered], "cues: {cues:?}");
    let recovered_at = cues.iter().find(|c| c.0 == Cue::Recovered).unwrap().1;
    assert!(
        recovered_at >= 25.6,
        "recovered at {recovered_at:.2}, before sitting up"
    );
}
