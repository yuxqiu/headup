//! Calibration decisions, driven directly with poses and times.

use super::*;

fn tilt(deg: f64) -> Quat {
    let h = deg.to_radians() / 2.0;
    Quat {
        w: h.cos(),
        x: 0.0,
        y: h.sin(),
        z: 0.0,
    }
}

/// `x` seconds, rounded to the millisecond.
fn secs(x: f64) -> Duration {
    Duration::from_millis((x * 1000.0).round() as u64)
}

const SAMPLE: Duration = Duration::from_millis(20);

/// Check `cal` every 20 ms from `from` to `to`, first passing a pose of pitch `pitch(t)` when it
/// gives one. Returns the first step other than `Wait`, and when it came.
fn drive(
    cal: &mut Calibration,
    from: Duration,
    to: Duration,
    pitch: impl Fn(Duration) -> Option<f64>,
) -> Option<(Duration, Step)> {
    let mut t = from;
    while t <= to {
        if let Some(p) = pitch(t) {
            cal.pose(t, tilt(p));
        }
        match cal.check(t) {
            Step::Wait => {}
            step => return Some((t, step)),
        }
        t += SAMPLE;
    }
    None
}

/// A head moving between 0° and 30° on every sample.
fn wobble(t: Duration) -> Option<f64> {
    Some(if (t.as_millis() / 20).is_multiple_of(2) {
        0.0
    } else {
        30.0
    })
}

#[test]
fn calibration_mean_and_spread() {
    let (mean, p90) = mean_and_p90(&[tilt(9.0), tilt(10.0), tilt(11.0)]);
    assert!((mean.euler().pitch - 10.0).abs() < 1e-6);
    assert!(p90 < 1.01, "p90 {p90}");
}

#[test]
fn calibration_mean_handles_sign_flipped_quaternions() {
    let q = tilt(10.0);
    let neg = Quat {
        w: -q.w,
        x: -q.x,
        y: -q.y,
        z: -q.z,
    }; // same rotation
    let (mean, p90) = mean_and_p90(&[q, neg, q]);
    assert!((mean.euler().pitch - 10.0).abs() < 1e-6);
    assert!(p90 < 1e-6, "p90 {p90}");
}

// A head held steady from the first pose calibrates once 3 s of posture are known, on its mean.
#[test]
fn steady_head_calibrates_after_three_seconds() {
    let mut cal = Calibration::new(Duration::ZERO);
    let (at, step) = drive(&mut cal, SAMPLE, secs(10.0), |_| Some(10.0)).unwrap();
    assert_eq!(at, SAMPLE + secs(3.0));
    let Step::Calibrated { upright, report } = step else {
        panic!("{step:?}");
    };
    assert!((upright.euler().pitch - 10.0).abs() < 1e-6);
    assert_eq!(report.attempt, 1);
}

// Moving first, then holding still, calibrates within the first attempt, without "you moved":
// the span judged ends at now, not 3 s after the first pose.
#[test]
fn moving_then_holding_still_calibrates() {
    let mut cal = Calibration::new(Duration::ZERO);
    let pitch = |t| if t < secs(1.0) { wobble(t) } else { Some(0.0) };
    let (at, step) = drive(&mut cal, SAMPLE, secs(10.0), pitch).unwrap();
    assert!(matches!(step, Step::Calibrated { .. }), "{step:?}");
    assert!(at < SAMPLE + CALIBRATION_ATTEMPT, "calibrated at {at:?}");
}

// A pose measured before the attempt started (during the prompt) does not count, so the attempt
// still has no pose and nudges 5 s after it started.
#[test]
fn poses_from_before_the_start_do_not_count() {
    let mut cal = Calibration::new(secs(2.0));
    cal.pose(secs(1.0), tilt(0.0));
    let (at, step) = drive(&mut cal, secs(2.0), secs(10.0), |_| None).unwrap();
    assert_eq!((at, step), (secs(7.0), Step::Nudge));
}

// The nudge comes 5 s after the start, then every 15 s after each nudge ends.
#[test]
fn nudges_repeat_while_no_pose_arrives() {
    let mut cal = Calibration::new(Duration::ZERO);
    let (at, step) = drive(&mut cal, Duration::ZERO, secs(60.0), |_| None).unwrap();
    assert_eq!((at, step), (secs(5.0), Step::Nudge));
    cal.spoken(secs(9.0)); // the prompt took 4 s
    let (at, step) = drive(&mut cal, secs(9.0), secs(60.0), |_| None).unwrap();
    assert_eq!((at, step), (secs(24.0), Step::Nudge));
}

// A head that keeps moving fails each attempt 5 s after its first pose, and calibration gives up
// after the last one.
#[test]
fn moving_head_fails_every_attempt_then_gives_up() {
    let mut cal = Calibration::new(Duration::ZERO);
    let mut t = Duration::ZERO;
    for attempt in 1..=CALIBRATION_TRIES {
        let (at, step) = drive(&mut cal, t + SAMPLE, secs(100.0), wobble).unwrap();
        assert_eq!(at, t + SAMPLE + CALIBRATION_ATTEMPT);
        match step {
            Step::Moved(r) if attempt < CALIBRATION_TRIES => assert_eq!(r.attempt, attempt),
            Step::GaveUp(r) if attempt == CALIBRATION_TRIES => assert_eq!(r.attempt, attempt),
            other => panic!("attempt {attempt}: {other:?}"),
        }
        if attempt < CALIBRATION_TRIES {
            t = at + secs(1.0); // "you moved" takes 1 s
            cal.spoken(t);
        }
    }
}
