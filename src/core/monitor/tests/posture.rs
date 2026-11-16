//! Posture on pitch samples, judged against an upright.

use super::*;
use crate::testing::secs;
use Change::{Recovered, SlouchStarted};

/// Feed `pitch` at 50 Hz from `t0` for `span` seconds, judging each sample against `upright` as
/// the monitor does, and collect (time, change). Returns the time after the last sample.
fn run(
    p: &mut Posture,
    t0: f64,
    span: f64,
    pitch: f64,
    upright: f64,
    out: &mut Vec<(f64, Change)>,
) -> f64 {
    let mut t = t0;
    while t < t0 + span {
        p.pose(secs(t), pitch);
        out.extend(p.judge(upright).map(|c| (t, c)));
        t += 0.02;
    }
    t
}

fn kinds(changes: &[(f64, Change)]) -> Vec<Change> {
    changes.iter().map(|c| c.1).collect()
}

#[test]
fn upright_never_changes_posture() {
    let mut p = Posture::new(Config::default());
    let mut changes = vec![];
    run(&mut p, 0.0, 30.0, 5.0, 0.0, &mut changes);
    assert!(changes.is_empty(), "{changes:?}");
}

// A slouch starts as the smoothed pitch reaches 20° and ends as it drops below 15°.
#[test]
fn slouch_starts_and_ends_at_the_thresholds() {
    let mut p = Posture::new(Config::default());
    let mut changes = vec![];
    let t = run(&mut p, 0.0, 5.0, 0.0, 0.0, &mut changes);
    let t = run(&mut p, t, 5.0, 30.0, 0.0, &mut changes);
    assert!(p.slouching());
    run(&mut p, t, 5.0, 0.0, 0.0, &mut changes);
    assert_eq!(kinds(&changes), [SlouchStarted, Recovered]);
    // A step to 30° crosses 20° within half a second of smoothing; back to 0° crosses 15° sooner.
    assert!((5.0..5.5).contains(&changes[0].0), "{changes:?}");
    assert!((10.0..10.5).contains(&changes[1].0), "{changes:?}");
}

#[test]
fn hysteresis_keeps_the_slouch_between_thresholds() {
    let mut p = Posture::new(Config::default());
    let mut changes = vec![];
    let t = run(&mut p, 0.0, 12.0, 25.0, 0.0, &mut changes);
    run(&mut p, t, 30.0, 17.0, 0.0, &mut changes); // between off (15) and on (20): still slouching
    assert!(p.slouching());
    assert_eq!(kinds(&changes), [SlouchStarted]);
}

// The posture is judged against the upright given: from an upright at -10°, a look down to +12°
// is 22°, a slouch; from 0° it is not.
#[test]
fn posture_is_judged_against_the_given_upright() {
    let mut changes = vec![];
    run(
        &mut Posture::new(Config::default()),
        0.0,
        15.0,
        12.0,
        -10.0,
        &mut changes,
    );
    assert_eq!(kinds(&changes), [SlouchStarted]);
    let mut changes = vec![];
    run(
        &mut Posture::new(Config::default()),
        0.0,
        15.0,
        12.0,
        0.0,
        &mut changes,
    );
    assert!(changes.is_empty(), "{changes:?}");
}

// Judging against an upright moved to the head ends a slouch at once, with no new sample: a
// tracker may be silent when the head holds still.
#[test]
fn upright_moved_to_the_head_ends_a_slouch_at_once() {
    let mut p = Posture::new(Config::default());
    let mut changes = vec![];
    run(&mut p, 0.0, 5.0, 25.0, 0.0, &mut changes);
    assert!(p.slouching());
    assert_eq!(p.judge(25.0), Some(Recovered));
    assert!(!p.slouching());
}

// The same head movement smooths the same at any sample rate: 0.24 s after a step from 0° to
// 30°, a 25 Hz and a 100 Hz tracker agree.
#[test]
fn smoothing_is_independent_of_sample_rate() {
    let after_step = |hz: f64| {
        let mut p = Posture::new(Config::default());
        p.pose(Duration::ZERO, 0.0);
        let steps = (0.24 * hz).round() as u32;
        for i in 1..=steps {
            p.pose(secs(f64::from(i) / hz), 30.0);
        }
        p.smoothed(0.0).unwrap()
    };
    let (slow, fast) = (after_step(25.0), after_step(100.0));
    assert!(
        (slow - fast).abs() < 0.01,
        "25 Hz {slow:.2} vs 100 Hz {fast:.2}"
    );
}

#[test]
fn looking_up_is_not_slouching() {
    let mut p = Posture::new(Config::default());
    let mut changes = vec![];
    run(&mut p, 0.0, 60.0, -40.0, 0.0, &mut changes);
    assert!(changes.is_empty());
}
