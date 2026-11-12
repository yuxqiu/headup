//! HUAWEI FreeClip 2 backend: MBB over RFCOMM channel 1 (docs/devices/freeclip2/FINDINGS.md).
//!
//! - 43/88 switches the spatial-audio pose stream on/off; 43/89 carries relative quaternions that
//!   re-centre themselves and restart with each new stream (`rebuild` makes them continuous).
//! - 43/37 reports per-earbud wear state, pushed on change and answered when queried. Tracking
//!   stops when either earbud is off, so [`Event::Worn`] means both are worn.
//! - 43/54 reports each connected source device's audio state; head tracking only flows while
//!   this computer is streaming audio, which becomes [`Event::Available`].
//! - Re-centres are inaccurate, so the rebuilt pose drifts ([`TrackerTraits::pose_drifts_up`]).

mod mbb;
mod rebuild;

use crate::core::quat::Quat;
use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use crate::devices::{Backend, Found};
use anyhow::{Context, Result, bail, ensure};
use bluer::rfcomm::{SocketAddr, Stream};
use mbb::{Decoder, Frame};
use rebuild::{Rebuilder, decode_sample};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

const RFCOMM_CHANNEL: u8 = 1;
const SVC: u8 = 43;
const CMD_STREAM_SWITCH: u8 = 88;
const CMD_STREAM_DATA: u8 = 89;
const CMD_WEAR_STATE: u8 = 37;
const CMD_DEVICE_STATE: u8 = 54;
const CMD_SPATIAL_CAPABILITY: u8 = 90;

/// 43/88: switch the pose stream on or off.
fn stream_switch(on: bool) -> Frame {
    Frame::new(SVC, CMD_STREAM_SWITCH, &[(1, &[u8::from(on)])])
}

/// Wear state from a 43/37 frame: TLV 1 = left, TLV 2 = right, 1 = worn. Head tracking stops as
/// soon as either earbud comes off (tools/freeclip2/single_bud_test.py), so "worn" means both.
fn worn_from(f: &Frame) -> Option<bool> {
    let side = |t| f.tlv(t).and_then(|v| v.first().copied());
    match (side(1), side(2)) {
        (Some(l), Some(r)) => Some(l == 1 && r == 1),
        _ => None,
    }
}

/// Audio state of one connected source device from a 43/54 push: TLV 5 = the device's Bluetooth
/// address (little-endian) and a state byte. Observed: 0x09 when it starts streaming audio, 0x03 when
/// it goes idle, 0x01 on connection; bit 0x08 is read as "streaming" [inferred from those values].
/// Head tracking only flows to the streaming device (docs/devices/freeclip2/FINDINGS.md).
fn device_streaming(f: &Frame) -> Option<(bluer::Address, bool)> {
    let v = f.tlv(5)?;
    let (addr, state) = (v.get(..6)?, *v.get(6)?);
    let mut be = [0u8; 6];
    be.copy_from_slice(addr);
    be.reverse();
    Some((bluer::Address::new(be), state & 0x08 != 0))
}

pub struct FreeClip2 {
    stream: Stream,
    decoder: Decoder,
    rebuild: Rebuilder,
    start: Instant,
    log: DeviceLog,
    worn: Option<bool>,
    /// This computer's adapter address, to pick its entry out of 43/54 pushes.
    adapter: bluer::Address,
    streaming: Option<bool>,
    /// Orientations released by `rebuild`, not yet returned as events.
    poses: VecDeque<Quat>,
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

    async fn send(&mut self, f: Frame) -> Result<()> {
        self.stream
            .write_all(&f.encode())
            .await
            .context("RFCOMM write")
    }

    /// Handle one frame from the earbuds; `None` if it produces no event.
    async fn handle(&mut self, f: Frame) -> Result<Option<Event>> {
        if (f.svc, f.cmd) != (SVC, CMD_STREAM_DATA) {
            self.log.record(&RawFrame::from(&f))?;
        }
        match (f.svc, f.cmd) {
            (SVC, CMD_WEAR_STATE) => self.on_wear_state(&f).await,
            (SVC, CMD_DEVICE_STATE) => self.on_device_state(&f).await,
            (SVC, CMD_STREAM_DATA) => self.on_pose_sample(&f),
            _ => Ok(None), // acks and status pushes we don't use
        }
    }

