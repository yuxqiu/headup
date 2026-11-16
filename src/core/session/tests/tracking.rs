//! The session's tracking phases, on events and times alone.

use super::*;
use crate::testing::secs;
use crate::testing::tilt;

fn live(t: &Tracking) -> bool {
    matches!(t, Tracking::Live)
}

/// Poses of `pitch` at 50 Hz from `from` to `to` seconds, each followed by a settle check.
fn poses(t: &mut Tracking, from: f64, to: f64, pitch: impl Fn(u32) -> f64) {
    let mut i = 0;
    while from + f64::from(i) * 0.02 <= to {
        let at = secs(from + f64::from(i) * 0.02);
        t.pose_arrived(at, tilt(pitch(i)));
        t.settle(at);
        i += 1;
    }
}

// Before `Available`, nothing makes the head judgeable: not poses, not time.
#[test]
fn waiting_ignores_poses_and_time() {
    let mut t = Tracking::Waiting;
    poses(&mut t, 0.0, 5.0, |_| 0.0);
    t.settle(secs(60.0));
    assert!(!live(&t));
}

// After `Available`, a head that holds still for 2 s makes tracking live, not sooner.
#[test]
fn a_still_head_goes_live_after_two_seconds() {
    let mut t = Tracking::Waiting;
    t.available(secs(1.0));
    poses(&mut t, 1.0, 2.9, |_| 5.0);
    assert!(!live(&t));
    poses(&mut t, 2.92, 3.1, |_| 5.0);
    assert!(live(&t));
}

// Silence after `Available` counts as a still head.
#[test]
fn silence_goes_live_after_two_seconds() {
    let mut t = Tracking::Waiting;
    t.available(secs(1.0));
    t.settle(secs(2.99));
    assert!(!live(&t));
    t.settle(secs(3.0));
    assert!(live(&t));
}

// A moving head keeps tracking settling.
#[test]
fn a_moving_head_keeps_settling() {
    let mut t = Tracking::Waiting;
    t.available(secs(0.0));
    poses(&mut t, 0.0, 10.0, |i| if i % 2 == 0 { 0.0 } else { 20.0 });
    assert!(!live(&t));
}

// Only losing a live head interrupts; `Unavailable` while waiting or settling just waits again.
#[test]
fn unavailable_interrupts_only_a_live_head() {
    let mut t = Tracking::Waiting;
    assert!(t.unavailable().is_ok());
    t.available(secs(0.0));
    assert!(t.unavailable().is_ok(), "while settling");
    t.available(secs(1.0));
    t.settle(secs(3.0));
    assert!(live(&t));
    assert!(matches!(t.unavailable(), Err(Interrupt::Unavailable)));
    assert!(matches!(t, Tracking::Waiting));
}

// `Available` while settling or live changes nothing: settling does not restart.
#[test]
fn available_again_changes_nothing() {
    let mut t = Tracking::Waiting;
    t.available(secs(0.0));
    t.available(secs(1.9));
    t.settle(secs(2.0));
    assert!(live(&t), "settling kept its start");
    t.available(secs(3.0));
    assert!(live(&t));
}
