//! The session's sounds: prompts it waits for, and reminders still playing.

use crate::core::sound::{Cue, Phrase, Player, Sound};
use anyhow::Result;
use futures_util::StreamExt;
use futures_util::future::LocalBoxFuture;
use futures_util::stream::FuturesUnordered;

/// The session's sounds: calibration waits for a prompt to be heard, because it measures the head
/// only after it. Monitoring does not wait for a reminder: it keeps judging while one plays, so the
/// recovery tink is on time even if the user sits up during speech. A reminder keeps playing to its
/// end even if monitoring stops, and its outcome is collected whenever the session next waits.
pub(super) struct Sounds<'a, C: Player> {
    player: &'a C,
    /// Reminders still playing, each ending with its outcome.
    reminders: FuturesUnordered<LocalBoxFuture<'static, (Cue, Result<()>)>>,
}

impl<'a, C: Player> Sounds<'a, C> {
    pub(super) fn new(player: &'a C) -> Self {
        Sounds {
            player,
            reminders: FuturesUnordered::new(),
        }
    }

    /// Speak a calibration prompt and return once it has been heard.
    pub(super) async fn prompt(&self, p: Phrase) -> Result<()> {
        self.player.play(Sound::Prompt(p)).await
    }

    /// Start a reminder and return at once; [`Sounds::ended`] gives its outcome.
    pub(super) fn remind(&mut self, cue: Cue) {
        let playing = self.player.play(Sound::Reminder(cue));
        self.reminders
            .push(Box::pin(async move { (cue, playing.await) }));
    }

    /// The next reminder to end, with its outcome. `None` at once if none is playing.
    pub(super) async fn ended(&mut self) -> Option<(Cue, Result<()>)> {
        self.reminders.next().await
    }
}
