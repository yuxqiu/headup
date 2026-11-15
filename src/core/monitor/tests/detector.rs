//! Slouch detection on pitch samples and time.

use super::*;
use crate::testing::secs;
use PostureEvent::{Reanchored, Recovered, SlouchStarted};

fn reanchoring() -> Config {
    Config {
        reanchor_after_recovery: true,
        ..Config::default()
    }
}

/// Feed `pitch` at 50 Hz from `t0` for `span` seconds, checking after every sample as the monitor
/// does, and hand each event to `seen` with its time. Returns the time after the last sample.
fn feed(
    d: &mut Detector,
    t0: f64,
    span: f64,
    pitch: f64,
    mut seen: impl FnMut(f64, PostureEvent),
) -> f64 {
    let mut t = t0;
    while t < t0 + span {
        d.pose(secs(t), pitch);
        for (_, event) in d.check(secs(t)) {
            seen(t, event);
        }
        t += 0.02;
    }
    t
}

/// The uprights re-anchored to among `events`.
fn uprights(events: impl IntoIterator<Item = PostureEvent>) -> Vec<f64> {
    events
        .into_iter()
        .filter_map(|e| match e {
            Reanchored { upright } => Some(upright),
            SlouchStarted | Recovered => None,
        })
        .collect()
}

/// [`feed`], collecting the uprights re-anchored to.
fn reanchors(d: &mut Detector, t0: f64, span: f64, pitch: f64, out: &mut Vec<f64>) -> f64 {
    feed(d, t0, span, pitch, |_, e| out.extend(uprights([e])))
}

/// [`feed`], collecting (time, posture change), without re-anchors.
fn run(
    d: &mut Detector,
    t0: f64,
    span: f64,
    pitch: f64,
    out: &mut Vec<(f64, PostureEvent)>,
) -> f64 {
    feed(d, t0, span, pitch, |t, e| {
        if !matches!(e, Reanchored { .. }) {
            out.push((t, e));
        }
    })
}

#[test]
fn upright_never_changes_posture() {
    let mut d = Detector::new(Config::default());
    let mut changes = vec![];
    run(&mut d, 0.0, 30.0, 5.0, &mut changes);
    assert!(changes.is_empty(), "{changes:?}");
}

// A slouch starts as the smoothed pitch reaches 20° and ends as it drops below 15°.
#[test]
fn slouch_starts_and_ends_at_the_thresholds() {
    let mut d = Detector::new(Config::default());
    let mut changes = vec![];
    let t = run(&mut d, 0.0, 5.0, 0.0, &mut changes);
    let t = run(&mut d, t, 5.0, 30.0, &mut changes);
    assert!(d.slouching());
    run(&mut d, t, 5.0, 0.0, &mut changes);
    let kinds: Vec<_> = changes.iter().map(|c| c.1).collect();
    assert_eq!(kinds, [SlouchStarted, Recovered]);
    // A step to 30° crosses 20° within half a second of smoothing; back to 0° crosses 15° sooner.
    assert!((5.0..5.5).contains(&changes[0].0), "{changes:?}");
    assert!((10.0..10.5).contains(&changes[1].0), "{changes:?}");
}

#[test]
fn hysteresis_keeps_the_slouch_between_thresholds() {
    let mut d = Detector::new(Config::default());
    let mut changes = vec![];
    let t = run(&mut d, 0.0, 12.0, 25.0, &mut changes);
    run(&mut d, t, 30.0, 17.0, &mut changes); // between off (15) and on (20): still slouching
    assert!(d.slouching());
    let kinds: Vec<_> = changes.iter().map(|c| c.1).collect();
    assert_eq!(kinds, [SlouchStarted]);
}

// Sitting back up "past" upright (tracker error) re-anchors upright, so the next slouch counts
// in full: from the new upright at -10°, a look down to +12° is 22°, a slouch.
#[test]
fn recovery_past_upright_reanchors() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    t = reanchors(&mut d, t, 3.0, -10.0, &mut re);
    assert!(
        re.len() == 1 && (re[0] + 10.0).abs() < 0.5,
        "re-anchored {re:?}"
    );
    let mut changes = vec![];
    run(&mut d, t, 15.0, 12.0, &mut changes);
    assert!(changes.iter().any(|c| c.1 == SlouchStarted), "{changes:?}");
}

