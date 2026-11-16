//! Monitoring posture after calibration. Four parts, each answering one question:
//!
//! - the private `posture`: is the head slouched, against a given upright?
//! - the private `head_up`: did a head-up just end, and where? (trackers whose pose drifts only)
//! - the private `reminders`: what to play during a slouch, and when?
//! - [`Monitor`] itself: where upright is, and the order things happen in.
//!
//! [`Monitor`] is a decision type without I/O, like [`super::calibration::Calibration`]: the
//! session passes in each pose and then the time, and gets back what to do, in order. Upright is
//! the calibrated pose, with its pitch moved to where each head-up ends. Everything happens at its
//! own time: each pose, a head-up when the head settled, and, in silence, the head staying at its
//! last pose. So moving upright ends a slouch when the head-up ends, even while the tracker is
//! silent.

mod head_up;
mod posture;
mod reminders;

use super::quat::{Euler, Quat};
use super::sound::Cue;
use super::tracker::TrackerTraits;
use head_up::HeadUp;
use posture::{Change, ON_DEG, Posture};
use reminders::Reminders;
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
    /// The latest time judged, for the status line.
    judged: Duration,
}

impl Monitor {
    /// Start monitoring against the calibrated `baseline`, with the corrections `traits` asks for.
    pub fn new(baseline: Quat, traits: TrackerTraits) -> Self {
        Monitor {
            baseline,
            upright: 0.0,
            posture: Posture::default(),
            // A head-up goes as far down, and then up, as a slouch starts (DECISIONS "Re-anchor
            // upright after each head-up"). Calibration held the head still at upright, pitch 0.
            head_up: traits.pose_drifts.then(|| HeadUp::new(ON_DEG, 0.0)),
            reminders: Reminders::default(),
            outputs: Vec::new(),
            judged: Duration::ZERO,
        }
    }

    /// A pose measured at `measured` (session time; not earlier than the previous pose's). Returns
    /// its angles relative to calibration, for the log. The silence before it, then the pose,
    /// are judged at its own time; what they cause comes out of the next [`Monitor::check`].
    pub fn pose(&mut self, measured: Duration, orientation: Quat) -> Euler {
        let angles = (self.baseline.conj() * orientation).euler();
        self.judge(measured);
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

    /// Judge the head up to `t`. A head-up that ended by then counts at its own time: what was
    /// due before it, then upright moves and the posture is judged against it. Then the rest up to
    /// `t`.
    fn judge(&mut self, t: Duration) {
        if let Some((at, held)) = self.head_up.as_mut().and_then(|head_up| head_up.check(t)) {
            self.advance(at);
            self.upright = held;
            self.outputs.push(Output::Reanchored { upright: held });
            self.judge_posture(at);
        }
        self.advance(t);
    }

    /// The reminders due by `t`, then the posture at `t`.
    fn advance(&mut self, t: Duration) {
        let due = self.reminders.due(t);
        self.outputs.extend(due.into_iter().map(Output::Remind));
        self.judge_posture(t);
        self.judged = self.judged.max(t);
    }

    fn judge_posture(&mut self, t: Duration) {
        match self.posture.judge(t, self.upright) {
            Some(Change::SlouchStarted) => self.reminders.slouch_started(t),
            Some(Change::Recovered) => {
                let tink = self.reminders.recovered();
                self.outputs.extend(tink.map(Output::Remind));
            }
            None => {}
        }
    }

    /// When to check next if the tracker stays silent: when the next reminder falls due if the
    /// slouch goes on, or when a head-up would end, whichever is sooner. `None` if neither.
    pub fn next_due(&self) -> Option<Duration> {
        let head_up = self.head_up.as_ref().and_then(HeadUp::next_end);
        [self.reminders.next_due(), head_up]
            .into_iter()
            .flatten()
            .min()
    }

    pub fn status(&self) -> Status {
        Status {
            pitch: self.posture.smoothed(self.judged, self.upright),
            slouching: self.posture.slouching(),
        }
    }
}

#[cfg(test)]
#[path = "tests/monitor.rs"]
mod tests;
