//! Slouch detection state machine (design: docs/UX_RESEARCH.md).
//!
//! Input is forward pitch relative to the calibrated upright pose (degrees, looking down > 0).
//! Trackers may stop sending while the head is still, so silence means "unchanged": the caller
//! must also call `tick` periodically and timers keep running on the last smoothed value.
//!
//! With `reanchor_after_recovery` (for trackers whose pose drifts), upright is re-anchored after
//! each recovery: the first time the head holds still after sitting back up, a pitch above upright
//! (< 0) becomes the new upright. People sit back up to their upright, not beyond it, so a reading
//! above it after a recovery is accumulated tracker error.

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
    pub ema_alpha: f64,
    pub on_deg: f64,
    pub off_deg: f64,
    pub tone_after_s: f64,
    pub speak_after_s: f64,
    pub notify_after_s: f64,
    pub renudge_s: f64,
    /// Re-anchor upright after each recovery (see the module docs).
    pub reanchor_after_recovery: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ema_alpha: 0.15,
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

/// One slouch, from crossing `on_deg` until dropping below `off_deg`.
struct Episode {
    since: f64,
    toned: bool,
    notified: bool,
    /// When the spoken reminder last played (`None` until the first one).
    last_speak: Option<f64>,
}

enum Posture {
    /// Not slouching. `Some` while waiting to re-anchor upright after a recovery.
    Upright(Option<Settling>),
    Slouching(Episode),
}

pub struct Detector {
    cfg: Config,
    smoothed: Option<f64>,
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
        self.smoothed
    }

    /// Pitch relative to calibration that currently counts as upright (0 until re-anchored).
    pub fn upright(&self) -> f64 {
        self.upright
    }

    pub fn slouching(&self) -> bool {
        matches!(self.posture, Posture::Slouching(_))
    }

    /// After a recovery, wait for the head to settle; if it settles above upright, move upright
    /// there (once). Returns `pitch` relative to the (possibly new) upright.
    fn reanchor(&mut self, t: f64, pitch: f64) -> f64 {
        let p = pitch - self.upright;
        let Posture::Upright(Some(settling)) = &mut self.posture else {
            return p;
        };
        if !settling.settled(t, p) {
            return p;
        }
        self.posture = Posture::Upright(None);
        if p >= 0.0 {
            return p;
        }
        self.upright += p;
        self.smoothed = self.smoothed.map(|s| s - p);
        0.0
    }

    /// New pitch sample at time `t` (seconds), relative to the calibrated upright pose.
    pub fn update(&mut self, t: f64, pitch: f64) -> Vec<Cue> {
        let pitch = self.reanchor(t, pitch);
        let a = self.cfg.ema_alpha;
        let s = self.smoothed.map_or(pitch, |s| s + a * (pitch - s));
        self.smoothed = Some(s);
        match &self.posture {
            Posture::Upright(_) if s >= self.cfg.on_deg => {
                self.posture = Posture::Slouching(Episode {
                    since: t,
                    toned: false,
                    notified: false,
                    last_speak: None,
                });
            }
            Posture::Slouching(ep) if s < self.cfg.off_deg => {
                let had_cue = ep.toned || ep.notified || ep.last_speak.is_some();
                let settling = self.cfg.reanchor_after_recovery.then(Settling::default);
                self.posture = Posture::Upright(settling);
                return if had_cue {
                    vec![Cue::Recovered]
                } else {
                    vec![]
                };
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
        let held = t - ep.since;
        let c = self.cfg;
        let mut out = Vec::new();
        if held >= c.tone_after_s && !ep.toned {
            ep.toned = true;
            out.push(Cue::Tone);
        }
        match ep.last_speak {
            None if held >= c.speak_after_s => {
                ep.last_speak = Some(t);
                out.push(Cue::Speak);
            }
            Some(last) if t - last >= c.renudge_s => {
                ep.last_speak = Some(t);
                out.push(Cue::Speak);
            }
            _ => {}
        }
        if held >= c.notify_after_s && !ep.notified {
            ep.notified = true;
            out.push(Cue::Notify);
        }
        out
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

    /// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting (time, cue).
    fn run(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<(f64, Cue)>) -> f64 {
        let mut t = t0;
        while t < t0 + secs {
            out.extend(d.update(t, pitch).into_iter().map(|c| (t, c)));
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
        let mut cues = vec![];
        let mut t = run(&mut d, 0.0, 15.0, 40.0, &mut cues);
        t = run(&mut d, t, 3.0, -10.0, &mut cues);
        assert!((d.upright() + 10.0).abs() < 0.5, "upright {}", d.upright());
        cues.clear();
        run(&mut d, t, 15.0, 12.0, &mut cues);
        assert!(cues.iter().any(|c| c.1 == Cue::Tone), "{cues:?}");
    }

    #[test]
    fn looking_up_without_a_slouch_keeps_upright() {
        let mut d = Detector::new(reanchoring());
        run(&mut d, 0.0, 30.0, -30.0, &mut vec![]);
        assert_eq!(d.upright(), 0.0);
    }

    #[test]
    fn partial_recovery_keeps_upright() {
        let mut d = Detector::new(reanchoring());
        let t = run(&mut d, 0.0, 15.0, 40.0, &mut vec![]);
        run(&mut d, t, 5.0, 10.0, &mut vec![]); // below off (15), still above upright
        assert_eq!(d.upright(), 0.0);
    }

    // Only the first settle after a recovery counts; a later look up must not move upright again.
    #[test]
    fn reanchors_once_per_recovery() {
        let mut d = Detector::new(reanchoring());
        let mut t = run(&mut d, 0.0, 15.0, 40.0, &mut vec![]);
        t = run(&mut d, t, 3.0, -5.0, &mut vec![]);
        run(&mut d, t, 10.0, -30.0, &mut vec![]);
        assert!((d.upright() + 5.0).abs() < 0.5, "upright {}", d.upright());
    }

    #[test]
    fn no_reanchoring_unless_enabled() {
        let mut d = Detector::new(Config::default());
        let t = run(&mut d, 0.0, 15.0, 40.0, &mut vec![]);
        run(&mut d, t, 3.0, -10.0, &mut vec![]);
        assert_eq!(d.upright(), 0.0);
    }

    #[test]
    fn looking_up_is_not_slouching() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        run(&mut d, 0.0, 60.0, -40.0, &mut cues);
        assert!(cues.is_empty());
    }
}
