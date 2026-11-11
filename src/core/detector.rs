//! Slouch detection state machine (design: docs/UX_RESEARCH.md).
//!
//! Input is forward pitch relative to the calibrated upright pose (degrees, looking down > 0).
//! Trackers may stop sending while the head is still, so silence means "unchanged": the caller
//! must also call `tick` periodically and timers keep running on the last smoothed value.

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
        }
    }
}

pub struct Detector {
    cfg: Config,
    smoothed: Option<f64>,
    slouch_since: Option<f64>,
    cued: Vec<Cue>,
    last_speak: f64,
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
            slouch_since: None,
            cued: Vec::new(),
            last_speak: f64::NEG_INFINITY,
        }
    }

    pub fn smoothed(&self) -> Option<f64> {
        self.smoothed
    }

    pub fn slouching(&self) -> bool {
        self.slouch_since.is_some()
    }

    /// New pitch sample at time `t` (seconds).
    pub fn update(&mut self, t: f64, pitch: f64) -> Vec<Cue> {
        let a = self.cfg.ema_alpha;
        let s = self.smoothed.map_or(pitch, |s| s + a * (pitch - s));
        self.smoothed = Some(s);
        if self.slouch_since.is_none() && s >= self.cfg.on_deg {
            self.slouch_since = Some(t);
        } else if self.slouch_since.is_some() && s < self.cfg.off_deg {
            self.slouch_since = None;
            let had_cue = !self.cued.is_empty();
            self.cued.clear();
            if had_cue {
                return vec![Cue::Recovered];
            }
            return vec![];
        }
        self.tick(t)
    }

    /// Advance timers without a new sample.
    pub fn tick(&mut self, t: f64) -> Vec<Cue> {
        let Some(since) = self.slouch_since else {
            return vec![];
        };
        let held = t - since;
        let c = self.cfg;
        let mut out = Vec::new();
        for (cue, after) in [
            (Cue::Tone, c.tone_after_s),
            (Cue::Speak, c.speak_after_s),
            (Cue::Notify, c.notify_after_s),
        ] {
            if held >= after && !self.cued.contains(&cue) {
                self.cued.push(cue);
                out.push(cue);
                if cue == Cue::Speak {
                    self.last_speak = t;
                }
            }
        }
        if self.cued.contains(&Cue::Speak)
            && !out.contains(&Cue::Speak)
            && t - self.last_speak >= c.renudge_s
        {
            self.last_speak = t;
            out.push(Cue::Speak);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn looking_up_is_not_slouching() {
        let mut d = Detector::new(Config::default());
        let mut cues = vec![];
        run(&mut d, 0.0, 60.0, -40.0, &mut cues);
        assert!(cues.is_empty());
    }
}
