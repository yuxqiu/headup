//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::cues::{CueSink, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::{Context, Result, ensure};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;

/// How long the head must hold steady, after the prompt, to calibrate.
const CALIBRATION: Duration = Duration::from_secs(3);
const CALIBRATION_MAX_P90_DEG: f64 = 8.0;
/// An attempt fails ("you moved") if the head has not held steady for CALIBRATION by this long
/// after the attempt's first pose.
const CALIBRATION_ATTEMPT: Duration = Duration::from_secs(5);
const CALIBRATION_TRIES: usize = 4;
/// Some trackers send nothing while the head is still. If no pose has arrived this long after the
/// prompt, ask the user to move a little. Ask again every NUDGE_EVERY.
const NUDGE_AFTER: Duration = Duration::from_secs(5);
const NUDGE_EVERY: Duration = Duration::from_secs(15);
/// After a failed calibration (head kept moving), wait this long before asking again.
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);
/// Say that headup is waiting if head tracking has not started after this long.
const TRACKING_WAIT_NOTE: Duration = Duration::from_secs(5);
/// How often the live status line is refreshed.
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// How long to wait for a tracker event before re-checking timers.
const POLL: Duration = Duration::from_secs(1);

/// Why a cycle ended early. A cycle waits until the earbuds are worn, calibrates, then monitors.
/// `?` carries the interrupt, so every wait in the session gives up the same way when the earbuds
/// come off.
enum Interrupt {
    /// The earbuds were taken off.
    Removed,
    /// The tracker became unavailable. The head may move unseen until tracking resumes, and
    /// resumed tracking may measure from a new pose. So the posture reference can't be trusted.
    /// The session drops the slouch state, so no reminder plays from stale data, and recalibrates
    /// once tracking resumes. Silence while tracking is available only means the head is still. It
    /// never ends a cycle.
    Unavailable,
    /// A tracker or output error. It ends the session.
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

/// The session's timeline. Session time is the time since the session started. It is monotonic,
/// unlike log time, and counts exact integer nanoseconds.
///
/// The timeline also tracks how far the delivered posture reaches. A backend that holds samples
/// delivers each pose `lag` after it was measured, so posture is known only up to `now - lag`.
/// Reminders and calibration judge posture up to that point, never beyond it. So a reminder
/// cannot play after the user has already sat up. During silence, that point advances with real
/// time, because silence means the posture is unchanged. With a backend that holds nothing, `lag`
/// is 0.
struct Timeline {
    start: Instant,
    /// Measurement time of the latest pose.
    measured: Duration,
    /// How long after its measurement the latest pose arrived.
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

    /// How far the delivered posture reaches. This is never before the latest pose's measurement
    /// time, because that pose was delivered in the past.
    fn known_until(&self) -> Duration {
        self.now() - self.lag
    }

    /// Take delivery of a pose measured at `at`, and return its measurement time in session time.
    /// A pose from before the session, or out of order, is a tracker bug and an error.
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

/// Whether head tracking flows, as the tracker has shown it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tracking {
    /// Nothing heard yet: silence may mean tracking has not started, not a still head.
    Starting,
    /// A pose or `Available(true)`: silence means the head is still.
    Available,
    /// `Available(false)`: the head may move unseen. Poses may still arrive: data measured
    /// before, or trailing data. They do not end this state. Only `Available(true)` does.
    Unavailable,
}

/// What the tracker has reported about itself.
struct TrackerStatus {
    worn: bool,
    tracking: Tracking,
}

impl TrackerStatus {
    /// Assume worn until told otherwise.
    fn new() -> Self {
        TrackerStatus {
            worn: true,
            tracking: Tracking::Starting,
        }
    }

    /// Record the reported wear state. Returns `true` if the earbuds were just taken off.
    fn worn_changed(&mut self, worn: bool) -> bool {
        std::mem::replace(&mut self.worn, worn) && !worn
    }

    /// A pose arrived: tracking has started.
    fn pose_arrived(&mut self) {
        if self.tracking == Tracking::Starting {
            self.tracking = Tracking::Available;
        }
    }

