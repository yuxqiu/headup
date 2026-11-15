//! Monitoring posture after calibration: whether the user slouches (the private `detector`) and
//! when to remind them (the private `reminders`).
//!
//! [`Monitor`] is a decision type without I/O, like [`super::calibration::Calibration`]: the
//! session passes in each pose and then the time, and gets back what to do, in order. It wires the
//! detector and the reminders together, so the session cannot get their order wrong.

mod detector;
mod reminders;

use super::quat::{Euler, Quat};
use super::sound::Cue;
use super::tracker::TrackerTraits;
use detector::{Config, Detector, PostureEvent};
use reminders::{Reminders, Schedule};
use std::time::Duration;

/// What the session should do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Output {
    /// Log the reminder and play it.
    Remind(Cue),
    /// Upright was re-anchored to this pitch (relative to calibration): log it.
    Reanchored { upright: f64 },
}

/// What the live status line shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    /// Smoothed pitch relative to upright; `None` before the first pose.
    pub pitch: Option<f64>,
    pub slouching: bool,
}

/// Posture monitoring for one stretch of tracking, from a calibration until tracking stops.
pub struct Monitor {
    /// The calibrated upright orientation.
    baseline: Quat,
    detector: Detector,
    reminders: Reminders,
}

impl Monitor {
    /// Start monitoring against the calibrated `baseline`, with the corrections `traits` asks for.
    pub fn new(baseline: Quat, traits: TrackerTraits) -> Self {
        let config = Config {
            reanchor_after_recovery: traits.pose_drifts_up,
            ..Config::default()
        };
        Monitor {
            baseline,
            detector: Detector::new(config),
            reminders: Reminders::new(Schedule::default()),
        }
    }

    /// A pose measured at `measured` (session time; not earlier than the previous pose's). Returns
    /// its angles relative to calibration, for the log. What it causes comes out of the next
    /// [`Monitor::check`].
    pub fn pose(&mut self, measured: Duration, orientation: Quat) -> Euler {
        let angles = (self.baseline.conj() * orientation).euler();
        self.detector.pose(measured, angles.pitch);
        angles
    }

    /// Judge up to `now`, and return what happened since the last check, in order. Pass the poses
    /// that have arrived first: silence then means the head has not moved, and a pose arriving
    /// later is judged from its own time (tracker contract, "Late poses"). Reminders that fell due
    /// before a posture change come before it.
    pub fn check(&mut self, now: Duration) -> Vec<Output> {
        let mut outputs = Vec::new();
        for (at, event) in self.detector.check(now) {
            outputs.extend(self.reminders.due(at).into_iter().map(Output::Remind));
            match event {
                PostureEvent::SlouchStarted => self.reminders.slouch_started(at),
                PostureEvent::Recovered => {
                    outputs.extend(self.reminders.recovered().map(Output::Remind))
                }
                PostureEvent::Reanchored { upright } => {
                    outputs.push(Output::Reanchored { upright })
                }
            }
        }
        outputs.extend(self.reminders.due(now).into_iter().map(Output::Remind));
        outputs
    }

    /// When the next reminder falls due if the slouch goes on: check by then for it to be on
    /// time. `None` while not slouching.
    pub fn next_due(&self) -> Option<Duration> {
        self.reminders.next_due()
    }

    pub fn status(&self) -> Status {
        Status {
            pitch: self.detector.smoothed(),
            slouching: self.detector.slouching(),
        }
    }
}

#[cfg(test)]
#[path = "tests/monitor.rs"]
mod tests;
