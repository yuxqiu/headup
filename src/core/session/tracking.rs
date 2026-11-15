//! The session's view of head tracking: whether the head can be judged.

use crate::core::quat::Quat;
use crate::core::steady::Stillness;
use std::time::Duration;

/// What the session's head tracking is doing, from what the tracker reported. Only the tracker's
/// `Available` and `Unavailable` move between the phases; poses never do. Every `Available` leads
/// through `Settling`, at startup as after an interruption: the user may be on the move when
/// tracking becomes available (putting the earbuds in, or a device that stops tracking while the
/// user walks around), and "sit upright" should not be spoken to a moving head.
pub(super) enum Tracking {
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
    pub(super) fn available(&mut self, now: Duration) {
        if let Tracking::Waiting = self {
            *self = Tracking::Settling(Stillness::new(now));
        }
    }

    /// `Unavailable` arrived. Returns `true` if the head was being judged and no longer can be.
    pub(super) fn unavailable(&mut self) -> bool {
        matches!(std::mem::replace(self, Tracking::Waiting), Tracking::Live)
    }

    /// A pose measured at `measured` arrived: while settling, it counts towards stillness.
    pub(super) fn pose_arrived(&mut self, measured: Duration, q: Quat) {
        if let Tracking::Settling(still) = self {
            still.pose(measured, q);
        }
    }

    /// Advance to `now`: settling ends once the head holds still. Every pose measured before `now`
    /// must have been passed to [`Tracking::pose_arrived`].
    pub(super) fn settle(&mut self, now: Duration) {
        if let Tracking::Settling(still) = self
            && still.check(now)
        {
            *self = Tracking::Live;
        }
    }
}

#[cfg(test)]
#[path = "tests/tracking.rs"]
mod tests;
