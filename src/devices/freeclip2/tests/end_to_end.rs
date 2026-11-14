//! Recorded FreeClip 2 sessions replayed through the protocol into the real posture session.

use super::replay::{ReplayTracker, phase_at, phases, read_csv, replay, secs};
use crate::core::cues::Phrase;
use crate::core::detector::Cue::{self, Recovered, Speak, Tone};
use crate::testing::Played::Prompt;
use crate::testing::run_session;
use std::time::Duration;

// Ground truth (guided.py --slouch): five cycles of upright, then look down (held ~25 s, the stream
// going quiet after a few seconds), then back to the same screen point. The earbuds' re-centres add
// ~10° towards "looking up" per cycle. Without the detector's re-anchoring, upright drifts to about
// -40° and the later look-downs never count as slouches. Through the real session, every look-down
// cues a tone, and every return to the screen after one plays the recovery cue.
#[tokio::test(start_paused = true)]
async fn repeated_slouches_stay_detectable_end_to_end() {
    let rows = read_csv("slouch_guided.csv");
    let ph = phases(&rows);
    let cues = session_cues("slouch_guided.csv").await;
    let in_phase = |cue: Cue| -> Vec<&str> {
        cues.iter()
            .filter(|&&(c, _)| c == cue)
            .map(|&(_, t)| ph[phase_at(&ph, t)].0.as_str())
            .collect()
    };
    let downs: Vec<&str> = ph
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("down"))
        .collect();
    assert_eq!(in_phase(Tone), downs, "cues: {cues:?}");
    assert!(
        in_phase(Recovered).iter().all(|&name| name == "upright"),
        "cues: {cues:?}"
    );
    assert_eq!(in_phase(Recovered).len(), downs.len(), "cues: {cues:?}");
}

/// The cues (not prompts) the real session plays on a recording, each with when it played
/// (since the recording began).
async fn session_cues(name: &str) -> Vec<(Cue, Duration)> {
    let mut tracker = ReplayTracker::new(&read_csv(name));
    run_session(&mut tracker, Duration::ZERO).await.reminders()
}

/// The cues, without the spoken reminder (its timing depends on exactly how long a hold lasted).
fn kinds(cues: &[(Cue, Duration)]) -> Vec<Cue> {
    cues.iter()
        .map(|&(cue, _)| cue)
        .filter(|&cue| cue != Speak)
        .collect()
}

// The real long-hold recording. Each 30 s look-down hold must cue, and sitting back up must
// register as recovery.
#[tokio::test(start_paused = true)]
async fn long_holds_trigger_cues_end_to_end() {
    let rows = read_csv("hold_test.csv");
    let ph = phases(&rows);
    let cues = session_cues("hold_test.csv").await;
    let key: Vec<(Cue, &str)> = cues
        .iter()
        .filter(|&&(cue, _)| cue != Speak)
        .map(|&(cue, t)| (cue, ph[phase_at(&ph, t)].0.as_str()))
        .collect();
    let want = [
        (Tone, "down"),
        (Recovered, "center"),
        (Tone, "down"),
        (Recovered, "center"),
    ];
    assert_eq!(key, want, "cues: {cues:?}");
}

// The stream-restart recording has one slouch, so exactly one tone and one recovery. The recovery
// comes only once the user actually sat up (~25.6 s), not at the restart.
#[tokio::test(start_paused = true)]
async fn stream_restart_end_to_end() {
    let cues = session_cues("stream_restart.csv").await;
    assert_eq!(kinds(&cues), [Tone, Recovered], "cues: {cues:?}");
    let recovered_at = cues.iter().find(|&&(cue, _)| cue == Recovered).unwrap().1;
    assert!(
        recovered_at >= secs(25.6),
        "recovered at {recovered_at:?}, before sitting up"
    );
}

// data/live_check_raw.jsonl (2026-11-12) is the one recording at 35 Hz instead of 52.5 Hz. It has
// a quick glance down, a held slouch, and sitting back up. At this rate each 68-sample glide takes
// ~1.95 s. All four glides are dropped whole, the glance is ignored, and the slouch cues once and
// recovers.
#[tokio::test(start_paused = true)]
async fn live_35hz_session_end_to_end() {
    let rows = read_csv("live_35hz.csv");
    // Exactly each glide's 67 samples.
    assert_eq!(rows.len() - replay(&rows).len(), 4 * 67, "glides dropped");
    let cues = session_cues("live_35hz.csv").await;
    assert_eq!(kinds(&cues), [Tone, Recovered], "cues: {cues:?}");
}

