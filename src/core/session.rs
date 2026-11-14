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
    /// never ends a cycle. Carries the tracker's reason.
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

/// Whether head tracking flows, as the tracker has shown it.
enum Tracking {
    /// Nothing heard yet: silence may mean tracking has not started, not a still head.
    Starting,
    /// `Unavailable`, with the tracker's reason: the head may move unseen. Poses may still arrive:
    /// data measured before, or trailing data. They do not end this state. Only `Available` does.
    Unavailable(&'static str),
    /// `Available` after `Unavailable`. Poses flow, but the user may still be on the move (a
    /// device can stop tracking while the user walks around), so the head is not judged until it
    /// holds still ([`Stillness`]).
    Resuming(Stillness),
    /// Poses flow and the head can be judged. Silence means the head is still.
    Available,
}

/// What the tracker has reported about itself, and whether the session can judge the head yet.
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

    /// A pose measured at `measured` arrived. At startup it means tracking has started.
    fn pose_arrived(&mut self, measured: Duration, q: Quat) {
        match &mut self.tracking {
            Tracking::Starting => self.tracking = Tracking::Available,
            Tracking::Resuming(still) => still.pose(measured, q),
            Tracking::Unavailable(_) | Tracking::Available => {}
        }
    }

    /// `Available` arrived at `now`.
    fn available(&mut self, now: Duration) {
        match self.tracking {
            Tracking::Unavailable(_) => self.tracking = Tracking::Resuming(Stillness::new(now)),
            Tracking::Starting => self.tracking = Tracking::Available,
            Tracking::Resuming(_) | Tracking::Available => {}
        }
    }

    /// Whether the session is waiting for the device, with what the user can do if anything is
    /// known: at startup the tracker's hint, while unavailable its reason. `None` while resuming,
    /// when data flows and only the head must settle.
    fn waiting_for(&self, traits: TrackerTraits) -> Option<Option<&'static str>> {
        match self.tracking {
            Tracking::Starting => Some(traits.tracking_hint),
            Tracking::Unavailable(reason) => Some(Some(reason)),
            Tracking::Resuming(_) | Tracking::Available => None,
        }
    }

    /// `Unavailable` arrived, for `reason`. Returns `true` if the head was being judged and no
    /// longer can be.
    fn unavailable(&mut self, reason: &'static str) -> bool {
        let was = std::mem::replace(&mut self.tracking, Tracking::Unavailable(reason));
        matches!(was, Tracking::Available)
    }

    /// Advance to `now`: a resuming tracker becomes available once the head holds still. Every
    /// pose measured before `now` must have been passed to [`TrackerStatus::pose_arrived`].
    fn settle(&mut self, now: Duration) {
        if let Tracking::Resuming(still) = &mut self.tracking
            && still.check(now)
        {
            self.tracking = Tracking::Available;
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
                self.status.pose_arrived(measured, orientation);
                return Ok(Some(Pose {
                    measured,
                    at,
                    orientation,
                }));
            }
            Event::Worn(w) => {
                if self.status.worn_changed(w) {
                    return Err(Interrupt::Removed);
                }
            }
            Event::Available => {
                self.rec.event(LogEvent::TrackingAvailable)?;
                self.status.available(self.time.now());
            }
            Event::Unavailable { reason } => {
                self.rec.event(LogEvent::TrackingUnavailable)?;
                if self.status.unavailable(reason) {
                    return Err(Interrupt::Unavailable(reason));
                }
            }
        }
        Ok(None)
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
    /// deadline. A note says so, with what the user can do, once the wait for the device gets
    /// long.
    async fn wait_for_tracking(&mut self) -> Flow<()> {
        let start = self.now();
        let mut noted = false;
        loop {
            self.status.settle(self.now());
            if matches!(self.status.tracking, Tracking::Available) {
                return Ok(());
            }
            if !noted && self.now() - start >= TRACKING_WAIT_NOTE {
                // While resuming, data flows: the session only waits for a still head.
                if let Some(hint) = self.status.waiting_for(self.tracker.traits()) {
                    let hint = hint.map_or(String::new(), |h| format!(" ({h})"));
                    self.rec.note(&format!(
                        "waiting for head tracking from the earbuds{hint}…"
                    ));
                    noted = true;
                }
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
                    self.rec.event(LogEvent::Cue(cue))?;
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
