//! Slouch detection state machine (design: docs/UX_RESEARCH.md).
//!
//! Input is forward pitch relative to the calibrated upright pose (degrees, looking down > 0).
//! Trackers may stop sending while the head is still, so silence means "unchanged": the caller
//! must also call `tick` periodically and timers keep running on the last smoothed value.
//! Smoothing is by time, not by sample, so it behaves the same at any sample rate.
//!
//! Each sample goes through three steps, in [`Detector::update`]: smooth it (relative to
//! calibration), let a `Settling` state finish and maybe re-anchor upright, then step the posture
//! `State` machine on the smoothed pitch relative to upright.
//!
//! With `reanchor_after_recovery` (for trackers whose pose drifts towards "looking up"), upright is
//! re-anchored after each recovery: the first time the head holds still after sitting back up, a
//! pitch above upright (< 0) becomes the new upright. People sit back up to their upright, not
//! beyond it, so a reading above it after a recovery is accumulated tracker error. Drift the other
//! way is not corrected.

use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
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

/// "Holding still" for the re-anchor: pitch samples spanning at least `SETTLE_MIN` of the last
/// `SETTLE_WINDOW` all lie within `SETTLE_RANGE_DEG`.
const SETTLE_WINDOW: Duration = Duration::from_secs(1);
const SETTLE_MIN: Duration = Duration::from_millis(800);
const SETTLE_RANGE_DEG: f64 = 2.0;

/// Recent (time, pitch) while waiting for the head to settle after a recovery.
#[derive(Default)]
struct Settling(VecDeque<(Duration, f64)>);

impl Settling {
    /// Add a sample; true once the head has held still (see `SETTLE_*`).
    fn settled(&mut self, t: Duration, pitch: f64) -> bool {
        let w = &mut self.0;
        w.push_back((t, pitch));
        while w.front().is_some_and(|&(t0, _)| t - t0 > SETTLE_WINDOW) {
            w.pop_front();
        }
        let span = t - w.front().map_or(t, |&(t0, _)| t0);
        let (lo, hi) = w
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &(_, x)| {
                (lo.min(x), hi.max(x))
            });
        span >= SETTLE_MIN && hi - lo < SETTLE_RANGE_DEG
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

/// A reminder timer: fires `cue` at `due`, then again `every` later (`None`: once).
struct Timer {
    cue: Cue,
    due: Option<Duration>,
    every: Option<Duration>,
}

/// One slouch, from crossing `on_deg` until dropping below `off_deg`.
struct Episode {
    /// Tone, spoken reminder and notification, in play order.
    timers: [Timer; 3],
    /// A reminder has played, so recovery earns the "tink".
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

/// The posture state machine. Its transitions (all in [`State::step`], except the last):
///
/// - `Upright` or `Settling` → `Slouching` when the smoothed pitch reaches `on_deg`.
/// - `Slouching` → `Settling` (re-anchoring trackers) or `Upright` when it drops below `off_deg`;
///   "recovered" if a reminder played.
/// - `Slouching` stays `Slouching` otherwise, and its timers fire reminders as they fall due.
/// - `Settling` → `Upright` once the head holds still ([`State::settle`]), re-anchoring upright if it
///   settled above it.
enum State {
    Upright,
    /// Just recovered from a slouch; waiting for the head to hold still, to re-anchor upright.
    Settling(Settling),
    Slouching(Episode),
}

impl State {
    /// While settling: once the head holds still, stop settling, and return how far above
    /// upright it settled (`p` < 0), if it did. `p` is the unsmoothed pitch relative to upright.
    fn settle(&mut self, t: Duration, p: f64) -> Option<f64> {
        let State::Settling(settling) = self else {
            return None;
        };
        if !settling.settled(t, p) {
            return None;
        }
        *self = State::Upright;
        (p < 0.0).then_some(p)
    }