    async fn on_wear_state(&mut self, f: &Frame) -> Result<Option<Event>> {
        let Some(worn) = worn_from(f) else {
            bail!("malformed 43/37: {f:?}")
        };
        let was = self.worn.replace(worn);
        if was == Some(worn) {
            return Ok(None);
        }
        if worn && was == Some(false) {
            // Back on the ear: start a fresh orientation frame.
            self.rebuild = Rebuilder::default();
            self.poses.clear();
            self.send(stream_switch(true)).await?;
        }
        Ok(Some(Event::Worn(worn)))
    }

    async fn on_device_state(&mut self, f: &Frame) -> Result<Option<Event>> {
        let Some((addr, streaming)) = device_streaming(f) else {
            return Ok(None);
        };
        if addr != self.adapter || self.streaming.replace(streaming) == Some(streaming) {
            return Ok(None);
        }
        if streaming {
            // A 43/88 sent while this computer was idle once left tracking off after audio began
            // (FINDINGS "Stream switch sent while idle"), so switch the stream on again now.
            self.send(stream_switch(true)).await?;
        }
        Ok(Some(Event::Available(streaming)))
    }

    /// Queue the orientations a raw sample releases (none while samples wait out the delay).
    fn on_pose_sample(&mut self, f: &Frame) -> Result<Option<Event>> {
        let (Some(s), Some(q)) = (f.tlv(1), f.tlv(2)) else {
            bail!("malformed 43/89: {f:?}")
        };
        let (seq, raw) = decode_sample(s, q).with_context(|| format!("malformed 43/89: {f:?}"))?;
        self.log.record(&RawSample::new(seq, raw))?;
        let t = self.elapsed();
        let released = self.rebuild.push(t, seq, raw);
        self.poses.extend(released.into_iter().map(|(_, abs)| abs));
        Ok(None)
    }

    fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// `--raw-log` record of a 43/89 sample as sent: `{"seq":7,"raw":[w,x,y,z]}`. The earbuds send
/// f32, so the components are logged as f32 (exact and short).
#[derive(Serialize)]
struct RawSample {
    seq: u32,
    raw: [f32; 4],
}

impl RawSample {
    fn new(seq: u32, q: Quat) -> Self {
        let raw = [q.w, q.x, q.y, q.z].map(|c| c as f32);
        RawSample { seq, raw }
    }
}

/// `--raw-log` record of any other frame, e.g. wear state or a device push, with TLV values in hex:
/// `{"svc":43,"cmd":37,"tlvs":{"1":"01","2":"01"}}`.
#[derive(Serialize)]
struct RawFrame {
    svc: u8,
    cmd: u8,
    tlvs: BTreeMap<u8, String>,
}

impl From<&Frame> for RawFrame {
    fn from(f: &Frame) -> Self {
        let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect();
        RawFrame {
            svc: f.svc,
            cmd: f.cmd,
            tlvs: f.tlvs.iter().map(|(k, v)| (*k, hex(v))).collect(),
        }
    }
}

impl Backend for FreeClip2 {
    fn recognises(bluetooth_name: &str) -> bool {
        // Same rule as the vendor app's device whitelist: `^(?=(HUAWEI FreeClip 2))`.
        bluetooth_name.starts_with("HUAWEI FreeClip 2")
    }

