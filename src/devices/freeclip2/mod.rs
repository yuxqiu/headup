//! HUAWEI FreeClip 2 backend: MBB over RFCOMM channel 1. 43/88 switches the spatial-audio pose
//! stream on/off; 43/89 carries relative quaternions that re-centre themselves; 43/37 reports
//! per-earbud wear state, pushed on change and answered when queried (docs/FINDINGS.md).

mod mbb;
mod rebuild;

use crate::core::tracker::{Event, HeadTracker};
use anyhow::{Context, Result, bail, ensure};
use bluer::rfcomm::{SocketAddr, Stream};
use mbb::{Decoder, Frame};
use rebuild::{Rebuilder, decode_sample};
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RFCOMM_CHANNEL: u8 = 1;
const SVC: u8 = 43;
const CMD_STREAM_SWITCH: u8 = 88;
const CMD_STREAM_DATA: u8 = 89;
const CMD_WEAR_STATE: u8 = 37;

/// Wear state from a 43/37 frame: TLV 1 = left, TLV 2 = right, 1 = worn. "Worn" means either.
fn worn_from(f: &Frame) -> Option<bool> {
    let side = |t| f.tlv(t).and_then(|v| v.first().copied());
    match (side(1), side(2)) {
        (None, None) => None,
        (l, r) => Some(l == Some(1) || r == Some(1)),
    }
}

pub struct FreeClip2 {
    stream: Stream,
    decoder: Decoder,
    rebuild: Rebuilder,
    start: Instant,
    raw_log: Option<std::fs::File>,
    worn: Option<bool>,
}

impl FreeClip2 {
    /// Connect, query wear state and start the pose stream. `raw_log` receives every raw sample as
    /// a JSON line (seq, quaternion as sent, rebuilt quaternion, whether it was a device re-centre).
    pub async fn connect(addr: bluer::Address, raw_log: Option<std::fs::File>) -> Result<Self> {
        let stream = Stream::connect(SocketAddr::new(addr, RFCOMM_CHANNEL))
            .await
            .with_context(|| format!("RFCOMM connect to {addr} channel {RFCOMM_CHANNEL} (is the FreeClip 2 connected?)"))?;
        let mut dev = FreeClip2 {
            stream,
            decoder: Decoder::default(),
            rebuild: Rebuilder::default(),
            start: Instant::now(),
            raw_log,
            worn: None,
        };
        dev.send(Frame::new(SVC, CMD_WEAR_STATE, &[(1, &[]), (2, &[])]))
            .await?;
        dev.send(Frame::new(SVC, CMD_STREAM_SWITCH, &[(1, &[1])]))
            .await?;
        Ok(dev)
    }

    async fn send(&mut self, f: Frame) -> Result<()> {
        self.stream
            .write_all(&f.encode())
            .await
            .context("RFCOMM write")
    }
}

impl HeadTracker for FreeClip2 {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            while let Some(f) = self.decoder.next_frame()? {
                if (f.svc, f.cmd) == (SVC, CMD_WEAR_STATE) {
                    let Some(worn) = worn_from(&f) else {
                        bail!("malformed 43/37: {f:?}")
                    };
                    if self.worn == Some(worn) {
                        continue;
                    }
                    let was = self.worn.replace(worn);
                    if worn && was == Some(false) {
                        // Back on the ear: start a fresh orientation frame.
                        self.rebuild = Rebuilder::default();
                        self.send(Frame::new(SVC, CMD_STREAM_SWITCH, &[(1, &[1])]))
                            .await?;
                    }
                    return Ok(Some(Event::Worn(worn)));
                }
                if (f.svc, f.cmd) != (SVC, CMD_STREAM_DATA) {
                    continue;
                }
                let (Some(s), Some(q)) = (f.tlv(1), f.tlv(2)) else {
                    bail!("malformed 43/89: {f:?}")
                };
                let (seq, raw) =
                    decode_sample(s, q).with_context(|| format!("malformed 43/89: {f:?}"))?;
                let t = self.start.elapsed().as_secs_f64();
                let out = self.rebuild.push(t, raw);
                if let Some(log) = self.raw_log.as_mut() {
                    let abs = out.map_or("null".to_string(), |(a, _)| {
                        format!("[{:.6},{:.6},{:.6},{:.6}]", a.w, a.x, a.y, a.z)
                    });
                    let reset = out.is_some_and(|(_, r)| r);
                    writeln!(
                        log,
                        r#"{{"t":{t:.4},"seq":{seq},"raw":[{:.6},{:.6},{:.6},{:.6}],"abs":{abs},"reset":{reset}}}"#,
                        raw.w, raw.x, raw.y, raw.z
                    )?;
                }
                let Some((abs, _)) = out else { continue }; // held back: possible re-centre glide
                return Ok(Some(Event::Pose(abs)));
            }
            let mut buf = [0u8; 1024];
            match tokio::time::timeout_at(deadline, self.stream.read(&mut buf)).await {
                Err(_) => return Ok(None),
                Ok(r) => {
                    let n = r.context("RFCOMM read")?;
                    ensure!(n > 0, "earbuds closed the connection");
                    self.decoder.push(&buf[..n]);
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<()> {
        self.send(Frame::new(SVC, CMD_STREAM_SWITCH, &[(1, &[0])]))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Frames as pushed by a real FreeClip 2 during tools/wear_test.py.
    #[test]
    fn wear_state_any_earbud_counts() {
        let f = |l: u8, r: u8| Frame::new(43, 37, &[(1, &[l]), (2, &[r]), (3, &[0]), (4, &[0])]);
        assert_eq!(worn_from(&f(1, 1)), Some(true));
        assert_eq!(worn_from(&f(0, 1)), Some(true)); // left off
        assert_eq!(worn_from(&f(1, 0)), Some(true)); // right off
        assert_eq!(worn_from(&f(0, 0)), Some(false)); // both off
        assert_eq!(worn_from(&Frame::new(43, 37, &[(3, &[0])])), None);
    }
}
