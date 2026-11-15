//! Slouch detection state machine (design: docs/UX_RESEARCH.md): it says when a slouch starts and
//! ends. When to remind the user during one is [`super::reminders`].
//!
//! The input is forward pitch in degrees, relative to the calibrated upright pose. Looking down is
//! positive.
//!
//! The caller passes each sample to [`Detector::pose`], then calls [`Detector::check`] with the
//! time, which returns what happened since the last check, in order ([`PostureEvent`]). Trackers
//! may stop sending while the head is still, so silence means the pitch is unchanged: a check with
//! no new sample still lets re-anchoring settle. Smoothing works on time, not on sample count, so
//! it behaves the same at any sample rate.
//!
//! [`Detector::pose`] takes each sample through three steps:
//!
//! 1. Smooth it. The smoothed value is relative to calibration.
//! 2. If the state is `Reanchoring`, check whether the head now holds still. This may re-anchor
//!    upright.
//! 3. Step the posture `State` machine on the smoothed pitch relative to upright.
//!
//! `reanchor_after_recovery` is for trackers whose pose drifts towards "looking up". With it,
//! upright is re-anchored after each recovery. The first time the head holds still after sitting
//! back up, a pitch above upright (< 0) becomes the new upright. People sit back up to their
//! upright, not beyond it. So a reading above upright after a recovery is accumulated tracker
//! error. Drift the other way is not corrected.

use crate::core::steady::Window;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Time constant of the exponential smoothing of pitch.
    pub smoothing: Duration,
    /// A slouch starts when the smoothed pitch (relative to upright) reaches this.
    pub on_deg: f64,
    /// A slouch ends when the smoothed pitch drops below this.
    pub off_deg: f64,
    /// Re-anchor upright after each recovery (see the module docs).
    pub reanchor_after_recovery: bool,
}

/// Something the detector noticed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PostureEvent {
    SlouchStarted,
    /// The slouch ended: the user sat back up.
    Recovered,
    /// Upright was re-anchored to this pitch (relative to calibration).
    Reanchored {
        upright: f64,
    },
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
            reanchor_after_recovery: false,
        }
    }
}

/// When the head counts as holding still, for the re-anchor: its pitch has stayed within
/// `SETTLE_RANGE_DEG` for `SETTLE` (see [`Window`]: silence counts as still). Chosen in DECISIONS
/// "Re-anchor upright after each recovery" and checked by replaying data/slouch_guided.csv; the
/// values themselves were not measured against alternatives \[unmeasured\].
const SETTLE: Duration = Duration::from_secs(1);
const SETTLE_RANGE_DEG: f64 = 2.0;

/// Whether the pitch samples of `span` lie within `SETTLE_RANGE_DEG` of each other.
fn within_settle_range(span: &[f64]) -> bool {
    let (lo, hi) = span
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
            (lo.min(x), hi.max(x))
        });
    hi - lo < SETTLE_RANGE_DEG
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
    fn smooth(&mut self, t: Duration, x: f64) -> f64 {
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
        s
    }

    fn value(&self) -> Option<f64> {
        self.last.map(|(_, s)| s)
    }
}

/// The posture state machine. [`State::step`] makes all its transitions except the last:
///
/// - `Upright` or `Reanchoring` → `Slouching` when the smoothed pitch reaches `on_deg`.
/// - `Slouching` → `Upright` when the smoothed pitch drops below `off_deg`. With
///   `reanchor_after_recovery`, it goes to `Reanchoring` instead.
/// - `Reanchoring` → `Upright` once the head holds still ([`State::settle`]). If the head settled
///   above upright, upright is re-anchored there.
enum State {
    Upright,
    /// Just recovered from a slouch; waiting for the head to hold still, to re-anchor upright. The
    /// window holds the unsmoothed pitch (relative to upright) since the recovery.
    Reanchoring(Window<f64>),
    Slouching,
}

