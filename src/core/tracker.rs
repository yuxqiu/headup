//! The interface every earbud backend implements.

use super::quat::Quat;
use anyhow::Result;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Head orientation. Absolute within one wear period (relative to an arbitrary but fixed frame
    /// chosen by the backend), in the convention of [`Quat::euler`]: pitch > 0 = looking down,
    /// yaw > 0 = turned left, roll > 0 = tilted right. The frame may change after `Worn(true)`.
    Pose(Quat),
    /// Whether the earbuds are being worn (at least one in/on an ear). Sent on change; a backend
    /// should report the initial state soon after connecting.
    Worn(bool),
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
}
