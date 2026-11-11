//! The interface every earbud backend implements.

use super::quat::Quat;
use anyhow::Result;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Head orientation. Absolute within one wear period (relative to an arbitrary but fixed frame
    /// chosen by the backend), in the convention of [`Quat::euler`]: pitch > 0 = looking down,
    /// yaw > 0 = turned left, roll > 0 = tilted right. The frame may change after `Worn(true)`, or
    /// when tracking resumes after a long `Available(false)`; the session recalibrates in both cases.
    Pose(Quat),
    /// Whether the earbuds are worn well enough for head tracking (the backend decides what that
    /// takes). Sent on change; a backend should report the initial state soon after connecting.
    Worn(bool),
    /// Whether the device can deliver head tracking right now, beyond being worn (a device
    /// precondition; see [`TrackerTraits::tracking_hint`]). Sent on change; assume available until told otherwise.
    /// While available, silence means the head is still.
    Available(bool),
}

/// A source of head orientation. Device quirks such as re-centring are the backend's job.
#[allow(async_fn_in_trait)] // used through generics only, not as dyn or across threads
pub trait HeadTracker {
    /// Wait up to `timeout` for the next event. `Ok(None)` means nothing arrived; trackers may go
    /// quiet while the head is still, so callers treat silence as "unchanged", not as an error.
    /// Lost connections and protocol errors are `Err`.
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
    /// What the user can do when no head tracking arrives, e.g. a device precondition. Shown in
    /// "waiting for head tracking" messages.
    pub tracking_hint: Option<&'static str>,
    /// The reported orientation accumulates error towards "looking up" over a session, so the same
    /// pose reads higher later. The session then re-anchors upright after each recovery. (Drift
    /// the other way is not corrected; it would need a different rule.)
    pub pose_drifts_up: bool,
}