// data/live8_raw.jsonl (2026-11-13): headup reading its socket in its own task, two held
// slouches, then several short glances. The first slouch cues a tone; the second is held past
// 30 s, mostly with the head still and the earbuds silent, so the spoken reminder comes 20 s
// after its tone. Each sit-up plays the recovery cue, and no glance cues.
#[tokio::test(start_paused = true)]
async fn live_session_with_two_slouches_end_to_end() {
    let cues = session_cues("live8.csv").await;
    let played: Vec<Cue> = cues.iter().map(|&(cue, _)| cue).collect();
    assert_eq!(played, [Tone, Recovered, Tone, Speak, Recovered]);
    let reminder = cues[3].1 - cues[2].1;
    assert!(
        (secs(20.0)..secs(20.1)).contains(&reminder),
        "spoken reminder {reminder:?} after the tone"
    );
}

// Three recorded walks (2026-11-13; FINDINGS "Instant resets while walking"). While the user
// walks, the earbuds hold their reading and lose the head's movement, so the reference from before
// the walk no longer applies. No reminder may come from it: each hold makes tracking unavailable,
// and headup calibrates again once the user has stopped and held still, so no attempt fails ("you
// moved") mid-walk.
#[tokio::test(start_paused = true)]
async fn walking_around_gives_no_reminder_end_to_end() {
    for name in ["walk_reconnect.csv", "turn_walk.csv", "walk_near_far.csv"] {
        let mut tracker = ReplayTracker::new(&read_csv(name));
        let run = run_session(&mut tracker, Duration::ZERO).await;
        let reminders = run.reminders();
        assert!(reminders.is_empty(), "{name}: {reminders:?}");
        let moved = run.count(Prompt(Phrase::Moved));
        assert_eq!(moved, 0, "{name}: {:?}", run.played());
        assert!(
            run.events.contains(&"TrackingUnavailable".to_string()),
            "{name}: {:?}",
            run.events
        );
    }
}

// data/walk3_raw.jsonl (2026-11-13): a live walk (24 s to about 102 s, turning ~140° on the way),
// then looking down at the desk. In the raw readings the look-downs (from about 126 s and 158 s)
// are +46° like every other look-down. After the walk "sit upright" comes only once the user is
// back; the look-downs read as looking down, so each one cues a tone and sitting up plays the
// recovery cue.
#[tokio::test(start_paused = true)]
async fn looking_down_after_a_walk_still_cues_end_to_end() {
    let mut tracker = ReplayTracker::new(&read_csv("walk_slouch.csv"));
    let run = run_session(&mut tracker, Duration::ZERO).await;
    let prompts = run.times(Prompt(Phrase::SitUpright));
    assert!(
        prompts.iter().all(|&t| t < secs(24.0) || t > secs(100.0)),
        "sit upright mid-walk: {prompts:?}"
    );
    let cues = run.reminders();
    assert_eq!(
        kinds(&cues),
        [Tone, Recovered, Tone, Recovered],
        "cues: {cues:?}"
    );
}

// After a hold the earbuds stay silent until the head moves (about 4°, FINDINGS "Instant resets
// while walking"), whether the user is still walking or has sat down and keeps still: the stream
// cannot tell the two apart, and neither can headup. So silence after a hold never brings a
// prompt, however long it lasts, and the first moving reading does. Here walk_slouch.csv's last
// hold (50.96 s) is followed by its recorded silence plus an extra minute before the first moving
// reading. The recording does not show when the user sat down: the silence covers both.
#[tokio::test(start_paused = true)]
async fn after_a_hold_only_a_head_movement_brings_a_prompt_end_to_end() {
    let delay = secs(60.0);
    let mut rows = read_csv("walk_slouch.csv");
    let resume = rows.iter().position(|r| r.seq == 5521).unwrap(); // first moving reading
    let moved_at = rows[resume].t + delay;
    for r in &mut rows[resume..] {
        r.t += delay;
    }
    let mut tracker = ReplayTracker::new(&rows);
    let run = run_session(&mut tracker, Duration::ZERO).await;
    let mut prompts = run.times(Prompt(Phrase::SitUpright));
    prompts.extend(run.times(Prompt(Phrase::MoveHead)));
    assert!(
        !prompts.iter().any(|&t| secs(51.0) < t && t < moved_at),
        "prompted before the head moved at {moved_at:?}: {prompts:?}"
    );
    assert!(
        prompts
            .iter()
            .any(|&t| moved_at < t && t < moved_at + secs(5.0)),
        "no prompt soon after the head moved at {moved_at:?}: {prompts:?}"
    );
}
