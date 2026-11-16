//! Session behaviour on a scripted tracker.

use super::*;
use crate::core::calibration::CALIBRATION_TRIES;
use crate::core::sound::Cue::*;
use crate::core::sound::Phrase::*;
use crate::core::sound::Sound::{self, Prompt, Reminder};
use crate::core::steady::STILL;
use crate::core::tracker::TrackerTraits;
use crate::testing::secs;
use crate::testing::tilt;
use crate::testing::{PlayerSim, ScriptFinished, SessionRun, run_session};
use std::collections::VecDeque;

// --- Scripted session tests (tokio's paused clock makes the waits instant). ---

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
    async fn next_event(&mut self) -> Result<Event> {
        let Some(&(due, event)) = self.events.front() else {
            tokio::time::sleep_until(self.start + self.end).await;
            return Err(ScriptFinished.into());
        };
        // Cancel-safe: the event leaves the script only once it is due.
        tokio::time::sleep_until(self.start + due).await;
        self.events.pop_front();
        Ok(event)
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn traits(&self) -> TrackerTraits {
        TrackerTraits {
            pose_drifts: self.drifts,
        }
    }
}

/// How the simulated tracker and player behave.
#[derive(Default)]
struct Sim {
    /// The tracker declares `pose_drifts`.
    drifts: bool,
    player: PlayerSim,
}

/// Run `script` to its end and report what the session did.
async fn simulate(script: Vec<Script>, sim: Sim) -> SessionRun {
    let mut tracker = FakeTracker::new(script, sim.drifts);
    run_session(&mut tracker, sim.player).await
}

use Script::*;

/// The tracker reports head tracking unavailable, for a reason of its own.
fn unavailable() -> Script {
    Ev(Event::Unavailable {
        reason: "audio stopped",
    })
}

// A head-up re-anchors only for trackers whose pose drifts.
#[tokio::test(start_paused = true)]
async fn reanchors_only_for_drifting_trackers() {
    let script = || {
        vec![
            Ev(Event::Available),
            Poses(0.0, STILL + secs(4.0)),
            Poses(40.0, secs(15.0)),
            Poses(-10.0, secs(3.0)),
        ]
    };
    let drifting = Sim {
        drifts: true,
        ..Sim::default()
    };
    let run = simulate(script(), drifting).await;
    let cues = |run: &SessionRun| run.reminders().iter().map(|&(c, _)| c).collect::<Vec<_>>();
    assert_eq!(
        run.events,
        ["TrackingAvailable", "Cue", "Cue", "Reanchored"]
    );
    assert_eq!(cues(&run), [Tone, Recovered]);
    let run = simulate(script(), Sim::default()).await;
    assert_eq!(run.events, ["TrackingAvailable", "Cue", "Cue"]);
    assert_eq!(cues(&run), [Tone, Recovered]);
}

#[tokio::test(start_paused = true)]
async fn sustained_slouch_cues_even_while_head_is_still() {
    // Upright (calibration), then look down and hold still: the device goes quiet, but timers
    // keep running on the last pose.
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        Poses(40.0, secs(2.0)),
        Quiet(secs(35.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.played(),
        [
            Prompt(SitUpright),
            Prompt(Calibrated),
            Reminder(Tone),
            Reminder(Speak)
        ]
    );
}

// A reminder that cannot be played (the sound server stopped answering, say) is shown and logged
// as `CueFailed`, and monitoring goes on: the next reminder still comes.
#[tokio::test(start_paused = true)]
async fn failed_reminder_is_logged_and_monitoring_goes_on() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        Poses(40.0, secs(2.0)),
        Quiet(secs(35.0)),
    ];
    let sim = Sim {
        player: PlayerSim {
            failing: Some(Tone),
            ..PlayerSim::default()
        },
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(
        run.events,
        ["TrackingAvailable", "Cue", "CueFailed", "Cue"],
        "{:?}",
        run.played()
    );
    assert_eq!(
        run.reminders().iter().map(|&(c, _)| c).collect::<Vec<_>>(),
        [Tone, Speak]
    );
}

