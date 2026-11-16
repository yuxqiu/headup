//! Whether the head is slouched (design: docs/UX_RESEARCH.md). When to remind the user during a
//! slouch is [`super::reminders`]; where upright is, the monitor knows.
//!
//! The input is forward pitch in degrees, relative to the calibrated upright pose; looking down is
//! positive. [`Posture::pose`] smooths each sample, in that fixed frame. [`Posture::judge`] then
//! compares the smoothed pitch at a time with the upright it is given (also relative to
//! calibration): a slouch starts when the head is `ON_DEG` below upright and ends when it comes
//! back within `OFF_DEG`. Moving upright needs no step of its own: the next judgement uses the new upright.
//! Smoothing works on time, not on sample count, so it behaves the same at any sample rate.

use std::time::Duration;

/// A slouch starts when the smoothed pitch (relative to upright) reaches this, and ends when it
/// drops below `OFF_DEG`. The design approved in docs/UX_RESEARCH.md (DECISIONS "Detector
/// behaviour").
pub const ON_DEG: f64 = 20.0;
/// See `ON_DEG`.
const OFF_DEG: f64 = 15.0;
const _: () = assert!(OFF_DEG < ON_DEG, "a slouch ends below where it starts");
/// Time constant of the smoothing (DECISIONS "Posture logic redesign").
const SMOOTHING: Duration = Duration::from_millis(123);

/// A change of posture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    SlouchStarted,
    /// The slouch ended: the user sat back up, or upright moved to the head.
    Recovered,
}

/// Exponential smoothing with the time constant `SMOOTHING`, so it behaves the same at any sample
/// rate. Each sample stands for the head from the previous sample's time to its own, and after
/// the last sample the head stays there (silence means unchanged).
#[derive(Default)]
struct Smoother {
    last: Option<Smoothed>,
}

#[derive(Clone, Copy)]
struct Smoothed {
    /// When the last sample was measured.
    t: Duration,
    /// The smoothed value at `t`.
    value: f64,
    /// The last sample, where the head stays until the next one.
    sample: f64,
}

/// How far a smoothed value moves towards a held sample in `dt`.
fn weight(dt: Duration) -> f64 {
    1.0 - (-dt.as_secs_f64() / SMOOTHING.as_secs_f64()).exp()
}

impl Smoother {
    /// Smooth sample `x` at time `t` (times must not go backwards).
    fn smooth(&mut self, t: Duration, x: f64) {
        let value = match self.last {
            None => x,
            Some(last) => {
                let dt = t.checked_sub(last.t).unwrap_or_else(|| {
                    panic!("pose times went backwards: {t:?} after {:?}", last.t)
                });
                last.value + weight(dt) * (x - last.value)
            }
        };
        self.last = Some(Smoothed {
            t,
            value,
            sample: x,
        });
    }

    /// The smoothed value at `t`, with the head at the last sample since then; `None` before the
    /// first sample. A time before the last sample gives the value at the last sample.
    fn at(&self, t: Duration) -> Option<f64> {
        let last = self.last?;
        let dt = t.saturating_sub(last.t);
        Some(last.value + weight(dt) * (last.sample - last.value))
    }
}

#[derive(Default)]
pub struct Posture {
    /// Pitch relative to calibration, smoothed.
    smoother: Smoother,
    slouching: bool,
}

impl Posture {
    /// A pitch sample measured at `t`, relative to calibration. Times must not go backwards.
    pub fn pose(&mut self, t: Duration, pitch: f64) {
        self.smoother.smooth(t, pitch);
    }

    /// Judge the smoothed pitch at `t` against `upright` (relative to calibration), and return the
    /// change of posture, if any. Nothing before the first sample.
    pub fn judge(&mut self, t: Duration, upright: f64) -> Option<Change> {
        let s = self.smoothed(t, upright)?;
        let change = match self.slouching {
            false if s >= ON_DEG => Change::SlouchStarted,
            true if s < OFF_DEG => Change::Recovered,
            _ => return None,
        };
        self.slouching = change == Change::SlouchStarted;
        Some(change)
    }

    /// Smoothed pitch at `t`, relative to `upright`; `None` before the first sample.
    pub fn smoothed(&self, t: Duration, upright: f64) -> Option<f64> {
        self.smoother.at(t).map(|s| s - upright)
    }

    pub fn slouching(&self) -> bool {
        self.slouching
    }
}

#[cfg(test)]
#[path = "tests/posture.rs"]
mod tests;
