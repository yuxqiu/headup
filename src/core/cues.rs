//! Audio and desktop cues.
//!
//! Sounds play in-process on the default audio output ([`audio`]). Tones are generated at startup.
//! Spoken prompts were pre-recorded with Piper's "alba" voice and are embedded in the binary
//! (assets/voice/). tools/gen_voice.sh regenerates them. Desktop notifications go through
//! `notify-rust`.

use super::audio;
use super::detector::Cue;
use super::wav::Pcm;
use anyhow::{Context, Result};
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

/// Something to play: a spoken calibration prompt, or a reminder cue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sound {
    Prompt(Phrase),
    Reminder(Cue),
}

/// Where the session sends sounds. [`Cues`] plays them; tests record them.
pub trait CueSink {
    /// Start `sound` now, and return a future that ends once it has been heard (a notification:
    /// once shown), with an error if it could not be played. Playback starts on the call, not
    /// when the future is first polled, so a caller may keep the future and do other work while
    /// the sound plays.
    fn play(&self, sound: Sound) -> impl Future<Output = Result<()>> + 'static;
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
            // Notes chosen by ear, not measured: the reminder falls from C5 to A4 (soft), the
            // tink rises from A5 to E6 (bright). The user heard both on the earbuds in the
            // 2026-11-13 live check (data/walk3*.jsonl).
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
    fn play(&self, sound: Sound) -> impl Future<Output = Result<()>> + 'static {
        let pcm = match sound {
            Sound::Prompt(p) => self.phrase(p),
            Sound::Reminder(Cue::Tone) => self.tone.clone(),
            Sound::Reminder(Cue::Recovered) => self.tink.clone(),
            Sound::Reminder(Cue::Speak) => self.phrase(Phrase::LiftHead),
            Sound::Reminder(Cue::Notify) => {
                let shown = tokio::spawn(notify());
                return finished(shown, sound);
            }
        };
        finished(tokio::spawn(audio::play_bounded(pcm)), sound)
    }
}

/// The outcome of the task playing `sound`.
async fn finished(task: tokio::task::JoinHandle<Result<()>>, sound: Sound) -> Result<()> {
    task.await
        .context("cue task")?
        .with_context(|| format!("playing {sound:?}"))
}

/// Show the "sit up" desktop notification.
async fn notify() -> Result<()> {
    let shown = notify_rust::Notification::new()
        .appname("headup")
        .summary("Sit up straight")
        // The text names no duration, because the detector's settings decide when this fires.
        .body("You have been looking down for a while.")
        .show_async()
        .await;
    shown.context("desktop notification")?;
    Ok(())
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
