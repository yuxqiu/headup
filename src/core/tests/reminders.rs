//! The reminder schedule, on session times alone.

use super::*;
use crate::testing::secs;
use Cue::{Notify, Recovered, Speak, Tone};

/// Judge `r` once a second from `from` to `to` (seconds), as silence does: collect (time, cue).
fn judge_every_second(r: &mut Reminders, from: u64, to: u64) -> Vec<(u64, Cue)> {
    (from..=to)
        .flat_map(|t| {
            r.due(Duration::from_secs(t))
                .into_iter()
                .map(move |c| (t, c))
        })
        .collect()
}

// A slouch held for 200 s: tone at 10 s, speech at 30 s and every 120 s after, a notification at
// 60 s. Sitting up then plays the tink.
#[test]
fn a_long_slouch_escalates_then_recovers() {
    let mut r = Reminders::new(Schedule::default());
    r.slouch_started(secs(5.0));
    let cues = judge_every_second(&mut r, 5, 205);
    assert_eq!(cues, [(15, Tone), (35, Speak), (65, Notify), (155, Speak)]);
    assert_eq!(r.recovered(), Some(Recovered));
    assert_eq!(r.next_due(), None);
}

// A glance shorter than the tone's 10 s plays nothing, not even the tink.
#[test]
fn a_short_slouch_plays_nothing() {
    let mut r = Reminders::new(Schedule::default());
    r.slouch_started(secs(0.0));
    assert_eq!(judge_every_second(&mut r, 0, 9), []);
    assert_eq!(r.recovered(), None);
}

// The next reminder is due exactly on time: one nanosecond earlier nothing falls due.
#[test]
fn next_due_is_exact() {
    let mut r = Reminders::new(Schedule::default());
    assert_eq!(r.next_due(), None);
    r.slouch_started(secs(2.5));
    let tone = r.next_due().unwrap();
    assert_eq!(tone, secs(12.5));
    assert_eq!(r.due(tone - Duration::from_nanos(1)), []);
    assert_eq!(r.due(tone), [Tone]);
    assert_eq!(r.next_due(), Some(secs(32.5)), "speech next");
    assert_eq!(r.due(secs(32.5)), [Speak]);
    assert_eq!(r.next_due(), Some(secs(62.5)), "notification next");
    assert_eq!(r.due(secs(62.5)), [Notify]);
    assert_eq!(r.next_due(), Some(secs(152.5)), "speech again");
}

// A time earlier than one already judged (a pose delivered late) plays nothing twice.
#[test]
fn late_times_play_nothing_again() {
    let mut r = Reminders::new(Schedule::default());
    r.slouch_started(secs(0.0));
    assert_eq!(r.due(secs(10.0)), [Tone]);
    assert_eq!(r.due(secs(9.9)), []);
    assert_eq!(r.due(secs(10.0)), []);
}

// A tone that started earns the tink even if the slouch then ends on a pose measured before it.
#[test]
fn recovery_after_a_started_reminder_tinks() {
    let mut r = Reminders::new(Schedule::default());
    r.slouch_started(secs(0.0));
    assert_eq!(r.due(secs(10.0)), [Tone]);
    assert_eq!(r.recovered(), Some(Recovered));
}
