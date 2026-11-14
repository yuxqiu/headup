//! Whether the head holds steady over the last stretch of time.
//!
//! A `Window` keeps the samples measured since a start and gives the ones that cover the last
//! stretch of time. Between samples the head stays where the last one put it: a tracker with
//! nothing to deliver means a head that has not moved. Each judgement of steadiness is a criterion
//! over such a span: calibration's, [`Stillness`] before a prompt, and the detector's settling
//! after a recovery.

use super::quat::Quat;
use std::collections::VecDeque;
use std::time::Duration;

/// The samples measured since `start`, oldest first. A sample measured before `start` does not
/// count: those measured during a prompt, for example, are delivered after it.
pub(crate) struct Window<T> {
    start: Duration,
    /// Samples (measurement time, value). The first is the latest that can still begin a span.
    samples: VecDeque<(Duration, T)>,
    /// When the first sample since `start` was measured.
    first: Option<Duration>,
}

impl<T: Copy> Window<T> {
    pub(crate) fn new(start: Duration) -> Self {
        Window {
            start,
            samples: VecDeque::new(),
            first: None,
        }
    }

    pub(crate) fn add(&mut self, measured: Duration, x: T) {
        if measured >= self.start {
            self.first.get_or_insert(measured);
            self.samples.push_back((measured, x));
        }
    }

    /// When the first sample since the start was measured.
    pub(crate) fn first(&self) -> Option<Duration> {
        self.first
    }

    /// Whether `len` has passed since the start with no sample: the head held still that long.
    pub(crate) fn silent_for(&self, now: Duration, len: Duration) -> bool {
        self.first.is_none() && now >= self.start + len
    }

    /// The samples that cover the `len` before `now`, if samples do: the latest measured at least
    /// `len` before `now` (the head stayed there until the next one) and every one after it.
    pub(crate) fn span(&mut self, now: Duration, len: Duration) -> Option<Vec<T>> {
        let from = now.checked_sub(len)?;
        let begin = self.samples.iter().rposition(|&(m, _)| m <= from)?;
        self.samples.drain(..begin);
        Some(self.samples.iter().map(|&(_, x)| x).collect())
    }
}

/// How long the head must hold still, once head tracking resumes, before calibration asks the user
/// to sit upright.
const STILL: Duration = Duration::from_secs(2);
/// The largest angle (degrees) from their mean that poses may have while the head holds still.
const STILL_MAX_DEG: f64 = 2.0;

/// Whether the head holds still, judged on the poses since the wait started. It does once its
/// posture has stayed within STILL_MAX_DEG of the mean for STILL, or nothing has arrived for STILL.
pub struct Stillness(Window<Quat>);

impl Stillness {
    /// Start waiting at `now`.
    pub fn new(now: Duration) -> Self {
        Stillness(Window::new(now))
    }

    pub fn pose(&mut self, measured: Duration, q: Quat) {
        self.0.add(measured, q);
    }

    /// Whether the head has held still up to `now`. Every pose measured before `now` must have
    /// been passed to [`Stillness::pose`].
    pub fn check(&mut self, now: Duration) -> bool {
        if self.0.first().is_none() {
            return self.0.silent_for(now, STILL);
        }
        let Some(span) = self.0.span(now, STILL) else {
            return false;
        };
        let mean = Quat::mean(&span);
        span.iter().all(|q| q.angle_to(mean) <= STILL_MAX_DEG)
    }
}

#[cfg(test)]
#[path = "tests/steady.rs"]
mod tests;
