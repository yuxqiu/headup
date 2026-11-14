//! Slouch detection state machine (design: docs/UX_RESEARCH.md).
//!
//! The input is forward pitch in degrees, relative to the calibrated upright pose. Looking down is
//! positive.
//!
//! Trackers may stop sending while the head is still, so silence means the pitch is unchanged. The
//! caller must also call `tick` periodically, and timers keep running on the last smoothed value.
//! Smoothing works on time, not on sample count, so it behaves the same at any sample rate.
//!
//! [`Detector::update`] takes each sample through three steps:
//!
//! 1. Smooth it. The smoothed value is relative to calibration.
//! 2. If the state is `Settling`, check whether the head now holds still. This may re-anchor
//!    upright.
//! 3. Step the posture `State` machine on the smoothed pitch relative to upright.
//!
//! `reanchor_after_recovery` is for trackers whose pose drifts towards "looking up". With it,
//! upright is re-anchored after each recovery. The first time the head holds still after sitting
//! back up, a pitch above upright (< 0) becomes the new upright. People sit back up to their
//! upright, not beyond it. So a reading above upright after a recovery is accumulated tracker
//! error. Drift the other way is not corrected.

use super::steady::Window;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Cue {
    /// Soft tone in the earbuds after the dwell time.
    Tone,
    /// Spoken reminder; repeated every `renudge` while the slouch continues.
    Speak,
    /// Desktop notification.
    Notify,
    /// Posture recovered after at least one cue.
    Recovered,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Time constant of the exponential smoothing of pitch.
    pub smoothing: Duration,
    pub on_deg: f64,
    pub off_deg: f64,
    pub tone_after: Duration,
    pub speak_after: Duration,
    pub notify_after: Duration,
    pub renudge: Duration,
    /// Re-anchor upright after each recovery (see the module docs).
    pub reanchor_after_recovery: bool,
}

/// What one pose sample produced.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    pub cues: Vec<Cue>,
    /// Upright was re-anchored to this pitch (relative to calibration).
    pub reanchored: Option<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // Time constant of the smoothing (DECISIONS "Posture logic redesign").
            smoothing: Duration::from_millis(123),
            on_deg: 20.0,
            off_deg: 15.0,
            tone_after: Duration::from_secs(10),
            speak_after: Duration::from_secs(30),
            notify_after: Duration::from_secs(60),
            renudge: Duration::from_secs(120),
            reanchor_after_recovery: false,
        }
    }
}

/// When the head counts as holding still, for the re-anchor: its pitch has stayed within
/// `SETTLE_RANGE_DEG` for `SETTLE` (see [`Window`]: silence counts as still).
const SETTLE: Duration = Duration::from_secs(1);
const SETTLE_RANGE_DEG: f64 = 2.0;