// While the head is held still the tracker is silent, and the tone must still come right at
// 10 s of slouching, not at the next once-a-second check.
#[tokio::test(start_paused = true)]
async fn tone_is_on_time_while_head_is_still() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        Poses(40.0, secs(2.0)),
        Quiet(secs(15.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    // The slouch starts at STILL + 4 s; smoothing crosses the threshold within 0.1 s of that.
    let tone = run.first(Reminder(Tone));
    assert!(
        (STILL + secs(14.0)..STILL + secs(14.15)).contains(&tone),
        "tone at {tone:?}"
    );
}

// The head moves while the 2 s "sit upright" prompt plays, then holds still. The poses measured
// during the prompt are delivered after it. They must not count, so calibration succeeds on the
// first attempt.
#[tokio::test(start_paused = true)]
async fn calibration_ignores_poses_measured_during_the_prompt() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(0.1)), // still: "sit upright" starts at STILL + SAMPLE
        Wobble(0.0, 30.0, secs(2.0)),
        Poses(0.0, secs(6.0)),
    ];
    let sim = Sim {
        player: PlayerSim {
            prompt: secs(2.0),
            ..PlayerSim::default()
        },
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(run.played(), [Prompt(SitUpright), Prompt(Calibrated)]);
}

// The head moves throughout a 4 s "move your head" prompt, as it asks, and a little after it,
// then holds still. The poses measured while the prompt played are not calibrated on, and the
// attempt starts afresh when it ends, so the head that holds still after it calibrates without
// "you moved".
#[tokio::test(start_paused = true)]
async fn poses_measured_during_the_nudge_do_not_count() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + SAMPLE), // still until settled
        Quiet(secs(9.0)),           // "sit upright" (4 s), then 5 s without a pose
        Wobble(0.0, 30.0, secs(4.5)),
        Poses(0.0, secs(20.0)),
    ];
    let sim = Sim {
        player: PlayerSim {
            prompt: secs(4.0),
            ..PlayerSim::default()
        },
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(
        run.played(),
        [Prompt(SitUpright), Prompt(MoveHead), Prompt(Calibrated)]
    );
}

// Taking the earbuds off reaches the session as tracking unavailable, like any interruption:
// monitoring pauses, and putting them back on recalibrates.
#[tokio::test(start_paused = true)]
async fn taking_earbuds_off_pauses_then_recalibrates() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Quiet(secs(10.0)),
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.events,
        [
            "TrackingAvailable",
            "TrackingUnavailable",
            "TrackingAvailable"
        ]
    );
    assert_eq!(run.count(Prompt(SitUpright)), 2, "{:?}", run.played());
    // Both calibrations finish: the first before tracking stops, the second after it resumes.
    assert_eq!(run.count(Prompt(Calibrated)), 2, "{:?}", run.played());
    assert_eq!(run.reminders(), [], "no reminder from the stale slouch");
}

// The slouch starts at about STILL + 4 s. Tracking becomes unavailable 2 s later, before the tone
// is due 10 s into the slouch. No reminder may fire from that stale posture. When tracking resumes, the
// session calibrates again, however short the gap.
#[tokio::test(start_paused = true)]
async fn unavailability_drops_the_slouch_and_recalibrates() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        Poses(40.0, secs(2.0)),
        unavailable(),
        Quiet(secs(20.0)),
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.events,
        [
            "TrackingAvailable",
            "TrackingUnavailable",
            "TrackingAvailable"
        ]
    );
    let prompts = run.count(Prompt(SitUpright));
    assert_eq!(
        prompts,
        2,
        "recalibrates after resuming: {:?}",
        run.played()
    );
    // Both calibrations finish: the first before tracking stops, the second after it resumes.
    assert_eq!(run.count(Prompt(Calibrated)), 2, "{:?}", run.played());
    assert_eq!(run.reminders(), [], "no reminder from the stale slouch");
}