// Sitting back up past upright, then holding so still that the tracker goes quiet, still
// re-anchors: silence means the pitch is unchanged, so the head has settled once a second passes.
#[test]
fn silent_head_after_a_recovery_reanchors() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    // A few samples back up past upright (the smoothing crosses below off_deg), then silence.
    let t = reanchors(&mut d, t, 0.3, -10.0, &mut re);
    assert!(re.is_empty(), "re-anchored before settling: {re:?}");
    let reanchored = uprights(d.check(secs(t + 1.0)).into_iter().map(|(_, e)| e));
    assert!(
        reanchored.len() == 1 && (reanchored[0] + 10.0).abs() < 0.5,
        "re-anchored {reanchored:?}"
    );
}

// The same when the tracker goes quiet at the very sample that ends the slouch: that sample alone
// says where the head settled.
#[test]
fn head_silent_from_the_recovery_sample_reanchors() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    while d.slouching() {
        d.pose(secs(t), -10.0);
        re.extend(uprights(d.check(secs(t)).into_iter().map(|(_, e)| e)));
        t += 0.02;
    }
    assert!(re.is_empty(), "re-anchored before settling: {re:?}");
    let reanchored = uprights(d.check(secs(t + 1.0)).into_iter().map(|(_, e)| e));
    assert_eq!(reanchored, [-10.0]);
}

#[test]
fn looking_up_without_a_slouch_keeps_upright() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    reanchors(&mut d, 0.0, 30.0, -30.0, &mut re);
    assert!(re.is_empty(), "re-anchored {re:?}");
}

#[test]
fn partial_recovery_keeps_upright() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    reanchors(&mut d, t, 5.0, 10.0, &mut re); // below off (15), still above upright
    assert!(re.is_empty(), "re-anchored {re:?}");
}

// Only the first settle after a recovery counts; a later look up must not move upright again.
#[test]
fn reanchors_once_per_recovery() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    t = reanchors(&mut d, t, 3.0, -5.0, &mut re);
    reanchors(&mut d, t, 10.0, -30.0, &mut re);
    assert!(
        re.len() == 1 && (re[0] + 5.0).abs() < 0.5,
        "re-anchored {re:?}"
    );
}

#[test]
fn no_reanchoring_unless_enabled() {
    let mut d = Detector::new(Config::default());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    reanchors(&mut d, t, 3.0, -10.0, &mut re);
    assert!(re.is_empty(), "re-anchored {re:?}");
}

// The same head movement smooths the same at any sample rate: 0.24 s after a step from 0° to
// 30°, a 25 Hz and a 100 Hz tracker agree.
#[test]
fn smoothing_is_independent_of_sample_rate() {
    let after_step = |hz: f64| {
        let mut d = Detector::new(Config::default());
        d.pose(Duration::ZERO, 0.0);
        let steps = (0.24 * hz).round() as u32;
        for i in 1..=steps {
            d.pose(secs(f64::from(i) / hz), 30.0);
        }
        d.smoothed().unwrap()
    };
    let (slow, fast) = (after_step(25.0), after_step(100.0));
    assert!(
        (slow - fast).abs() < 0.01,
        "25 Hz {slow:.2} vs 100 Hz {fast:.2}"
    );
}

#[test]
fn looking_up_is_not_slouching() {
    let mut d = Detector::new(Config::default());
    let mut changes = vec![];
    run(&mut d, 0.0, 60.0, -40.0, &mut changes);
    assert!(changes.is_empty());
}

// Samples passed in one batch come out of one check in order, each event at the time of the
// sample that showed it.
#[test]
fn a_batch_reports_each_event_at_its_sample_time() {
    let mut d = Detector::new(Config::default());
    let mut t = 0.0;
    let mut times = vec![];
    while t < 5.0 {
        let pitch = if t < 2.0 || t >= 4.0 { 0.0 } else { 40.0 };
        d.pose(secs(t), pitch);
        times.push(secs(t));
        t += 0.02;
    }
    let events = d.check(secs(t));
    let kinds: Vec<_> = events.iter().map(|&(_, e)| e).collect();
    assert_eq!(kinds, [SlouchStarted, Recovered]);
    for (at, _) in &events {
        assert!(times.contains(at), "{at:?} is not a sample time");
    }
    // Smoothing crosses 20° about 0.09 s after a step to 40°, and 15° about 0.12 s after a step
    // back to 0° (time constant 0.123 s).
    assert!((secs(2.0)..secs(2.15)).contains(&events[0].0), "{events:?}");
    assert!((secs(4.0)..secs(4.2)).contains(&events[1].0), "{events:?}");
}