/// Whether the pitch samples of `span` lie within `SETTLE_RANGE_DEG` of each other.
fn steady(span: &[f64]) -> bool {
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

/// A reminder timer. It fires `cue` at `due`, then again each `every` later. With `every` set to
/// `None`, it fires once.
struct Timer {
    cue: Cue,
    due: Option<Duration>,
    every: Option<Duration>,
}

/// One slouch, from crossing `on_deg` until dropping below `off_deg`.
struct Episode {
    /// Tone, spoken reminder and notification, in play order.
    timers: [Timer; 3],
    /// A reminder has played, so recovery plays the "tink" (`Cue::Recovered`).
    cued: bool,
}

impl Episode {
    fn new(since: Duration, c: &Config) -> Self {
        let timer = |cue, after, every| Timer {
            cue,
            due: Some(since + after),
            every,
        };
        Episode {
            timers: [
                timer(Cue::Tone, c.tone_after, None),
                timer(Cue::Speak, c.speak_after, Some(c.renudge)),
                timer(Cue::Notify, c.notify_after, None),
            ],
            cued: false,
        }
    }

    fn fire(&mut self, t: Duration) -> Vec<Cue> {
        let mut out = Vec::new();
        for tm in &mut self.timers {
            if tm.due.is_some_and(|d| t >= d) {
                out.push(tm.cue);
                tm.due = tm.every.map(|e| t + e);
            }
        }
        self.cued |= !out.is_empty();
        out
    }

    fn next_due(&self) -> Option<Duration> {
        self.timers.iter().filter_map(|tm| tm.due).min()
    }
}

/// The posture state machine. [`State::step`] makes all its transitions except the last:
///
/// - `Upright` or `Settling` → `Slouching` when the smoothed pitch reaches `on_deg`.
/// - `Slouching` → `Upright` when the smoothed pitch drops below `off_deg`. With
///   `reanchor_after_recovery`, it goes to `Settling` instead. If a reminder played, this plays
///   `Cue::Recovered`.
/// - Otherwise `Slouching` stays `Slouching`, and its timers fire reminders as they fall due.
/// - `Settling` → `Upright` once the head holds still ([`State::settle`]). If the head settled
///   above upright, upright is re-anchored there.
enum State {
    Upright,
    /// Just recovered from a slouch; waiting for the head to hold still, to re-anchor upright. The
    /// window holds the unsmoothed pitch (relative to upright) since the recovery.
    Settling(Window<f64>),
    Slouching(Episode),
}

impl State {
    /// In a `Settling` state, take the pitch `p` (unsmoothed, relative to upright) measured at `t`,
    /// if there is a new sample, and judge the head at `t`. Once it holds still, the state becomes
    /// `Upright`. If the head settled above upright (its last pitch < 0), this returns that pitch.
    /// Otherwise it returns `None`.
    fn settle(&mut self, t: Duration, p: Option<f64>) -> Option<f64> {
        let State::Settling(window) = self else {
            return None;
        };
        if let Some(p) = p {
            window.add(t, p);
        }
        let span = window.span(t, SETTLE)?;
        if !steady(&span) {
            return None;
        }
        let last = *span.last().expect("a span holds at least one sample");
        *self = State::Upright;
        (last < 0.0).then_some(last)
    }

    /// Step on smoothed pitch `s` (relative to upright) at time `t`. Returns the next state and
    /// the cues of the step. Cues come from entering a slouch, recovering from one, or a reminder
    /// falling due.
    fn step(self, t: Duration, s: f64, cfg: &Config) -> (State, Vec<Cue>) {
        match self {
            State::Upright | State::Settling(_) if s >= cfg.on_deg => {
                let mut episode = Episode::new(t, cfg);
                let cues = episode.fire(t);
                (State::Slouching(episode), cues)
            }
            State::Slouching(episode) if s < cfg.off_deg => {
                let next = if cfg.reanchor_after_recovery {
                    State::Settling(Window::new(t))
                } else {
                    State::Upright
                };
                let cues = if episode.cued {
                    vec![Cue::Recovered]
                } else {
                    vec![]
                };
                (next, cues)
            }
            State::Slouching(mut episode) => {
                let cues = episode.fire(t);
                (State::Slouching(episode), cues)
            }
            unchanged => (unchanged, vec![]),
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
        }
    }

    /// Smoothed pitch relative to upright.
    pub fn smoothed(&self) -> Option<f64> {
        self.smoother.value().map(|s| s - self.upright)
    }

    pub fn slouching(&self) -> bool {
        matches!(self.state, State::Slouching(_))
    }

    /// New pitch sample at time `t`, relative to the calibrated upright pose. Times must not go
    /// backwards.
    pub fn update(&mut self, t: Duration, pitch: f64) -> Update {
        let smoothed = self.smoother.smooth(t, pitch);
        let reanchored = self.reanchor(t, Some(pitch - self.upright));
        let cues = self.transition(t, smoothed - self.upright);
        Update { cues, reanchored }
    }

    /// Re-anchor upright if the head has settled by `t` (see [`State::settle`]). Returns the new
    /// upright.
    fn reanchor(&mut self, t: Duration, p: Option<f64>) -> Option<f64> {
        self.state.settle(t, p).map(|p| {
            self.upright += p;
            self.upright
        })
    }

    /// Advance the timers to `t` without a new sample. If `t` is at or after `next_due()`, the due
    /// reminder always fires, because times are exact.
    pub fn tick(&mut self, t: Duration) -> Update {
        let reanchored = self.reanchor(t, None);
        let cues = match &mut self.state {
            State::Slouching(episode) => episode.fire(t),
            _ => vec![],
        };
        Update { cues, reanchored }
    }

    /// When the next reminder falls due if the slouch goes on. It uses the same clock as `update`
    /// and `tick`. `None` while not slouching.
    pub fn next_due(&self) -> Option<Duration> {
        match &self.state {
            State::Slouching(episode) => episode.next_due(),
            _ => None,
        }
    }

    fn transition(&mut self, t: Duration, s: f64) -> Vec<Cue> {
        let (next, cues) = std::mem::replace(&mut self.state, State::Upright).step(t, s, &self.cfg);
        self.state = next;
        cues
    }
}

#[cfg(test)]
#[path = "tests/detector.rs"]
mod tests;