    /// Connect, query wear state and start the pose stream. `log` receives every raw sample
    /// (`RawSample`) and every other frame (`RawFrame`).
    async fn connect(found: &Found, log: DeviceLog) -> Result<Self> {
        let addr = found.address;
        let stream = Stream::connect(SocketAddr::new(addr, RFCOMM_CHANNEL))
            .await
            .with_context(|| format!("RFCOMM connect to {addr} channel {RFCOMM_CHANNEL} (is the FreeClip 2 connected?)"))?;
        let mut dev = FreeClip2 {
            stream,
            decoder: Decoder::default(),
            rebuild: Rebuilder::default(),
            start: Instant::now(),
            log,
            worn: None,
            adapter: found.adapter,
            streaming: None,
            poses: VecDeque::new(),
        };
        dev.send(Frame::new(SVC, CMD_WEAR_STATE, &[(1, &[]), (2, &[])]))
            .await?;
        // Possibly needed to arm the stream: tracking came back right after this query once
        // (FINDINGS "Tracking outage") [unverified]. Its reply is ignored.
        dev.send(Frame::new(SVC, CMD_SPATIAL_CAPABILITY, &[(1, &[])]))
            .await?;
        dev.send(stream_switch(true)).await?;
        Ok(dev)
    }
}

impl HeadTracker for FreeClip2 {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(q) = self.poses.pop_front() {
                return Ok(Some(Event::Pose(q)));
            }
            if let Some(f) = self.decoder.next_frame()? {
                if let Some(event) = self.handle(f).await? {
                    return Ok(Some(event));
                }
                continue;
            }
            // Wake for whichever comes first: the caller's deadline or the next held sample.
            let release = self
                .rebuild
                .next_release()
                .map(|t| self.start + Duration::from_secs_f64(t));
            let wake = release.map_or(deadline, |r| r.min(deadline));
            let mut buf = [0u8; 1024];
            match tokio::time::timeout_at(wake, self.stream.read(&mut buf)).await {
                Ok(read) => {
                    let n = read.context("RFCOMM read")?;
                    ensure!(n > 0, "earbuds closed the connection");
                    self.decoder.push(&buf[..n]);
                }
                Err(_) => {
                    let now = self.elapsed();
                    let released = self.rebuild.release(now);
                    self.poses.extend(released.into_iter().map(|(_, abs)| abs));
                    if self.poses.is_empty() && Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<()> {
        self.send(stream_switch(false)).await
    }

    fn traits(&self) -> TrackerTraits {
        Self::TRAITS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only modules in tests/: recordings replayed through the rebuilder (and detector).
    mod end_to_end;
    pub(super) mod replay;

    // Frames as pushed by a real FreeClip 2 during tools/freeclip2/single_bud_test.py: head
    // tracking only runs while both earbuds are worn.
    #[test]
    fn wear_state_needs_both_earbuds() {
        let f = |l: u8, r: u8| Frame::new(43, 37, &[(1, &[l]), (2, &[r]), (3, &[0]), (4, &[0])]);
        assert_eq!(worn_from(&f(1, 1)), Some(true));
        assert_eq!(worn_from(&f(0, 1)), Some(false)); // left off
        assert_eq!(worn_from(&f(1, 0)), Some(false)); // right off
        assert_eq!(worn_from(&f(0, 0)), Some(false)); // both off
        assert_eq!(worn_from(&Frame::new(43, 37, &[(3, &[0])])), None);
    }

    #[test]
    fn raw_log_records() {
        let q = Quat {
            w: 1.0,
            x: f64::from(-0.25f32),
            y: 0.0,
            z: 0.0,
        };
        assert_eq!(
            serde_json::to_string(&RawSample::new(7, q)).unwrap(),
            r#"{"seq":7,"raw":[1.0,-0.25,0.0,0.0]}"#
        );
        let f = Frame::new(43, 37, &[(2, &[0]), (1, &[1, 0xab])]);
        assert_eq!(
            serde_json::to_string(&RawFrame::from(&f)).unwrap(),
            r#"{"svc":43,"cmd":37,"tlvs":{"1":"01ab","2":"00"}}"#
        );
    }

    // 43/54 pushes from a real FreeClip 2 (tools/freeclip2/single_bud_test.py, data/reclaim_test.log):
    // this computer is D4:94:A9:22:86:6F, sent little-endian.
    #[test]
    fn device_streaming_state() {
        let f = |hex: &str| {
            let v: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            Frame::new(43, 54, &[(5, &v)])
        };
        let pc: bluer::Address = "D4:94:A9:22:86:6F".parse().unwrap();
        assert_eq!(device_streaming(&f("6f8622a994d409")), Some((pc, true)));
        assert_eq!(device_streaming(&f("6f8622a994d403")), Some((pc, false)));
        assert_eq!(device_streaming(&f("6f8622a994d401")), Some((pc, false)));
        let phone: bluer::Address = "EC:81:50:6E:86:B5".parse().unwrap();
        assert_eq!(device_streaming(&f("b5866e5081ec01")), Some((phone, false)));
        assert_eq!(device_streaming(&Frame::new(43, 54, &[(5, &[1, 2])])), None);
    }
}