    /// The transition for smoothed pitch `s` (relative to upright) at time `t`, and the cues it
    /// produces: entering a slouch, recovering from one, or a reminder falling due.
    fn step(self, t: Duration, s: f64, cfg: &Config) -> (State, Vec<Cue>) {
        match self {
            State::Upright | State::Settling(_) if s >= cfg.on_deg => {
                let mut episode = Episode::new(t, cfg);
                let cues = episode.fire(t);
                (State::Slouching(episode), cues)
            }
            State::Slouching(episode) if s < cfg.off_deg => {
                let next = if cfg.reanchor_after_recovery {
                    State::Settling(Settling::default())
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
        let reanchored = self.state.settle(t, pitch - self.upright).map(|p| {
            self.upright += p;
            self.upright
        });
        let cues = self.transition(t, smoothed - self.upright);
        Update { cues, reanchored }
    }

    /// Advance timers to `t` without a new sample. At (or after) `next_due()` the due reminder
    /// always fires: times are exact.
    pub fn tick(&mut self, t: Duration) -> Vec<Cue> {
        match &mut self.state {
            State::Slouching(episode) => episode.fire(t),
            _ => vec![],
        }
    }

    /// When the next reminder falls due if the slouch goes on (same clock as `update` and
    /// `tick`); `None` while not slouching.
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
mod tests {
    use super::*;

    /// `x` seconds.
    fn s(x: f64) -> Duration {
        Duration::from_secs_f64(x)
    }

    fn reanchoring() -> Config {
        Config {
            reanchor_after_recovery: true,
            ..Config::default()
        }
    }

    /// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting the uprights re-anchored to.
    fn reanchors(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<f64>) -> f64 {
        let mut t = t0;
        while t < t0 + secs {
            out.extend(d.update(s(t), pitch).reanchored);
            t += 0.02;
        }
        t
    }

    /// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting (time, cue).
    fn run(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<(f64, Cue)>) -> f64 {
        let mut t = t0;
        while t < t0 + secs {
            out.extend(d.update(s(t), pitch).cues.into_iter().map(|c| (t, c)));
            t += 0.02;
        }
        t
    }

    #[test]
    fn upright_and_brief_glances_never_cue() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        let mut t = run(&mut d, 0.0, 30.0, 5.0, &mut cues);
        t = run(&mut d, t, 6.0, 40.0, &mut cues); // look at phone for 6 s < 10 s dwell
        run(&mut d, t, 30.0, 5.0, &mut cues);
        assert!(cues.is_empty(), "{cues:?}");
    }

    #[test]
    fn sustained_slouch_escalates_then_recovers() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        let t = run(&mut d, 0.0, 200.0, 30.0, &mut cues);
        run(&mut d, t, 5.0, 0.0, &mut cues);
        let kinds: Vec<Cue> = cues.iter().map(|c| c.1).collect();
        assert_eq!(
            kinds,
            [
                Cue::Tone,
                Cue::Speak,
                Cue::Notify,
                Cue::Speak,
                Cue::Recovered
            ]
        );
        assert!((cues[0].0 - 10.0).abs() < 0.5, "tone at {}", cues[0].0);
        assert!(
            (cues[3].0 - cues[1].0 - 120.0).abs() < 0.5,
            "re-nudge after {}",
            cues[3].0 - cues[1].0
        );
    }

    #[test]
    fn silence_keeps_timers_running() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        let t = run(&mut d, 0.0, 3.0, 30.0, &mut cues); // device then goes quiet: head held still
        let mut t2 = t;
        while t2 < 35.0 {
            cues.extend(d.tick(s(t2)).into_iter().map(|c| (t2, c)));
            t2 += 1.0;
        }
        let kinds: Vec<Cue> = cues.iter().map(|c| c.1).collect();
        assert_eq!(kinds, [Cue::Tone, Cue::Speak]);
    }

    #[test]
    fn next_due_tracks_the_next_reminder() {
        let mut d = Detector::new(Config::default());
        assert_eq!(d.next_due(), None);
        let t = run(&mut d, 0.0, 1.0, 30.0, &mut vec![]);
        let tone = d.next_due().unwrap();
        assert!(
            (tone.as_secs_f64() - 10.0).abs() < 0.1,
            "tone due at {tone:?}"
        );
        assert_eq!(d.tick(tone - Duration::from_nanos(1)), []);
        assert_eq!(d.tick(tone), [Cue::Tone]);
        assert_eq!(d.next_due(), Some(tone + s(20.0)), "speak next");
        run(&mut d, t + 10.0, 1.0, 0.0, &mut vec![]);
        assert_eq!(d.next_due(), None);
    }

    #[test]
    fn hysteresis_keeps_episode_between_thresholds() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        let t = run(&mut d, 0.0, 12.0, 25.0, &mut cues); // tone at ~10 s
        run(&mut d, t, 30.0, 17.0, &mut cues); // between off(15) and on(20): still slouching
        assert!(d.slouching());
        assert!(cues.iter().all(|c| c.1 != Cue::Recovered), "{cues:?}");
    }

