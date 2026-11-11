//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::cues::{Cues, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::Recorder;
use super::tracker::{Event, HeadTracker};
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

const CALIBRATION_S: f64 = 3.0;
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
const CALIBRATION_TRIES: usize = 4;
/// After a failed calibration (head kept moving), wait this long before asking again.
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);

/// What the tracker produced, from the session's point of view.
enum Step {
    Pose(Quat),
    /// Nothing new: the head is still (or the tracker is quiet), so the last state holds.
    Silence,
    /// The earbuds were taken off.
    Removed,
}

enum Calibration {
    /// Upright pose captured.
    Done(Quat),
    /// The earbuds were taken off meanwhile.
    Removed,
    /// The head kept moving on every attempt.
    Failed,
}

struct Session<'a, T: HeadTracker> {
    tracker: &'a mut T,
    cues: &'a Cues,
    rec: &'a mut Recorder,
    start: Instant,
    worn: bool,
}

/// Calibrate, then monitor; pause while the earbuds are off and recalibrate when they are put back
/// on (reseating changes how the earbud sits). Runs until a tracker error; the caller handles
/// Ctrl-C.
pub async fn run<T: HeadTracker>(tracker: &mut T, cues: &Cues, rec: &mut Recorder) -> Result<()> {
    let mut s = Session {
        tracker,
        cues,
        rec,
        start: Instant::now(),
        worn: true,
    };
    loop {
        s.wait_until_worn().await?;
        match s.calibrate().await? {
            Calibration::Done(baseline) => s.monitor(baseline).await?,
            Calibration::Removed => {}
            Calibration::Failed => {
                s.event("CalibrationFailed")?;
                s.idle(CALIBRATION_RETRY).await?;
            }
        }
    }
}

impl<T: HeadTracker> Session<'_, T> {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn event(&mut self, name: &str) -> Result<()> {
        let t = self.now();
        self.rec.event(t, name)
    }

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

    async fn wait_until_worn(&mut self) -> Result<()> {
        if self.worn {
            return Ok(());
        }
        self.event("Paused")?;
        while !self.worn {
            self.next(Duration::from_secs(1)).await?;
        }
        self.event("Resumed")
    }

    /// Consume events for `duration`, or until the earbuds are taken off.
    async fn idle(&mut self, duration: Duration) -> Result<()> {
        let end = Instant::now() + duration;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            if let Step::Removed = self.next(left.min(Duration::from_secs(1))).await? {
                break;
            }
        }
        Ok(())
    }

    /// Capture the upright pose: CALIBRATION_S of poses whose 90th-percentile deviation from their
    /// mean is under CALIBRATION_MAX_P90_DEG. Silence means the head is still, so the last pose
    /// counts.
    async fn calibrate(&mut self) -> Result<Calibration> {
        self.cues.say(Phrase::SitUpright).await?;
        let mut last = None;
        for attempt in 1..=CALIBRATION_TRIES {
            let end = self.now() + CALIBRATION_S;
            let mut samples = Vec::new();
            while self.now() < end {
                match self.next(Duration::from_millis(200)).await? {
                    Step::Pose(q) => {
                        last = Some(q);
                        samples.push(q);
                    }
                    Step::Silence => {}
                    Step::Removed => return Ok(Calibration::Removed),
                }
            }
            if samples.is_empty() {
                let Some(q) = last else {
                    bail!("no head pose from the earbuds during calibration")
                };
                samples.push(q);
            }
            let (mean, p90) = mean_and_p90(&samples);
            self.rec.note(&format!(
                "calibration attempt {attempt}: {} samples, p90 deviation {p90:.1}°",
                samples.len()
            ));
            if p90 < CALIBRATION_MAX_P90_DEG {
                self.cues.say(Phrase::Calibrated).await?;
                return Ok(Calibration::Done(mean));
            }
            self.cues.say(Phrase::Moved).await?;
        }
        Ok(Calibration::Failed)
    }

    /// Monitor until the earbuds are taken off.
    async fn monitor(&mut self, baseline: Quat) -> Result<()> {
        let mut det = Detector::new(Config::default());
        let mut last_status = f64::NEG_INFINITY;
        loop {
            let step = self.next(Duration::from_secs(1)).await?;
            let t = self.now();
            let cues = match step {
                Step::Removed => return Ok(()),
                Step::Pose(q) => {
                    let angles = (baseline.conj() * q).euler();
                    self.rec.pose(t, angles)?;
                    det.update(t, angles.pitch)
                }
                Step::Silence => det.tick(t),
            };
            for cue in cues {
                self.rec.event(t, &format!("{cue:?}"))?;
                self.cues.play(cue);
            }
            if t - last_status >= 1.0 {
                last_status = t;
                self.rec.status(t, det.smoothed(), det.slouching())?;
            }
        }
    }
}

/// Mean orientation of `samples` and the 90th-percentile angle (degrees) of samples from it.
fn mean_and_p90(samples: &[Quat]) -> (Quat, f64) {
    assert!(!samples.is_empty(), "mean of zero samples");
    let first = samples[0];
    let (mut w, mut x, mut y, mut z) = (0.0, 0.0, 0.0, 0.0);
    for q in samples {
        // q and -q are the same rotation; align signs before averaging.
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

    fn tilt(deg: f64) -> Quat {
        let h = deg.to_radians() / 2.0;
        Quat {
            w: h.cos(),
            x: 0.0,
            y: h.sin(),
            z: 0.0,
        }
    }

    #[test]
    fn calibration_mean_and_spread() {
        let (mean, p90) = mean_and_p90(&[tilt(9.0), tilt(10.0), tilt(11.0)]);
        assert!((mean.euler().pitch - 10.0).abs() < 1e-6);
        assert!(p90 < 1.01, "p90 {p90}");
    }

    #[test]
    fn calibration_mean_handles_sign_flipped_quaternions() {
        let q = tilt(10.0);
        let neg = Quat {
            w: -q.w,
            x: -q.x,
            y: -q.y,
            z: -q.z,
        }; // same rotation
        let (mean, p90) = mean_and_p90(&[q, neg, q]);
        assert!((mean.euler().pitch - 10.0).abs() < 1e-6);
        assert!(p90 < 1e-6, "p90 {p90}");
    }
}
