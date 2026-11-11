//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::cues::{Cues, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::tracker::{Event, HeadTracker};
use anyhow::{Result, bail};
use std::io::Write;
use std::time::{Duration, Instant};

const CALIBRATION_S: f64 = 3.0;
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
const CALIBRATION_TRIES: usize = 4;

struct Clock(Instant);

impl Clock {
    fn now(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// What the tracker produced, from the session's point of view.
enum Step {
    Pose(Quat),
    /// Nothing new: the head is still (or the tracker is quiet), so the last state holds.
    Silence,
    /// The earbuds were taken off.
    Removed,
}

struct Session<'a, T: HeadTracker> {
    tracker: &'a mut T,
    cues: &'a Cues,
    log: &'a mut Option<std::fs::File>,
    clock: Clock,
    worn: bool,
}

/// Calibrate, then monitor; pause while the earbuds are off and recalibrate when they are put back
/// on (reseating changes how the earbud sits). Runs until a tracker error; the caller handles
/// Ctrl-C. `log` receives JSON lines.
pub async fn run<T: HeadTracker>(
    tracker: &mut T,
    cues: &Cues,
    log: &mut Option<std::fs::File>,
) -> Result<()> {
    let mut s = Session {
        tracker,
        cues,
        log,
        clock: Clock(Instant::now()),
        worn: true,
    };
    loop {
        s.wait_until_worn().await?;
        let Some(baseline) = s.calibrate().await? else {
            continue;
        };
        s.monitor(baseline).await?;
    }
}

impl<T: HeadTracker> Session<'_, T> {
    async fn next(&mut self, timeout: Duration) -> Result<Step> {
        Ok(match self.tracker.next_event(timeout).await? {
            Some(Event::Pose(q)) => Step::Pose(q),
            Some(Event::Worn(w)) => {
                self.worn = w;
                if w { Step::Silence } else { Step::Removed }
            }
            None => Step::Silence,
        })
    }

    fn log_event(&mut self, event: &str) -> Result<()> {
        let t = self.clock.now();
        println!("\n{t:8.1}s  {event}");
        if let Some(f) = self.log.as_mut() {
            writeln!(f, r#"{{"t":{t:.3},"event":"{event}"}}"#)?;
        }
        Ok(())
    }

    async fn wait_until_worn(&mut self) -> Result<()> {
        if self.worn {
            return Ok(());
        }
        self.log_event("Paused")?;
        while !self.worn {
            self.next(Duration::from_secs(1)).await?;
        }
        self.log_event("Resumed")
    }

    /// Capture the upright pose: CALIBRATION_S of poses whose 90th-percentile deviation from their
    /// mean is under CALIBRATION_MAX_P90_DEG. Silence means the head is still, so the last pose
    /// counts. `None` if the earbuds were taken off meanwhile.
    async fn calibrate(&mut self) -> Result<Option<Quat>> {
        self.cues.say(Phrase::SitUpright).await?;
        let mut last = None;
        for attempt in 1..=CALIBRATION_TRIES {
            let end = self.clock.now() + CALIBRATION_S;
            let mut samples = Vec::new();
            while self.clock.now() < end {
                match self.next(Duration::from_millis(200)).await? {
                    Step::Pose(q) => {
                        last = Some(q);
                        samples.push(q);
                    }
                    Step::Silence => {}
                    Step::Removed => return Ok(None),
                }
            }
            if samples.is_empty() {
                let Some(q) = last else {
                    bail!("no head pose from the earbuds during calibration")
                };
                samples.push(q);
            }
            let (mean, p90) = mean_and_p90(&samples);
            println!(
                "calibration attempt {attempt}: {} samples, p90 deviation {p90:.1}°",
                samples.len()
            );
            if p90 < CALIBRATION_MAX_P90_DEG {
                self.cues.say(Phrase::Calibrated).await?;
                return Ok(Some(mean));
            }
            self.cues.say(Phrase::Moved).await?;
        }
        bail!("calibration failed: head kept moving after {CALIBRATION_TRIES} attempts")
    }

    /// Monitor until the earbuds are taken off.
    async fn monitor(&mut self, baseline: Quat) -> Result<()> {
        let mut det = Detector::new(Config::default());
        let mut last_print = 0.0;
        loop {
            let step = self.next(Duration::from_secs(1)).await?;
            let t = self.clock.now();
            let cue_list = match step {
                Step::Removed => return Ok(()),
                Step::Pose(q) => {
                    let (roll, pitch, yaw) = (baseline.conj() * q).euler();
                    if let Some(f) = self.log.as_mut() {
                        writeln!(
                            f,
                            r#"{{"t":{t:.3},"pitch":{pitch:.2},"roll":{roll:.2},"yaw":{yaw:.2}}}"#
                        )?;
                    }
                    det.update(t, pitch)
                }
                Step::Silence => det.tick(t),
            };
            for cue in cue_list {
                self.log_event(&format!("{cue:?}"))?;
                self.cues.play(cue);
            }
            if t - last_print >= 1.0 {
                last_print = t;
                let s = det.smoothed().map_or("--".into(), |p| format!("{p:5.1}°"));
                let state = if det.slouching() {
                    "SLOUCHING"
                } else {
                    "ok       "
                };
                print!("\r{t:8.1}s  pitch {s}  {state}   ");
                std::io::stdout().flush()?;
            }
        }
    }
}

fn mean_and_p90(samples: &[Quat]) -> (Quat, f64) {
    assert!(!samples.is_empty(), "mean of zero samples");
    let first = samples[0];
    let (mut w, mut x, mut y, mut z) = (0.0, 0.0, 0.0, 0.0);
    for q in samples {
        let s = if first.w * q.w + first.x * q.x + first.y * q.y + first.z * q.z < 0.0 {
            -1.0
        } else {
            1.0
        };
        (w, x, y, z) = (w + s * q.w, x + s * q.x, y + s * q.y, z + s * q.z);
    }
    let n = (w * w + x * x + y * y + z * z).sqrt();
    let mean = Quat {
        w: w / n,
        x: x / n,
        y: y / n,
        z: z / n,
    };
    let mut dev: Vec<f64> = samples.iter().map(|q| q.angle_to(mean)).collect();
    dev.sort_by(f64::total_cmp);
    (mean, dev[(dev.len() * 9 / 10).min(dev.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_mean_and_spread() {
        let tilt = |deg: f64| {
            let h = deg.to_radians() / 2.0;
            Quat {
                w: h.cos(),
                x: 0.0,
                y: h.sin(),
                z: 0.0,
            }
        };
        let (mean, p90) = mean_and_p90(&[tilt(9.0), tilt(10.0), tilt(11.0)]);
        assert!((mean.euler().1 - 10.0).abs() < 1e-6);
        assert!(p90 < 1.01, "p90 {p90}");
    }
}
