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

/// Generates [`Voice`] and [`VoiceClips`] (one decoded clip per voice) from `Variant => "clip"`
/// pairs. Each clip is embedded from `assets/voice/<clip>.wav`.
macro_rules! voices {
    ($($voice:ident => $clip:literal),+ $(,)?) => {
        /// The embedded spoken clips. Their text is in tools/gen_voice.sh.
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Voice {
            $($voice),+
        }

        /// Every voice clip, decoded.
        #[allow(non_snake_case)] // one field per `Voice` variant, named after it
        struct VoiceClips {
            $($voice: Arc<Pcm>),+
        }

        impl VoiceClips {
            /// Decode every embedded clip.
            fn decode() -> Result<Self> {
                Ok(VoiceClips {
                    $($voice: Arc::new(Voice::$voice.pcm()?)),+
                })
            }

            fn get(&self, v: Voice) -> Arc<Pcm> {
                match v {
                    $(Voice::$voice => self.$voice.clone()),+
                }
            }
        }

        impl Voice {
            #[cfg(test)]
            const ALL: &[Voice] = &[$(Voice::$voice),+];

            /// WAV bytes of the clip.
            fn clip(self) -> &'static [u8] {
                match self {
                    $(Voice::$voice => {
                        include_bytes!(concat!("../../../assets/voice/", $clip, ".wav"))
                    })+
                }
            }
        }
    };
}

voices! {
    SitUpright => "sit_upright",
    Calibrated => "calibrated",
    Moved => "moved",
    MoveHead => "move_head",
    LiftHead => "lift_head",
}

impl Voice {
    fn pcm(self) -> Result<Pcm> {
        Pcm::parse(self.clip()).with_context(|| format!("embedded clip for {self:?}"))
    }
}

/// A spoken calibration prompt (its text is in tools/gen_voice.sh, under the same clip name). The
/// spoken reminder is not one: it plays as [`Cue::Speak`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phrase {
    SitUpright,
    Calibrated,
    Moved,
    MoveHead,
}

impl Phrase {
    fn voice(self) -> Voice {
        match self {
            Phrase::SitUpright => Voice::SitUpright,
            Phrase::Calibrated => Voice::Calibrated,
            Phrase::Moved => Voice::Moved,
            Phrase::MoveHead => Voice::MoveHead,
        }
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
        for &p in Voice::ALL {
            let pcm = p.pcm().unwrap();
            assert!(
                pcm.samples.len() > pcm.rate as usize / 2,
                "{p:?} is under 0.5 s"
            );
        }
    }
}
