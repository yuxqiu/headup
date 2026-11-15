//! Calibration: find the upright pose after the "sit upright" prompt.
//!
//! [`Calibration`] makes every decision without I/O. The session feeds it poses and asks it what
//! comes next ([`Calibration::check`]). Between measurements, while the user hears a prompt, it is
//! a [`Paused`] calibration, which resumes measuring when the prompt has ended.
//!
//! The rule: the head is upright once the posture since the prompt has stayed steady for
//! CALIBRATION. The posture holds between poses, because a tracker with nothing to deliver means a
//! head that has not moved. Only poses measured after the prompt count, so the head must be
//! measured after it.
//!
//! The retry policy: an attempt fails ("you moved") CALIBRATION_ATTEMPT after its first pose if
//! the head has not held steady by then. After CALIBRATION_TRIES failed attempts, calibration gives
//! up. A still head may send nothing, so while an attempt has no pose, the user is asked to move a
//! little (NUDGE_AFTER, then every NUDGE_EVERY).

use super::quat::Quat;
use super::steady::Window;
use std::time::Duration;

// Calibration: a 3 s steady capture, p90 under 8°, up to 4 tries (DECISIONS "Detector behaviour",
// from docs/UX_RESEARCH.md). Each attempt fails 5 s after its first pose; with no pose 5 s after
// an attempt starts, a nudge, repeated every 15 s (DECISIONS "Calibration asks a still head to
// move").

/// How long the head must hold steady, after the prompt, to calibrate.
const CALIBRATION: Duration = Duration::from_secs(3);
/// The 90th-percentile angle (degrees) from the mean pose below which the head counts as steady.
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
/// An attempt fails if the head has not held steady for CALIBRATION by this long after the
/// attempt's first pose.
const CALIBRATION_ATTEMPT: Duration = Duration::from_secs(5);
/// Attempts before calibration fails and headup asks again later.
pub(crate) const CALIBRATION_TRIES: usize = 4;
/// With no pose this long after an attempt starts, ask the user to move a little. Ask again every
/// NUDGE_EVERY.
const NUDGE_AFTER: Duration = Duration::from_secs(5);
const NUDGE_EVERY: Duration = Duration::from_secs(15);

/// What comes next, from [`Calibration::check`].
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Keep feeding poses.
    Measuring(Calibration),
    /// No pose has arrived: ask the user to move a little, then resume.
    Nudge(Paused),
    /// An attempt ended. Show `report`, then act on `outcome`.
    AttemptEnded { report: Report, outcome: Outcome },
}

/// How an attempt ended.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The head held steady; this is its mean pose, the upright pose.
    Calibrated(Quat),
    /// The head moved: tell the user, then resume for the next attempt.
    Retry(Paused),
    /// The last attempt failed: tell the user they moved. Calibration is over.
    GaveUp,
}

/// An attempt's numbers, for the console.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// 1 for the first attempt.
    pub attempt: usize,
    /// The span judged last, or `None` if poses never covered CALIBRATION.
    pub spread: Option<Spread>,
}

impl std::fmt::Display for Report {
    /// The console line for the attempt, e.g. "calibration attempt 1: 151 samples, p90 deviation
    /// 0.4°".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "calibration attempt {}: ", self.attempt)?;
        match self.spread {
            Some(s) => write!(f, "{} samples, p90 deviation {:.1}°", s.samples, s.p90),
            None => write!(f, "poses never covered the span"),
        }
    }
}

/// How spread out the poses of a judged span were.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spread {
    /// Poses in the span.
    pub samples: usize,
    /// 90th-percentile angle (degrees) of those poses from their mean.
    pub p90: f64,
}

/// Calibration while it measures: one attempt, from a start.
#[derive(Debug, PartialEq)]
pub struct Calibration {
    /// Attempts started so far, including this one.
    attempts: usize,
    /// The poses measured since the attempt started (or since the nudge ended).
    poses: Window<Quat>,
    /// When to ask the user to move if no pose has come by then.
    nudge_at: Duration,
}

