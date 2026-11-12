//! Slouch detection state machine (design: docs/UX_RESEARCH.md).
//!
//! Input is forward pitch relative to the calibrated upright pose (degrees, looking down > 0).
//! Trackers may stop sending while the head is still, so silence means "unchanged": the caller
//! must also call `tick` periodically and timers keep running on the last smoothed value.
//! Smoothing is by time, not by sample, so it behaves the same at any sample rate.
//!
//! With `reanchor_after_recovery` (for trackers whose pose drifts towards "looking up"), upright is
//! re-anchored after each recovery: the first time the head holds still after sitting back up, a
//! pitch above upright (< 0) becomes the new upright. People sit back up to their upright, not
//! beyond it, so a reading above it after a recovery is accumulated tracker error. Drift the other
//! way is not corrected.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cue {
    /// Soft tone in the earbuds after the dwell time.
    Tone,
    /// Spoken reminder; repeated every `renudge_s` while the slouch continues.
    Speak,
    /// Desktop notification.
    Notify,
    /// Posture recovered after at least one cue.
    Recovered,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Time constant of the exponential smoothing of pitch, seconds.
    pub smoothing_s: f64,
    pub on_deg: f64,
    pub off_deg: f64,
    pub tone_after_s: f64,
    pub speak_after_s: f64,
    pub notify_after_s: f64,
    pub renudge_s: f64,
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
            // Same as a per-sample weight of 0.15 at 50 Hz: -0.02 s / ln(0.85).
            smoothing_s: 0.123,
            on_deg: 20.0,
            off_deg: 15.0,
            tone_after_s: 10.0,
            speak_after_s: 30.0,
            notify_after_s: 60.0,
            renudge_s: 120.0,
            reanchor_after_recovery: false,
        }
    }
}

/// "Holding still" for the re-anchor: pitch samples spanning at least `SETTLE_MIN_S` of the last
/// `SETTLE_WINDOW_S` all lie within `SETTLE_RANGE_DEG`.
const SETTLE_WINDOW_S: f64 = 1.0;
const SETTLE_MIN_S: f64 = 0.8;
const SETTLE_RANGE_DEG: f64 = 2.0;

/// Recent (time, pitch) while waiting for the head to settle after a recovery.
#[derive(Default)]
struct Settling(VecDeque<(f64, f64)>);

impl Settling {
    /// Add a sample; true once the head has held still (see `SETTLE_*`).
    fn settled(&mut self, t: f64, pitch: f64) -> bool {
        let w = &mut self.0;
        w.push_back((t, pitch));
        while w.front().is_some_and(|&(t0, _)| t - t0 > SETTLE_WINDOW_S) {
            w.pop_front();
        }
        let span = t - w.front().map_or(t, |&(t0, _)| t0);
        let (lo, hi) = w
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &(_, x)| {
                (lo.min(x), hi.max(x))
            });
        span >= SETTLE_MIN_S && hi - lo < SETTLE_RANGE_DEG
    }
}

/// The smoothed pitch and when it was last updated.
#[derive(Clone, Copy)]
struct Smoothed {
    t: f64,
    pitch: f64,
}

/// A reminder timer: fires `cue` at `due`, then again `every` seconds later (`None`: once).
struct Timer {
    cue: Cue,
    due: Option<f64>,
    every: Option<f64>,
}

/// One slouch, from crossing `on_deg` until dropping below `off_deg`.
struct Episode {
    /// Tone, spoken reminder and notification, in play order.
    timers: [Timer; 3],
    /// A reminder has played, so recovery earns the "tink".
    cued: bool,
}

impl Episode {
    fn new(since: f64, c: &Config) -> Self {
        let timer = |cue, after, every| Timer {
            cue,
            due: Some(since + after),
            every,
        };
        Episode {
            timers: [
                timer(Cue::Tone, c.tone_after_s, None),
                timer(Cue::Speak, c.speak_after_s, Some(c.renudge_s)),
                timer(Cue::Notify, c.notify_after_s, None),
            ],
            cued: false,
        }
    }

