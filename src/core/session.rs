//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::cues::{CueSink, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::{Context, Result, ensure};
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;

const CALIBRATION: Duration = Duration::from_secs(3);
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
const CALIBRATION_TRIES: usize = 4;
/// After a failed calibration (head kept moving), wait this long before asking again.
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);
/// Say that headup is waiting if head tracking has not started after this long.
const TRACKING_WAIT_NOTE: Duration = Duration::from_secs(5);
/// Tracker unavailable this long counts as tracking lost. Shorter gaps carry the posture over;
/// after a longer gap the head may have moved unseen and resumed tracking may measure from a new
/// pose, so headup drops its slouch state (no reminders from stale data) and recalibrates when
/// tracking resumes. Silence while available just means the head is still and never ends
/// monitoring.
const TRACKING_LOST: Duration = Duration::from_secs(30);
/// How often the live status line is refreshed.
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// How long to wait for a tracker event before re-checking timers.
const POLL: Duration = Duration::from_secs(1);

/// Why a cycle (wait until worn, calibrate, monitor) ended early. Carried by `?`, so every wait
/// in the session gives up the same way when the earbuds come off.
enum Interrupt {
    /// The earbuds were taken off.
    Removed,
    /// The tracker was unavailable for TRACKING_LOST: the posture reference can't be trusted.
    TrackingLost,
    /// A tracker or output error: ends the session.
    Fatal(anyhow::Error),
}

impl From<anyhow::Error> for Interrupt {
    fn from(e: anyhow::Error) -> Self {
        Interrupt::Fatal(e)
    }
}

/// A session step's outcome: its value, or why the cycle ended early.
type Flow<T> = std::result::Result<T, Interrupt>;

/// A delivered pose: when it was measured (since the session started) and the orientation.
type Pose = (Duration, Quat);

/// The session's timeline: time since the session started (monotonic, unlike log time; exact
/// integer nanoseconds), and how far along it the delivered posture reaches. A backend that holds
/// samples delivers each pose `lag` after it was measured, so posture is known only up to
/// `now - lag`. Reminders and calibration judge posture up to there, never beyond it, so a
/// reminder cannot play after the user has already sat up. During silence that point advances
/// with real time, because silence means the posture is unchanged. With a backend that holds
/// nothing, `lag` is 0.
struct Timeline {
    start: Instant,
    /// Measurement time of the latest pose, and how long after measurement it arrived.
    measured: Duration,
    lag: Duration,
}

impl Timeline {
    fn new() -> Self {
        Timeline {
            start: Instant::now(),
            measured: Duration::ZERO,
            lag: Duration::ZERO,
        }
    }

    /// Time since the session started.
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    /// How far the delivered posture reaches. Never before the latest pose's measurement time,
    /// since its delivery is in the past.
    fn known_until(&self) -> Duration {
        self.now() - self.lag
    }

    /// Take delivery of a pose measured at `at`; returns its measurement time. A pose from before
    /// the session or out of order is a tracker bug.
    fn deliver(&mut self, at: Instant) -> Result<Duration> {
        let measured = at
            .checked_duration_since(self.start)
            .context("tracker sent a pose from before the session started")?;
        ensure!(
            measured >= self.measured,
            "tracker sent poses out of order: {measured:?} after {:?}",
            self.measured
        );
        (self.measured, self.lag) = (measured, self.now() - measured);
        Ok(measured)
    }

    /// The instant of session time `t`.
    fn instant(&self, t: Duration) -> Instant {
        self.start + t
    }
}

/// What the tracker has reported about itself.
struct TrackerStatus {
    worn: bool,
    /// When the tracker last became unavailable (`None` while available).
    unavailable_since: Option<Duration>,
}

impl TrackerStatus {
    /// Assume worn and available until told otherwise.
    fn new() -> Self {
        TrackerStatus {
            worn: true,
            unavailable_since: None,
        }
    }

    /// A pose proves tracking works, whether or not an `Available(true)` was announced.
    fn pose_arrived(&mut self) {
        self.unavailable_since = None;
    }

    /// The wear state changed; `true` if the earbuds were just taken off.
    fn worn_changed(&mut self, worn: bool) -> bool {
        std::mem::replace(&mut self.worn, worn) && !worn
    }

    fn availability_changed(&mut self, available: bool, now: Duration) {
        self.unavailable_since = (!available).then_some(now);
    }

