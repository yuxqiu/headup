//! The FreeClip 2's MBB service 43 messages that headup uses. This module holds the frames headup
//! sends, one typed parse of every frame it receives, and their `--raw-log` records. Sources:
//! docs/devices/freeclip2/FINDINGS.md, and the catalogue in
//! docs/devices/freeclip2/mbb_commands.txt.

use super::mbb::Frame;
use crate::core::quat::Quat;
use crate::devices::bluetooth::Address;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

const SVC: u8 = 43;
const CMD_WEAR_STATE: u8 = 37;
const CMD_DEVICE_STATE: u8 = 54;
const CMD_STREAM_SWITCH: u8 = 88;
const CMD_STREAM_DATA: u8 = 89;
const CMD_SPATIAL_CAPABILITY: u8 = 90;
/// The current device's audio state while it streams. The 43/54 state byte is
/// `connected | audio << 1`.
const AUDIO_STREAMING: u8 = 4;

/// 43/37 query: report the wear state of both earbuds.
pub fn wear_query() -> Frame {
    Frame::new(SVC, CMD_WEAR_STATE, &[(1, &[]), (2, &[])])
}

/// 43/90 query: spatial-audio capability. Its reply is ignored. headup sends it because tracking
/// once came back right after it (FINDINGS "Tracking outage"). The firmware records the asking
/// device in a list, but the 43/89 path does not check that list (FINDINGS "Firmware head-tracking
/// logic").
pub fn capability_query() -> Frame {
    Frame::new(SVC, CMD_SPATIAL_CAPABILITY, &[(1, &[])])
}

/// 43/88: switch the pose stream on or off.
pub fn stream_switch(on: bool) -> Frame {
    Frame::new(SVC, CMD_STREAM_SWITCH, &[(1, &[u8::from(on)])])
}

/// A frame from the earbuds, parsed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Message {
    /// 43/37: whether both earbuds are worn. Head tracking stops as soon as either comes off
    /// (tools/freeclip2/device/single_bud_test.py), so one worn earbud counts as not worn.
    Wear { both_worn: bool },
    /// 43/54 (TLV 5 form): one connected source device's audio state. Head tracking only flows
    /// to the earbuds' current device, the one that last started streaming audio.
    DeviceAudio { address: Address, streaming: bool },
    /// 43/88 reply: whether the earbuds accepted a stream switch.
    StreamSwitch(SwitchReply),
    /// 43/89: one pose sample, relative to the earbuds' current reference.
    Sample { seq: u32, raw: Quat },
    /// Acks and pushes headup does not use.
    Other,
}

impl Message {
    /// Parse `f`. A malformed frame of a kind headup uses is an error.
    pub fn parse(f: &Frame) -> Result<Message> {
        let parsed = match (f.svc, f.cmd) {
            (SVC, CMD_WEAR_STATE) => wear(f),
            (SVC, CMD_DEVICE_STATE) => device_audio(f),
            (SVC, CMD_STREAM_DATA) => sample(f),
            (SVC, CMD_STREAM_SWITCH) => switch_reply(f),
            _ => return Ok(Message::Other),
        };
        parsed.with_context(|| format!("malformed {}/{}: {f:?}", f.svc, f.cmd))
    }
}

/// 43/37: TLV 1 = left, TLV 2 = right, 1 = worn.
fn wear(f: &Frame) -> Result<Message> {
    let side = |t| f.tlv(t).and_then(|v| v.first().copied());
    let (Some(left), Some(right)) = (side(1), side(2)) else {
        bail!("no wear state for both earbuds");
    };
    Ok(Message::Wear {
        both_worn: left == 1 && right == 1,
    })
}

