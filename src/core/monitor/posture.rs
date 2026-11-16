//! Whether the head is slouched (design: docs/UX_RESEARCH.md). When to remind the user during a
//! slouch is [`super::reminders`]; where upright is, the monitor knows.
//!
//! The input is forward pitch in degrees, relative to the calibrated upright pose; looking down is
//! positive. [`Posture::pose`] smooths each sample, in that fixed frame. [`Posture::judge`] then
//! compares the smoothed pitch with the upright it is given (also relative to calibration): a
//! slouch starts when the head is `on_deg` below upright and ends when it comes back within
//! `off_deg`. Moving upright needs no step of its own: the next judgement uses the new upright.
//! Smoothing works on time, not on sample count, so it behaves the same at any sample rate.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Time constant of the exponential smoothing of pitch.
    pub smoothing: Duration,
    /// A slouch starts when the smoothed pitch (relative to upright) reaches this.
    pub on_deg: f64,
    /// A slouch ends when the smoothed pitch drops below this.
    pub off_deg: f64,
}

/// A change of posture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    SlouchStarted,
    /// The slouch ended: the user sat back up, or upright moved to the head.
    Recovered,
}

impl Default for Config {
    fn default() -> Self {
        // The design approved in docs/UX_RESEARCH.md (DECISIONS "Detector behaviour"): on at 20°,
        // off below 15°.
        Config {
            // Time constant of the smoothing (DECISIONS "Posture logic redesign").
            smoothing: Duration::from_millis(123),
            on_deg: 20.0,
            off_deg: 15.0,
        }
    }
}

/// Exponential smoothing with a time constant, so it behaves the same at any sample rate.
struct Smoother {
    tau: Duration,
    /// The smoothed value and when it was last updated.
    last: Option<(Duration, f64)>,
}

impl Smoother {
    fn new(tau: Duration) -> Self {
        Smoother { tau, last: None }
    }

    /// Smooth sample `x` at time `t` (times must not go backwards).
    fn smooth(&mut self, t: Duration, x: f64) {
        let s = match self.last {
            None => x,
            Some((prev_t, prev)) => {
                let dt = t
                    .checked_sub(prev_t)
                    .unwrap_or_else(|| panic!("pose times went backwards: {t:?} after {prev_t:?}"));
                let weight = 1.0 - (-dt.as_secs_f64() / self.tau.as_secs_f64()).exp();
                prev + weight * (x - prev)
            }
        };
        self.last = Some((t, s));
    }

    fn value(&self) -> Option<f64> {
        self.last.map(|(_, s)| s)
    }
}

pub struct Posture {
    cfg: Config,
    /// Pitch relative to calibration, smoothed.
    smoother: Smoother,
    slouching: bool,
}

impl Posture {
    pub fn new(cfg: Config) -> Self {
        assert!(
            cfg.off_deg < cfg.on_deg,
            "off threshold must be below on threshold"
        );
        Posture {
            cfg,
            smoother: Smoother::new(cfg.smoothing),
            slouching: false,
        }
    }

    /// A pitch sample measured at `t`, relative to calibration. Times must not go backwards.
    pub fn pose(&mut self, t: Duration, pitch: f64) {
        self.smoother.smooth(t, pitch);
    }

    /// Judge the smoothed pitch against `upright` (relative to calibration), and return the
    /// change of posture, if any. Nothing before the first sample.
    pub fn judge(&mut self, upright: f64) -> Option<Change> {
        let s = self.smoothed(upright)?;
        let change = match self.slouching {
            false if s >= self.cfg.on_deg => Change::SlouchStarted,
            true if s < self.cfg.off_deg => Change::Recovered,
            _ => return None,
        };
        self.slouching = change == Change::SlouchStarted;
        Some(change)
    }

    /// Smoothed pitch relative to `upright`; `None` before the first sample.
    pub fn smoothed(&self, upright: f64) -> Option<f64> {
        self.smoother.value().map(|s| s - upright)
    }

    pub fn slouching(&self) -> bool {
        self.slouching
    }
}

#[cfg(test)]
#[path = "tests/posture.rs"]
mod tests;