    /// Unavailable for TRACKING_LOST by `now`.
    fn lost(&self, now: Duration) -> bool {
        self.unavailable_since
            .is_some_and(|since| now - since >= TRACKING_LOST)
    }
}

/// One calibration attempt: the poses measured in a window that opens when the prompt ends and
/// closes once the delivered posture covers CALIBRATION.
struct CalibrationWindow {
    start: Duration,
    samples: Vec<Quat>,
}

/// What an attempt measured: the mean pose, and the 90th-percentile angle (degrees) of the
/// samples from it.
struct CalibrationResult {
    mean: Quat,
    p90: f64,
    samples: usize,
}

impl CalibrationWindow {
    fn open(start: Duration) -> Self {
        CalibrationWindow {
            start,
            samples: Vec::new(),
        }
    }

    /// The window is complete once the delivered posture reaches this.
    fn closes_at(&self) -> Duration {
        self.start + CALIBRATION
    }

    /// A pose measured at `measured`; poses measured before the window opened (during the
    /// prompt, delivered late by a backend that holds samples) don't count.
    fn add(&mut self, measured: Duration, q: Quat) {
        if measured >= self.start {
            self.samples.push(q);
        }
    }

    /// The result; with no pose in the window the head was still, so `last` counts.
    fn result(mut self, last: Quat) -> CalibrationResult {
        if self.samples.is_empty() {
            self.samples.push(last);
        }
        let (mean, p90) = mean_and_p90(&self.samples);
        CalibrationResult {
            mean,
            p90,
            samples: self.samples.len(),
        }
    }
}

struct Session<'a, T: HeadTracker, C: CueSink> {
    tracker: &'a mut T,
    cues: &'a C,
    rec: &'a mut Recorder,
    time: Timeline,
    status: TrackerStatus,
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
        time: Timeline::new(),
        status: TrackerStatus::new(),
    };
    loop {
        match s.cycle().await {
            Ok(()) | Err(Interrupt::Removed) => {}
            Err(Interrupt::TrackingLost) => s.on_tracking_lost()?,
            Err(Interrupt::Fatal(e)) => return Err(e),
        }
    }
}

