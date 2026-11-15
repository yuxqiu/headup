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
use detector::{Config, Detector, PostureChange};
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
    /// What happened since the last [`Monitor::check`], in order.
    outputs: Vec<Output>,
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
            outputs: Vec::new(),
        }
    }

    /// A pose measured at `measured` (session time; not earlier than the previous pose's). Returns
    /// its angles relative to calibration, for the log. What it causes comes out of the next
    /// [`Monitor::check`].
    pub fn pose(&mut self, measured: Duration, orientation: Quat) -> Euler {
        let angles = (self.baseline.conj() * orientation).euler();
        let update = self.detector.update(measured, angles.pitch);
        self.reanchored(update.reanchored);
        match update.posture {
            Some(PostureChange::SlouchStarted) => self.reminders.slouch_started(measured),
            Some(PostureChange::Recovered) => {
                let tink = self.reminders.recovered();
                self.remind(tink);
            }
            None => {}
        }
        let due = self.reminders.due(measured);
        self.remind(due);
        angles
    }

    /// Judge up to `now`, and return what happened since the last check, in order. Every pose
    /// measured before `now` must have been passed to [`Monitor::pose`]: silence then means the
    /// head has not moved.
    pub fn check(&mut self, now: Duration) -> Vec<Output> {
        let reanchored = self.detector.tick(now);
        self.reanchored(reanchored);
        let due = self.reminders.due(now);
        self.remind(due);
        std::mem::take(&mut self.outputs)
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

    fn remind(&mut self, cues: impl IntoIterator<Item = Cue>) {
        self.outputs.extend(cues.into_iter().map(Output::Remind));
    }

    fn reanchored(&mut self, upright: Option<f64>) {
        self.outputs
            .extend(upright.map(|upright| Output::Reanchored { upright }));
    }
}

#[cfg(test)]
#[path = "tests/monitor.rs"]
mod tests;
