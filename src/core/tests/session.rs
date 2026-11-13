//! Session behaviour on a scripted tracker, and the calibration statistics.

use super::*;
use crate::core::tracker::TrackerTraits;
use crate::testing::{Outcome, run_session};
use anyhow::bail;
use std::collections::VecDeque;

fn tilt(deg: f64) -> Quat {
    let h = deg.to_radians() / 2.0;
    Quat {
        w: h.cos(),
        x: 0.0,
        y: h.sin(),
        z: 0.0,
    }
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

// --- Scripted session tests (tokio's paused clock makes the waits instant). ---

/// `x` seconds, rounded to the millisecond. Script times are then exact integers.
fn secs(x: f64) -> Duration {
    Duration::from_millis((x * 1000.0).round() as u64)
}

enum Script {
    /// Deliver this event immediately.
    Ev(Event),
    /// Poses of this pitch at 50 Hz for this long (a whole number of samples).
    Poses(f64, Duration),
    /// Poses alternating between two pitches (a moving head) for this long.
    Wobble(f64, f64, Duration),
    /// No events for this long.
    Quiet(Duration),
}

struct FakeTracker {
    script: VecDeque<Script>,
    drifts: bool,
    /// How long after measurement each pose is delivered (a backend that holds samples).
    lag: Duration,
    /// Time until the next pose sample is due.
    until_sample: Duration,
}

impl FakeTracker {
    fn new(script: Vec<Script>, drifts: bool, lag: Duration) -> Self {
        FakeTracker {
            script: script.into(),
            drifts,
            lag,
            // The first pose is measured one sample in and delivered `lag` later.
            until_sample: SAMPLE + lag,
        }
    }
}

const SAMPLE: Duration = Duration::from_millis(20);

/// Wait for the next pose sample, like a real backend. The pose has pitch `pitch`, and `left`
/// is how much of the pose span remains. The pose is delivered `lag` after its measurement.
/// Returns `None` if `timeout` passes first.
async fn sample(
    until_sample: &mut Duration,
    lag: Duration,
    timeout: Duration,
    left: &mut Duration,
    pitch: f64,
) -> Option<Event> {
    if timeout < *until_sample {
        *until_sample -= timeout;
        tokio::time::sleep(timeout).await;
        return None;
    }
    tokio::time::sleep(*until_sample).await;
    *until_sample = SAMPLE;
    *left = left
        .checked_sub(SAMPLE)
        .expect("script pose spans are whole samples");
    Some(Event::Pose {
        at: Instant::now() - lag,
        orientation: tilt(pitch),
    })
}

impl HeadTracker for FakeTracker {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        loop {
            let Some(front) = self.script.front_mut() else {
                bail!("script finished");
            };
            match front {
                Script::Ev(e) => {
                    let e = *e;
                    self.script.pop_front();
                    return Ok(Some(e));
                }
                Script::Poses(_, left) | Script::Wobble(_, _, left) | Script::Quiet(left)
                    if left.is_zero() =>
                {
                    self.script.pop_front();
                }
                Script::Poses(pitch, left) => {
                    let pitch = *pitch;
                    return Ok(
                        sample(&mut self.until_sample, self.lag, timeout, left, pitch).await,
                    );
                }
                Script::Wobble(a, b, left) => {
                    let next = (*left - SAMPLE).as_nanos() / SAMPLE.as_nanos();
                    let pitch = if next.is_multiple_of(2) { *a } else { *b };
                    return Ok(
                        sample(&mut self.until_sample, self.lag, timeout, left, pitch).await,
                    );
                }
                Script::Quiet(left) => {
                    let d = (*left).min(timeout);
                    *left -= d;
                    tokio::time::sleep(d).await;
                    return Ok(None);
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn traits(&self) -> TrackerTraits {
        TrackerTraits {
            pose_drifts_up: self.drifts,
            ..TrackerTraits::default()
        }
    }
}

/// How the simulated tracker and cue output behave.
#[derive(Default)]
struct Sim {
    /// The tracker declares `pose_drifts_up`.
    drifts: bool,
    /// Each pose is delivered this long after it is measured.
    lag: Duration,
    /// Each spoken prompt takes this long.
    prompt: Duration,
}

/// Run `script` to its end and report what the session did.
async fn simulate(script: Vec<Script>, sim: Sim) -> Outcome {
    let mut tracker = FakeTracker::new(script, sim.drifts, sim.lag);
    run_session(&mut tracker, sim.prompt).await
}

use Script::*;

// Sitting back up "past" upright re-anchors only for trackers whose pose drifts.
#[tokio::test(start_paused = true)]
async fn reanchors_only_for_drifting_trackers() {
    let script = || {
        vec![
            Poses(0.0, secs(4.0)),
            Poses(40.0, secs(15.0)),
            Poses(-10.0, secs(3.0)),
        ]
    };
    let drifting = Sim {
        drifts: true,
        ..Sim::default()
    };
    let run = simulate(script(), drifting).await;
    assert_eq!(run.events, ["Tone", "Recovered", "Reanchored"]);
    let run = simulate(script(), Sim::default()).await;
    assert_eq!(run.events, ["Tone", "Recovered"]);
}

#[tokio::test(start_paused = true)]
async fn sustained_slouch_cues_even_while_head_is_still() {
    // Upright (calibration), then look down and hold still: the device goes quiet, but timers
    // keep running on the last pose.
    let script = vec![
        Poses(0.0, secs(4.0)),
        Poses(40.0, secs(2.0)),
        Quiet(secs(35.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.names(),
        ["say SitUpright", "say Calibrated", "Tone", "Speak"]
    );
}

// While the head is held still the tracker is silent, and the tone must still come right at
// 10 s of slouching, not at the next once-a-second check.
#[tokio::test(start_paused = true)]
async fn tone_is_on_time_while_head_is_still() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Poses(40.0, secs(2.0)),
        Quiet(secs(15.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    // The slouch starts at 4 s; smoothing crosses the threshold within 0.1 s of that.
    let tone = run.first("Tone");
    assert!(
        (secs(14.0)..secs(14.15)).contains(&tone),
        "tone at {tone:?}"
    );
}

// A backend that holds samples (here 1.5 s) delivers a sit-up late. Sitting up 9.5 s into a
// slouch, before the 10 s tone, must give no tone: reminders wait for delivered posture.
#[tokio::test(start_paused = true)]
async fn no_reminder_after_sitting_up_when_poses_arrive_late() {
    // Upright long enough for calibration to see 3 s of posture after the 1.5 s lag.
    let script = vec![
        Poses(0.0, secs(6.0)),
        Poses(40.0, secs(9.5)),
        Poses(0.0, secs(6.0)),
    ];
    let lagging = Sim {
        lag: secs(1.5),
        ..Sim::default()
    };
    let run = simulate(script, lagging).await;
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}

// With the same lag, a slouch held past 10 s still cues once its posture has arrived. The
// slouch starts at 6 s. The tone is due about 10.1 s into it and plays 1.5 s (the lag) later.
#[tokio::test(start_paused = true)]
async fn late_poses_delay_the_reminder_by_the_lag() {
    let script = vec![
        Poses(0.0, secs(6.0)),
        Poses(40.0, secs(14.0)),
        Poses(0.0, secs(4.0)),
    ];
    let lagging = Sim {
        lag: secs(1.5),
        ..Sim::default()
    };
    let tone = simulate(script, lagging).await.first("Tone");
    assert!((secs(17.5)..secs(17.7)).contains(&tone), "tone at {tone:?}");
}

// The head moves while the 2 s "sit upright" prompt plays, then holds still. Poses arrive
// 1.5 s late, so the ones measured during the prompt arrive inside the capture window. They
// must not count, so calibration succeeds on the first attempt.
#[tokio::test(start_paused = true)]
async fn calibration_ignores_poses_measured_during_the_prompt() {
    let script = vec![
        Poses(0.0, secs(0.1)),
        Wobble(0.0, 30.0, secs(2.0)),
        Poses(0.0, secs(6.0)),
    ];
    let sim = Sim {
        lag: secs(1.5),
        prompt: secs(2.0),
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}

#[tokio::test(start_paused = true)]
async fn taking_earbuds_off_pauses_then_recalibrates() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Ev(Event::Worn(false)),
        Quiet(secs(10.0)),
        Ev(Event::Worn(true)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["Paused", "Resumed"]);
    assert_eq!(run.count("say SitUpright"), 2, "{:?}", run.names());
}

// The slouch starts at about 4 s. Tracking becomes unavailable at 6 s, before the tone is due
// at about 14 s. No reminder may fire from that stale posture. When tracking resumes, the
// session calibrates again, however short the gap.
#[tokio::test(start_paused = true)]
async fn unavailability_drops_the_slouch_and_recalibrates() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Poses(40.0, secs(2.0)),
        Ev(Event::Available(false)),
        Quiet(secs(20.0)),
        Ev(Event::Available(true)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["TrackingUnavailable", "TrackingAvailable"]);
    let prompts = run.count("say SitUpright");
    assert_eq!(prompts, 2, "recalibrates after resuming: {:?}", run.names());
}

// A still head sends no poses. When tracking resumes, headup asks the user to sit upright at
// once. With no pose 5 s later, it asks them to move a little. It then calibrates on the poses
// that follow. Moving, then holding still, does not count as "you moved".
#[tokio::test(start_paused = true)]
async fn still_head_is_asked_to_move_when_tracking_resumes() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Ev(Event::Available(false)),
        Quiet(secs(10.0)),
        Ev(Event::Available(true)),
        Quiet(secs(6.0)),
        Wobble(0.0, 20.0, secs(1.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.names(),
        [
            "say SitUpright",
            "say Calibrated",
            "say SitUpright",
            "say MoveHead",
            "say Calibrated"
        ]
    );
}

// The same after putting the earbuds back on: the calibration does not depend on the tracker
// sending poses by itself.
#[tokio::test(start_paused = true)]
async fn still_head_is_asked_to_move_after_rewearing() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Ev(Event::Worn(false)),
        Quiet(secs(5.0)),
        Ev(Event::Worn(true)),
        Quiet(secs(6.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.count("say MoveHead"), 1, "{:?}", run.names());
    assert_eq!(run.count("say Calibrated"), 2, "{:?}", run.names());
}

// The nudge repeats while nothing arrives.
#[tokio::test(start_paused = true)]
async fn nudge_repeats_while_no_pose_arrives() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Ev(Event::Available(false)),
        Ev(Event::Available(true)),
        Quiet(secs(36.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    // Nudges at 5, 20 and 35 s after the prompt.
    assert_eq!(run.count("say MoveHead"), 3, "{:?}", run.names());
}

// At startup no pose has arrived yet, so silence may mean tracking has not started (e.g. a
// device precondition is not met). headup waits without prompting or nudging.
#[tokio::test(start_paused = true)]
async fn startup_waits_silently_for_tracking() {
    let script = vec![Quiet(secs(30.0)), Poses(0.0, secs(4.0))];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}

// Poses can still arrive after the tracker becomes unavailable (data measured before it, or
// trailing data). They do not start a calibration: that waits for `Available(true)`.
#[tokio::test(start_paused = true)]
async fn poses_after_unavailability_do_not_start_calibration() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        Ev(Event::Available(false)),
        Poses(0.0, secs(1.0)),
        Quiet(secs(40.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["TrackingUnavailable"]);
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}

#[tokio::test(start_paused = true)]
async fn failed_calibration_retries_later() {
    let script = vec![
        Wobble(0.0, 30.0, secs(30.0)), // keeps moving through all 4 attempts
        Quiet(CALIBRATION_RETRY + secs(1.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["CalibrationFailed"]);
    let expected: Vec<&str> = ["say SitUpright"]
        .into_iter()
        .chain(["say Moved"; CALIBRATION_TRIES])
        // The retry prompts at once; the head is still and silent, so it is asked to move.
        .chain(["say SitUpright", "say MoveHead", "say Calibrated"])
        .collect();
    assert_eq!(run.names(), expected);
}
