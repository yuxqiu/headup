//! HUAWEI FreeClip 2 backend: MBB over RFCOMM channel 1 (docs/devices/freeclip2/FINDINGS.md).
//!
//! - 43/88 switches the spatial-audio pose stream on/off; 43/89 carries relative quaternions that
//!   re-centre themselves and restart with each new stream (`rebuild` makes them continuous).
//! - 43/37 reports per-earbud wear state, pushed on change and answered when queried. Tracking
//!   stops when either earbud is off, so [`Event::Worn`] means both are worn.
//! - 43/54 reports each connected source device's audio state; head tracking only flows while
//!   this computer is streaming audio, which becomes [`Event::Available`].
//! - Re-centres are inaccurate, so the rebuilt pose drifts ([`TrackerTraits::pose_drifts_up`]).

mod link;
mod mbb;
mod protocol;
mod rebuild;

use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use crate::devices::{Backend, Found};
use anyhow::{Context, Result, ensure};
use bluer::rfcomm::{SocketAddr, Stream};
use link::Link;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

const RFCOMM_CHANNEL: u8 = 1;

/// The connected earbuds: the RFCOMM socket and the protocol state (`Link`).
pub struct FreeClip2 {
    stream: Stream,
    link: Link,
}

impl FreeClip2 {
    pub const TRAITS: TrackerTraits = TrackerTraits {
        // Head tracking is only sent while this computer streams audio to the earbuds (FINDINGS).
        tracking_hint: Some(
            "play audio on this computer; FreeClip 2 only tracks your head while it is playing",
        ),
        // Each re-centre lands ~10° off, always towards "looking up", so the rebuilt pose drifts
        // over a session (FINDINGS "Ground truth").
        pose_drifts_up: true,
    };

    /// Send the frames the protocol has queued.
    async fn flush(&mut self) -> Result<()> {
        for f in self.link.take_outbox() {
            self.stream
                .write_all(&f.encode())
                .await
                .context("RFCOMM write")?;
        }
        Ok(())
    }
}

impl Backend for FreeClip2 {
    fn recognises(bluetooth_name: &str) -> bool {
        // Same rule as the vendor app's device whitelist: `^(?=(HUAWEI FreeClip 2))`.
        bluetooth_name.starts_with("HUAWEI FreeClip 2")
    }

    /// Connect, query wear state and start the pose stream. `log` receives every raw sample and
    /// every other frame.
    async fn connect(found: &Found, log: DeviceLog) -> Result<Self> {
        let addr = found.address;
        let stream = Stream::connect(SocketAddr::new(addr, RFCOMM_CHANNEL))
            .await
            .with_context(|| format!("RFCOMM connect to {addr} channel {RFCOMM_CHANNEL} (is the FreeClip 2 connected?)"))?;
        let link = Link::new(found.adapter, log, Instant::now());
        let mut dev = FreeClip2 { stream, link };
        dev.flush().await?;
        Ok(dev)
    }
}

impl HeadTracker for FreeClip2 {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            self.flush().await?;
            if let Some(event) = self.link.next_event() {
                return Ok(Some(event));
            }
            // Wake for whichever comes first: the caller's deadline or the protocol's timer.
            let wake = self.link.next_wake().map_or(deadline, |w| w.min(deadline));
            let mut buf = [0u8; 1024];
            match tokio::time::timeout_at(wake, self.stream.read(&mut buf)).await {
                Ok(read) => {
                    let n = read.context("RFCOMM read")?;
                    ensure!(n > 0, "earbuds closed the connection");
                    self.link.on_bytes(Instant::now(), &buf[..n])?;
                }
                Err(_) => {
                    let now = Instant::now();
                    self.link.on_timer(now);
                    if now >= deadline {
                        return Ok(self.link.next_event());
                    }
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<()> {
        self.link.stop();
        self.flush().await
    }

    fn traits(&self) -> TrackerTraits {
        Self::TRAITS
    }
}

#[cfg(test)]
mod tests {
    // Test-only modules in tests/: recordings replayed through the protocol (and the session).
    mod end_to_end;
    pub(super) mod replay;
}
