//! Minimal mono 16-bit PCM WAV: parse, encode, and generate tones.

use anyhow::{Result, bail, ensure};

#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Consecutive sine notes (Hz, seconds) at 44.1 kHz with 10 ms fades.
    pub fn tones(notes: &[(f64, f64)]) -> Pcm {
        const RATE: u32 = 44_100;
        let mut samples = Vec::new();
        for &(hz, secs) in notes {
            let n = (secs * RATE as f64) as usize;
            let fade = (0.01 * RATE as f64) as usize;
            for i in 0..n {
                let env = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
                let v =
                    (2.0 * std::f64::consts::PI * hz * i as f64 / RATE as f64).sin() * env * 0.3;
                samples.push((v * i16::MAX as f64) as i16);
            }
        }
        Pcm {
            rate: RATE,
            samples,
        }
    }

    /// Parse a RIFF/WAVE file; only mono 16-bit PCM is accepted.
    pub fn parse(bytes: &[u8]) -> Result<Pcm> {
        ensure!(
            bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
            "not a RIFF/WAVE file"
        );
        let (mut rate, mut data) = (None, None);
        let mut pos = 12;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let body = bytes.get(pos + 8..pos + 8 + len).ok_or_else(|| {
                anyhow::anyhow!("truncated WAV chunk {:?}", String::from_utf8_lossy(id))
            })?;
            match id {
                b"fmt " => {
                    ensure!(body.len() >= 16, "short fmt chunk");
                    let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                    let (format, channels, bits) = (u16_at(0), u16_at(2), u16_at(14));
                    if (format, channels, bits) != (1, 1, 16) {
                        bail!(
                            "unsupported WAV: format {format}, {channels} channels, {bits} bits (need PCM mono 16-bit)"
                        );
                    }
                    rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()));
                }
                b"data" => data = Some(body),
                _ => {}
            }
            pos += 8 + len + (len & 1); // chunks are word-aligned
        }
        let (Some(rate), Some(data)) = (rate, data) else {
            bail!("WAV without fmt or data chunk")
        };
        let (pairs, _) = data.as_chunks::<2>();
        let samples = pairs.iter().map(|b| i16::from_le_bytes(*b)).collect();
        Ok(Pcm { rate, samples })
    }

    #[cfg(test)]
    pub fn encode(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_parse_round_trip() {
        let pcm = Pcm::tones(&[(440.0, 0.05)]);
        assert_eq!(Pcm::parse(&pcm.encode()).unwrap(), pcm);
    }

    #[test]
    fn rejects_non_mono_16_bit() {
        let mut bytes = Pcm {
            rate: 8000,
            samples: vec![1],
        }
        .encode();
        bytes[22] = 2; // channels = 2
        assert!(Pcm::parse(&bytes).is_err());
        assert!(Pcm::parse(b"not a wav").is_err());
    }
}
