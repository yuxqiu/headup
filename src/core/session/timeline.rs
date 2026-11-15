//! Session time.

use anyhow::{Context, Result, ensure};
use std::time::Duration;
use tokio::time::Instant;

/// The session's timeline. Session time is the time since the session started. It is monotonic,
/// unlike log time, and counts exact integer nanoseconds.
pub(super) struct Timeline {
    start: Instant,
    /// Measurement time of the latest pose.
    measured: Duration,
}

impl Timeline {
    pub(super) fn new() -> Self {
        Timeline {
            start: Instant::now(),
            measured: Duration::ZERO,
        }
    }

    /// Time since the session started.
    pub(super) fn now(&self) -> Duration {
        self.start.elapsed()
    }

    /// Take delivery of a pose measured at `at`, and return its measurement time in session time.
    /// A pose from before the session, or out of order, is a tracker bug and an error.
    pub(super) fn deliver(&mut self, at: Instant) -> Result<Duration> {
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
