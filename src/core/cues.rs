//! Audio and desktop cues. Sounds are played with `pw-play`: generated tones, and spoken prompts
//! pre-recorded with Piper's "alba" voice and embedded in the binary (assets/voice/, regenerate with tools/gen_voice.sh). Notifications use
//! `notify-send`.

use super::detector::Cue;
use super::wav::Pcm;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Generates [`Phrase`] from `Variant => "clip"` pairs; each clip is embedded from
/// assets/voice/<clip>.wav.
macro_rules! phrases {
    ($($phrase:ident => $clip:literal),+ $(,)?) => {
        /// Fixed spoken prompts; the text is in tools/gen_voice.sh.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum Phrase {
            $($phrase),+
        }

        impl Phrase {
            const ALL: &[Phrase] = &[$(Phrase::$phrase),+];

            /// File name and WAV bytes of the phrase's clip.
            fn clip(self) -> (&'static str, &'static [u8]) {
                match self {
                    $(Phrase::$phrase => (
                        concat!($clip, ".wav"),
                        include_bytes!(concat!("../../assets/voice/", $clip, ".wav")),
                    ),)+
                }
            }
        }
    };
}

phrases! {
    SitUpright => "sit_upright",
    Calibrated => "calibrated",
    Moved => "moved",
    LiftHead => "lift_head",
}

impl Phrase {
    fn pcm(self) -> Result<Pcm> {
        Pcm::parse(self.clip().1).with_context(|| format!("embedded clip for {self:?}"))
    }
}

/// Where the session sends reminders and prompts. [`Cues`] plays them; tests record them.
#[allow(async_fn_in_trait)] // used through generics only
pub trait CueSink {
    /// Start a reminder cue without waiting for it to finish.
    fn play(&self, cue: Cue);
    /// Speak a prompt and wait until it has been played.
    async fn say(&self, p: Phrase) -> Result<()>;
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
        let write = |name: &str, pcm: Pcm| -> Result<PathBuf> {
            let path = dir.join(name);
            std::fs::write(&path, pcm.encode())?;
            Ok(path)
        };
        let tone = write("tone.wav", Pcm::tones(&[(523.0, 0.18), (440.0, 0.25)]))?;
        let tink = write("tink.wav", Pcm::tones(&[(880.0, 0.08), (1318.5, 0.14)]))?;
        for &p in Phrase::ALL {
            write(p.clip().0, p.pcm()?)?;
        }
        Ok(Cues { dir, tone, tink })
    }

    fn phrase_path(&self, p: Phrase) -> PathBuf {
        self.dir.join(p.clip().0)
    }
}

impl CueSink for Cues {
    fn play(&self, cue: Cue) {
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

    async fn say(&self, p: Phrase) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_clips_are_mono_16_bit_pcm() {
        for &p in Phrase::ALL {
            let pcm = p.pcm().unwrap();
            assert!(
                pcm.samples.len() > pcm.rate as usize / 2,
                "{p:?} is under 0.5 s"
            );
        }
    }
}
