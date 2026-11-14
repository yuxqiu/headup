//! The interface every earbud backend implements.
//!
//! A tracker reports three things: poses, and whether poses can follow the head right now. The
//! contract, in three rules:
//!
//! 1. **Availability.** Report [`Event::Available`] as soon as poses can follow the head, and
//!    [`Event::Unavailable`] whenever they cannot, for any cause: the earbuds not worn, a device
//!    precondition, a stall. Each comes with a sentence for the user. Only `Available` ends an
//!    unavailability; poses that still arrive after `Unavailable` (trailing data) do not. Until a
//!    tracker reports `Available`, the session does not judge the head.
//! 2. **Silence.** While tracking is available, a tracker with nothing to deliver means a head that
//!    has not moved. A backend whose silence can mean something else (a stall, a continuous stream
//!    that stopped) must report `Unavailable`, after a timeout derived from its report interval.
//! 3. **Frames.** Orientations are relative to a frame the backend chooses, arbitrary but fixed
//!    while tracking is available. The frame may change across an `Unavailable`; the session
//!    recalibrates after every interruption. A device that changes its frame while available
//!    cannot be supported yet.

use super::quat::Quat;
use anyhow::Result;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Head orientation, in the convention of [`Quat::euler`]: pitch > 0 is looking down, yaw > 0
    /// is turned left, roll > 0 is tilted right, relative to the backend's frame (rule 3).
    ///
    /// `at` is when the backend received the sample the pose comes from. It is the best available
    /// measure of when the pose was measured. Smoothing and slouch timing run on `at`. It must not
    /// go backwards, and it must not be earlier than the session's start.
    ///
    /// Deliver each pose as soon as it is known to be real. The session treats a tracker with
    /// nothing to deliver as a head that has not moved (rule 2), so a pose held back makes
    /// reminders and calibration judge stale posture. Hold a sample only while it may be device
    /// noise that will be dropped, and only briefly.
    Pose { at: Instant, orientation: Quat },
    /// Poses can follow the head (rule 1). Sent on change.
    Available,
    /// Poses cannot follow the head (rule 1). `reason` says why and what the user can do, in the
    /// backend's words; the session shows and logs it as is. Sent on change, including a change of
    /// reason.
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
    /// The reported orientation accumulates error towards "looking up" over a session, so the same
    /// pose reads higher later. The session then re-anchors upright after each recovery. Drift
    /// the other way is not corrected, because that would need a different rule.
    pub pose_drifts_up: bool,
}
