//! Audio and desktop cues.
//!
//! Sounds play in-process on the default audio output ([`audio`]). Tones are generated at startup.
//! Spoken prompts were pre-recorded with Piper's "alba" voice and are embedded in the binary
//! (assets/voice/). tools/gen_voice.sh regenerates them. Desktop notifications go through
//! `notify-rust`.

use super::audio;
use super::detector::Cue;
use super::wav::Pcm;
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;

/// Generates [`Phrase`] and [`PhraseClips`] (one decoded clip per phrase) from `Variant => "clip"`
/// pairs. Each clip is embedded from assets/voice/<clip>.wav.
macro_rules! phrases {
    ($($phrase:ident => $clip:literal),+ $(,)?) => {
        /// Fixed spoken prompts. Their text is in tools/gen_voice.sh.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum Phrase {
            $($phrase),+
        }

        /// Every phrase's clip, decoded.
        #[allow(non_snake_case)] // one field per `Phrase` variant, named after it
        struct PhraseClips {
            $($phrase: Arc<Pcm>),+
        }

        impl PhraseClips {
            /// Decode every embedded clip.
            fn decode() -> Result<Self> {
                Ok(PhraseClips {
                    $($phrase: Arc::new(Phrase::$phrase.pcm()?)),+
                })
            }

            fn get(&self, p: Phrase) -> Arc<Pcm> {
                match p {
                    $(Phrase::$phrase => self.$phrase.clone()),+
                }
            }
        }

        impl Phrase {
            #[cfg(test)]
            const ALL: &[Phrase] = &[$(Phrase::$phrase),+];

            /// WAV bytes of the phrase's clip.
            fn clip(self) -> &'static [u8] {
                match self {
                    $(Phrase::$phrase => {
                        include_bytes!(concat!("../../assets/voice/", $clip, ".wav"))
                    })+
                }
            }
        }
    };
}

phrases! {
    SitUpright => "sit_upright",
    Calibrated => "calibrated",
    Moved => "moved",
    MoveHead => "move_head",
    LiftHead => "lift_head",
}

impl Phrase {
    fn pcm(self) -> Result<Pcm> {
        Pcm::parse(self.clip()).with_context(|| format!("embedded clip for {self:?}"))
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
    tone: Arc<Pcm>,
    tink: Arc<Pcm>,
    /// Every phrase's clip, decoded once at startup.
    phrases: PhraseClips,
}

impl Cues {
    pub fn new() -> Result<Self> {
        let phrases = PhraseClips::decode()?;
        Ok(Cues {
            tone: Arc::new(Pcm::tones(&[(523.0, 0.18), (440.0, 0.25)])),
            tink: Arc::new(Pcm::tones(&[(880.0, 0.08), (1318.5, 0.14)])),
            phrases,
        })
    }

    fn phrase(&self, p: Phrase) -> Arc<Pcm> {
        self.phrases.get(p)
    }
}

impl CueSink for Cues {
    fn play(&self, cue: Cue) {
        let pcm = match cue {
            Cue::Tone => self.tone.clone(),
            Cue::Recovered => self.tink.clone(),
            Cue::Speak => self.phrase(Phrase::LiftHead),
            Cue::Notify => {
                tokio::spawn(notify());
                return;
            }
        };
        tokio::task::spawn_blocking(move || {
            if let Err(e) = audio::play(&pcm) {
                report(&e);
            }
        });
    }

    async fn say(&self, p: Phrase) -> Result<()> {
        let pcm = self.phrase(p);
        let limit = audio::time_limit(&pcm);
        let playing = tokio::task::spawn_blocking(move || audio::play(&pcm));
        // A sound server that stops answering would block `play` forever; the session moves on.
        tokio::time::timeout(limit, playing)
            .await
            .map_err(|_| anyhow!("the sound server did not answer"))?
            .context("audio task")?
            .with_context(|| format!("playing {p:?}"))
    }
}

/// Show the "sit up" desktop notification.
async fn notify() {
    let shown = notify_rust::Notification::new()
        .appname("headup")
        .summary("Sit up straight")
        // The text names no duration, because the detector's settings decide when this fires.
        .body("You have been looking down for a while.")
        .show_async()
        .await;
    if let Err(e) = shown {
        report(&anyhow::Error::from(e).context("desktop notification"));
    }
}

/// Report a failed cue. It does not stop monitoring. It goes straight to stderr, not through
/// `Recorder`, because the cue runs in its own task, which cannot borrow the recorder. Failures
/// are rare, so breaking into a terminal status line is acceptable.
fn report(e: &anyhow::Error) {
    eprintln!("cue failed: {e:#}");
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
