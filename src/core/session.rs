//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::cues::{CueSink, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::Result;
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;

const CALIBRATION_S: f64 = 3.0;
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
const CALIBRATION_TRIES: usize = 4;
/// After a failed calibration (head kept moving), wait this long before asking again.
const CALIBRATION_RETRY_S: f64 = 60.0;
/// Say that headup is waiting if head tracking has not started after this long.
const TRACKING_WAIT_NOTE_S: f64 = 5.0;
/// Tracker unavailable this long counts as tracking lost. Shorter gaps carry the posture over;
/// after a longer gap the head may have moved unseen and resumed tracking may measure from a new
/// pose, so headup drops its slouch state (no reminders from stale data) and recalibrates when
/// tracking resumes. Silence while available just means the head is still and never ends
/// monitoring.
const TRACKING_LOST_S: f64 = 30.0;
/// How long to wait for a tracker event before re-checking timers.
const POLL: Duration = Duration::from_secs(1);

/// What the tracker produced, from the session's point of view.
enum Step {
    Pose(Quat),
    /// Nothing new: the head is still (or the tracker is quiet), so the last state holds.
    Silence,
    /// The earbuds were taken off.
    Removed,
}

/// Why monitoring stopped.
enum MonitorEnd {
    /// The earbuds were taken off.
    Removed,
    /// The tracker was unavailable for TRACKING_LOST_S: the posture reference can't be trusted.
    TrackingLost,
}

enum Calibration {
    /// Upright pose captured.
    Done(Quat),
    /// The earbuds were taken off meanwhile.
    Removed,
    /// The head kept moving on every attempt.
    Failed,
}

struct Session<'a, T: HeadTracker, C: CueSink> {
    tracker: &'a mut T,
    cues: &'a C,
    rec: &'a mut Recorder,
    /// When the session started; timing uses seconds since then (monotonic, unlike log time).
    start: Instant,
    worn: bool,
    /// When the tracker last became unavailable (`None` while available).
    unavailable_since: Option<f64>,
}

/// Calibrate, then monitor; pause while the earbuds are off and recalibrate when they are put back
/// on (reseating changes how the earbud sits) or after tracking was lost. Returns only with a
/// tracker error; the caller handles Ctrl-C.
pub async fn run<T: HeadTracker, C: CueSink>(
    tracker: &mut T,
    cues: &C,
    rec: &mut Recorder,
) -> Result<Infallible> {
    let mut s = Session {
        tracker,
        cues,
        rec,
        start: Instant::now(),
        worn: true,
        unavailable_since: None,
    };
    loop {
        s.wait_until_worn().await?;
        match s.calibrate().await? {
            Calibration::Done(baseline) => match s.monitor(baseline).await? {
                MonitorEnd::Removed => {}
                MonitorEnd::TrackingLost => s.on_tracking_lost()?,
            },
            Calibration::Removed => {}
            Calibration::Failed => {
                s.rec.event(LogEvent::CalibrationFailed)?;
                s.idle(CALIBRATION_RETRY_S).await?;
            }
        }
    }
}

