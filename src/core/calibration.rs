//! Calibration: find the upright pose after the "sit upright" prompt.
//!
//! [`Calibration`] makes every decision without I/O. The session feeds it poses and the time,
//! speaks what it asks for, and tells it when that prompt has ended. [`Stillness`] tells the
//! session when a head has settled, before it asks the user to sit upright after tracking resumed.
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
pub enum Output {
    /// No pose has arrived: ask the user to move a little, then call
    /// [`Calibration::prompt_ended`].
    Nudge,
    /// An attempt ended. Show `report`, then act on `outcome`.
    AttemptEnded { report: Report, outcome: Outcome },
}

/// How an attempt ended.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The head held steady; this is its mean pose, the upright pose.
    Calibrated(Quat),
    /// The head moved: tell the user, then call [`Calibration::prompt_ended`] to start the next
    /// attempt.
    Retry,
    /// The last attempt failed: tell the user they moved. Calibration is over.
    GaveUp,
}

/// An attempt's numbers, for the console.
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
    /// An attempt is measuring. `nudge_at` is when to ask the user to move if it has no pose yet.
    Measuring { poses: Window, nudge_at: Duration },
    /// A prompt asked for by an [`Output`] is playing. What follows it is in `then`.
    Prompting { then: AfterPrompt },
    /// Calibrated or gave up.
    Done,
}

/// What a calibration prompt leads to once it ends. Either way measuring starts afresh then:
/// poses measured while a prompt plays do not count.
enum AfterPrompt {
    /// "Move a little" ended: the same attempt goes on.
    SameAttempt,
    /// "You moved" ended: the next attempt starts.
    NextAttempt,
}

impl Calibration {
    /// Start the first attempt at `now`, when the "sit upright" prompt has finished.
    pub fn new(now: Duration) -> Self {
        Calibration {
            attempts: 1,
            state: State::measuring(now, NUDGE_AFTER),
        }
    }

    /// A pose measured at `measured`. Poses that arrive while a prompt plays, or after the end,
    /// are not measured.
    pub fn pose(&mut self, measured: Duration, q: Quat) {
        if let State::Measuring { poses, .. } = &mut self.state {
            poses.add(measured, q);
        }
    }

    /// Decide at `now`. Every pose measured before `now` must have been passed to
    /// [`Calibration::pose`]. `None` means keep feeding poses (or a prompt is playing, or
    /// calibration is over).
    pub fn check(&mut self, now: Duration) -> Option<Output> {
        let State::Measuring { poses, nudge_at } = &mut self.state else {
            return None;
        };
        let span = poses.span(now, CALIBRATION).map(|s| {
            let (mean, p90) = mean_and_p90(&s);
            (mean, p90, s.len())
        });
        let report = Report {
            attempt: self.attempts,
            samples: span.as_ref().map_or(0, |s| s.2),
            p90: span.as_ref().map_or(f64::NAN, |s| s.1),
        };
        let (next, output) = match (span, poses.first()) {
            (Some((mean, p90, _)), _) if p90 < CALIBRATION_MAX_P90_DEG => {
                (State::Done, ended(report, Outcome::Calibrated(mean)))
            }
            (_, Some(first)) if now >= first + CALIBRATION_ATTEMPT => {
                if self.attempts == CALIBRATION_TRIES {
                    (State::Done, ended(report, Outcome::GaveUp))
                } else {
                    let then = AfterPrompt::NextAttempt;
                    (State::Prompting { then }, ended(report, Outcome::Retry))
                }
            }
            (_, None) if now >= *nudge_at => {
                let then = AfterPrompt::SameAttempt;
                (State::Prompting { then }, Output::Nudge)
            }
            _ => return None,
        };
        self.state = next;
        Some(output)
    }

    /// The prompt asked for by an [`Output`] ended at `now`. Does nothing if none was playing.
    pub fn prompt_ended(&mut self, now: Duration) {
        let State::Prompting { then } = &self.state else {
            return;
        };
        let wait = match then {
            AfterPrompt::SameAttempt => NUDGE_EVERY,
            AfterPrompt::NextAttempt => {
                self.attempts += 1;
                NUDGE_AFTER
            }
        };
        self.state = State::measuring(now, wait);
    }
}

fn ended(report: Report, outcome: Outcome) -> Output {
    Output::AttemptEnded { report, outcome }
}

impl State {
    /// Measuring from `now`, asking the user to move if no pose has come `nudge_in` later.
    fn measuring(now: Duration, nudge_in: Duration) -> State {
        State::Measuring {
            poses: Window::new(now),
            nudge_at: now + nudge_in,
        }
    }
}

/// The poses measured since `start`, for judging the head over the last stretch of time. A pose
/// measured before `start` does not count: those measured during a prompt are delivered after it.
struct Window {
    start: Duration,
    /// Poses (measurement time, orientation), oldest first. The first is the latest pose that can
    /// still begin a span.
    poses: VecDeque<(Duration, Quat)>,
    /// When the first pose since `start` was measured.
    first: Option<Duration>,
}

impl Window {
    fn new(start: Duration) -> Self {
        Window {
            start,
            poses: VecDeque::new(),
            first: None,
        }
    }

    fn add(&mut self, measured: Duration, q: Quat) {
        if measured >= self.start {
            self.first.get_or_insert(measured);
            self.poses.push_back((measured, q));
        }
    }

    fn first(&self) -> Option<Duration> {
        self.first
    }

    /// The poses that cover the `len` before `now`, if poses do: the latest pose measured at least
    /// `len` before `now` (the head stayed at it until the next one) and every pose after it.
    fn span(&mut self, now: Duration, len: Duration) -> Option<Vec<Quat>> {
        let from = now.checked_sub(len)?;
        let begin = self.poses.iter().rposition(|&(m, _)| m <= from)?;
        self.poses.drain(..begin);
        Some(self.poses.iter().map(|&(_, q)| q).collect())
    }
}

/// How long the head must hold still, once head tracking resumes, before calibration asks the user
/// to sit upright.
const STILL: Duration = Duration::from_secs(2);
/// The largest angle (degrees) from their mean that poses may have while the head holds still.
const STILL_MAX_DEG: f64 = 2.0;

/// Whether the head holds still, judged on the poses since the wait started. It does once its
/// posture has stayed within STILL_MAX_DEG of the mean for STILL. With no pose at all for STILL
/// since the wait started, it does too: a tracker with nothing to deliver means a head that has
/// not moved.
pub struct Stillness(Window);

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
            return now >= self.0.start + STILL;
        }
        let Some(span) = self.0.span(now, STILL) else {
            return false;
        };
        let mean = Quat::mean(&span);
        span.iter().all(|q| q.angle_to(mean) <= STILL_MAX_DEG)
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
