//! Stillness, driven directly with poses and times.

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

/// A head moving between 0° and 30° on every sample.
fn wobble(t: Duration) -> Option<f64> {
    Some(if (t.as_millis() / 20).is_multiple_of(2) {
        0.0
    } else {
        30.0
    })
}

// With nothing delivered, the head counts as still once 2 s have passed since the wait began.
#[test]
fn silence_counts_as_still() {
    let mut still = Stillness::new(secs(1.0));
    assert!(!still.check(secs(2.9)));
    assert!(still.check(secs(3.0)));
}

// A moving head is not still; it is once its posture has stayed within 2° for 2 s.
#[test]
fn head_is_still_after_two_seconds_without_moving() {
    let mut still = Stillness::new(Duration::ZERO);
    let mut t = SAMPLE;
    while t <= secs(5.0) {
        still.pose(t, tilt(wobble(t).unwrap()));
        assert!(!still.check(t), "moving head still at {t:?}");
        t += SAMPLE;
    }
    let stopped = t - SAMPLE;
    loop {
        still.pose(t, tilt(1.0));
        if still.check(t) {
            break;
        }
        t += SAMPLE;
    }
    assert!(
        (stopped + secs(2.0)..stopped + secs(2.1)).contains(&t),
        "still at {t:?}, stopped at {stopped:?}"
    );
}