impl<T: HeadTracker, C: CueSink> Session<'_, T, C> {
    /// Seconds since the session started.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// The tracker's hint as a message suffix (" (hint)"), or "" if it has none.
    fn tracking_hint_suffix(&self) -> String {
        self.tracker
            .traits()
            .tracking_hint
            .map_or(String::new(), |hint| format!(" ({hint})"))
    }

    async fn next(&mut self, timeout: Duration) -> Result<Step> {
        Ok(match self.tracker.next_event(timeout).await? {
            Some(Event::Pose(q)) => {
                // A pose proves tracking works, whether or not an `Available(true)` was announced.
                self.unavailable_since = None;
                Step::Pose(q)
            }
            Some(Event::Worn(w)) => {
                self.worn = w;
                if w { Step::Silence } else { Step::Removed }
            }
            Some(Event::Available(a)) => {
                let now = self.now();
                self.unavailable_since = if a { None } else { Some(now) };
                self.rec.event(if a {
                    LogEvent::TrackingAvailable
                } else {
                    LogEvent::TrackingUnavailable
                })?;
                Step::Silence
            }
            None => Step::Silence,
        })
    }

    fn tracking_lost(&self, t: f64) -> bool {
        self.unavailable_since
            .is_some_and(|since| t - since >= TRACKING_LOST_S)
    }

    fn on_tracking_lost(&mut self) -> Result<()> {
        self.rec.event(LogEvent::TrackingLost)?;
        let hint = self.tracking_hint_suffix();
        self.rec.note(&format!(
            "head tracking stopped; will recalibrate when it resumes{hint}"
        ));
        Ok(())
    }

    async fn wait_until_worn(&mut self) -> Result<()> {
        if self.worn {
            return Ok(());
        }
        self.rec.event(LogEvent::Paused)?;
        self.rec.note("paused until the earbuds are worn");
        while !self.worn {
            self.next(POLL).await?;
        }
        self.rec.event(LogEvent::Resumed)
    }

    /// Consume events for `secs`, or until the earbuds are taken off.
    async fn idle(&mut self, secs: f64) -> Result<()> {
        let end = self.now() + secs;
        while self.now() < end {
            let left = Duration::from_secs_f64(end - self.now());
            if let Step::Removed = self.next(left.min(POLL)).await? {
                break;
            }
        }
        Ok(())
    }

    /// Wait for the first head pose; `None` if the earbuds were taken off first. Tracking can take
    /// a while to start (until the device's precondition is met; see `TrackerTraits`), so this
    /// waits without a deadline.
    async fn first_pose(&mut self) -> Result<Option<Quat>> {
        let start = self.now();
        let mut noted = false;
        loop {
            match self.next(POLL).await? {
                Step::Pose(q) => return Ok(Some(q)),
                Step::Removed => return Ok(None),
                Step::Silence if !noted && self.now() - start >= TRACKING_WAIT_NOTE_S => {
                    let hint = self.tracking_hint_suffix();
                    self.rec.note(&format!(
                        "waiting for head tracking from the earbuds{hint}…"
                    ));
                    noted = true;
                }
                Step::Silence => {}
            }
        }
    }

    /// Capture the upright pose: CALIBRATION_S of poses whose 90th-percentile deviation from their
    /// mean is under CALIBRATION_MAX_P90_DEG. Silence means the head is still, so the last pose
    /// counts.
    async fn calibrate(&mut self) -> Result<Calibration> {
        let Some(mut last) = self.first_pose().await? else {
            return Ok(Calibration::Removed);
        };
        self.cues.say(Phrase::SitUpright).await?;
        for attempt in 1..=CALIBRATION_TRIES {
            let end = self.now() + CALIBRATION_S;
            let mut samples = Vec::new();
            while self.now() < end {
                match self.next(Duration::from_millis(200)).await? {
                    Step::Pose(q) => {
                        last = q;
                        samples.push(q);
                    }
                    Step::Silence => {}
                    Step::Removed => return Ok(Calibration::Removed),
                }
            }
            if samples.is_empty() {
                samples.push(last);
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

    /// Monitor until the earbuds are taken off or head tracking is lost.
    async fn monitor(&mut self, baseline: Quat) -> Result<MonitorEnd> {
        let mut det = Detector::new(detector_config(self.tracker.traits()));
        let mut last_status = f64::NEG_INFINITY;
        loop {
            let step = self.next(POLL).await?;
            let t = self.now();
            let cues = match step {
                Step::Removed => return Ok(MonitorEnd::Removed),
                Step::Silence if self.tracking_lost(t) => return Ok(MonitorEnd::TrackingLost),
                Step::Silence => det.tick(t),
                Step::Pose(q) => {
                    let angles = (baseline.conj() * q).euler();
                    self.rec.pose(angles)?;
                    let update = det.update(t, angles.pitch);
                    if let Some(upright) = update.reanchored {
                        self.rec.event(LogEvent::Reanchored { upright })?;
                    }
                    update.cues
                }
            };
            for cue in cues {
                self.rec.event(LogEvent::Cue(cue))?;
                self.cues.play(cue);
            }
            if t - last_status >= 1.0 {
                last_status = t;
                self.rec.status(det.smoothed(), det.slouching())?;
            }
        }
    }
}

/// Detector settings for a tracker: the defaults, plus corrections for what it declares.
pub(crate) fn detector_config(traits: TrackerTraits) -> Config {
    Config {
        reanchor_after_recovery: traits.pose_drifts_up,
        ..Config::default()
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
mod tests {
    use super::*;
    use crate::core::detector::Cue;
    use crate::core::tracker::TrackerTraits;
    use anyhow::bail;
    use std::cell::RefCell;
    use std::collections::VecDeque;

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

    // --- Scripted session tests (tokio's paused clock makes the waits instant). ---

    enum Script {
        /// Deliver this event immediately.
        Ev(Event),
        /// Poses of this pitch at 50 Hz for this many seconds.
        Poses(f64, f64),
        /// Poses alternating between two pitches (a moving head) for this many seconds.
        Wobble(f64, f64, f64),
        /// No events for this many seconds.
        Quiet(f64),
    }

    struct FakeTracker {
        script: VecDeque<Script>,
        drifts: bool,
    }

    impl HeadTracker for FakeTracker {
        async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
            const SAMPLE: f64 = 0.02;
            loop {
                let Some(front) = self.script.front_mut() else {
                    bail!("script finished");
                };
                match front {
                    Script::Ev(e) => {
                        let e = *e;
                        self.script.pop_front();
                        return Ok(Some(e));
                    }
                    Script::Poses(_, left) | Script::Wobble(_, _, left) | Script::Quiet(left)
                        if *left <= 1e-9 =>
                    {
                        self.script.pop_front();
                    }
                    Script::Poses(pitch, left) => {
                        *left -= SAMPLE;
                        let q = tilt(*pitch);
                        tokio::time::sleep(Duration::from_secs_f64(SAMPLE)).await;
                        return Ok(Some(Event::Pose(q)));
                    }
                    Script::Wobble(a, b, left) => {
                        *left -= SAMPLE;
                        let pitch = if (*left / SAMPLE).round() as i64 % 2 == 0 {
                            *a
                        } else {
                            *b
                        };
                        tokio::time::sleep(Duration::from_secs_f64(SAMPLE)).await;
                        return Ok(Some(Event::Pose(tilt(pitch))));
                    }
                    Script::Quiet(left) => {
                        let d = left.min(timeout.as_secs_f64());
                        *left -= d;
                        tokio::time::sleep(Duration::from_secs_f64(d)).await;
                        return Ok(None);
                    }
                }
            }
        }

        async fn stop(&mut self) -> Result<()> {
            Ok(())
        }

        fn traits(&self) -> TrackerTraits {
            TrackerTraits {
                pose_drifts_up: self.drifts,
                ..TrackerTraits::default()
            }
        }
    }

    /// Records prompts and cues instead of playing them.
    #[derive(Default)]
    struct FakeCues(RefCell<Vec<String>>);

    impl CueSink for FakeCues {
        fn play(&self, cue: Cue) {
            self.0.borrow_mut().push(format!("{cue:?}"));
        }
        async fn say(&self, p: Phrase) -> Result<()> {
            self.0.borrow_mut().push(format!("say {p:?}"));
            Ok(())
        }
    }

    /// Runs a script to its end; returns (prompts and cues, logged event names).
    async fn run_script(name: &str, script: Vec<Script>) -> (Vec<String>, Vec<String>) {
        run_script_with(name, script, false).await
    }

    async fn run_script_with(
        name: &str,
        script: Vec<Script>,
        drifts: bool,
    ) -> (Vec<String>, Vec<String>) {
        let path = std::env::temp_dir().join(format!(
            "headup-session-{name}-{}.jsonl",
            std::process::id()
        ));
        let mut rec = Recorder::new(Some(std::fs::File::create(&path).unwrap()));
        let cues = FakeCues::default();
        let mut tracker = FakeTracker {
            script: script.into(),
            drifts,
        };
        let Err(err) = run(&mut tracker, &cues, &mut rec).await;
        assert_eq!(err.to_string(), "script finished");
        let log = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let events = log
            .lines()
            .filter_map(|l| l.split(r#""event":""#).nth(1))
            .map(|rest| rest.split('"').next().unwrap_or_default().to_string())
            .collect();
        (cues.0.into_inner(), events)
    }

    use Script::*;

    // Sitting back up "past" upright re-anchors only for trackers whose pose drifts.
    #[tokio::test(start_paused = true)]
    async fn reanchors_only_for_drifting_trackers() {
        let script = || vec![Poses(0.0, 4.0), Poses(40.0, 15.0), Poses(-10.0, 3.0)];
        let (_, events) = run_script_with("drift", script(), true).await;
        assert_eq!(events, ["Tone", "Recovered", "Reanchored"]);
        let (_, events) = run_script_with("nodrift", script(), false).await;
        assert_eq!(events, ["Tone", "Recovered"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_slouch_cues_even_while_head_is_still() {
        // Upright (calibration), then look down and hold still: the device goes quiet, but timers
        // keep running on the last pose.
        let (cues, _) = run_script(
            "still",
            vec![Poses(0.0, 4.0), Poses(40.0, 2.0), Quiet(35.0)],
        )
        .await;
        assert_eq!(cues, ["say SitUpright", "say Calibrated", "Tone", "Speak"]);
    }

    #[tokio::test(start_paused = true)]
    async fn taking_earbuds_off_pauses_then_recalibrates() {
        let (cues, events) = run_script(
            "wear",
            vec![
                Poses(0.0, 4.0),
                Ev(Event::Worn(false)),
                Quiet(10.0),
                Ev(Event::Worn(true)),
                Poses(0.0, 4.0),
            ],
        )
        .await;
        assert_eq!(events, ["Paused", "Resumed"]);
        let prompts = cues.iter().filter(|c| *c == "say SitUpright").count();
        assert_eq!(prompts, 2, "{cues:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn short_unavailability_carries_posture_over() {
        let (cues, events) = run_script(
            "short-gap",
            vec![
                Poses(0.0, 4.0),
                Ev(Event::Available(false)),
                Quiet(20.0),
                Ev(Event::Available(true)),
                Quiet(20.0),
            ],
        )
        .await;
        assert_eq!(events, ["TrackingUnavailable", "TrackingAvailable"]);
        assert_eq!(
            cues,
            ["say SitUpright", "say Calibrated"],
            "no recalibration"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn long_unavailability_drops_posture_and_recalibrates() {
        // Slouch from ~4 s; tracking becomes unavailable at 6 s. Tone (~14 s) and Speak (~34 s)
        // still fire within the 30 s grace, but tracking is lost at 36 s, so the 60 s notification
        // (~64 s) must not fire from that stale data.
        let (cues, events) = run_script(
            "long-gap",
            vec![
                Poses(0.0, 4.0),
                Poses(40.0, 2.0),
                Ev(Event::Available(false)),
                Quiet(70.0),
                Ev(Event::Available(true)),
                Poses(0.0, 4.0),
            ],
        )
        .await;
        assert_eq!(
            events,
            [
                "TrackingUnavailable",
                "Tone",
                "Speak",
                "TrackingLost",
                "TrackingAvailable"
            ]
        );
        assert!(!cues.contains(&"Notify".to_string()), "{cues:?}");
        let prompts = cues.iter().filter(|c| *c == "say SitUpright").count();
        assert_eq!(prompts, 2, "recalibrates after resuming: {cues:?}");
    }

    // Tracking resumes without an `Available(true)` announcement: the poses themselves prove it,
    // and a still head afterwards must not count as lost again (this used to loop forever).
    #[tokio::test(start_paused = true)]
    async fn poses_end_unavailability_without_announcement() {
        let (_, events) = run_script(
            "no-announce",
            vec![
                Poses(0.0, 4.0),
                Ev(Event::Available(false)),
                Quiet(35.0),
                Poses(0.0, 4.0),
                Quiet(60.0),
            ],
        )
        .await;
        assert_eq!(events, ["TrackingUnavailable", "TrackingLost"]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_calibration_retries_later() {
        let (cues, events) = run_script(
            "calib-fail",
            vec![
                Wobble(0.0, 30.0, 14.0), // keeps moving through all 4 attempts
                Quiet(CALIBRATION_RETRY_S + 1.0),
                Poses(0.0, 4.0),
            ],
        )
        .await;
        assert_eq!(events, ["CalibrationFailed"]);
        let expected: Vec<String> = ["say SitUpright"]
            .into_iter()
            .chain(["say Moved"; CALIBRATION_TRIES])
            .chain(["say SitUpright", "say Calibrated"])
            .map(String::from)
            .collect();
        assert_eq!(cues, expected);
    }
}
