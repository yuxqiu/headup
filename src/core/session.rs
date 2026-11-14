//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::calibration::{Calibration, Outcome, Output};
use super::cues::{CueSink, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::steady::Stillness;
use super::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::{Context, Result, ensure};
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;

/// After a failed calibration (head kept moving), wait this long before asking again.
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);
/// Say that headup is waiting if head tracking has not started after this long.
const TRACKING_WAIT_NOTE: Duration = Duration::from_secs(5);
/// How often the live status line is refreshed.
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// How long to wait for a tracker event before re-checking timers.
const POLL: Duration = Duration::from_secs(1);
/// How often calibration and the wait for a still head judge the head. A prompt comes at most this
/// long after the head meets its condition.
const JUDGE_EVERY: Duration = Duration::from_millis(200);

/// Why a cycle ended early. A cycle waits for tracking, calibrates, then monitors. `?` carries the
/// interrupt, so every wait in the session gives up the same way when tracking stops.
enum Interrupt {
    /// The tracker became unavailable, for this reason (the earbuds taken off, a device
    /// precondition, ...). The head may move unseen until tracking resumes, and resumed tracking
    /// may measure in a new frame. So the posture reference can't be trusted: the session drops
    /// the slouch state, so no reminder plays from stale data, and recalibrates once tracking
    /// resumes. Silence while tracking is available only means the head is still; it never ends a
    /// cycle.
    Unavailable(&'static str),
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

/// A delivered pose.
struct Pose {
    /// When it was measured, in session time.
    measured: Duration,
    /// When it was measured, as the tracker gave it (for logging).
    at: Instant,
    orientation: Quat,
}

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
}

/// What the session's head tracking is doing, from what the tracker reported. Two paths lead to
/// `Live`: at startup from `NotStarted`, and after each interruption from `Interrupted` through
/// `Settling`. Only the tracker's `Available` and `Unavailable` move between them; poses never do.
enum Tracking {
    /// Tracking has not started, for this reason. Silence here may mean "not yet", not a still
    /// head. `Available` makes it `Live` at once: there is no posture yet that the user could have
    /// moved away from.
    NotStarted(&'static str),
    /// Tracking had started, then the tracker said it stopped, for this reason. The head may move
    /// unseen. `Available` makes it `Settling`.
    Interrupted(&'static str),
    /// Back after an interruption. Poses flow, but the user may still be on the move (a device
    /// can stop tracking while the user walks around), so the head is not judged until it holds
    /// still ([`Stillness`]); then `Live`.
    Settling(Stillness),
    /// The head can be judged. Silence means the head is still.
    Live,
}

/// The reason tracking has not started before the tracker reports anything.
const NOT_REPORTED: &str = "the earbuds have not reported yet";

impl Tracking {
    /// Why tracking has not started or has stopped, in the tracker's words. `None` while settling
    /// or live, when the session waits for nothing from the device.
    fn reason(&self) -> Option<&'static str> {
        match self {
            Tracking::NotStarted(reason) | Tracking::Interrupted(reason) => Some(reason),
            Tracking::Settling(_) | Tracking::Live => None,
        }
    }

    /// `Available` arrived at `now`.
    fn available(&mut self, now: Duration) {
        match self {
            Tracking::NotStarted(_) => *self = Tracking::Live,
            Tracking::Interrupted(_) => *self = Tracking::Settling(Stillness::new(now)),
            Tracking::Settling(_) | Tracking::Live => {}
        }
    }

    /// `Unavailable` arrived, for `reason`. Returns `true` if the head was being judged and no
    /// longer can be.
    fn unavailable(&mut self, reason: &'static str) -> bool {
        let next = match self {
            Tracking::NotStarted(_) => Tracking::NotStarted(reason),
            Tracking::Interrupted(_) | Tracking::Settling(_) | Tracking::Live => {
                Tracking::Interrupted(reason)
            }
        };
        matches!(std::mem::replace(self, next), Tracking::Live)
    }

    /// A pose measured at `measured` arrived: while settling, it counts towards stillness.
    fn pose_arrived(&mut self, measured: Duration, q: Quat) {
        if let Tracking::Settling(still) = self {
            still.pose(measured, q);
        }
    }

    /// Advance to `now`: settling ends once the head holds still. Every pose measured before `now`
    /// must have been passed to [`Tracking::pose_arrived`].
    fn settle(&mut self, now: Duration) {
        if let Tracking::Settling(still) = self
            && still.check(now)
        {
            *self = Tracking::Live;
        }
    }
}

struct Session<'a, T: HeadTracker, C: CueSink> {
    tracker: &'a mut T,
    cues: &'a C,
    rec: &'a mut Recorder,
    time: Timeline,
    tracking: Tracking,
}

/// Calibrate, then monitor. Pause while tracking is unavailable (for example while the earbuds are
/// off), and recalibrate when it resumes: the head may have moved unseen, and reseated earbuds sit
/// differently. Returns only with a tracker or output error. The caller handles Ctrl-C.
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
        tracking: Tracking::NotStarted(NOT_REPORTED),
    };
    loop {
        match s.cycle().await {
            Ok(()) => {}
            Err(Interrupt::Unavailable(reason)) => s.rec.note(&format!(
                "head tracking is unavailable ({reason}); will calibrate when it resumes"
            )),
            Err(Interrupt::Fatal(e)) => return Err(e),
        }
    }
}