impl State {
    /// In a `Reanchoring` state, take the pitch `p` (unsmoothed, relative to upright) measured at
    /// `t`, if there is a new sample, and judge the head at `t`. Once it holds still, the state
    /// becomes `Upright`. If the head settled above upright (its last pitch < 0), this returns that
    /// pitch. Otherwise it returns `None`.
    fn settle(&mut self, t: Duration, p: Option<f64>) -> Option<f64> {
        let State::Reanchoring(window) = self else {
            return None;
        };
        if let Some(p) = p {
            window.add(t, p);
        }
        let span = window.span(t, SETTLE)?;
        if !within_settle_range(&span) {
            return None;
        }
        let last = *span.last().expect("a span holds at least one sample");
        *self = State::Upright;
        (last < 0.0).then_some(last)
    }

    /// Step on smoothed pitch `s` and unsmoothed pitch `p` (both relative to upright) at time `t`.
    /// Returns the next state and the posture change it shows, if any.
    fn step(self, t: Duration, s: f64, p: f64, cfg: &Config) -> (State, Option<PostureEvent>) {
        match self {
            State::Upright | State::Reanchoring(_) if s >= cfg.on_deg => {
                (State::Slouching, Some(PostureEvent::SlouchStarted))
            }
            State::Slouching if s < cfg.off_deg => {
                let next = if cfg.reanchor_after_recovery {
                    // The sample that ends the slouch is the first the re-anchoring judges: if the
                    // tracker goes quiet from here, it alone says where the head is.
                    let mut window = Window::new(t);
                    window.add(t, p);
                    State::Reanchoring(window)
                } else {
                    State::Upright
                };
                (next, Some(PostureEvent::Recovered))
            }
            unchanged => (unchanged, None),
        }
    }
}

pub struct Detector {
    cfg: Config,
    /// Pitch relative to calibration, smoothed.
    smoother: Smoother,
    /// Pitch (relative to calibration) taken as upright: 0 until re-anchored.
    upright: f64,
    state: State,
    /// What happened since the last [`Detector::check`], in order, each at the time that showed it.
    events: Vec<(Duration, PostureEvent)>,
}

impl Detector {
    pub fn new(cfg: Config) -> Self {
        assert!(
            cfg.off_deg < cfg.on_deg,
            "off threshold must be below on threshold"
        );
        Detector {
            cfg,
            smoother: Smoother::new(cfg.smoothing),
            upright: 0.0,
            state: State::Upright,
            events: Vec::new(),
        }
    }

    /// Smoothed pitch relative to upright.
    pub fn smoothed(&self) -> Option<f64> {
        self.smoother.value().map(|s| s - self.upright)
    }

    pub fn slouching(&self) -> bool {
        matches!(self.state, State::Slouching)
    }

    /// A pitch sample measured at `t`, relative to the calibrated upright pose. Times must not go
    /// backwards. What it shows comes out of the next [`Detector::check`].
    pub fn pose(&mut self, t: Duration, pitch: f64) {
        let smoothed = self.smoother.smooth(t, pitch);
        self.reanchor(t, Some(pitch - self.upright));
        let (next, change) = std::mem::replace(&mut self.state, State::Upright).step(
            t,
            smoothed - self.upright,
            pitch - self.upright,
            &self.cfg,
        );
        self.state = next;
        self.events.extend(change.map(|event| (t, event)));
    }

    /// Judge up to `now`, and return what happened since the last check, in order, each at the
    /// time of the sample or check that showed it. Pass the samples that have arrived first:
    /// silence then means the pitch is unchanged, so re-anchoring may settle (tracker contract,
    /// "Late poses").
    pub fn check(&mut self, now: Duration) -> Vec<(Duration, PostureEvent)> {
        self.reanchor(now, None);
        std::mem::take(&mut self.events)
    }

    /// Re-anchor upright if the head has settled by `t` (see [`State::settle`]).
    fn reanchor(&mut self, t: Duration, p: Option<f64>) {
        if let Some(p) = self.state.settle(t, p) {
            self.upright += p;
            let upright = self.upright;
            self.events.push((t, PostureEvent::Reanchored { upright }));
        }
    }
}

#[cfg(test)]
#[path = "tests/detector.rs"]
mod tests;