impl<T: HeadTracker, C: CueSink> Session<'_, T, C> {
    /// Time since the session started; see [`Timeline`].
    fn now(&self) -> Duration {
        self.time.now()
    }

    /// The tracker's hint as a message suffix (" (hint)"), or "" if it has none.
    fn tracking_hint_suffix(&self) -> String {
        self.tracker
            .traits()
            .tracking_hint
            .map_or(String::new(), |hint| format!(" ({hint})"))
    }

    /// One cycle: wait until worn, calibrate, then monitor (or back off after a failed
    /// calibration).
    async fn cycle(&mut self) -> Flow<()> {
        self.wait_until_worn().await?;
        match self.calibrate().await? {
            Some(baseline) => self.monitor(baseline).await,
            None => {
                self.rec.event(LogEvent::CalibrationFailed)?;
                self.idle(CALIBRATION_RETRY).await
            }
        }
    }

    /// The next pose within `timeout`, or `None` for silence. Taking the earbuds off interrupts.
    async fn next(&mut self, timeout: Duration) -> Flow<Option<Pose>> {
        match self.tracker.next_event(timeout).await? {
            Some(Event::Pose { at, orientation }) => {
                let measured = self.time.deliver(at)?;
                self.status.pose_arrived();
                return Ok(Some((measured, orientation)));
            }
            Some(Event::Worn(w)) => {
                if self.status.worn_changed(w) {
                    return Err(Interrupt::Removed);
                }
            }
            Some(Event::Available(a)) => {
                let now = self.now();
                self.status.availability_changed(a, now);
                self.rec.event(if a {
                    LogEvent::TrackingAvailable
                } else {
                    LogEvent::TrackingUnavailable
                })?;
            }
            None => {}
        }
        Ok(None)
    }

    fn on_tracking_lost(&mut self) -> Result<()> {
        self.rec.event(LogEvent::TrackingLost)?;
        let hint = self.tracking_hint_suffix();
        self.rec.note(&format!(
            "head tracking stopped; will recalibrate when it resumes{hint}"
        ));
        Ok(())
    }

    async fn wait_until_worn(&mut self) -> Flow<()> {
        if self.status.worn {
            return Ok(());
        }
        self.rec.event(LogEvent::Paused)?;
        self.rec.note("paused until the earbuds are worn");
        while !self.status.worn {
            self.next(POLL).await?;
        }
        Ok(self.rec.event(LogEvent::Resumed)?)
    }

    /// Consume events for `period` (taking the earbuds off interrupts).
    async fn idle(&mut self, period: Duration) -> Flow<()> {
        let end = self.now() + period;
        while self.now() < end {
            let left = end - self.now();
            self.next(left.min(POLL)).await?;
        }
        Ok(())
    }

    /// Wait for the first head pose. Tracking can take a while to start (until the device's
    /// precondition is met; see `TrackerTraits`), so this waits without a deadline.
    async fn first_pose(&mut self) -> Flow<Quat> {
        let start = self.now();
        let mut noted = false;
        loop {
            if let Some((_, q)) = self.next(POLL).await? {
                return Ok(q);
            }
            if !noted && self.now() - start >= TRACKING_WAIT_NOTE {
                let hint = self.tracking_hint_suffix();
                self.rec.note(&format!(
                    "waiting for head tracking from the earbuds{hint}…"
                ));
                noted = true;
            }
        }
    }

    /// Capture the upright pose: a [`CalibrationWindow`] of poses whose 90th-percentile deviation
    /// from their mean is under CALIBRATION_MAX_P90_DEG. `None` if the head kept moving on every
    /// attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        let mut last = self.first_pose().await?;
        self.cues.say(Phrase::SitUpright).await?;
        for attempt in 1..=CALIBRATION_TRIES {
            let mut window = CalibrationWindow::open(self.now());
            while self.time.known_until() < window.closes_at() {
                if let Some((measured, q)) = self.next(Duration::from_millis(200)).await? {
                    last = q;
                    window.add(measured, q);
                }
            }
            let result = window.result(last);
            self.rec.note(&format!(
                "calibration attempt {attempt}: {} samples, p90 deviation {:.1}°",
                result.samples, result.p90
            ));
            if result.p90 < CALIBRATION_MAX_P90_DEG {
                self.cues.say(Phrase::Calibrated).await?;
                return Ok(Some(result.mean));
            }
            self.cues.say(Phrase::Moved).await?;
        }
        Ok(None)
    }

    /// Monitor until interrupted: earbuds off, tracking lost, or an error.
    async fn monitor(&mut self, baseline: Quat) -> Flow<()> {
        let mut det = Detector::new(detector_config(self.tracker.traits()));
        let mut last_status: Option<Duration> = None;
        loop {
            // Wake when the known posture reaches the next reminder (now, if it already has). Time
            // is exact, so at that point `tick` fires it.
            let wait = det.next_due().map_or(POLL, |due| {
                due.saturating_sub(self.time.known_until()).min(POLL)
            });
            let pose = self.next(wait).await?;
            let t = self.now();
            let cues = match pose {
                None if self.status.lost(t) => return Err(Interrupt::TrackingLost),
                None => det.tick(self.time.known_until()),
                Some((measured, q)) => {
                    let angles = (baseline.conj() * q).euler();
                    self.rec.pose(self.time.instant(measured), angles)?;
                    let update = det.update(measured, angles.pitch);
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
            if last_status.is_none_or(|last| t - last >= STATUS_EVERY) {
                last_status = Some(t);
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
    use crate::core::tracker::TrackerTraits;
    use crate::testing::{Outcome, run_session};
    use anyhow::bail;
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

    /// `x` seconds, to the millisecond (script times are exact integers from here on).
    fn secs(x: f64) -> Duration {
        Duration::from_millis((x * 1000.0).round() as u64)
    }

    enum Script {
        /// Deliver this event immediately.
        Ev(Event),
        /// Poses of this pitch at 50 Hz for this long (a whole number of samples).
        Poses(f64, Duration),
        /// Poses alternating between two pitches (a moving head) for this long.
        Wobble(f64, f64, Duration),
        /// No events for this long.
        Quiet(Duration),
    }

    struct FakeTracker {
        script: VecDeque<Script>,
        drifts: bool,
        /// How long after measurement each pose is delivered (a backend that holds samples).
        lag: Duration,
        /// Time until the next pose sample is due.
        until_sample: Duration,
    }

    impl FakeTracker {
        fn new(script: Vec<Script>, drifts: bool, lag: Duration) -> Self {
            FakeTracker {
                script: script.into(),
                drifts,
                lag,
                // The first pose is measured one sample in and delivered `lag` later.
                until_sample: SAMPLE + lag,
            }
        }
    }

    const SAMPLE: Duration = Duration::from_millis(20);

    /// Like a real backend: wait for the next pose sample (of `pitch`, `left` of poses remaining)
    /// and deliver it `lag` after its measurement, or time out first.
    async fn sample(
        until_sample: &mut Duration,
        lag: Duration,
        timeout: Duration,
        left: &mut Duration,
        pitch: f64,
    ) -> Option<Event> {
        if timeout < *until_sample {
            *until_sample -= timeout;
            tokio::time::sleep(timeout).await;
            return None;
        }
        tokio::time::sleep(*until_sample).await;
        *until_sample = SAMPLE;
        *left = left
            .checked_sub(SAMPLE)
            .expect("script pose spans are whole samples");
        Some(Event::Pose {
            at: Instant::now() - lag,
            orientation: tilt(pitch),
        })
    }

    impl HeadTracker for FakeTracker {
        async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
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
                        if left.is_zero() =>
                    {
                        self.script.pop_front();
                    }
                    Script::Poses(pitch, left) => {
                        let pitch = *pitch;
                        return Ok(
                            sample(&mut self.until_sample, self.lag, timeout, left, pitch).await,
                        );
                    }
                    Script::Wobble(a, b, left) => {
                        let next = (*left - SAMPLE).as_nanos() / SAMPLE.as_nanos();
                        let pitch = if next.is_multiple_of(2) { *a } else { *b };
                        return Ok(
                            sample(&mut self.until_sample, self.lag, timeout, left, pitch).await,
                        );
                    }
                    Script::Quiet(left) => {
                        let d = (*left).min(timeout);
                        *left -= d;
                        tokio::time::sleep(d).await;
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

    /// How the simulated tracker and cue output behave.
    #[derive(Default)]
    struct Sim {
        /// The tracker declares `pose_drifts_up`.
        drifts: bool,
        /// Each pose is delivered this long after it is measured.
        lag: Duration,
        /// Each spoken prompt takes this long.
        prompt: Duration,
    }

    /// Run `script` to its end and report what the session did.
    async fn simulate(script: Vec<Script>, sim: Sim) -> Outcome {
        let mut tracker = FakeTracker::new(script, sim.drifts, sim.lag);
        run_session(&mut tracker, sim.prompt).await
    }

    use Script::*;

    // Sitting back up "past" upright re-anchors only for trackers whose pose drifts.
    #[tokio::test(start_paused = true)]
    async fn reanchors_only_for_drifting_trackers() {
        let script = || {
            vec![
                Poses(0.0, secs(4.0)),
                Poses(40.0, secs(15.0)),
                Poses(-10.0, secs(3.0)),
            ]
        };
        let drifting = Sim {
            drifts: true,
            ..Sim::default()
        };
        let run = simulate(script(), drifting).await;
        assert_eq!(run.events, ["Tone", "Recovered", "Reanchored"]);
        let run = simulate(script(), Sim::default()).await;
        assert_eq!(run.events, ["Tone", "Recovered"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_slouch_cues_even_while_head_is_still() {
        // Upright (calibration), then look down and hold still: the device goes quiet, but timers
        // keep running on the last pose.
        let script = vec![
            Poses(0.0, secs(4.0)),
            Poses(40.0, secs(2.0)),
            Quiet(secs(35.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(
            run.names(),
            ["say SitUpright", "say Calibrated", "Tone", "Speak"]
        );
    }

    // While the head is held still the tracker is silent, and the tone must still come right at
    // 10 s of slouching, not at the next once-a-second check.
    #[tokio::test(start_paused = true)]
    async fn tone_is_on_time_while_head_is_still() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Poses(40.0, secs(2.0)),
            Quiet(secs(15.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        // The slouch starts at 4 s; smoothing crosses the threshold within 0.1 s of that.
        let tone = run.first("Tone");
        assert!(
            (secs(14.0)..secs(14.15)).contains(&tone),
            "tone at {tone:?}"
        );
    }

    // A backend that holds samples (here 1.5 s) delivers a sit-up late. Sitting up 9.5 s into a
    // slouch, before the 10 s tone, must give no tone: reminders wait for delivered posture.
    #[tokio::test(start_paused = true)]
    async fn no_reminder_after_sitting_up_when_poses_arrive_late() {
        // Upright long enough for calibration to see 3 s of posture after the 1.5 s lag.
        let script = vec![
            Poses(0.0, secs(6.0)),
            Poses(40.0, secs(9.5)),
            Poses(0.0, secs(6.0)),
        ];
        let lagging = Sim {
            lag: secs(1.5),
            ..Sim::default()
        };
        let run = simulate(script, lagging).await;
        assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
    }

    // With the same lag, a slouch held past 10 s still cues, once its posture has arrived: the
    // tone is due ~10.1 s into the slouch (measured from 6 s) and comes the 1.5 s lag later.
    #[tokio::test(start_paused = true)]
    async fn late_poses_delay_the_reminder_by_the_lag() {
        let script = vec![
            Poses(0.0, secs(6.0)),
            Poses(40.0, secs(14.0)),
            Poses(0.0, secs(4.0)),
        ];
        let lagging = Sim {
            lag: secs(1.5),
            ..Sim::default()
        };
        let tone = simulate(script, lagging).await.first("Tone");
        assert!((secs(17.5)..secs(17.7)).contains(&tone), "tone at {tone:?}");
    }

    // The head moves while the 2 s "sit upright" prompt plays and is still afterwards. With poses
    // delivered 1.5 s late, the ones measured during the prompt arrive inside the capture window;
    // they must not count, so calibration succeeds at once.
    #[tokio::test(start_paused = true)]
    async fn calibration_ignores_poses_measured_during_the_prompt() {
        let script = vec![
            Poses(0.0, secs(0.1)),
            Wobble(0.0, 30.0, secs(2.0)),
            Poses(0.0, secs(6.0)),
        ];
        let sim = Sim {
            lag: secs(1.5),
            prompt: secs(2.0),
            ..Sim::default()
        };
        let run = simulate(script, sim).await;
        assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
    }

    #[tokio::test(start_paused = true)]
    async fn taking_earbuds_off_pauses_then_recalibrates() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Worn(false)),
            Quiet(secs(10.0)),
            Ev(Event::Worn(true)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["Paused", "Resumed"]);
        assert_eq!(run.count("say SitUpright"), 2, "{:?}", run.names());
    }

    #[tokio::test(start_paused = true)]
    async fn short_unavailability_carries_posture_over() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Available(false)),
            Quiet(secs(20.0)),
            Ev(Event::Available(true)),
            Quiet(secs(20.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["TrackingUnavailable", "TrackingAvailable"]);
        assert_eq!(
            run.names(),
            ["say SitUpright", "say Calibrated"],
            "no recalibration"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn long_unavailability_drops_posture_and_recalibrates() {
        // Slouch from ~4 s; tracking becomes unavailable at 6 s. Tone (~14 s) and Speak (~34 s)
        // still fire within the 30 s grace, but tracking is lost at 36 s, so the 60 s notification
        // (~64 s) must not fire from that stale data.
        let script = vec![
            Poses(0.0, secs(4.0)),
            Poses(40.0, secs(2.0)),
            Ev(Event::Available(false)),
            Quiet(secs(70.0)),
            Ev(Event::Available(true)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(
            run.events,
            [
                "TrackingUnavailable",
                "Tone",
                "Speak",
                "TrackingLost",
                "TrackingAvailable"
            ]
        );
        assert_eq!(run.count("Notify"), 0, "{:?}", run.names());
        let prompts = run.count("say SitUpright");
        assert_eq!(prompts, 2, "recalibrates after resuming: {:?}", run.names());
    }

    // Tracking resumes without an `Available(true)` announcement: the poses themselves prove it,
    // and a still head afterwards must not count as lost again.
    #[tokio::test(start_paused = true)]
    async fn poses_end_unavailability_without_announcement() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Available(false)),
            Quiet(secs(35.0)),
            Poses(0.0, secs(4.0)),
            Quiet(secs(60.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["TrackingUnavailable", "TrackingLost"]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_calibration_retries_later() {
        let script = vec![
            Wobble(0.0, 30.0, secs(14.0)), // keeps moving through all 4 attempts
            Quiet(CALIBRATION_RETRY + secs(1.0)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["CalibrationFailed"]);
        let expected: Vec<&str> = ["say SitUpright"]
            .into_iter()
            .chain(["say Moved"; CALIBRATION_TRIES])
            .chain(["say SitUpright", "say Calibrated"])
            .collect();
        assert_eq!(run.names(), expected);
    }
}
