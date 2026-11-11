//! Huawei "MBB" SPP framing: `0x5A | len(u16 BE) | 0x00 | svc | cmd | TLV* | CRC16-XMODEM(BE)`,
//! where `len` counts the bytes from the 0x00 byte through the last TLV.

use anyhow::{Result, bail, ensure};

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub svc: u8,
    pub cmd: u8,
    pub tlvs: Vec<(u8, Vec<u8>)>,
}

impl Frame {
    pub fn new(svc: u8, cmd: u8, tlvs: &[(u8, &[u8])]) -> Self {
        Frame {
            svc,
            cmd,
            tlvs: tlvs.iter().map(|(t, v)| (*t, v.to_vec())).collect(),
        }
    }

    pub fn tlv(&self, t: u8) -> Option<&[u8]> {
        self.tlvs
            .iter()
            .find(|(k, _)| *k == t)
            .map(|(_, v)| v.as_slice())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = vec![0x00, self.svc, self.cmd];
        for (t, v) in &self.tlvs {
            body.push(*t);
            body.push(u8::try_from(v.len()).expect("TLV value longer than 255 bytes"));
            body.extend_from_slice(v);
        }
        let mut out = vec![0x5A];
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend_from_slice(&body);
        let crc = crc16_xmodem(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }
}

/// Incremental decoder for the byte stream coming from the earbuds.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Next complete frame, `Ok(None)` if more bytes are needed. Desync and CRC errors are fatal.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.buf.len() < 3 {
            return Ok(None);
        }
        ensure!(
            self.buf[0] == 0x5A,
            "MBB desync: {:02x?}",
            &self.buf[..self.buf.len().min(16)]
        );
        let len = u16::from_be_bytes([self.buf[1], self.buf[2]]) as usize;
        let total = 3 + len + 2;
        if self.buf.len() < total {
            return Ok(None);
        }
        let raw: Vec<u8> = self.buf.drain(..total).collect();
        let crc = u16::from_be_bytes([raw[total - 2], raw[total - 1]]);
        ensure!(
            crc16_xmodem(&raw[..total - 2]) == crc,
            "MBB CRC mismatch: {:02x?}",
            raw
        );
        ensure!(
            len >= 3 && raw[3] == 0x00,
            "MBB unexpected header: {:02x?}",
            raw
        );
        let (svc, cmd) = (raw[4], raw[5]);
        let mut tlvs = Vec::new();
        let (mut pos, end) = (6, 3 + len);
        while pos < end {
            if pos + 2 > end || pos + 2 + raw[pos + 1] as usize > end {
                bail!("MBB truncated TLV: {:02x?}", raw);
            }
            let (t, l) = (raw[pos], raw[pos + 1] as usize);
            tlvs.push((t, raw[pos + 2..pos + 2 + l].to_vec()));
            pos += 2 + l;
        }
        Ok(Some(Frame { svc, cmd, tlvs }))
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

    // Frames below were sent to a real FreeClip 2 and accepted (data/probe_0*.log).
    #[test]
    fn encodes_known_frames() {
        assert_eq!(
            Frame::new(43, 88, &[(1, &[1])]).encode(),
            hex("5a0006002b580101014209")
        );
        assert_eq!(
            Frame::new(43, 88, &[(1, &[0])]).encode(),
            hex("5a0006002b580101005228")
        );
        assert_eq!(
            Frame::new(43, 180, &[(1, &[0x18]), (2, &[]), (3, &[])]).encode(),
            hex("5a000a002bb4010118020003009b3f")
        );
    }

    #[test]
    fn decodes_split_stream_of_frames() {
        let a = Frame::new(
            43,
            89,
            &[
                (1, &hex("0b000000")),
                (2, &hex("f2ff7f3f6afcefb97b6eedb885909dba")),
            ],
        );
        let b = Frame::new(1, 6, &[]);
        let bytes = [a.encode(), b.encode()].concat();
        let mut d = Decoder::default();
        d.push(&bytes[..7]);
        assert_eq!(d.next_frame().unwrap(), None);
        d.push(&bytes[7..]);
        assert_eq!(d.next_frame().unwrap(), Some(a));
        assert_eq!(d.next_frame().unwrap(), Some(b));
        assert_eq!(d.next_frame().unwrap(), None);
    }

    #[test]
    fn rejects_bad_crc_and_desync() {
        let mut f = Frame::new(43, 88, &[(1, &[1])]).encode();
        *f.last_mut().unwrap() ^= 1;
        let mut d = Decoder::default();
        d.push(&f);
        assert!(d.next_frame().is_err());
        let mut d = Decoder::default();
        d.push(&[0x00, 0x01, 0x02]);
        assert!(d.next_frame().is_err());
    }
}