// A still head sends no poses. When tracking resumes, headup asks the user to sit upright once the
// head has held still for 2 s (here at once: the silence means it is still). With no pose 5 s
// after the prompt, it asks them to move a little. It then calibrates on the poses that follow.
// Moving, then holding still, does not count as "you moved".
#[tokio::test(start_paused = true)]
async fn still_head_is_asked_to_move_when_tracking_resumes() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Quiet(secs(10.0)),
        Ev(Event::Available),
        Quiet(secs(8.0)),
        Wobble(0.0, 20.0, secs(1.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(
        run.played(),
        [
            Prompt(SitUpright),
            Prompt(Calibrated),
            Prompt(SitUpright),
            Prompt(MoveHead),
            Prompt(Calibrated)
        ]
    );
}

// The same after putting the earbuds back on: the calibration does not depend on the tracker
// sending poses by itself.
#[tokio::test(start_paused = true)]
async fn still_head_is_asked_to_move_after_rewearing() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Quiet(secs(5.0)),
        Ev(Event::Available),
        Quiet(secs(8.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.count(Prompt(MoveHead)), 1, "{:?}", run.played());
    assert_eq!(run.count(Prompt(Calibrated)), 2, "{:?}", run.played());
}

// The nudge repeats while nothing arrives.
#[tokio::test(start_paused = true)]
async fn nudge_repeats_while_no_pose_arrives() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Ev(Event::Available),
        Quiet(secs(38.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    // The prompt comes 2 s after tracking resumes; nudges at 5, 20 and 35 s after it.
    assert_eq!(run.count(Prompt(MoveHead)), 3, "{:?}", run.played());
}

// At startup no pose has arrived yet, so silence may mean tracking has not started (e.g. a
// device precondition is not met). headup waits without prompting or nudging.
#[tokio::test(start_paused = true)]
async fn startup_waits_silently_for_tracking() {
    let script = vec![
        Quiet(secs(30.0)),
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.played(), [Prompt(SitUpright), Prompt(Calibrated)]);
}

// Poses can still arrive after the tracker becomes unavailable (data measured before it, or
// trailing data). They do not start a calibration: that waits for `Available`.
#[tokio::test(start_paused = true)]
async fn poses_after_unavailability_do_not_start_calibration() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Poses(0.0, secs(1.0)),
        Quiet(secs(40.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["TrackingAvailable", "TrackingUnavailable"]);
    assert_eq!(run.played(), [Prompt(SitUpright), Prompt(Calibrated)]);
}

#[tokio::test(start_paused = true)]
async fn failed_calibration_retries_later() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + SAMPLE), // still until settled: "sit upright"
        Wobble(0.0, 30.0, secs(30.0)), // keeps moving through all 4 attempts
        Quiet(CALIBRATION_RETRY + secs(1.0)),
        Poses(0.0, secs(4.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.events, ["TrackingAvailable", "CalibrationFailed"]);
    let expected: Vec<Sound> = [Prompt(SitUpright)]
        .into_iter()
        .chain([Prompt(Moved); CALIBRATION_TRIES])
        // The retry prompts at once; the head is still and silent, so it is asked to move.
        .chain([Prompt(SitUpright), Prompt(MoveHead), Prompt(Calibrated)])
        .collect();
    assert_eq!(run.played(), expected);
}

// Tracking resumes while the user is still on the move (a device can stop tracking while the
// user moves around). "Sit upright" waits until the head has held still for 2 s: here the head
// moves for 5 s after tracking resumes at STILL + 4 s.
#[tokio::test(start_paused = true)]
async fn prompt_after_tracking_resumes_waits_until_the_head_is_still() {
    let script = vec![
        Ev(Event::Available),
        Poses(0.0, STILL + secs(4.0)),
        unavailable(),
        Ev(Event::Available),
        Wobble(0.0, 20.0, secs(5.0)),
        Poses(0.0, secs(8.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    let prompts = run.times(Prompt(SitUpright));
    assert_eq!(prompts.len(), 2, "{:?}", run.played());
    assert!(
        (STILL + secs(11.0)..STILL + secs(11.3)).contains(&prompts[1]),
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
        Ev(Event::Available),
        Poses(0.0, secs(19.0)),
        Poses(40.0, secs(9.0)),
        Poses(0.0, secs(20.0)),
    ];
    let sim = Sim {
        player: PlayerSim {
            prompt: secs(15.0),
            ..PlayerSim::default()
        },
        ..Sim::default()
    };
    let run = simulate(script, sim).await;
    assert_eq!(run.played(), [Prompt(SitUpright), Prompt(Calibrated)]);
}

// At startup the tracker says tracking is unavailable (a device precondition), then available,
// while the user is still moving (putting the earbuds in, say). "Sit upright" waits until the
// head has held still for STILL, as after any interruption: the head moves from 10 s to 13 s.
#[tokio::test(start_paused = true)]
async fn startup_waits_for_a_still_head() {
    let script = vec![
        unavailable(),
        Quiet(secs(10.0)),
        Ev(Event::Available),
        Wobble(0.0, 20.0, secs(3.0)),
        Poses(0.0, secs(8.0)),
    ];
    let run = simulate(script, Sim::default()).await;
    let prompt = run.first(Prompt(SitUpright));
    assert!(
        (secs(13.0) + STILL..secs(13.3) + STILL).contains(&prompt),
        "prompt at {prompt:?}"
    );
}

// Until the tracker reports `Available`, the session does not judge the head: poses alone do not
// start tracking (tracker contract, rule 1).
#[tokio::test(start_paused = true)]
async fn poses_before_available_do_not_start_tracking() {
    let script = vec![Poses(0.0, secs(4.0)), Quiet(secs(20.0))];
    let run = simulate(script, Sim::default()).await;
    assert_eq!(run.played(), []);
}
