//! [`DesktopPlayer`]: plays sounds on this computer and shows desktop notifications.

use super::audio;
use super::wav::Pcm;
use super::{Cue, Player, Sound, Voice, VoiceClips};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Plays sounds on this computer's default audio output and shows desktop notifications.
pub struct DesktopPlayer {
    tone: Arc<Pcm>,
    tink: Arc<Pcm>,
    /// Every voice clip, decoded once at startup.
    voices: VoiceClips,
}

impl DesktopPlayer {
    pub fn new() -> Result<Self> {
        let voices = VoiceClips::decode()?;
        Ok(DesktopPlayer {
            // Notes chosen by ear, not measured: the reminder falls from C5 to A4 (soft), the
            // tink rises from A5 to E6 (bright). The user heard both on the earbuds in the
            // 2026-11-13 live check (data/walk3*.jsonl).
            tone: Arc::new(Pcm::tones(&[(523.0, 0.18), (440.0, 0.25)])),
            tink: Arc::new(Pcm::tones(&[(880.0, 0.08), (1318.5, 0.14)])),
            voices,
        })
    }
}

impl Player for DesktopPlayer {
    fn play(&self, sound: Sound) -> impl Future<Output = Result<()>> + 'static {
        let pcm = match sound {
            Sound::Prompt(p) => self.voices.get(p.voice()),
            Sound::Reminder(Cue::Tone) => self.tone.clone(),
            Sound::Reminder(Cue::Recovered) => self.tink.clone(),
            Sound::Reminder(Cue::Speak) => self.voices.get(Voice::LiftHead),
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
        // The text names no duration, because the reminder schedule decides when this fires.
        .body("You have been looking down for a while.")
        .show_async()
        .await;
    shown.context("desktop notification")?;
    Ok(())
}
