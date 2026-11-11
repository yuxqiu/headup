//! Audio and desktop cues, played with `pw-play`: generated tones, and spoken prompts that are
//! pre-recorded with Piper's "alba" voice and embedded in the binary (assets/voice/, regenerate
//! with tools/gen_voice.sh). Notifications use `notify-send`.

use super::detector::Cue;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Fixed spoken prompts; the text is in tools/gen_voice.sh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phrase {
    SitUpright,
    Calibrated,
    Moved,
    LiftHead,
}

impl Phrase {
    const ALL: [Phrase; 4] = [
        Phrase::SitUpright,
        Phrase::Calibrated,
        Phrase::Moved,
        Phrase::LiftHead,
    ];

    fn file_name(self) -> &'static str {
        match self {
            Phrase::SitUpright => "sit_upright.wav",
            Phrase::Calibrated => "calibrated.wav",
            Phrase::Moved => "moved.wav",
            Phrase::LiftHead => "lift_head.wav",
        }
    }

    fn wav(self) -> &'static [u8] {
        match self {
            Phrase::SitUpright => include_bytes!("../../assets/voice/sit_upright.wav"),
            Phrase::Calibrated => include_bytes!("../../assets/voice/calibrated.wav"),
            Phrase::Moved => include_bytes!("../../assets/voice/moved.wav"),
            Phrase::LiftHead => include_bytes!("../../assets/voice/lift_head.wav"),
        }
    }
}

pub struct Cues {
    dir: PathBuf,
    tone: PathBuf,
    tink: PathBuf,
}

impl Cues {
    pub fn new() -> Result<Self> {
        for (cmd, hint) in [
            ("pw-play", "PipeWire's pw-play"),
            ("notify-send", "libnotify's notify-send"),
        ] {
            ensure!(on_path(cmd), "`{cmd}` not found on PATH ({hint})");
        }
        let dir =
            PathBuf::from(std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR not set")?)
                .join("headup");
        std::fs::create_dir_all(&dir)?;
        let tone = dir.join("tone.wav");
        let tink = dir.join("tink.wav");
        std::fs::write(&tone, wav(&[(523.0, 0.18), (440.0, 0.25)]))?;
        std::fs::write(&tink, wav(&[(880.0, 0.08), (1318.5, 0.14)]))?;
        for p in Phrase::ALL {
            std::fs::write(dir.join(p.file_name()), p.wav())?;
        }
        Ok(Cues { dir, tone, tink })
    }

    pub fn play(&self, cue: Cue) {
        match cue {
            Cue::Tone => spawn("pw-play", &[path_str(&self.tone)]),
            Cue::Recovered => spawn("pw-play", &[path_str(&self.tink)]),
            Cue::Speak => spawn("pw-play", &[path_str(&self.phrase_path(Phrase::LiftHead))]),
            Cue::Notify => spawn(
                "notify-send",
                &[
                    "-a",
                    "headup",
                    "Sit up straight",
                    "You have been looking down for a minute.",
                ],
            ),
        }
    }

    fn phrase_path(&self, p: Phrase) -> PathBuf {
        self.dir.join(p.file_name())
    }

    /// Speak a prompt and wait until it has been played.
    pub async fn say(&self, p: Phrase) -> Result<()> {
        let st = Command::new("pw-play")
            .arg(self.phrase_path(p))
            .status()
            .await?;
        ensure!(st.success(), "pw-play failed for {p:?}: {st}");
        Ok(())
    }
}

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("runtime dir path is UTF-8")
}

/// Fire-and-forget a cue command; failures are reported but don't stop monitoring.
fn spawn(cmd: &str, args: &[&str]) {
    match Command::new(cmd).args(args).spawn() {
        Ok(mut child) => {
            let cmd = cmd.to_string();
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(st) if st.success() => {}
                    other => eprintln!("cue `{cmd}` failed: {other:?}"),
                }
            });
        }
        Err(e) => eprintln!("cue `{cmd}` could not start: {e}"),
    }
}

/// 16-bit mono 44.1 kHz WAV of consecutive sine notes (Hz, seconds) with short fades.
fn wav(notes: &[(f64, f64)]) -> Vec<u8> {
    const RATE: u32 = 44_100;
    let mut pcm = Vec::new();
    for &(hz, secs) in notes {
        let n = (secs * RATE as f64) as usize;
        let fade = (0.01 * RATE as f64) as usize;
        for i in 0..n {
            let env = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
            let v = (2.0 * std::f64::consts::PI * hz * i as f64 / RATE as f64).sin() * env * 0.3;
            pcm.extend_from_slice(&((v * i16::MAX as f64) as i16).to_le_bytes());
        }
    }
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}
