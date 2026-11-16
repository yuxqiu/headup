//! Monitoring posture after calibration. Four parts, each answering one question:
//!
//! - the private `posture`: is the head slouched, against a given upright?
//! - the private `head_up`: did a head-up just end, and where? (trackers whose pose drifts only)
//! - the private `reminders`: what to play during a slouch, and when?
//! - [`Monitor`] itself: where upright is, and the order things happen in.
//!
//! [`Monitor`] is a decision type without I/O, like [`super::calibration::Calibration`]: the
//! session passes in each pose and then the time, and gets back what to do, in order. Upright is
//! the calibrated pose, with its pitch moved to where each head-up ends; the posture is judged
//! against it, so moving it ends a slouch at the next judgement, even while the tracker is silent.

mod head_up;
mod posture;
mod reminders;

use super::quat::{Euler, Quat};
use super::sound::Cue;
use super::tracker::TrackerTraits;
use head_up::HeadUp;
use posture::{Change, Config, Posture};
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
    /// The calibrated upright orientation; angles are relative to it.
    baseline: Quat,
    /// The pitch (relative to calibration) taken as upright: 0 until a head-up moves it.
    upright: f64,
    posture: Posture,
    /// Present when the tracker's pose drifts.
    head_up: Option<HeadUp>,
    reminders: Reminders,
    /// What to do, in order, since the last [`Monitor::check`].
    outputs: Vec<Output>,
}

impl Monitor {
    /// Start monitoring against the calibrated `baseline`, with the corrections `traits` asks for.
    pub fn new(baseline: Quat, traits: TrackerTraits) -> Self {
        let config = Config::default();
        Monitor {
            baseline,
            upright: 0.0,
            posture: Posture::new(config),
            // A head-up goes as far down, and then up, as a slouch starts (DECISIONS "Re-anchor
            // upright after each head-up").
            head_up: traits.pose_drifts.then(|| HeadUp::new(config.on_deg)),
            reminders: Reminders::new(Schedule::default()),
            outputs: Vec::new(),
        }
    }

    /// A pose measured at `measured` (session time; not earlier than the previous pose's). Returns
    /// its angles relative to calibration, for the log. It is judged at its own time; what it
    /// causes comes out of the next [`Monitor::check`].
    pub fn pose(&mut self, measured: Duration, orientation: Quat) -> Euler {
        let angles = (self.baseline.conj() * orientation).euler();
        self.posture.pose(measured, angles.pitch);
        if let Some(head_up) = &mut self.head_up {
            head_up.pose(measured, angles.pitch);
        }
        self.judge(measured);
        angles
    }

    /// Judge up to `now`, and return what happened since the last check, in order. Pass the poses
    /// that have arrived first: silence then means the head has not moved, and a pose arriving
    /// later is judged from its own time (tracker contract, "Late poses").
    pub fn check(&mut self, now: Duration) -> Vec<Output> {
        self.judge(now);
        std::mem::take(&mut self.outputs)
    }

    /// Judge the head at `t`: first the reminders due by then, then a head-up that ends there
    /// (which moves upright), then the posture against upright.
    fn judge(&mut self, t: Duration) {
        let due = self.reminders.due(t);
        self.outputs.extend(due.into_iter().map(Output::Remind));
        if let Some(held) = self.head_up.as_mut().and_then(|head_up| head_up.check(t)) {
            self.upright = held;
            self.outputs.push(Output::Reanchored { upright: held });
        }
        match self.posture.judge(self.upright) {
            Some(Change::SlouchStarted) => self.reminders.slouch_started(t),
            Some(Change::Recovered) => {
                let tink = self.reminders.recovered();
                self.outputs.extend(tink.map(Output::Remind));
            }
            None => {}
        }
    }

    /// When the next reminder falls due if the slouch goes on: check by then for it to be on
    /// time. `None` while not slouching.
    pub fn next_due(&self) -> Option<Duration> {
        self.reminders.next_due()
    }

    pub fn status(&self) -> Status {
        Status {
            pitch: self.posture.smoothed(self.upright),
            slouching: self.posture.slouching(),
        }
    }
}

#[cfg(test)]
#[path = "tests/monitor.rs"]
mod tests;