/// Calibration between measurements, while the user hears a prompt. It resumes measuring when the
/// prompt has ended ([`Paused::resume`]); poses measured before then do not count.
#[derive(Debug, PartialEq)]
pub struct Paused {
    attempts: usize,
    then: AfterPrompt,
}

/// What a paused calibration does when it resumes.
#[derive(Debug, PartialEq)]
enum AfterPrompt {
    /// "Move a little" ended: the same attempt goes on.
    SameAttempt,
    /// "You moved" ended: the next attempt starts.
    NextAttempt,
}

impl Calibration {
    /// Start the first attempt at `now`, when the "sit upright" prompt has finished.
    pub fn new(now: Duration) -> Self {
        Calibration::measuring(1, now, NUDGE_AFTER)
    }

    /// Attempt `attempts`, measuring from `now`, asking the user to move if no pose has come
    /// `nudge_in` later.
    fn measuring(attempts: usize, now: Duration, nudge_in: Duration) -> Self {
        Calibration {
            attempts,
            poses: Window::new(now),
            nudge_at: now + nudge_in,
        }
    }

    /// A pose measured at `measured`. A pose measured before the measurement started (during a
    /// prompt) does not count.
    pub fn pose(&mut self, measured: Duration, q: Quat) {
        self.poses.add(measured, q);
    }

    /// Decide at `now`, after passing the poses that have arrived (tracker contract, "Late
    /// poses").
    pub fn check(mut self, now: Duration) -> Next {
        let judged = self.poses.span(now, CALIBRATION).map(|span| {
            let (mean, p90) = mean_and_p90(&span);
            let spread = Spread {
                samples: span.len(),
                p90,
            };
            (mean, spread)
        });
        let report = |spread| Report {
            attempt: self.attempts,
            spread,
        };
        match (judged, self.poses.first()) {
            (Some((mean, spread)), _) if spread.p90 < CALIBRATION_MAX_P90_DEG => {
                ended(report(Some(spread)), Outcome::Calibrated(mean))
            }
            (judged, Some(first)) if now >= first + CALIBRATION_ATTEMPT => {
                let report = report(judged.map(|(_, spread)| spread));
                if self.attempts == CALIBRATION_TRIES {
                    ended(report, Outcome::GaveUp)
                } else {
                    let then = AfterPrompt::NextAttempt;
                    ended(report, Outcome::Retry(self.pause(then)))
                }
            }
            (_, None) if now >= self.nudge_at => Next::Nudge(self.pause(AfterPrompt::SameAttempt)),
            _ => Next::Measuring(self),
        }
    }

    fn pause(self, then: AfterPrompt) -> Paused {
        Paused {
            attempts: self.attempts,
            then,
        }
    }
}

impl Paused {
    /// The prompt ended at `now`: measuring starts afresh.
    pub fn resume(self, now: Duration) -> Calibration {
        match self.then {
            AfterPrompt::SameAttempt => Calibration::measuring(self.attempts, now, NUDGE_EVERY),
            AfterPrompt::NextAttempt => Calibration::measuring(self.attempts + 1, now, NUDGE_AFTER),
        }
    }
}

fn ended(report: Report, outcome: Outcome) -> Next {
    Next::AttemptEnded { report, outcome }
}

/// Mean orientation of `samples` and the 90th-percentile angle (degrees) of samples from it.
fn mean_and_p90(samples: &[Quat]) -> (Quat, f64) {
    let mean = Quat::mean(samples);
    let mut dev: Vec<f64> = samples.iter().map(|q| q.angle_to(mean)).collect();
    dev.sort_by(f64::total_cmp);
    (mean, dev[(dev.len() * 9 / 10).min(dev.len() - 1)])
}

#[cfg(test)]
#[path = "tests/calibration.rs"]
mod tests;
