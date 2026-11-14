//! Session behaviour on a scripted tracker.

use super::*;
use crate::core::calibration::CALIBRATION_TRIES;
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

/// A tracker that plays `Script` on the clock from when it is created, like a backend that reads
/// all the time: events that fall due while the session is busy wait until it asks for them, with
/// their own measurement times.
struct FakeTracker {
    start: Instant,
    /// Events and when they fall due (since `start`), in order.
    events: VecDeque<(Duration, Event)>,
    /// When the script ends.
    end: Duration,
    drifts: bool,
}

const SAMPLE: Duration = Duration::from_millis(20);

impl FakeTracker {
    fn new(script: Vec<Script>, drifts: bool) -> Self {
        let start = Instant::now();
        let mut events = VecDeque::new();
        let mut t = Duration::ZERO;
        for step in script {
            match step {
                Script::Poses(pitch, span) => {
                    for _ in samples(span) {
                        t += SAMPLE;
                        events.push_back((t, pose(start + t, pitch)));
                    }
                }
                // Samples alternate, ending on `a`.
                Script::Wobble(a, b, span) => {
                    for left in samples(span).rev() {
                        t += SAMPLE;
                        let pitch = if left % 2 == 0 { a } else { b };
                        events.push_back((t, pose(start + t, pitch)));
                    }
                }
                Script::Ev(e) => events.push_back((t, e)),
                Script::Quiet(d) => t += d,
            }
        }
        FakeTracker {
            start,
            events,
            end: t,
            drifts,
        }
    }
}

/// Indices of the samples in a script pose span.
fn samples(span: Duration) -> std::ops::Range<u128> {
    assert!(
        span.as_nanos().is_multiple_of(SAMPLE.as_nanos()),
        "script pose spans are whole samples"
    );
    0..span.as_nanos() / SAMPLE.as_nanos()
}

fn pose(at: Instant, pitch: f64) -> Event {
    Event::Pose {
        at,
        orientation: tilt(pitch),
    }
}

impl HeadTracker for FakeTracker {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        let Some(&(due, event)) = self.events.front() else {
            let end = self.start + self.end;
            if end > deadline {
                tokio::time::sleep_until(deadline).await;
                return Ok(None);
            }
            tokio::time::sleep_until(end).await;
            bail!("script finished");
        };
        let due = self.start + due;
        if due > deadline {
            tokio::time::sleep_until(deadline).await;
            return Ok(None);
        }
        tokio::time::sleep_until(due).await;
        self.events.pop_front();
        Ok(Some(event))
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
    /// Each spoken prompt takes this long.
    prompt: Duration,
}

/// Run `script` to its end and report what the session did.
async fn simulate(script: Vec<Script>, sim: Sim) -> Outcome {
    let mut tracker = FakeTracker::new(script, sim.drifts);
    run_session(&mut tracker, sim.prompt).await
}

use Script::*;

/// The tracker reports head tracking unavailable, for a reason of its own.
fn unavailable() -> Script {
    Ev(Event::Unavailable {
        reason: "audio stopped",
    })
}

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

// The head moves while the 2 s "sit upright" prompt plays, then holds still. The poses measured
// during the prompt are delivered after it. They must not count, so calibration succeeds on the
// first attempt.
#[tokio::test(start_paused = true)]
async fn calibration_ignores_poses_measured_during_the_prompt() {
    let script = vec![
        Poses(0.0, secs(0.1)),
        Wobble(0.0, 30.0, secs(2.0)),
        Poses(0.0, secs(6.0)),
    ];
    let sim = Sim {
        prompt: secs(2.0),
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}

// The head moves throughout a 4 s "move your head" prompt, as it asks, and a little after it,
// then holds still. The poses measured while the prompt played are not calibrated on, and the
// attempt starts afresh when it ends, so the head that holds still after it calibrates without
// "you moved".
#[tokio::test(start_paused = true)]
async fn poses_measured_during_the_nudge_do_not_count() {
    let script = vec![
        Poses(0.0, secs(0.02)),
        Quiet(secs(9.0)), // "sit upright" (4 s), then 5 s without a pose
        Wobble(0.0, 30.0, secs(4.5)),
        Poses(0.0, secs(20.0)),
    ];
    let sim = Sim {
        prompt: secs(4.0),
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(
        run.names(),
        ["say SitUpright", "say MoveHead", "say Calibrated"]
    );
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
        unavailable(),
        Quiet(secs(20.0)),
        Ev(Event::Available),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["TrackingUnavailable", "TrackingAvailable"]);
    let prompts = run.count("say SitUpright");
    assert_eq!(prompts, 2, "recalibrates after resuming: {:?}", run.names());
}

// A still head sends no poses. When tracking resumes, headup asks the user to sit upright once the
// head has held still for 2 s (here at once: the silence means it is still). With no pose 5 s
// after the prompt, it asks them to move a little. It then calibrates on the poses that follow.
// Moving, then holding still, does not count as "you moved".
#[tokio::test(start_paused = true)]
async fn still_head_is_asked_to_move_when_tracking_resumes() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        unavailable(),
        Quiet(secs(10.0)),
        Ev(Event::Available),
        Quiet(secs(8.0)),
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
        unavailable(),
        Ev(Event::Available),
        Quiet(secs(38.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    // The prompt comes 2 s after tracking resumes; nudges at 5, 20 and 35 s after it.
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
// trailing data). They do not start a calibration: that waits for `Available`.
#[tokio::test(start_paused = true)]
async fn poses_after_unavailability_do_not_start_calibration() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        unavailable(),
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

// Tracking resumes while the user is still on the move (a device can stop tracking while the
// user walks around). "Sit upright" waits until the head has held still for 2 s: here the head
// moves for 5 s after tracking resumes at 4 s.
#[tokio::test(start_paused = true)]
async fn prompt_after_tracking_resumes_waits_until_the_head_is_still() {
    let script = vec![
        Poses(0.0, secs(4.0)),
        unavailable(),
        Ev(Event::Available),
        Wobble(0.0, 20.0, secs(5.0)),
        Poses(0.0, secs(8.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    let prompts: Vec<Duration> = run
        .cues
        .iter()
        .filter(|(name, _)| name == "say SitUpright")
        .map(|&(_, t)| t)
        .collect();
    assert_eq!(prompts.len(), 2, "{:?}", run.names());
    assert!(
        (secs(11.0)..secs(11.3)).contains(&prompts[1]),
        "prompt at {:?}",
        prompts[1]
    );
}

// Poses queue while "calibrated" plays (15 s here). During it the head goes down for 9 s and
// comes back up, short of the 10 s the tone needs. Monitoring takes every queued pose before it
// judges, so it sees the sit-up and plays nothing; judging after the first pose would play a tone
// for a slouch that already ended.
#[tokio::test(start_paused = true)]
async fn monitoring_judges_only_after_taking_every_queued_pose() {
    let script = vec![
        Poses(0.0, secs(19.0)),
        Poses(40.0, secs(9.0)),
        Poses(0.0, secs(20.0)),
    ];
    let sim = Sim {
        prompt: secs(15.0),
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
}
