//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

mod sounds;
mod timeline;
mod tracking;

use super::calibration::{Calibration, Next, Outcome};
use super::monitor::{self, Monitor};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::sound::{Cue, Phrase, Player};
use super::tracker::{Event, HeadTracker};
use anyhow::Result;
use sounds::Sounds;
use std::convert::Infallible;
use std::time::Duration;
use timeline::Timeline;
use tokio::time::Instant;
use tracking::Tracking;

/// After a failed calibration (head kept moving), wait this long before asking again (DECISIONS
/// "Refactor (user approved)", 2026-11-11).
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);
/// How often the live status line is refreshed (DECISIONS "Thermo review applied").
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// How often calibration and the wait for a still head judge the head. A prompt comes at most this
/// long after the head meets its condition (DECISIONS "Calibration outputs, one window type, review
/// fixes"); a delay that short was not measured for noticeability \[unmeasured\].
const JUDGE_EVERY: Duration = Duration::from_millis(200);

/// Why a cycle ended early. A cycle waits for tracking, calibrates, then monitors. `?` carries the
/// interrupt, so every wait in the session gives up the same way when tracking stops.
enum Interrupt {
    /// The tracker became unavailable (the earbuds taken off, a device precondition, ...). The
    /// head may move unseen until tracking resumes, and resumed tracking may measure in a new
    /// frame. So the posture reference can't be trusted: the session drops
    /// the slouch state, so no reminder plays from stale data, and recalibrates once tracking
    /// resumes. Silence while tracking is available only means the head is still; it never ends a
    /// cycle.
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

/// A delivered pose.
struct Pose {
    /// When it was measured, in session time.
    measured: Duration,
    /// When it was measured, as the tracker gave it (for logging).
    at: Instant,
    orientation: Quat,
}

struct Session<'a, T: HeadTracker, C: Player> {
    tracker: &'a mut T,
    sounds: Sounds<'a, C>,
    rec: &'a mut Recorder,
    time: Timeline,
    tracking: Tracking,
}

/// Calibrate, then monitor. Pause while tracking is unavailable (for example while the earbuds are
/// off), and recalibrate when it resumes: the head may have moved unseen, and reseated earbuds sit
/// differently. Returns only with a tracker or output error. The caller handles Ctrl-C.
pub async fn run<T: HeadTracker, C: Player>(
    tracker: &mut T,
    player: &C,
    rec: &mut Recorder,
) -> Result<Infallible> {
    let mut s = Session {
        tracker,
        sounds: Sounds::new(player),
        rec,
        time: Timeline::new(),
        tracking: Tracking::Waiting,
    };
    loop {
        match s.cycle().await {
            Ok(()) => {}
            Err(Interrupt::Unavailable) => s
                .rec
                .note("monitoring paused; will calibrate when tracking resumes"),
            Err(Interrupt::Fatal(e)) => return Err(e),
        }
    }
}

