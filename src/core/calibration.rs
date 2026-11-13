//! Calibration: find the upright pose after the "sit upright" prompt.
//!
//! [`Calibration`] makes every decision without I/O. The session feeds it poses and the time,
//! speaks what it asks for, and tells it when a prompt has finished.
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
use std::collections::VecDeque;
use std::time::Duration;

/// How long the head must hold steady, after the prompt, to calibrate.
const CALIBRATION: Duration = Duration::from_secs(3);
/// The 90th-percentile angle (degrees) from the mean pose below which the head counts as steady.
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
/// An attempt fails if the head has not held steady for CALIBRATION by this long after the
/// attempt's first pose.
const CALIBRATION_ATTEMPT: Duration = Duration::from_secs(5);
pub(crate) const CALIBRATION_TRIES: usize = 4;
/// With no pose this long after an attempt starts, ask the user to move a little. Ask again every
/// NUDGE_EVERY.
const NUDGE_AFTER: Duration = Duration::from_secs(5);
const NUDGE_EVERY: Duration = Duration::from_secs(15);

/// What the session should do after [`Calibration::check`].
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Keep feeding poses.
    Wait,
    /// No pose has arrived: ask the user to move a little, then call [`Calibration::spoken`].
    Nudge,
    /// The attempt failed: tell the user they moved, then call [`Calibration::spoken`] to start
    /// the next attempt.
    Moved(Report),
    /// The head held steady. `upright` is its mean pose.
    Calibrated { upright: Quat, report: Report },
    /// The last attempt failed: tell the user they moved. Calibration is over.
    GaveUp(Report),
}

/// How an attempt ended, for the console.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// 1 for the first attempt.
    pub attempt: usize,
    /// Poses in the span judged; 0 if no span was covered.
    pub samples: usize,
    /// 90th-percentile angle (degrees) of those poses from their mean; NaN if no span was covered.
    pub p90: f64,
}

pub struct Calibration {
    /// Attempts started so far, including the current one.
    attempts: usize,
    state: State,
}

enum State {
    /// An attempt is running. `nudge_at` is when to ask the user to move if it has no pose yet.
    Measuring {
        attempt: Attempt,
        nudge_at: Duration,
    },
    /// "Move a little" is playing. The attempt that started at `start` goes on after it; it has no
    /// pose yet.
    Nudging { start: Duration },
    /// "You moved" is playing. The next attempt starts after it.
    Moved,
    /// Calibrated or gave up.
    Done,
}

impl Calibration {
    /// Start the first attempt at `now`, when the "sit upright" prompt has finished.
    pub fn new(now: Duration) -> Self {
        Calibration {
            attempts: 1,
            state: State::measuring(now),
        }
    }

    /// A pose measured at `measured`. Panics unless an attempt is running.
    pub fn pose(&mut self, measured: Duration, q: Quat) {
        let State::Measuring { attempt, .. } = &mut self.state else {
            panic!("calibration got a pose while not measuring");
        };
        attempt.add(measured, q);
    }

    /// Decide at `now`. Every pose measured before `now` must have been passed to
    /// [`Calibration::pose`]. Panics unless an attempt is running.
    pub fn check(&mut self, now: Duration) -> Step {
        let State::Measuring { attempt, nudge_at } = &mut self.state else {
            panic!("calibration checked while not measuring");
        };
        let span = attempt.span(now);
        let report = Report {
            attempt: self.attempts,
            samples: span.as_ref().map_or(0, |s| s.samples),
            p90: span.as_ref().map_or(f64::NAN, |s| s.p90),
        };
        let (next, step) = match (span, attempt.first) {
            (Some(span), _) if span.p90 < CALIBRATION_MAX_P90_DEG => (
                State::Done,
                Step::Calibrated {
                    upright: span.mean,
                    report,
                },
            ),
            (_, Some(first)) if now >= first + CALIBRATION_ATTEMPT => {
                if self.attempts == CALIBRATION_TRIES {
                    (State::Done, Step::GaveUp(report))
                } else {
                    (State::Moved, Step::Moved(report))
                }
            }
            (_, None) if now >= *nudge_at => (
                State::Nudging {
                    start: attempt.start,
                },
                Step::Nudge,
            ),
            _ => return Step::Wait,
        };
        self.state = next;
        step
    }

    /// The prompt asked for by [`Step::Nudge`] or [`Step::Moved`] finished at `now`. Panics if
    /// none was asked for.
    pub fn spoken(&mut self, now: Duration) {
        self.state = match std::mem::replace(&mut self.state, State::Done) {
            State::Nudging { start } => State::Measuring {
                attempt: Attempt::start(start),
                nudge_at: now + NUDGE_EVERY,
            },
            State::Moved => {
                self.attempts += 1;
                State::measuring(now)
            }
            State::Measuring { .. } | State::Done => panic!("no calibration prompt was playing"),
        };
    }
}

impl State {
    /// A new attempt starting at `now`.
    fn measuring(now: Duration) -> State {
        State::Measuring {
            attempt: Attempt::start(now),
            nudge_at: now + NUDGE_AFTER,
        }
    }
}

/// One attempt: the poses measured since it started.
///
/// The span judged starts at the latest pose measured at least CALIBRATION before now, because
/// the head stayed at that pose until the next one. That pose and every pose after it must lie
/// within CALIBRATION_MAX_P90_DEG of their mean, at the 90th percentile.
struct Attempt {
    start: Duration,
    /// Poses (measurement time, orientation), oldest first. The first is the latest pose that can
    /// still begin a span.
    poses: VecDeque<(Duration, Quat)>,
    /// When the attempt's first pose was measured.
    first: Option<Duration>,
}

/// A span of poses: their mean, the 90th-percentile angle (degrees) from it, and their number.
struct Span {
    mean: Quat,
    p90: f64,
    samples: usize,
}

impl Attempt {
    fn start(start: Duration) -> Self {
        Attempt {
            start,
            poses: VecDeque::new(),
            first: None,
        }
    }

    /// Add a pose measured at `measured`. Poses measured before the attempt started don't count:
    /// those measured during a prompt are delivered after it.
    fn add(&mut self, measured: Duration, q: Quat) {
        if measured >= self.start {
            self.first.get_or_insert(measured);
            self.poses.push_back((measured, q));
        }
    }

    /// The span of length CALIBRATION that ends at `now`, if poses cover it.
    fn span(&mut self, now: Duration) -> Option<Span> {
        let from = now.checked_sub(CALIBRATION)?;
        let begin = self.poses.iter().rposition(|&(m, _)| m <= from)?;
        self.poses.drain(..begin);
        let samples: Vec<Quat> = self.poses.iter().map(|&(_, q)| q).collect();
        let (mean, p90) = mean_and_p90(&samples);
        Some(Span {
            mean,
            p90,
            samples: samples.len(),
        })
    }
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
