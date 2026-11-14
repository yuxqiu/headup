//! HUAWEI FreeClip 2 backend: MBB over RFCOMM channel 1 (docs/devices/freeclip2/FINDINGS.md).
//!
//! - 43/88 switches the spatial-audio pose stream on or off.
//! - 43/89 carries the poses as relative quaternions. They re-centre themselves and restart with
//!   each new stream. `rebuild` makes them continuous.
//! - 43/37 reports the wear state of each earbud. The earbuds push it on change and answer it when
//!   queried. Tracking stops when either earbud is off, so [`Event::Worn`] means both are worn.
//! - 43/54 reports the audio state of each connected source device. Head tracking only flows to
//!   the earbuds' current device, which is the last one to start streaming audio. Whether this
//!   computer is that device becomes [`Event::Available`] or [`Event::Unavailable`]. The earbuds
//!   send no 43/54 on connect while audio already plays; a 43/89 reading shows it then. The earbuds
//!   refuse headup's 43/88 while another device is current, so headup sends it again once this
//!   computer streams.
//! - Re-centres are inaccurate, so the rebuilt pose drifts ([`TrackerTraits::pose_drifts_up`]).
//! - While the user walks around, the earbuds hold the reading and lose the head's movement, so
//!   tracking is unavailable until the reading moves again (`rebuild`).

mod link;
mod mbb;
mod protocol;
mod rebuild;

use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use crate::devices::bluetooth::{Address, Host};
use crate::devices::{Backend, Found};
use anyhow::{Context, Result, bail, ensure};
use link::Link;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

const RFCOMM_CHANNEL: u8 = 1;

/// The connected earbuds. A task owns the socket and the protocol state (`Link`) and reads all the
/// time, so each sample is timed when it arrives, even while the session is busy speaking.
pub struct FreeClip2 {
    /// Events from the task, in order. If the task fails, its error comes last.
    events: mpsc::UnboundedReceiver<Result<Event>>,
    /// Asks the task to switch the pose stream off and end. Dropping it ends the task without
    /// switching the stream off.
    stop: Option<oneshot::Sender<()>>,
}

impl FreeClip2 {
    pub const TRAITS: TrackerTraits = TrackerTraits {
        // Head tracking is only sent while this computer streams audio to the earbuds (FINDINGS).
        tracking_hint: Some(link::NOT_STREAMING),
        // Each re-centre lands ~10° off, always towards "looking up", so the rebuilt pose drifts
        // over a session (FINDINGS "Ground truth").
        pose_drifts_up: true,
    };

    /// Start the task on a connected `stream`. `adapter` is this computer's Bluetooth address.
    fn start<S>(stream: S, adapter: Address, log: DeviceLog) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (events_tx, events) = mpsc::unbounded_channel();
        let (stop, stop_rx) = oneshot::channel();
        let link = Link::new(adapter, Instant::now());
        tokio::spawn(async move {
            if let Err(e) = serve(stream, link, log, &events_tx, stop_rx).await {
                // Fails only if the tracker is gone, and then nobody is left to tell.
                let _ = events_tx.send(Err(e));
            }
        });
        FreeClip2 {
            events,
            stop: Some(stop),
        }
    }
}

/// Move bytes between `stream` and `link`, pass its events on and write its log, until asked to
/// stop or the tracker is dropped.
async fn serve<S>(
    mut stream: S,
    mut link: Link,
    mut log: DeviceLog,
    events: &mpsc::UnboundedSender<Result<Event>>,
    mut stop: oneshot::Receiver<()>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = [0u8; 1024];
    loop {
        send(&mut stream, &mut link).await?;
        while let Some(event) = link.next_event() {
            if events.send(Ok(event)).is_err() {
                return Ok(()); // the tracker was dropped
            }
        }
        let read = tokio::select! {
            read = stream.read(&mut buf) => read,
            asked = &mut stop => {
                if asked.is_ok() {
                    link.stop();
                    send(&mut stream, &mut link).await?;
                }
                return Ok(());
            }
        };
        let n = read.context("RFCOMM read")?;
        ensure!(n > 0, "earbuds closed the connection");
        let handled = link.on_bytes(Instant::now(), &buf[..n]);
        // Written before a protocol error is returned, so the log shows the frame that caused it.
        for record in link.take_log() {
            log.record(&record)?;
        }
        handled?;
    }
}

/// Send the frames `link` has queued.
async fn send<S: AsyncWrite + Unpin>(stream: &mut S, link: &mut Link) -> Result<()> {
    for f in link.take_outbox() {
        stream
            .write_all(&f.encode())
            .await
            .context("RFCOMM write")?;
    }
    Ok(())
}

impl Backend for FreeClip2 {
    fn recognises(bluetooth_name: &str) -> bool {
        // Same rule as the vendor app's device whitelist: `^(?=(HUAWEI FreeClip 2))`.
        bluetooth_name.starts_with("HUAWEI FreeClip 2")
    }

    /// Connect, then query wear state and start the pose stream. `log` receives every raw sample
    /// and every other frame.
    async fn connect(host: &Host, found: &Found, log: DeviceLog) -> Result<Self> {
        let addr = found.device.address;
        let stream = host
            .connect_rfcomm(addr, RFCOMM_CHANNEL)
            .await
            .with_context(|| format!("RFCOMM connect to {addr} channel {RFCOMM_CHANNEL} (is the FreeClip 2 connected?)"))?;
        Ok(FreeClip2::start(stream, found.adapter, log))
    }
}

impl HeadTracker for FreeClip2 {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        match tokio::time::timeout(timeout, self.events.recv()).await {
            Err(_) => Ok(None),
            Ok(Some(event)) => event.map(Some),
            Ok(None) => bail!("the connection to the earbuds has ended"),
        }
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            // Fails only if the task has ended; the loop below then returns its error.
            let _ = stop.send(());
        }
        // The task closes the channel once the stream is switched off. Events still queued are
        // dropped.
        while let Some(event) = self.events.recv().await {
            event?;
        }
        Ok(())
    }

    fn traits(&self) -> TrackerTraits {
        Self::TRAITS
    }
}

#[cfg(test)]
mod tests {
    // Test-only modules in tests/. They replay recordings through the protocol and the session.
    mod end_to_end;
    mod rebuild;
    pub(super) mod replay;
    mod socket;
}
