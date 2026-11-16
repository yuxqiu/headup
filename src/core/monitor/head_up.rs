//! The head-up: a head movement that re-anchors upright for trackers whose pose drifts.
//!
//! From where the head last held still, it goes down at least `travel_deg`, comes up at least
//! `travel_deg` from its deepest point, and holds still (`Settle`). Where it holds is the new
//! upright, above or below the old one. After looking down, people look back up at what they work
//! on, so that pose is their upright; a reading elsewhere is accumulated tracker error. The same
//! move lets a user re-anchor on purpose: look down, then look up to the posture they want and
//! hold it.
//!
//! It is judged on the unsmoothed pitch alone, without upright or the posture. Holding still
//! while down does not cancel it. Looking up from rest is no head-up, because no descent comes
//! first; nor is coming back from a quick glance up, because the descent starts only where the
//! head held still.
//!
//! [`HeadUp::pose`] takes each pitch and [`HeadUp::check`] judges the head at a time, as the
//! monitor runs them; silence means the pitch is unchanged, so a head-up can end in silence, at
//! the time [`HeadUp::next_end`] gives.

use std::collections::VecDeque;
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
    /// No descent yet. `held` is the pitch the head last held still at.
    Level { held: f64 },
    /// The head went down at least `travel_deg` from where it held; `deepest` is the deepest point
    /// (the largest pitch) since.
    Down { deepest: f64 },
}

impl HeadUp {
    /// Watch for head-ups of `travel_deg`, starting with the head held still at pitch `held`.
    pub fn new(travel_deg: f64, held: f64) -> Self {
        HeadUp {
            travel_deg,
            settle: Settle::default(),
            phase: Phase::Level { held },
        }
    }

    /// A pitch measured at `t`. Times must not go backwards.
    pub fn pose(&mut self, t: Duration, pitch: f64) {
        self.settle.pitch(t, pitch);
        self.phase = match self.phase {
            Phase::Level { held } if pitch - held >= self.travel_deg => {
                Phase::Down { deepest: pitch }
            }
            Phase::Level { held } => Phase::Level { held },
            Phase::Down { deepest } => Phase::Down {
                deepest: deepest.max(pitch),
            },
        };
    }

    /// Judge the head at `t`, after passing the pitches that have arrived. If a head-up ended by
    /// then, returns when the head settled and the pitch it holds. A hold before the descent
    /// becomes where the descent starts; a hold while down, short of coming up `travel_deg`,
    /// changes nothing. After a head-up, the descent starts again where the head holds, so one
    /// head-up counts once.
    pub fn check(&mut self, t: Duration) -> Option<(Duration, f64)> {
        let (at, held) = self.settle.settles().filter(|&(at, _)| at <= t)?;
        match self.phase {
            Phase::Down { deepest } if deepest - held < self.travel_deg => None,
            Phase::Down { .. } => {
                self.phase = Phase::Level { held };
                Some((at, held))
            }
            Phase::Level { .. } => {
                self.phase = Phase::Level { held };
                None
            }
        }
    }

    /// When a head-up ends if no new pitch arrives: the head is up far enough and settles there.
    /// `None` if it would not.
    pub fn next_end(&self) -> Option<Duration> {
        let Phase::Down { deepest } = self.phase else {
            return None;
        };
        let (at, held) = self.settle.settles()?;
        (deepest - held >= self.travel_deg).then_some(at)
    }
}

/// How long the pitch must stay within `SETTLE_RANGE_DEG` for the head to have settled (see
/// `Settle`). Chosen in DECISIONS "Re-anchor upright after each head-up" and checked by replaying
/// data/slouch_guided.csv; the values themselves were not measured against alternatives
/// \[unmeasured\].
const SETTLE: Duration = Duration::from_secs(1);
/// The largest spread (degrees) of pitch while the head has settled (see SETTLE).
const SETTLE_RANGE_DEG: f64 = 2.0;

/// When the head settles: the first time its pitch has stayed within SETTLE_RANGE_DEG for SETTLE.
/// Between pitches, and after the last, the head stays where the last one put it.
#[derive(Default)]
struct Settle {
    /// (measurement time, pitch), oldest first: the pitches of the last SETTLE before the latest,
    /// and the one before them, where the head was as that stretch began.
    pitches: VecDeque<(Duration, f64)>,
}

impl Settle {
    fn pitch(&mut self, measured: Duration, pitch: f64) {
        self.pitches.push_back((measured, pitch));
        let start = measured.saturating_sub(SETTLE);
        while self.pitches.get(1).is_some_and(|&(m, _)| m <= start) {
            self.pitches.pop_front();
        }
    }

    /// When the head settles at its latest pitch if no new one arrives, and that pitch; `None`
    /// before the first. The time is earlier than the latest pitch's if it already had.
    fn settles(&self) -> Option<(Duration, f64)> {
        let &(_, latest) = self.pitches.back()?;
        let (mut lo, mut hi) = (latest, latest);
        let mut since = Duration::MAX;
        for &(m, p) in self.pitches.iter().rev() {
            (lo, hi) = (lo.min(p), hi.max(p));
            if hi - lo >= SETTLE_RANGE_DEG {
                break;
            }
            since = m;
        }
        Some((since + SETTLE, latest))
    }
}

#[cfg(test)]
#[path = "tests/head_up.rs"]
mod tests;