    /// Record the reported availability. Returns `true` if tracking was available and just
    /// stopped.
    fn availability_changed(&mut self, available: bool) -> bool {
        let next = if available {
            Tracking::Available
        } else {
            Tracking::Unavailable
        };
        std::mem::replace(&mut self.tracking, next) == Tracking::Available && !available
    }
}

/// One calibration attempt: the poses measured since it started.
///
/// It succeeds once the delivered posture shows the head steady for CALIBRATION. The span starts
/// at the latest pose measured at least CALIBRATION before the known end. That pose and every
/// pose after it must lie within CALIBRATION_MAX_P90_DEG of their mean, at the 90th percentile.
/// Silence after a pose means the head stayed there. Only poses measured after the attempt
/// started count, so data from before the prompt never calibrates.
struct CalibrationAttempt {
    start: Duration,
    /// Poses (measurement time, orientation), oldest first. The first is the latest pose that can
    /// still begin a span.
    poses: VecDeque<(Duration, Quat)>,
    /// When the attempt's first pose was measured.
    first: Option<Duration>,
}

/// A span of poses to calibrate on. It holds the mean pose, the 90th-percentile angle (degrees)
/// of the poses from that mean, and the number of poses. The span is steady only if `p90` is
/// below CALIBRATION_MAX_P90_DEG.
struct CalibrationResult {
    mean: Quat,
    p90: f64,
    samples: usize,
}

impl CalibrationAttempt {
    fn start(start: Duration) -> Self {
        CalibrationAttempt {
            start,
            poses: VecDeque::new(),
            first: None,
        }
    }

    /// Add a pose measured at `measured`. Poses measured before the attempt started don't count.
    /// A backend that holds samples can deliver such poses late, for example ones measured during
    /// the prompt.
    fn add(&mut self, measured: Duration, q: Quat) {
        if measured >= self.start {
            self.first.get_or_insert(measured);
            self.poses.push_back((measured, q));
        }
    }

    /// The span of length CALIBRATION that ends at `known`, if poses cover it. `known` is how far
    /// the delivered posture reaches.
    fn span(&mut self, known: Duration) -> Option<CalibrationResult> {
        let from = known.checked_sub(CALIBRATION)?;
        let begin = self.poses.iter().rposition(|&(m, _)| m <= from)?;
        self.poses.drain(..begin);
        let samples: Vec<Quat> = self.poses.iter().map(|&(_, q)| q).collect();
        let (mean, p90) = mean_and_p90(&samples);
        Some(CalibrationResult {
            mean,
            p90,
            samples: samples.len(),
        })
    }
}

struct Session<'a, T: HeadTracker, C: CueSink> {
    tracker: &'a mut T,
    cues: &'a C,
    rec: &'a mut Recorder,
    time: Timeline,
    status: TrackerStatus,
}

