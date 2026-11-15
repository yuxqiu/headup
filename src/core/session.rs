//! Device-agnostic posture session: spoken calibration, then monitoring with cues.

use super::calibration::{Calibration, Outcome, Output};
use super::cues::Cue;
use super::cues::{Phrase, Player, Sound};
use super::detector::{Config, Detector, PostureChange};
use super::quat::Quat;
use super::record::{LogEvent, Recorder};
use super::reminders::{Reminders, Schedule};
use super::steady::Stillness;
use super::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use futures_util::future::LocalBoxFuture;
use futures_util::stream::FuturesUnordered;
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;

/// After a failed calibration (head kept moving), wait this long before asking again (DECISIONS
/// "Refactor (user approved)", 2026-11-11).
const CALIBRATION_RETRY: Duration = Duration::from_secs(60);
/// How often the live status line is refreshed (DECISIONS "Thermo review applied"). Monitoring
/// wakes at least this often, which also lets re-anchoring settle on a silent head.
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

/// What the session's head tracking is doing, from what the tracker reported. Only the tracker's
/// `Available` and `Unavailable` move between the phases; poses never do. Every `Available` leads
/// through `Settling`, at startup as after an interruption: the user may be on the move when
/// tracking becomes available (putting the earbuds in, or a device that stops tracking while the
/// user walks around), and "sit upright" should not be spoken to a moving head.
enum Tracking {
    /// Tracking is unavailable: it has not started yet, or the tracker said it stopped. The head
    /// may move unseen. `Available` makes it `Settling`.
    Waiting,
    /// Tracking is available. Poses flow, but the head is not judged until it holds still
    /// ([`Stillness`]); then `Live`.
    Settling(Stillness),
    /// The head can be judged. Silence means the head is still.
    Live,
}

impl Tracking {
    /// `Available` arrived at `now`.
    fn available(&mut self, now: Duration) {
        if let Tracking::Waiting = self {
            *self = Tracking::Settling(Stillness::new(now));
        }
    }

    /// `Unavailable` arrived. Returns `true` if the head was being judged and no longer can be.
    fn unavailable(&mut self) -> bool {
        matches!(std::mem::replace(self, Tracking::Waiting), Tracking::Live)
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

/// The session's sounds: calibration waits for a prompt to be heard, because it measures the head
/// only after it. Monitoring does not wait for a reminder: it keeps judging while one plays, so the
/// recovery tink is on time even if the user sits up during speech. A reminder keeps playing to its
/// end even if monitoring stops, and its outcome is collected whenever the session next waits.
struct Sounds<'a, C: Player> {
    player: &'a C,
    /// Reminders still playing, each ending with its outcome.
    reminders: FuturesUnordered<LocalBoxFuture<'static, (Cue, Result<()>)>>,
}

impl<'a, C: Player> Sounds<'a, C> {
    fn new(player: &'a C) -> Self {
        Sounds {
            player,
            reminders: FuturesUnordered::new(),
        }
    }

    /// Speak a calibration prompt and return once it has been heard.
    async fn prompt(&self, p: Phrase) -> Result<()> {
        self.player.play(Sound::Prompt(p)).await
    }

    /// Start a reminder and return at once; [`Sounds::ended`] gives its outcome.
    fn remind(&mut self, cue: Cue) {
        let playing = self.player.play(Sound::Reminder(cue));
        self.reminders
            .push(Box::pin(async move { (cue, playing.await) }));
    }

    /// The next reminder to end, with its outcome. `None` at once if none is playing.
    async fn ended(&mut self) -> Option<(Cue, Result<()>)> {
        self.reminders.next().await
    }
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
    /// becoming unavailable, interrupts. A reminder that ends meanwhile is reported, and ends the
    /// wait like an event.
    async fn next(&mut self, timeout: Duration) -> Flow<Vec<Pose>> {
        let mut poses = Vec::new();
        let mut wait = timeout;
        loop {
            tokio::select! {
                event = self.tracker.next_event(wait) => match event? {
                    Some(event) => poses.extend(self.handle(event)?),
                    None => return Ok(poses),
                },
                Some((cue, played)) = self.sounds.ended() => self.reminder_ended(cue, played)?,
            }
            wait = Duration::ZERO;
        }
    }