/// 43/54, the paired-device-list change report. Its TLV 5 form carries one device's audio state.
/// TLV 5 holds the device's Bluetooth address (little-endian), then a state byte.
///
/// The firmware builds the state byte as `connected | audio << 1`. `audio` is the audio state of
/// the earbuds' current device, and 0 for any other device. `audio` 4 means streaming (FINDINGS
/// "43/54 state byte"). Recordings show 0x01, 0x03 and 0x09. Any other `audio` value is read as
/// not streaming: it is a variant, not a broken frame.
///
/// Other forms of 43/54 (e.g. TLV 1 = `01 00`) are other list reports.
fn device_audio(f: &Frame) -> Result<Message> {
    let Some(v) = f.tlv(5) else {
        return Ok(Message::Other);
    };
    let (Some(address), Some(&state)) = (v.get(..6), v.get(6)) else {
        bail!("TLV 5 too short");
    };
    let mut be: [u8; 6] = address.try_into()?;
    be.reverse();
    Ok(Message::DeviceAudio {
        address: Address(be),
        streaming: state >> 1 == AUDIO_STREAMING,
    })
}

/// The earbuds' answer to a 43/88 stream switch (TLV 2 of the reply), from the firmware's 43/88
/// handler (FINDINGS "43/88 (switch the data report on or off)").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReply {
    /// 0: switched. The stream also needs the earbuds' own conditions (worn, audio).
    Accepted,
    /// 1: refused while the link between the two earbuds is down (FINDINGS "Reply value 1").
    PeerLinkDown,
    /// 2: refused because this computer is not the earbuds' current device, which is the last
    /// one to start streaming audio. Seen when headup connects while the phone was last active.
    NotCurrentDevice,
}

/// 43/88 reply: TLV 2 = one result byte.
fn switch_reply(f: &Frame) -> Result<Message> {
    let reply = match f.tlv(2) {
        Some([0]) => SwitchReply::Accepted,
        Some([1]) => SwitchReply::PeerLinkDown,
        Some([2]) => SwitchReply::NotCurrentDevice,
        other => bail!("unknown stream switch result {other:?}"),
    };
    Ok(Message::StreamSwitch(reply))
}

/// 43/89: TLV 1 = u32 LE sequence number, TLV 2 = 4 x f32 LE (w, x, y, z).
fn sample(f: &Frame) -> Result<Message> {
    let seq: [u8; 4] = f.tlv(1).context("no sequence number")?.try_into()?;
    let quat: [u8; 16] = f.tlv(2).context("no quaternion")?.try_into()?;
    let c = |i: usize| {
        f64::from(f32::from_le_bytes(
            quat[i * 4..i * 4 + 4].try_into().unwrap(),
        ))
    };
    Ok(Message::Sample {
        seq: u32::from_le_bytes(seq),
        raw: Quat {
            w: c(0),
            x: c(1),
            y: c(2),
            z: c(3),
        },
    })
}

/// The 43/89 frame for a sample: the inverse of [`Message::parse`], for replaying recordings.
#[cfg(test)]
pub fn sample_frame(seq: u32, raw: Quat) -> Frame {
    let quat: Vec<u8> = [raw.w, raw.x, raw.y, raw.z]
        .into_iter()
        .flat_map(|c| (c as f32).to_le_bytes())
        .collect();
    Frame::new(SVC, CMD_STREAM_DATA, &[(1, &seq.to_le_bytes()), (2, &quat)])
}

/// `--raw-log` record of a 43/89 sample as the earbuds sent it: `{"seq":7,"raw":[w,x,y,z]}`. The
/// earbuds send f32, so the components are logged as f32, which is exact and short.
#[derive(Serialize)]
pub struct RawSample {
    seq: u32,
    raw: [f32; 4],
}

impl RawSample {
    pub fn new(seq: u32, q: Quat) -> Self {
        let raw = [q.w, q.x, q.y, q.z].map(|c| c as f32);
        RawSample { seq, raw }
    }
}

