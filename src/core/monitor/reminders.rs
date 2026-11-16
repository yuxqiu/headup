//! When to remind the user during a slouch (design: docs/UX_RESEARCH.md).
//!
//! The posture says when a slouch starts and ends; [`Reminders`] turns that into cues. The
//! schedule is a function of how long the slouch has lasted: a tone at `TONE_AFTER`, a spoken
//! reminder at `SPEAK_AFTER` and every `RENUDGE` after it, a desktop notification at
//! `NOTIFY_AFTER`. Recovering plays the "tink" only if a reminder started during the slouch. A
//! reminder that then failed to play still counts: it was due, and the failure is reported.
//!
//! Times are session time, like the monitor's. A time earlier than one already judged (a pose
//! delivered late) adds nothing: the schedule has been judged up to the later time.

use crate::core::sound::Cue;
use std::time::Duration;

// When each reminder falls due, as time since the slouch started: the design approved in
// docs/UX_RESEARCH.md (DECISIONS "Detector behaviour"), a tone at 10 s, speech at 30 s and every
// 120 s after, a notification at 60 s.
const TONE_AFTER: Duration = Duration::from_secs(10);
const SPEAK_AFTER: Duration = Duration::from_secs(30);
/// The spoken reminder repeats this often while the slouch goes on.
const RENUDGE: Duration = Duration::from_secs(120);
const NOTIFY_AFTER: Duration = Duration::from_secs(60);

/// The first spoken reminder after slouch time `after` (strictly later).
fn next_speak(after: Duration) -> Duration {
    let Some(past) = after.checked_sub(SPEAK_AFTER) else {
        return SPEAK_AFTER;
    };
    let repeats = past.as_nanos() / RENUDGE.as_nanos() + 1;
    let repeats = u32::try_from(repeats).expect("a slouch shorter than 2^32 re-nudges");
    SPEAK_AFTER + RENUDGE * repeats
}

/// The reminders due after slouch time `after`, up to and including `until`, each once, in play
/// order.
fn due_between(after: Duration, until: Duration) -> Vec<Cue> {
    let falls = |at: Duration| after < at && at <= until;
    [
        (Cue::Tone, falls(TONE_AFTER)),
        (Cue::Speak, next_speak(after) <= until),
        (Cue::Notify, falls(NOTIFY_AFTER)),
    ]
    .into_iter()
    .filter_map(|(cue, due)| due.then_some(cue))
    .collect()
}

/// The first reminder after slouch time `after`.
fn next_after(after: Duration) -> Duration {
    [TONE_AFTER, NOTIFY_AFTER]
        .into_iter()
        .filter(|&at| at > after)
        .chain([next_speak(after)])
        .min()
        .expect("the spoken reminder always comes again")
}

/// One slouch.
struct Slouch {
    since: Duration,
    /// The schedule has been judged up to this slouch time.
    judged: Duration,
    /// A reminder started, so recovering plays the "tink".
    cued: bool,
}

/// The reminders of the current slouch, if any.
#[derive(Default)]
pub struct Reminders {
    slouch: Option<Slouch>,
}

impl Reminders {
    /// A slouch started at `at`.
    pub fn slouch_started(&mut self, at: Duration) {
        self.slouch = Some(Slouch {
            since: at,
            judged: Duration::ZERO,
            cued: false,
        });
    }

    /// The slouch ended (the user sat up). Returns the "tink" if a reminder started during it.
    pub fn recovered(&mut self) -> Option<Cue> {
        self.slouch
            .take()
            .is_some_and(|s| s.cued)
            .then_some(Cue::Recovered)
    }

    /// The reminders that fall due by `now`, each once.
    pub fn due(&mut self, now: Duration) -> Vec<Cue> {
        let Some(slouch) = &mut self.slouch else {
            return vec![];
        };
        let elapsed = now.saturating_sub(slouch.since);
        if elapsed <= slouch.judged {
            return vec![];
        }
        let cues = due_between(slouch.judged, elapsed);
        slouch.judged = elapsed;
        slouch.cued |= !cues.is_empty();
        cues
    }

    /// When the next reminder falls due if the slouch goes on. `None` while not slouching.
    pub fn next_due(&self) -> Option<Duration> {
        let slouch = self.slouch.as_ref()?;
        Some(slouch.since + next_after(slouch.judged))
    }
}

#[cfg(test)]
#[path = "tests/reminders.rs"]
mod tests;