    /// Log and start each reminder in `cues`.
    fn remind(&mut self, cues: impl IntoIterator<Item = Cue>) -> Result<()> {
        for cue in cues {
            self.rec.event(LogEvent::Cue { cue })?;
            self.sounds.remind(cue);
        }
        Ok(())
    }

    /// Log a re-anchored upright, if there is one.
    fn log_reanchored(&mut self, upright: Option<f64>) -> Result<()> {
        if let Some(upright) = upright {
            self.rec.event(LogEvent::Reanchored { upright })?;
        }
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
                self.rec.note("waiting for your head to hold still…");
            }
            Event::Unavailable { reason } => {
                self.rec.event(LogEvent::TrackingUnavailable { reason })?;
                if self.tracking.unavailable() {
                    return Err(Interrupt::Unavailable);
                }
            }
        }
        Ok(None)
    }

    /// Consume events for `period`. Interrupts as [`Session::next`] does.
    async fn idle(&mut self, period: Duration) -> Flow<()> {
        let end = self.now() + period;
        loop {
            // One reading of the clock: a second could pass `end` after the first did not.
            let now = self.now();
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
            self.tracking.settle(self.now());
            if let Tracking::Live = self.tracking {
                return Ok(());
            }
            self.next(JUDGE_EVERY).await?;
        }
    }

    /// Capture the upright pose (see [`Calibration`]). Returns `None` if the head kept moving on
    /// every attempt.
    async fn calibrate(&mut self) -> Flow<Option<Quat>> {
        self.wait_for_tracking().await?;
        self.sounds.prompt(Phrase::SitUpright).await?;
        let mut cal = Calibration::new(self.now());
        loop {
            for pose in self.next(JUDGE_EVERY).await? {
                cal.pose(pose.measured, pose.orientation);
            }
            match cal.check(self.now()) {
                None => {}
                Some(Output::Nudge) => {
                    self.sounds.prompt(Phrase::MoveHead).await?;
                    cal.prompt_ended(self.now());
                }
                Some(Output::AttemptEnded { report, outcome }) => {
                    self.rec.note(&report.to_string());
                    match outcome {
                        Outcome::Calibrated(upright) => {
                            self.sounds.prompt(Phrase::Calibrated).await?;
                            return Ok(Some(upright));
                        }
                        Outcome::Retry => {
                            self.sounds.prompt(Phrase::Moved).await?;
                            cal.prompt_ended(self.now());
                        }
                        Outcome::GaveUp => {
                            self.sounds.prompt(Phrase::Moved).await?;
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
        let mut reminders = Reminders::new(Schedule::default());
        let mut last_status: Option<Duration> = None;
        loop {
            // Wake for the next status refresh, or when the next reminder is due if that is sooner.
            // Times are exact, so the reminder falls due at that point.
            let status_due = last_status.map_or(Duration::ZERO, |last| last + STATUS_EVERY);
            let wake = reminders
                .next_due()
                .map_or(status_due, |due| due.min(status_due));
            let wait = wake.saturating_sub(self.now());
            for pose in self.next(wait).await? {
                let angles = (baseline.conj() * pose.orientation).euler();
                self.rec.pose(pose.at, angles)?;
                let update = det.update(pose.measured, angles.pitch);
                self.log_reanchored(update.reanchored)?;
                match update.posture {
                    Some(PostureChange::SlouchStarted) => reminders.slouch_started(pose.measured),
                    Some(PostureChange::Recovered) => self.remind(reminders.recovered())?,
                    None => {}
                }
                self.remind(reminders.due(pose.measured))?;
            }
            let t = self.now();
            self.log_reanchored(det.tick(t))?;
            self.remind(reminders.due(t))?;
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
