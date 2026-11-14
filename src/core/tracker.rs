//! The interface every earbud backend implements.

use super::quat::Quat;
use anyhow::Result;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Head orientation, in the convention of [`Quat::euler`]: pitch > 0 is looking down, yaw > 0
    /// is turned left, roll > 0 is tilted right.
    ///
    /// The orientation is absolute within one wear period. It is relative to a frame the backend
    /// chooses, arbitrary but fixed. The frame may change after `Worn(true)`, or when tracking
    /// resumes after `Unavailable`. The session recalibrates in both cases.
    ///
    /// `at` is when the backend received the sample the pose comes from. It is the best available
    /// measure of when the pose was measured. Smoothing and slouch timing run on `at`. It must not
    /// go backwards, and it must not be earlier than the session's start.
    ///
    /// Deliver each pose as soon as it is known to be real. The session treats a tracker with
    /// nothing to deliver as a head that has not moved, so a pose held back makes reminders and
    /// calibration judge stale posture. Hold a sample only while it may be device noise that will
    /// be dropped, and only briefly.
    Pose { at: Instant, orientation: Quat },
    /// Whether the earbuds are worn well enough for head tracking. The backend decides what that
    /// takes. Sent on change. A backend should report the initial state soon after connecting.
    Worn(bool),
    /// The device delivers head tracking again after `Unavailable`. Sent on change. Until told
    /// otherwise, assume tracking is available.
    Available,
    /// The device cannot deliver head tracking right now, beyond not being worn. `reason` says why
    /// and what the user can do, in the backend's words; the session shows it as is. Sent on
    /// change, including a change of reason.
    ///
    /// While tracking is available, silence means the head is still. A backend whose silence can
    /// also mean something else, such as a stall or data that stopped, must send `Unavailable`
    /// then. Otherwise the session keeps the last posture forever.
    ///
    /// Poses may still arrive after `Unavailable`, from held or trailing data. They do not end the
    /// unavailability. Only `Available` does.
    Unavailable { reason: &'static str },
}

/// A source of head orientation. Device quirks such as re-centring are the backend's job.
#[allow(async_fn_in_trait)] // used through generics only, not as dyn or across threads
pub trait HeadTracker {
    /// Wait up to `timeout` for the next event. `Ok(None)` means nothing arrived. Trackers may go
    /// quiet while the head is still, so callers treat silence as "unchanged", not as an error.
    /// An event that is already waiting is returned at once, even with a zero `timeout`. Lost
    /// connections and protocol errors are `Err`.
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>>;

    /// Stop streaming and leave the device in its normal state.
    async fn stop(&mut self) -> Result<()>;

    /// What the session needs to know about this tracker. Quirks the backend can absorb itself
    /// (re-centring, wear rules, preconditions behind `Available`) do not belong here.
    fn traits(&self) -> TrackerTraits {
        TrackerTraits::default()
    }
}

/// Device properties that change how the session behaves. Every field defaults to "no quirk", so
/// adding a field leaves existing backends unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackerTraits {
    /// What the user can do when no head tracking arrives at startup, before the tracker has said
    /// anything, e.g. a device precondition. Later, `Event::Unavailable` carries its own reason.
    pub tracking_hint: Option<&'static str>,
    /// The reported orientation accumulates error towards "looking up" over a session, so the same
    /// pose reads higher later. The session then re-anchors upright after each recovery. Drift
    /// the other way is not corrected, because that would need a different rule.
    pub pose_drifts_up: bool,
}
