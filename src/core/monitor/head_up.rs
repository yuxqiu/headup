//! The head-up: a head movement that re-anchors upright for trackers whose pose drifts.
//!
//! The head goes down at least `travel_deg` from where it was, comes up at least `travel_deg` from
//! its deepest point, and holds still (`Settle`). Where it holds is the new upright, above or
//! below the old one. After looking down, people look back up at what they work on, so that pose
//! is their upright; a reading elsewhere is accumulated tracker error. The same move lets a user
//! re-anchor on purpose: look down, then look up to the posture they want and hold it.
//!
//! It is judged on the unsmoothed pitch alone, without upright or the posture. Holding still
//! while down does not cancel it. Looking up from rest is no head-up, because no descent comes
//! first.
//!
//! [`HeadUp::pose`] takes each pitch and [`HeadUp::check`] judges the head at a time, as the
//! monitor runs them; silence means the pitch is unchanged.

use crate::core::steady::Window;
use std::time::Duration;

/// Watches the pitch (relative to calibration; larger is further down) for head-ups.
pub struct HeadUp {
    /// How far the head must go down, and then come up.
    travel_deg: f64,
    settle: Settle,
    phase: Phase,
}

/// How far a head-up has got.
#[derive(Clone, Copy)]
enum Phase {
    /// No descent yet. `top` is the highest point (the smallest pitch) since the head last held
    /// still; `None` before the first pitch.
    Level { top: Option<f64> },
    /// The head went down at least `travel_deg` from its top; `deepest` is the deepest point
    /// (the largest pitch) since.
    Down { deepest: f64 },
}

impl HeadUp {
    pub fn new(travel_deg: f64) -> Self {
        HeadUp {
            travel_deg,
            settle: Settle::new(),
            phase: Phase::Level { top: None },
        }
    }

    /// A pitch measured at `t`. Times must not go backwards.
    pub fn pose(&mut self, t: Duration, pitch: f64) {
        self.settle.pitch(t, pitch);
        self.phase = match self.phase {
            Phase::Level { top } => {
                let top = top.map_or(pitch, |top| top.min(pitch));
                if pitch - top >= self.travel_deg {
                    Phase::Down { deepest: pitch }
                } else {
                    Phase::Level { top: Some(top) }
                }
            }
            Phase::Down { deepest } => Phase::Down {
                deepest: deepest.max(pitch),
            },
        };
    }

    /// Judge the head at `t`, after passing the pitches that have arrived. If it holds still at
    /// the end of a head-up, returns the pitch it holds. A hold before the descent moves the top
    /// there; a hold while down, short of coming up `travel_deg`, changes nothing. After a
    /// head-up, the top starts again where the head holds, so one head-up counts once.
    pub fn check(&mut self, t: Duration) -> Option<f64> {
        let held = self.settle.held(t)?;
        match self.phase {
            Phase::Down { deepest } if deepest - held < self.travel_deg => None,
            Phase::Down { .. } => {
                self.phase = Phase::Level { top: Some(held) };
                Some(held)
            }
            Phase::Level { .. } => {
                self.phase = Phase::Level { top: Some(held) };
                None
            }
        }
    }
}

/// How long the pitch must stay within `SETTLE_RANGE_DEG` for the head to have settled (see
/// [`Settle`]). Chosen in DECISIONS "Re-anchor upright after each recovery", kept by "Re-anchor
/// upright after each head-up", and checked by replaying data/slouch_guided.csv; the values
/// themselves were not measured against alternatives \[unmeasured\].
const SETTLE: Duration = Duration::from_secs(1);
/// The largest spread (degrees) of pitch while the head has settled (see SETTLE).
const SETTLE_RANGE_DEG: f64 = 2.0;

/// Whether the head has settled at some pitch: the pitch has stayed within SETTLE_RANGE_DEG for
/// the last SETTLE, judged over a continuous stream of pitches (silence counts as unchanged).
struct Settle(Window<f64>);

impl Settle {
    fn new() -> Self {
        Settle(Window::new(Duration::ZERO))
    }

    fn pitch(&mut self, measured: Duration, pitch: f64) {
        self.0.add(measured, pitch);
    }

    /// The pitch the head holds at `now`, if it has settled, after passing the pitches that have
    /// arrived (tracker contract, "Late poses").
    fn held(&mut self, now: Duration) -> Option<f64> {
        let span = self.0.span(now, SETTLE)?;
        let (lo, hi) = span
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
                (lo.min(x), hi.max(x))
            });
        (hi - lo < SETTLE_RANGE_DEG)
            .then(|| *span.last().expect("a span holds at least one sample"))
    }
}

#[cfg(test)]
#[path = "tests/head_up.rs"]
mod tests;
