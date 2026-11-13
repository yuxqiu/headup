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
struct Timeline {
    start: Instant,
    /// Measurement time of the latest pose.
    measured: Duration,
}

impl Timeline {
    fn new() -> Self {
        Timeline {
            start: Instant::now(),
            measured: Duration::ZERO,
        }
    }

    /// Time since the session started.
    fn now(&self) -> Duration {
        self.start.elapsed()
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
        self.measured = measured;
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
/// It succeeds once the posture shows the head steady for CALIBRATION. The span starts at the
/// latest pose measured at least CALIBRATION before now. That pose and every
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

    /// Add a pose measured at `measured`. Poses measured before the attempt started don't count:
    /// those measured during a prompt are delivered after it.
    fn add(&mut self, measured: Duration, q: Quat) {
        if measured >= self.start {
            self.first.get_or_insert(measured);
            self.poses.push_back((measured, q));
        }
    }

    /// The span of length CALIBRATION that ends at `now`, if poses cover it. Every pose measured
    /// before `now` must have been added.
    fn span(&mut self, now: Duration) -> Option<CalibrationResult> {
        let from = now.checked_sub(CALIBRATION)?;
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

    /// Wait up to `timeout` for tracker events, then take every event already waiting, and return
    /// the poses among them. Afterwards the tracker has nothing left to deliver, so the posture is
    /// known up to now: silence means the head has not moved. Taking the earbuds off, or tracking
    /// becoming unavailable, interrupts.
    async fn next(&mut self, timeout: Duration) -> Flow<Vec<Pose>> {
        let mut poses = Vec::new();
        let mut wait = timeout;
        while let Some(event) = self.tracker.next_event(wait).await? {
            poses.extend(self.handle(event)?);
            wait = Duration::ZERO;
        }
        Ok(poses)
    }

    /// Record one tracker event. Returns the pose if it was one.
    fn handle(&mut self, event: Event) -> Flow<Option<Pose>> {
        match event {
            Event::Pose { at, orientation } => {
                let measured = self.time.deliver(at)?;
                self.status.pose_arrived();
                return Ok(Some((measured, orientation)));
            }
            Event::Worn(w) => {
                if self.status.worn_changed(w) {
                    return Err(Interrupt::Removed);
                }
            }
            Event::Available(a) => {
                self.rec.event(if a {
                    LogEvent::TrackingAvailable
                } else {
                    LogEvent::TrackingUnavailable
                })?;
                if self.status.availability_changed(a) {
                    return Err(Interrupt::Unavailable);
                }
            }
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
                for (measured, q) in self.next(Duration::from_millis(200)).await? {
                    cal.add(measured, q);
                }
                let now = self.now();
                let span = cal.span(now);
                match (span, cal.first) {
                    (Some(span), _) if span.p90 < CALIBRATION_MAX_P90_DEG => break Some(span),
                    (span, Some(first)) if now >= first + CALIBRATION_ATTEMPT => break span,
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
            // Wake when the next reminder is due, or now if it already is. Times are exact, so
            // `tick` fires the reminder at that point.
            let wait = det
                .next_due()
                .map_or(POLL, |due| due.saturating_sub(self.now()).min(POLL));
            let mut cues = Vec::new();
            for (measured, q) in self.next(wait).await? {
                let angles = (baseline.conj() * q).euler();
                self.rec.pose(self.time.instant(measured), angles)?;
                let update = det.update(measured, angles.pitch);
                if let Some(upright) = update.reanchored {
                    self.rec.event(LogEvent::Reanchored { upright })?;
                }
                cues.extend(update.cues);
            }
            let t = self.now();
            cues.extend(det.tick(t));
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
#[path = "tests/session.rs"]
mod tests;