/// Calibrate, then monitor. Pause while the earbuds are off. Recalibrate when they are put back
/// on, because reseating changes how the earbud sits. Also recalibrate after tracking was
/// unavailable. Returns only with a tracker or output error. The caller handles Ctrl-C.
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
            Err(Interrupt::Unavailable) => s.on_unavailable(),
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

    /// Wait up to `timeout` for the next tracker event. Returns the pose if the event was one, and
    /// `None` otherwise. Taking the earbuds off, or tracking becoming unavailable, interrupts.
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
                self.rec.event(if a {
                    LogEvent::TrackingAvailable
                } else {
                    LogEvent::TrackingUnavailable
                })?;
                if self.status.availability_changed(a) {
                    return Err(Interrupt::Unavailable);
                }
            }
            None => {}
        }
        Ok(None)
    }

    fn on_unavailable(&mut self) {
        let hint = self.tracking_hint_suffix();
        self.rec.note(&format!(
            "head tracking is unavailable; will calibrate when it resumes{hint}"
        ));
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

    /// Consume events for `period`. Interrupts as [`Session::next`] does.
    async fn idle(&mut self, period: Duration) -> Flow<()> {
        let end = self.now() + period;
        while self.now() < end {
            let left = end - self.now();
            self.next(left.min(POLL)).await?;
        }
        Ok(())
    }

    /// Wait until tracking is available (see [`Tracking`]). Tracking may not start until a device
    /// precondition is met (see `TrackerTraits`). That can take a while, so this waits without a
    /// deadline.
    async fn wait_for_tracking(&mut self) -> Flow<()> {
        let start = self.now();
        let mut noted = false;
        while self.status.tracking != Tracking::Available {
            self.next(POLL).await?;
            if !noted && self.now() - start >= TRACKING_WAIT_NOTE {
                let hint = self.tracking_hint_suffix();
                self.rec.note(&format!(
                    "waiting for head tracking from the earbuds{hint}…"
                ));
                noted = true;
            }
        }
        Ok(())
    }

    /// Capture the upright pose: the head held steady for CALIBRATION after the prompt (see
    /// [`CalibrationAttempt`]). A still head may send nothing. So while no pose has arrived, the
    /// user is asked to move a little. Returns `None` if the head kept moving on every attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        self.wait_for_tracking().await?;
        self.cues.say(Phrase::SitUpright).await?;
        for attempt in 1..=CALIBRATION_TRIES {
            let mut cal = CalibrationAttempt::start(self.now());
            let mut nudge_at = cal.start + NUDGE_AFTER;
            let result = loop {
                if let Some((measured, q)) = self.next(Duration::from_millis(200)).await? {
                    cal.add(measured, q);
                }
                let known = self.time.known_until();
                let span = cal.span(known);
                match (span, cal.first) {
                    (Some(span), _) if span.p90 < CALIBRATION_MAX_P90_DEG => break Some(span),
                    (span, Some(first)) if known >= first + CALIBRATION_ATTEMPT => break span,
                    (_, None) if self.now() >= nudge_at => {
                        self.cues.say(Phrase::MoveHead).await?;
                        nudge_at = self.now() + NUDGE_EVERY;
                    }
                    _ => {}
                }
            };
            let (samples, p90) = result
                .as_ref()
                .map_or((0, f64::NAN), |r| (r.samples, r.p90));
            self.rec.note(&format!(
                "calibration attempt {attempt}: {samples} samples, p90 deviation {p90:.1}°"
            ));
            if let Some(span) = result
                && span.p90 < CALIBRATION_MAX_P90_DEG
            {
                self.cues.say(Phrase::Calibrated).await?;
                return Ok(Some(span.mean));
            }
            self.cues.say(Phrase::Moved).await?;
        }
        Ok(None)
    }

    /// Monitor until interrupted: earbuds off, tracking unavailable, or an error.
    async fn monitor(&mut self, baseline: Quat) -> Flow<()> {
        let mut det = Detector::new(detector_config(self.tracker.traits()));
        let mut last_status: Option<Duration> = None;
        loop {
            // Wake when the known posture reaches the next reminder, or now if it already has.
            // Times are exact, so `tick` fires the reminder at that point.
            let wait = det.next_due().map_or(POLL, |due| {
                due.saturating_sub(self.time.known_until()).min(POLL)
            });
            let pose = self.next(wait).await?;
            let t = self.now();
            let cues = match pose {
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

    /// `x` seconds, rounded to the millisecond. Script times are then exact integers.
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

    /// Wait for the next pose sample, like a real backend. The pose has pitch `pitch`, and `left`
    /// is how much of the pose span remains. The pose is delivered `lag` after its measurement.
    /// Returns `None` if `timeout` passes first.
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

    // With the same lag, a slouch held past 10 s still cues once its posture has arrived. The
    // slouch starts at 6 s. The tone is due about 10.1 s into it and plays 1.5 s (the lag) later.
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

    // The head moves while the 2 s "sit upright" prompt plays, then holds still. Poses arrive
    // 1.5 s late, so the ones measured during the prompt arrive inside the capture window. They
    // must not count, so calibration succeeds on the first attempt.
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

    // The slouch starts at about 4 s. Tracking becomes unavailable at 6 s, before the tone is due
    // at about 14 s. No reminder may fire from that stale posture. When tracking resumes, the
    // session calibrates again, however short the gap.
    #[tokio::test(start_paused = true)]
    async fn unavailability_drops_the_slouch_and_recalibrates() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Poses(40.0, secs(2.0)),
            Ev(Event::Available(false)),
            Quiet(secs(20.0)),
            Ev(Event::Available(true)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["TrackingUnavailable", "TrackingAvailable"]);
        let prompts = run.count("say SitUpright");
        assert_eq!(prompts, 2, "recalibrates after resuming: {:?}", run.names());
    }

    // A still head sends no poses. When tracking resumes, headup asks the user to sit upright at
    // once. With no pose 5 s later, it asks them to move a little. It then calibrates on the poses
    // that follow. Moving, then holding still, does not count as "you moved".
    #[tokio::test(start_paused = true)]
    async fn still_head_is_asked_to_move_when_tracking_resumes() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Available(false)),
            Quiet(secs(10.0)),
            Ev(Event::Available(true)),
            Quiet(secs(6.0)),
            Wobble(0.0, 20.0, secs(1.0)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(
            run.names(),
            [
                "say SitUpright",
                "say Calibrated",
                "say SitUpright",
                "say MoveHead",
                "say Calibrated"
            ]
        );
    }

    // The same after putting the earbuds back on: the calibration does not depend on the tracker
    // sending poses by itself.
    #[tokio::test(start_paused = true)]
    async fn still_head_is_asked_to_move_after_rewearing() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Worn(false)),
            Quiet(secs(5.0)),
            Ev(Event::Worn(true)),
            Quiet(secs(6.0)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.count("say MoveHead"), 1, "{:?}", run.names());
        assert_eq!(run.count("say Calibrated"), 2, "{:?}", run.names());
    }

    // The nudge repeats while nothing arrives.
    #[tokio::test(start_paused = true)]
    async fn nudge_repeats_while_no_pose_arrives() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Available(false)),
            Ev(Event::Available(true)),
            Quiet(secs(36.0)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        // Nudges at 5, 20 and 35 s after the prompt.
        assert_eq!(run.count("say MoveHead"), 3, "{:?}", run.names());
    }

    // At startup no pose has arrived yet, so silence may mean tracking has not started (e.g. a
    // device precondition is not met). headup waits without prompting or nudging.
    #[tokio::test(start_paused = true)]
    async fn startup_waits_silently_for_tracking() {
        let script = vec![Quiet(secs(30.0)), Poses(0.0, secs(4.0))];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
    }

    // Poses can still arrive after the tracker becomes unavailable (data measured before it, or
    // trailing data). They do not start a calibration: that waits for `Available(true)`.
    #[tokio::test(start_paused = true)]
    async fn poses_after_unavailability_do_not_start_calibration() {
        let script = vec![
            Poses(0.0, secs(4.0)),
            Ev(Event::Available(false)),
            Poses(0.0, secs(1.0)),
            Quiet(secs(40.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["TrackingUnavailable"]);
        assert_eq!(run.names(), ["say SitUpright", "say Calibrated"]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_calibration_retries_later() {
        let script = vec![
            Wobble(0.0, 30.0, secs(30.0)), // keeps moving through all 4 attempts
            Quiet(CALIBRATION_RETRY + secs(1.0)),
            Poses(0.0, secs(4.0)),
        ];
        let run = simulate(script, Sim::default()).await;
        assert_eq!(run.events, ["CalibrationFailed"]);
        let expected: Vec<&str> = ["say SitUpright"]
            .into_iter()
            .chain(["say Moved"; CALIBRATION_TRIES])
            // The retry prompts at once; the head is still and silent, so it is asked to move.
            .chain(["say SitUpright", "say MoveHead", "say Calibrated"])
            .collect();
        assert_eq!(run.names(), expected);
    }
}