/// `--raw-log` record of any other frame, e.g. wear state or a device push, with TLV values in hex:
/// `{"svc":43,"cmd":37,"tlvs":{"1":"01","2":"01"}}`.
#[derive(Serialize)]
pub struct RawFrame {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Frames as pushed by a real FreeClip 2 during tools/freeclip2/device/single_bud_test.py. Head
    // tracking only runs while both earbuds are worn.
    #[test]
    fn wear_state_needs_both_earbuds() {
        let f = |l: u8, r: u8| Frame::new(43, 37, &[(1, &[l]), (2, &[r]), (3, &[0]), (4, &[0])]);
        let worn = |f: Frame| match Message::parse(&f).unwrap() {
            Message::Wear { both_worn } => both_worn,
            other => panic!("{other:?}"),
        };
        assert!(worn(f(1, 1)));
        assert!(!worn(f(0, 1))); // left off
        assert!(!worn(f(1, 0))); // right off
        assert!(!worn(f(0, 0))); // both off
        assert!(Message::parse(&Frame::new(43, 37, &[(3, &[0])])).is_err());
    }

    // 43/54 pushes from a real FreeClip 2 (tools/freeclip2/device/single_bud_test.py,
    // data/reclaim_test.log). This computer is D4:94:A9:22:86:6F, sent little-endian.
    #[test]
    fn device_audio_state() {
        let parse = |s: &str| Message::parse(&Frame::new(43, 54, &[(5, &hex(s))]));
        let audio = |address: &str, streaming| Message::DeviceAudio {
            address: address.parse().unwrap(),
            streaming,
        };
        let pc = "D4:94:A9:22:86:6F";
        assert_eq!(parse("6f8622a994d409").unwrap(), audio(pc, true));
        assert_eq!(parse("6f8622a994d403").unwrap(), audio(pc, false));
        assert_eq!(parse("6f8622a994d401").unwrap(), audio(pc, false));
        let phone = "EC:81:50:6E:86:B5";
        assert_eq!(parse("b5866e5081ec01").unwrap(), audio(phone, false));
        assert!(parse("0102").is_err());
    }

    // 43/54 is the paired-device-list change report ("连接管理—PDL列表变化上报" in the vendor
    // app). Only its TLV 5 form carries a device's audio state. This other form came from a real
    // FreeClip 2 (data/live3_raw.jsonl) and must not end the session.
    #[test]
    fn other_device_list_reports_are_not_errors() {
        let f = Frame::new(43, 54, &[(1, &[0x01, 0x00])]);
        assert_eq!(Message::parse(&f).unwrap(), Message::Other);
    }

    // 43/88 replies from a real FreeClip 2 (data/live2_raw.jsonl, data/live3_raw.jsonl). The reply
    // was `02` while the phone was the current device, and `00` once this computer streamed.
    #[test]
    fn stream_switch_replies() {
        let parse = |v: &[u8]| Message::parse(&Frame::new(43, 88, &[(2, v)]));
        let reply = Message::StreamSwitch;
        assert_eq!(parse(&[0]).unwrap(), reply(SwitchReply::Accepted));
        assert_eq!(parse(&[2]).unwrap(), reply(SwitchReply::NotCurrentDevice));
        assert_eq!(parse(&[1]).unwrap(), reply(SwitchReply::PeerLinkDown));
        assert!(parse(&[3]).is_err());
        assert!(Message::parse(&Frame::new(43, 88, &[])).is_err());
    }

    #[test]
    fn decodes_real_sample() {
        let f = Frame::new(
            43,
            89,
            &[
                (1, &hex("0b000000")),
                (2, &hex("f2ff7f3f6afcefb97b6eedb885909dba")),
            ],
        );
        let Message::Sample { seq, raw } = Message::parse(&f).unwrap() else {
            panic!("not a sample");
        };
        assert_eq!(seq, 11);
        assert!((raw.w - 1.0).abs() < 1e-5 && raw.x.abs() < 1e-3);
        assert_eq!(
            Message::parse(&sample_frame(seq, raw)).unwrap(),
            Message::Sample { seq, raw }
        );
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
}