impl<T: HeadTracker, C: CueSink> Session<'_, T, C> {
    /// Time since the session started; see [`Timeline`].
    fn now(&self) -> Duration {
        self.time.now()
    }

    /// One cycle: calibrate, then monitor (or back off after a failed calibration).
    async fn cycle(&mut self) -> Flow<()> {
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
                self.tracking.pose_arrived(measured, orientation);
                return Ok(Some(Pose {
                    measured,
                    at,
                    orientation,
                }));
            }
            Event::Available => {
                self.rec.event(LogEvent::TrackingAvailable)?;
                self.tracking.available(self.time.now());
            }
            Event::Unavailable { reason } => {
                self.rec.event(LogEvent::TrackingUnavailable { reason })?;
                if self.tracking.unavailable(reason) {
                    return Err(Interrupt::Unavailable(reason));
                }
            }
        }
        Ok(None)
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

    /// Wait until the head can be judged ([`Tracking::Live`]). Tracking may not start until a
    /// device precondition is met (the tracker's reason says which). That can take a while, so this
    /// waits without a deadline; a note gives the reason once the wait for the device gets long.
    async fn wait_for_tracking(&mut self) -> Flow<()> {
        let start = self.now();
        let mut noted = false;
        loop {
            self.tracking.settle(self.now());
            if matches!(self.tracking, Tracking::Live) {
                return Ok(());
            }
            // While settling, data flows: the session waits only for a still head.
            if let Some(reason) = self.tracking.reason()
                && !noted
                && self.now() - start >= TRACKING_WAIT_NOTE
            {
                self.rec
                    .note(&format!("waiting for head tracking ({reason})…"));
                noted = true;
            }
            self.next(JUDGE_EVERY).await?;
        }
    }

    /// Capture the upright pose (see [`Calibration`]). Returns `None` if the head kept moving on
    /// every attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        self.wait_for_tracking().await?;
        self.cues.say(Phrase::SitUpright).await?;
        let mut cal = Calibration::new(self.now());
        loop {
            for pose in self.next(JUDGE_EVERY).await? {
                cal.pose(pose.measured, pose.orientation);
            }
            match cal.check(self.now()) {
                None => {}
                Some(Output::Nudge) => {
                    self.cues.say(Phrase::MoveHead).await?;
                    cal.prompt_ended(self.now());
                }
                Some(Output::AttemptEnded { report, outcome }) => {
                    let spread = report
                        .spread
                        .map_or("poses never covered the span".into(), |s| {
                            format!("{} samples, p90 deviation {:.1}°", s.samples, s.p90)
                        });
                    self.rec
                        .note(&format!("calibration attempt {}: {spread}", report.attempt));
                    match outcome {
                        Outcome::Calibrated(upright) => {
                            self.cues.say(Phrase::Calibrated).await?;
                            return Ok(Some(upright));
                        }
                        Outcome::Retry => {
                            self.cues.say(Phrase::Moved).await?;
                            cal.prompt_ended(self.now());
                        }
                        Outcome::GaveUp => {
                            self.cues.say(Phrase::Moved).await?;
                            return Ok(None);
                        }
                    }
                }
            }
        }
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
            let mut updates = Vec::new();
            for pose in self.next(wait).await? {
                let angles = (baseline.conj() * pose.orientation).euler();
                self.rec.pose(pose.at, angles)?;
                updates.push(det.update(pose.measured, angles.pitch));
            }
            let t = self.now();
            updates.push(det.tick(t));
            for update in updates {
                if let Some(upright) = update.reanchored {
                    self.rec.event(LogEvent::Reanchored { upright })?;
                }
                for cue in update.cues {
                    self.rec.event(LogEvent::Cue { cue })?;
                    self.cues.play(cue);
                }
            }
            if last_status.is_none_or(|last| t - last >= STATUS_EVERY) {
                last_status = Some(t);
                self.rec.status(det.smoothed(), det.slouching())?;
            }
        }
    }
}

/// Detector settings for a tracker: the defaults, plus corrections for what it declares.
fn detector_config(traits: TrackerTraits) -> Config {
    Config {
        reanchor_after_recovery: traits.pose_drifts_up,
        ..Config::default()
    }
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
