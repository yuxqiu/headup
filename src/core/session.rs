//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::calibration::{Calibration, Report, Step};
use super::cues::{CueSink, Phrase};
use super::detector::{Config, Detector};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
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

    /// Capture the upright pose (see [`Calibration`]). Returns `None` if the head kept moving on
    /// every attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        self.wait_for_tracking().await?;
        self.cues.say(Phrase::SitUpright).await?;
        let mut cal = Calibration::new(self.now());
        loop {
            for (measured, q) in self.next(Duration::from_millis(200)).await? {
                cal.pose(measured, q);
            }
            match cal.check(self.now()) {
                Step::Wait => {}
                Step::Nudge => {
                    self.cues.say(Phrase::MoveHead).await?;
                    cal.spoken(self.now());
                }
                Step::Moved(report) => {
                    self.note_attempt(report);
                    self.cues.say(Phrase::Moved).await?;
                    cal.spoken(self.now());
                }
                Step::Calibrated { upright, report } => {
                    self.note_attempt(report);
                    self.cues.say(Phrase::Calibrated).await?;
                    return Ok(Some(upright));
                }
                Step::GaveUp(report) => {
                    self.note_attempt(report);
                    self.cues.say(Phrase::Moved).await?;
                    return Ok(None);
                }
            }
        }
    }

    fn note_attempt(&mut self, r: Report) {
        self.rec.note(&format!(
            "calibration attempt {}: {} samples, p90 deviation {:.1}°",
            r.attempt, r.samples, r.p90
        ));
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

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
