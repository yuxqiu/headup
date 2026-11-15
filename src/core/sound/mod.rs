//! What headup plays: spoken calibration prompts ([`Phrase`]) and reminders ([`Cue`]), both a
//! [`Sound`], and the [`Player`] that plays them on this computer ([`DesktopPlayer`]).
//!
//! Sounds play in-process on the default audio output (the private `audio`). Tones are generated
//! at startup. Spoken prompts were pre-recorded with Piper's "alba" voice and are embedded in the
//! binary (assets/voice/). tools/gen_voice.sh regenerates them. Desktop notifications go through
//! `notify-rust`.

mod audio;
mod desktop;
mod wav;

use anyhow::{Context, Result};
pub use desktop::DesktopPlayer;
use std::sync::Arc;
use wav::Pcm;

/// Generates [`Phrase`] and [`PhraseClips`] (one decoded clip per phrase) from `Variant => "clip"`
/// pairs. Each clip is embedded from `assets/voice/<clip>.wav`.
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
                        include_bytes!(concat!("../../../assets/voice/", $clip, ".wav"))
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

/// A reminder while monitoring: how headup tells the user about their posture. Monitoring decides
/// when each is due ([`super::monitor::Monitor`]); a [`Player`] plays them.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Cue {
    /// Soft tone in the earbuds, the first reminder of a slouch.
    Tone,
    /// Spoken reminder; repeated while the slouch goes on.
    Speak,
    /// Desktop notification.
    Notify,
    /// The "tink": the user sat back up after at least one reminder started.
    Recovered,
}

/// Something to play: a spoken calibration prompt, or a reminder cue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sound {
    Prompt(Phrase),
    Reminder(Cue),
}

/// Where the session sends sounds. [`DesktopPlayer`] plays them; tests record them.
pub trait Player {
    /// Start `sound` now, and return a future that ends once it has been heard (a notification:
    /// once shown), with an error if it could not be played. Playback starts on the call, not
    /// when the future is first polled, so a caller may keep the future and do other work while
    /// the sound plays.
    fn play(&self, sound: Sound) -> impl Future<Output = Result<()>> + 'static;
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