    // Sitting back up "past" upright (tracker error) re-anchors upright, so the next slouch counts
    // in full: from the new upright at -10°, a look down to +12° is 22° and must cue.
    #[test]
    fn recovery_past_upright_reanchors() {
        let mut d = Detector::new(reanchoring());
        let mut re = vec![];
        let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
        t = reanchors(&mut d, t, 3.0, -10.0, &mut re);
        assert!(
            re.len() == 1 && (re[0] + 10.0).abs() < 0.5,
            "re-anchored {re:?}"
        );
        let mut cues = vec![];
        run(&mut d, t, 15.0, 12.0, &mut cues);
        assert!(cues.iter().any(|c| c.1 == Cue::Tone), "{cues:?}");
    }

    #[test]
    fn looking_up_without_a_slouch_keeps_upright() {
        let mut d = Detector::new(reanchoring());
        let mut re = vec![];
        reanchors(&mut d, 0.0, 30.0, -30.0, &mut re);
        assert!(re.is_empty(), "re-anchored {re:?}");
    }

    #[test]
    fn partial_recovery_keeps_upright() {
        let mut d = Detector::new(reanchoring());
        let mut re = vec![];
        let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
        reanchors(&mut d, t, 5.0, 10.0, &mut re); // below off (15), still above upright
        assert!(re.is_empty(), "re-anchored {re:?}");
    }

    // Only the first settle after a recovery counts; a later look up must not move upright again.
    #[test]
    fn reanchors_once_per_recovery() {
        let mut d = Detector::new(reanchoring());
        let mut re = vec![];
        let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
        t = reanchors(&mut d, t, 3.0, -5.0, &mut re);
        reanchors(&mut d, t, 10.0, -30.0, &mut re);
        assert!(
            re.len() == 1 && (re[0] + 5.0).abs() < 0.5,
            "re-anchored {re:?}"
        );
    }

    #[test]
    fn no_reanchoring_unless_enabled() {
        let mut d = Detector::new(Config::default());
        let mut re = vec![];
        let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
        reanchors(&mut d, t, 3.0, -10.0, &mut re);
        assert!(re.is_empty(), "re-anchored {re:?}");
    }

    // The same head movement smooths the same at any sample rate: 0.24 s after a step from 0° to
    // 30°, a 25 Hz and a 100 Hz tracker agree.
    #[test]
    fn smoothing_is_independent_of_sample_rate() {
        let after_step = |hz: f64| {
            let mut d = Detector::new(Config::default());
            d.update(Duration::ZERO, 0.0);
            let steps = (0.24 * hz).round() as u32;
            for i in 1..=steps {
                d.update(s(f64::from(i) / hz), 30.0);
            }
            d.smoothed().unwrap()
        };
        let (slow, fast) = (after_step(25.0), after_step(100.0));
        assert!(
            (slow - fast).abs() < 0.01,
            "25 Hz {slow:.2} vs 100 Hz {fast:.2}"
        );
    }

    #[test]
    fn looking_up_is_not_slouching() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        run(&mut d, 0.0, 60.0, -40.0, &mut cues);
        assert!(cues.is_empty());
    }
}