    fn fire(&mut self, t: f64) -> Vec<Cue> {
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

    fn next_due(&self) -> Option<f64> {
        self.timers.iter().filter_map(|tm| tm.due).reduce(f64::min)
    }
}

enum Posture {
    /// Not slouching. `Some` while waiting to re-anchor upright after a recovery.
    Upright(Option<Settling>),
    Slouching(Episode),
}

pub struct Detector {
    cfg: Config,
    smoothed: Option<Smoothed>,
    /// Pitch (relative to calibration) currently taken as upright.
    upright: f64,
    posture: Posture,
}

impl Detector {
    pub fn new(cfg: Config) -> Self {
        assert!(
            cfg.off_deg < cfg.on_deg,
            "off threshold must be below on threshold"
        );
        Detector {
            cfg,
            smoothed: None,
            upright: 0.0,
            posture: Posture::Upright(None),
        }
    }

    pub fn smoothed(&self) -> Option<f64> {
        self.smoothed.map(|s| s.pitch)
    }

    pub fn slouching(&self) -> bool {
        matches!(self.posture, Posture::Slouching(_))
    }

    /// After a recovery, wait for the head to settle; if it settles above upright, move upright
    /// there (once) and return the new upright.
    fn reanchor(&mut self, t: f64, pitch: f64) -> Option<f64> {
        let p = pitch - self.upright;
        let Posture::Upright(Some(settling)) = &mut self.posture else {
            return None;
        };
        if !settling.settled(t, p) {
            return None;
        }
        self.posture = Posture::Upright(None);
        if p >= 0.0 {
            return None;
        }
        self.upright += p;
        if let Some(s) = &mut self.smoothed {
            s.pitch -= p;
        }
        Some(self.upright)
    }

    /// New pitch sample at time `t` (seconds), relative to the calibrated upright pose.
    pub fn update(&mut self, t: f64, pitch: f64) -> Update {
        let reanchored = self.reanchor(t, pitch);
        let cues = self.classify(t, pitch - self.upright);
        Update { cues, reanchored }
    }

    /// Smooth `pitch` (relative to upright) and step the posture state machine.
    fn classify(&mut self, t: f64, pitch: f64) -> Vec<Cue> {
        let s = match self.smoothed {
            None => pitch,
            Some(prev) => {
                assert!(
                    t >= prev.t,
                    "pose times went backwards: {t} after {}",
                    prev.t
                );
                let weight = 1.0 - (-(t - prev.t) / self.cfg.smoothing_s).exp();
                prev.pitch + weight * (pitch - prev.pitch)
            }
        };
        self.smoothed = Some(Smoothed { t, pitch: s });
        match &self.posture {
            Posture::Upright(_) if s >= self.cfg.on_deg => {
                self.posture = Posture::Slouching(Episode::new(t, &self.cfg));
            }
            Posture::Slouching(ep) if s < self.cfg.off_deg => {
                let cued = ep.cued;
                let settling = self.cfg.reanchor_after_recovery.then(Settling::default);
                self.posture = Posture::Upright(settling);
                return if cued { vec![Cue::Recovered] } else { vec![] };
            }
            _ => {}
        }
        self.tick(t)
    }

    /// Advance timers without a new sample.
    pub fn tick(&mut self, t: f64) -> Vec<Cue> {
        let Posture::Slouching(ep) = &mut self.posture else {
            return vec![];
        };
        ep.fire(t)
    }

    /// When the next reminder falls due if the slouch goes on (same clock as `update` and
    /// `tick`); `None` while upright.
    pub fn next_due(&self) -> Option<f64> {
        let Posture::Slouching(ep) = &self.posture else {
            return None;
        };
        ep.next_due()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            out.extend(d.update(t, pitch).reanchored);
            t += 0.02;
        }
        t
    }

    /// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting (time, cue).
    fn run(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<(f64, Cue)>) -> f64 {
        let mut t = t0;
        while t < t0 + secs {
            out.extend(d.update(t, pitch).cues.into_iter().map(|c| (t, c)));
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
            cues.extend(d.tick(t2).into_iter().map(|c| (t2, c)));
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
        assert!((tone - 10.0).abs() < 0.1, "tone due at {tone}");
        assert_eq!(d.tick(tone - 0.001), []);
        assert_eq!(d.tick(tone), [Cue::Tone]);
        assert!(
            (d.next_due().unwrap() - (tone + 20.0)).abs() < 1e-9,
            "speak next"
        );
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
            d.update(0.0, 0.0);
            let steps = (0.24 * hz).round() as u32;
            for i in 1..=steps {
                d.update(f64::from(i) / hz, 30.0);
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
