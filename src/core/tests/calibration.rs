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
/// gives one. Returns the first output, and when it came.
fn drive(
    cal: &mut Calibration,
    from: Duration,
    to: Duration,
    pitch: impl Fn(Duration) -> Option<f64>,
) -> Option<(Duration, Output)> {
    let mut t = from;
    while t <= to {
        if let Some(p) = pitch(t) {
            cal.pose(t, tilt(p));
        }
        if let Some(output) = cal.check(t) {
            return Some((t, output));
        }
        t += SAMPLE;
    }
    None
}

/// The outcome of an attempt that ended, with its report.
fn ended(output: Output) -> (Report, Outcome) {
    match output {
        Output::AttemptEnded { report, outcome } => (report, outcome),
        other => panic!("no attempt ended: {other:?}"),
    }
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
    let (at, output) = drive(&mut cal, SAMPLE, secs(10.0), |_| Some(10.0)).unwrap();
    assert_eq!(at, SAMPLE + secs(3.0));
    let (report, outcome) = ended(output);
    let Outcome::Calibrated(upright) = outcome else {
        panic!("{outcome:?}");
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
    let (at, output) = drive(&mut cal, SAMPLE, secs(10.0), pitch).unwrap();
    assert!(matches!(ended(output).1, Outcome::Calibrated(_)));
    assert!(at < SAMPLE + CALIBRATION_ATTEMPT, "calibrated at {at:?}");
}

// A pose measured before the attempt started (during the prompt) does not count, so the attempt
// still has no pose and nudges 5 s after it started.
#[test]
fn poses_from_before_the_start_do_not_count() {
    let mut cal = Calibration::new(secs(2.0));
    cal.pose(secs(1.0), tilt(0.0));
    let (at, output) = drive(&mut cal, secs(2.0), secs(10.0), |_| None).unwrap();
    assert_eq!((at, output), (secs(7.0), Output::Nudge));
}

// The nudge comes 5 s after the start, then every 15 s after each nudge ends.
#[test]
fn nudges_repeat_while_no_pose_arrives() {
    let mut cal = Calibration::new(Duration::ZERO);
    let (at, output) = drive(&mut cal, Duration::ZERO, secs(60.0), |_| None).unwrap();
    assert_eq!((at, output), (secs(5.0), Output::Nudge));
    cal.prompt_ended(secs(9.0)); // the prompt took 4 s
    let (at, output) = drive(&mut cal, secs(9.0), secs(60.0), |_| None).unwrap();
    assert_eq!((at, output), (secs(24.0), Output::Nudge));
}

// A head that keeps moving fails each attempt 5 s after its first pose, and calibration gives up
// after the last one.
#[test]
fn moving_head_fails_every_attempt_then_gives_up() {
    let mut cal = Calibration::new(Duration::ZERO);
    let mut t = Duration::ZERO;
    for attempt in 1..=CALIBRATION_TRIES {
        let (at, output) = drive(&mut cal, t + SAMPLE, secs(100.0), wobble).unwrap();
        assert_eq!(at, t + SAMPLE + CALIBRATION_ATTEMPT);
        let (report, outcome) = ended(output);
        assert_eq!(report.attempt, attempt);
        let last = attempt == CALIBRATION_TRIES;
        let want = if last {
            Outcome::GaveUp
        } else {
            Outcome::Retry
        };
        assert_eq!(outcome, want, "attempt {attempt}");
        if attempt < CALIBRATION_TRIES {
            t = at + secs(1.0); // "you moved" takes 1 s
            cal.prompt_ended(t);
        }
    }
}

// After a nudge the attempt measures afresh: poses measured while the nudge played (the user moving,
// as asked) do not count, and the 5 s limit runs from the first pose after it.
#[test]
fn nudge_starts_measuring_afresh() {
    let mut cal = Calibration::new(Duration::ZERO);
    let (at, output) = drive(&mut cal, Duration::ZERO, secs(60.0), |_| None).unwrap();
    assert_eq!((at, output), (secs(5.0), Output::Nudge));
    // The prompt plays from 5 s to 9 s while the head moves. Its poses are taken after it ends, as
    // the session does.
    cal.prompt_ended(secs(9.0));
    let mut t = secs(5.0) + SAMPLE;
    while t < secs(9.0) {
        cal.pose(t, tilt(wobble(t).unwrap()));
        t += SAMPLE;
    }
    let (at, output) = drive(&mut cal, secs(9.0), secs(30.0), |_| Some(0.0)).unwrap();
    assert!(matches!(ended(output).1, Outcome::Calibrated(_)));
    assert_eq!(at, secs(9.0) + secs(3.0));
}

// Every call is valid in every state: poses and checks while a prompt plays, or after calibration
// is over, do nothing, and so does a prompt's end when none was asked for.
#[test]
fn calls_out_of_turn_do_nothing() {
    let mut cal = Calibration::new(Duration::ZERO);
    cal.prompt_ended(secs(1.0)); // no prompt playing
    let (_, output) = drive(&mut cal, Duration::ZERO, secs(60.0), |_| None).unwrap();
    assert_eq!(output, Output::Nudge);
    cal.pose(secs(6.0), tilt(0.0)); // while the nudge plays
    assert_eq!(cal.check(secs(6.0)), None);
    cal.prompt_ended(secs(9.0));
    let (_, output) = drive(&mut cal, secs(9.0), secs(30.0), |_| Some(0.0)).unwrap();
    assert!(matches!(ended(output).1, Outcome::Calibrated(_)));
    cal.pose(secs(20.0), tilt(0.0));
    assert_eq!(cal.check(secs(20.0)), None); // over
}