impl<T: HeadTracker, C: Player> Session<'_, T, C> {
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
    /// becoming unavailable, interrupts. A reminder that ends meanwhile is reported, and ends the
    /// wait like an event.
    async fn next(&mut self, timeout: Duration) -> Flow<Vec<Pose>> {
        let mut poses = Vec::new();
        let mut deadline = Instant::now() + timeout;
        loop {
            // Biased, tracker first: an event that is waiting at the deadline is still taken.
            tokio::select! {
                biased;
                event = self.tracker.next_event() => poses.extend(self.handle(event?)?),
                Some((cue, played)) = self.sounds.ended() => self.reminder_ended(cue, played)?,
                () = tokio::time::sleep_until(deadline) => return Ok(poses),
            }
            deadline = Instant::now();
        }
    }

    /// Log a reminder and start it.
    fn remind(&mut self, cue: Cue) -> Result<()> {
        self.rec.event(LogEvent::Cue { cue })?;
        self.sounds.remind(cue);
        Ok(())
    }

    /// A reminder ended. One that could not be played is shown and logged; monitoring goes on.
    fn reminder_ended(&mut self, cue: Cue, played: Result<()>) -> Result<()> {
        if let Err(e) = played {
            self.rec.warn(&format!("{e:#}"));
            self.rec.event(LogEvent::CueFailed { cue })?;
        }
        Ok(())
    }

    /// Record one tracker event. Returns the pose if it was one.
    fn handle(&mut self, event: Event) -> Flow<Option<Pose>> {
        match event {
            Event::Pose { at, orientation } => {
                let measured = self.time.deliver(at)?;
                return Ok(Some(Pose {
                    measured,
                    at,
                    orientation,
                }));
            }
            Event::Available => {
                self.rec.event(LogEvent::TrackingAvailable)?;
                self.tracking.available(self.time.now());
                self.rec.note("waiting for your head to hold still…");
            }
            Event::Unavailable { reason } => {
                self.rec.event(LogEvent::TrackingUnavailable { reason })?;
                self.tracking.unavailable()?;
            }
        }
        Ok(None)
    }

    /// Consume events for `period`. Interrupts as [`Session::next`] does.
    async fn idle(&mut self, period: Duration) -> Flow<()> {
        let end = self.time.now() + period;
        loop {
            // One reading of the clock: a second could pass `end` after the first did not.
            let now = self.time.now();
            if now >= end {
                return Ok(());
            }
            self.next(end - now).await?;
        }
    }

    /// Wait until the head can be judged ([`Tracking::Live`]): for the device, which may not track
    /// until a precondition is met (its `Unavailable` line says which), then for a still head.
    /// Either can take a while, so this waits without a deadline.
    async fn wait_for_tracking(&mut self) -> Flow<()> {
        loop {
            self.tracking.settle(self.time.now());
            if let Tracking::Live = self.tracking {
                return Ok(());
            }
            for pose in self.next(JUDGE_EVERY).await? {
                self.tracking.pose_arrived(pose.measured, pose.orientation);
            }
        }
    }

    /// Capture the upright pose (see [`Calibration`]). Returns `None` if the head kept moving on
    /// every attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        self.wait_for_tracking().await?;
        self.sounds.prompt(Phrase::SitUpright).await?;
        let mut cal = Calibration::new(self.time.now());
        loop {
            for pose in self.next(JUDGE_EVERY).await? {
                cal.pose(pose.measured, pose.orientation);
            }
            cal = match cal.check(self.time.now()) {
                Next::Measuring(cal) => cal,
                Next::Nudge(paused) => {
                    self.sounds.prompt(Phrase::MoveHead).await?;
                    paused.resume(self.time.now())
                }
                Next::AttemptEnded { report, outcome } => {
                    self.rec.note(&report.to_string());
                    match outcome {
                        Outcome::Calibrated(upright) => {
                            self.sounds.prompt(Phrase::Calibrated).await?;
                            return Ok(Some(upright));
                        }
                        Outcome::Retry(paused) => {
                            self.sounds.prompt(Phrase::Moved).await?;
                            paused.resume(self.time.now())
                        }
                        Outcome::GaveUp => {
                            self.sounds.prompt(Phrase::Moved).await?;
                            return Ok(None);
                        }
                    }
                }
            };
        }
    }

    /// Monitor until interrupted: earbuds off, tracking unavailable, or an error.
    async fn monitor(&mut self, baseline: Quat) -> Flow<()> {
        let mut monitor = Monitor::new(baseline, self.tracker.traits());
        // When the live status line is next refreshed: at once, then every STATUS_EVERY.
        let mut status_due = Duration::ZERO;
        loop {
            // Wake for the next status refresh, or when the monitor is next due (a reminder, or a
            // head-up ending in silence) if that is sooner. Times are exact, so it falls due then.
            let wake = monitor
                .next_due()
                .map_or(status_due, |due| due.min(status_due));
            let wait = wake.saturating_sub(self.time.now());
            for pose in self.next(wait).await? {
                let angles = monitor.pose(pose.measured, pose.orientation);
                self.rec.pose(pose.at, angles)?;
            }
            let t = self.time.now();
            for output in monitor.check(t) {
                match output {
                    monitor::Output::Remind(cue) => self.remind(cue)?,
                    monitor::Output::Reanchored { upright } => {
                        self.rec.event(LogEvent::Reanchored { upright })?
                    }
                }
            }
            if t >= status_due {
                status_due = t + STATUS_EVERY;
                self.rec.status(monitor.status())?;
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
