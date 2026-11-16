//! The monitor on poses and times: what the session is told to do, and in what order.

use super::*;
use crate::testing::secs;

fn tilt(deg: f64) -> Quat {
    let h = deg.to_radians() / 2.0;
    Quat {
        w: h.cos(),
        x: 0.0,
        y: h.sin(),
        z: 0.0,
    }
}

/// A tracker without quirks, and one whose pose drifts (so a head-up re-anchors upright).
fn traits(drifts: bool) -> TrackerTraits {
    TrackerTraits {
        pose_drifts: drifts,
    }
}

/// Feed `pitch` (degrees) at 50 Hz from `t0` for `span` seconds, checking after every pose as the
/// session does, and collect (time, output). Returns the time after the last pose.
fn hold(m: &mut Monitor, t0: f64, span: f64, pitch: f64, out: &mut Vec<(f64, Output)>) -> f64 {
    let mut t = t0;
    while t < t0 + span {
        m.pose(secs(t), tilt(pitch));
        out.extend(m.check(secs(t)).into_iter().map(|o| (t, o)));
        t += 0.02;
    }
    t
}

fn outputs(out: &[(f64, Output)]) -> Vec<Output> {
    out.iter().map(|&(_, o)| o).collect()
}

// A slouch held past the tone and the spoken reminder reminds on schedule; sitting up then tinks,
// and nothing more comes after.
#[test]
fn a_slouch_reminds_then_sitting_up_tinks() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    let mut out = vec![];
    let t = hold(&mut m, 0.0, 2.0, 0.0, &mut out);
    let t = hold(&mut m, t, 35.0, 40.0, &mut out);
    let t = hold(&mut m, t, 3.0, 0.0, &mut out);
    assert_eq!(
        outputs(&out),
        [
            Output::Remind(Cue::Tone),
            Output::Remind(Cue::Speak),
            Output::Remind(Cue::Recovered)
        ]
    );
    // The slouch starts within 0.1 s of looking down at 2 s; the tone 10 s after it.
    assert!((12.0..12.15).contains(&out[0].0), "{out:?}");
    assert_eq!(m.next_due(), None);
    assert_eq!(m.check(secs(t + 300.0)), []);
}

// A glance shorter than the tone's delay says nothing, not even the tink.
#[test]
fn a_glance_says_nothing() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    let mut out = vec![];
    let t = hold(&mut m, 0.0, 6.0, 40.0, &mut out);
    hold(&mut m, t, 5.0, 0.0, &mut out);
    assert_eq!(outputs(&out), []);
}

// Silence keeps time: a slouch held so still that the tracker goes quiet still reminds when the
// session checks at the due time.
#[test]
fn a_silent_slouch_reminds_at_the_due_time() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    hold(&mut m, 0.0, 1.0, 40.0, &mut vec![]);
    let due = m.next_due().unwrap();
    assert_eq!(m.check(due - Duration::from_nanos(1)), []);
    assert_eq!(m.check(due), [Output::Remind(Cue::Tone)]);
}

// For a tracker whose pose drifts, a head-up past upright re-anchors after the tink.
#[test]
fn sitting_up_past_upright_reanchors_after_the_tink() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(true));
    let mut out = vec![];
    let t = hold(&mut m, 0.0, 3.0, 0.0, &mut out);
    let t = hold(&mut m, t, 15.0, 40.0, &mut out);
    hold(&mut m, t, 3.0, -10.0, &mut out);
    let got = outputs(&out);
    assert_eq!(
        got[..2],
        [Output::Remind(Cue::Tone), Output::Remind(Cue::Recovered)]
    );
    let [Output::Reanchored { upright }] = got[2..] else {
        panic!("{got:?}");
    };
    assert!((upright + 10.0).abs() < 0.5, "{upright}");
}

// A head-up that settles while the tracker is silent ends the slouch with the re-anchor: no slouch
// reminder follows (the slouch ended before its first tone, so no tink either).
#[test]
fn head_up_in_silence_stops_reminders() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(true));
    let mut out = vec![];
    let t = hold(&mut m, 0.0, 3.0, 25.0, &mut out);
    let t = hold(&mut m, t, 1.0, 50.0, &mut out);
    let t = hold(&mut m, t, 0.6, 25.0, &mut out);
    let mut now = t;
    while now < t + 30.0 {
        out.extend(m.check(secs(now)).into_iter().map(|o| (now, o)));
        now += 0.5;
    }
    assert_eq!(outputs(&out), [Output::Reanchored { upright: 25.0 }]);
    assert!(!m.status().slouching);
}

// Poses passed in one batch are each judged at their own time: a slouch that starts mid-batch
// (smoothing crosses 20° about 0.09 s after a step to 40°) is timed from that pose, so its tone
// falls due 10 s later, not 10 s after the check.
#[test]
fn a_batch_is_judged_at_each_pose_time() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    let mut t = 0.0;
    while t < 4.0 {
        m.pose(secs(t), tilt(if t < 2.0 { 0.0 } else { 40.0 }));
        t += 0.02;
    }
    assert_eq!(m.check(secs(4.0)), []);
    let due = m.next_due().expect("slouching");
    assert!((secs(12.0)..secs(12.15)).contains(&due), "{due:?}");
}

// Angles are relative to the calibrated baseline.
#[test]
fn angles_are_relative_to_the_baseline() {
    let mut m = Monitor::new(tilt(10.0), traits(false));
    let angles = m.pose(secs(0.0), tilt(35.0));
    assert!((angles.pitch - 25.0).abs() < 1e-9, "{angles:?}");
    assert_eq!(
        m.status(),
        Status {
            pitch: Some(angles.pitch),
            slouching: true
        }
    );
}

// Poses delivered in one batch, as after a prompt or a late delivery: a slouch held past the tone,
// then a sit-up, checked once. The reminder that fell due before the sit-up comes before the tink.
#[test]
fn a_batch_keeps_reminders_before_the_change_that_follows_them() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    let mut t = 0.0;
    while t < 12.0 {
        m.pose(secs(t), tilt(40.0));
        t += 0.02;
    }
    while t < 14.0 {
        m.pose(secs(t), tilt(0.0));
        t += 0.02;
    }
    assert_eq!(
        m.check(secs(t)),
        [Output::Remind(Cue::Tone), Output::Remind(Cue::Recovered)]
    );
}

// Sitting up at the very moment a reminder falls due, before a check at that time ran: the user
// was still slouching when it fell due, so it plays, then the tink. The outcome does not depend on
// whether the pose or the check came first.
#[test]
fn sitting_up_as_a_reminder_falls_due_plays_it_then_tinks() {
    let mut m = Monitor::new(Quat::IDENTITY, traits(false));
    m.pose(secs(0.0), tilt(40.0));
    assert_eq!(m.check(secs(0.0)), []);
    let due = m.next_due().unwrap();
    m.pose(due, tilt(-10.0));
    assert_eq!(
        m.check(due),
        [Output::Remind(Cue::Tone), Output::Remind(Cue::Recovered)]
    );
}
