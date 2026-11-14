//! Slouch detection on pitch samples and time.

use super::*;

/// `x` seconds.
fn s(x: f64) -> Duration {
    Duration::from_secs_f64(x)
}

fn reanchoring() -> Config {
    Config {
        reanchor_after_recovery: true,
        ..Config::default()
    }
}

/// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting the uprights re-anchored to.
fn reanchors(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<f64>) -> f64 {
    let mut t = t0;
    while t < t0 + secs {
        out.extend(d.update(s(t), pitch).reanchored);
        t += 0.02;
    }
    t
}

/// Feed `pitch` at 50 Hz from `t0` for `secs`, collecting (time, cue).
fn run(d: &mut Detector, t0: f64, secs: f64, pitch: f64, out: &mut Vec<(f64, Cue)>) -> f64 {
    let mut t = t0;
    while t < t0 + secs {
        out.extend(d.update(s(t), pitch).cues.into_iter().map(|c| (t, c)));
        t += 0.02;
    }
    t
}

#[test]
fn upright_and_brief_glances_never_cue() {
    let mut d = Detector::new(Config::default());
    let mut cues = vec![];
    let mut t = run(&mut d, 0.0, 30.0, 5.0, &mut cues);
    t = run(&mut d, t, 6.0, 40.0, &mut cues); // look at phone for 6 s < 10 s dwell
    run(&mut d, t, 30.0, 5.0, &mut cues);
    assert!(cues.is_empty(), "{cues:?}");
}

#[test]
fn sustained_slouch_escalates_then_recovers() {
    let mut d = Detector::new(Config::default());
    let mut cues = vec![];
    let t = run(&mut d, 0.0, 200.0, 30.0, &mut cues);
    run(&mut d, t, 5.0, 0.0, &mut cues);
    let kinds: Vec<Cue> = cues.iter().map(|c| c.1).collect();
    assert_eq!(
        kinds,
        [
            Cue::Tone,
            Cue::Speak,
            Cue::Notify,
            Cue::Speak,
            Cue::Recovered
        ]
    );
    assert!((cues[0].0 - 10.0).abs() < 0.5, "tone at {}", cues[0].0);
    assert!(
        (cues[3].0 - cues[1].0 - 120.0).abs() < 0.5,
        "re-nudge after {}",
        cues[3].0 - cues[1].0
    );
}

#[test]
fn silence_keeps_timers_running() {
    let mut d = Detector::new(Config::default());
    let mut cues = vec![];
    let t = run(&mut d, 0.0, 3.0, 30.0, &mut cues); // device then goes quiet: head held still
    let mut t2 = t;
    while t2 < 35.0 {
        cues.extend(d.tick(s(t2)).cues.into_iter().map(|c| (t2, c)));
        t2 += 1.0;
    }
    let kinds: Vec<Cue> = cues.iter().map(|c| c.1).collect();
    assert_eq!(kinds, [Cue::Tone, Cue::Speak]);
}

#[test]
fn next_due_tracks_the_next_reminder() {
    let mut d = Detector::new(Config::default());
    assert_eq!(d.next_due(), None);
    let t = run(&mut d, 0.0, 1.0, 30.0, &mut vec![]);
    let tone = d.next_due().unwrap();
    assert!(
        (tone.as_secs_f64() - 10.0).abs() < 0.1,
        "tone due at {tone:?}"
    );
    assert_eq!(d.tick(tone - Duration::from_nanos(1)).cues, []);
    assert_eq!(d.tick(tone).cues, [Cue::Tone]);
    assert_eq!(d.next_due(), Some(tone + s(20.0)), "speak next");
    run(&mut d, t + 10.0, 1.0, 0.0, &mut vec![]);
    assert_eq!(d.next_due(), None);
}

#[test]
fn hysteresis_keeps_episode_between_thresholds() {
    let mut d = Detector::new(Config::default());
    let mut cues = vec![];
    let t = run(&mut d, 0.0, 12.0, 25.0, &mut cues); // tone at ~10 s
    run(&mut d, t, 30.0, 17.0, &mut cues); // between off(15) and on(20): still slouching
    assert!(d.slouching());
    assert!(cues.iter().all(|c| c.1 != Cue::Recovered), "{cues:?}");
}

// Sitting back up "past" upright (tracker error) re-anchors upright, so the next slouch counts
// in full: from the new upright at -10°, a look down to +12° is 22° and must cue.
#[test]
fn recovery_past_upright_reanchors() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    t = reanchors(&mut d, t, 3.0, -10.0, &mut re);
    assert!(
        re.len() == 1 && (re[0] + 10.0).abs() < 0.5,
        "re-anchored {re:?}"
    );
    let mut cues = vec![];
    run(&mut d, t, 15.0, 12.0, &mut cues);
    assert!(cues.iter().any(|c| c.1 == Cue::Tone), "{cues:?}");
}

// Sitting back up past upright, then holding so still that the tracker goes quiet, still
// re-anchors: silence means the pitch is unchanged, so the head has settled once a second passes.
#[test]
fn silent_head_after_a_recovery_reanchors() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    // A few samples back up past upright (the smoothing crosses below off_deg), then silence.
    let t = reanchors(&mut d, t, 0.3, -10.0, &mut re);
    assert!(re.is_empty(), "re-anchored before settling: {re:?}");
    let reanchored = d.tick(s(t + 1.0)).reanchored;
    assert!(
        reanchored.is_some_and(|u| (u + 10.0).abs() < 0.5),
        "re-anchored {reanchored:?}"
    );
}

#[test]
fn looking_up_without_a_slouch_keeps_upright() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    reanchors(&mut d, 0.0, 30.0, -30.0, &mut re);
    assert!(re.is_empty(), "re-anchored {re:?}");
}

#[test]
fn partial_recovery_keeps_upright() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    reanchors(&mut d, t, 5.0, 10.0, &mut re); // below off (15), still above upright
    assert!(re.is_empty(), "re-anchored {re:?}");
}

// Only the first settle after a recovery counts; a later look up must not move upright again.
#[test]
fn reanchors_once_per_recovery() {
    let mut d = Detector::new(reanchoring());
    let mut re = vec![];
    let mut t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    t = reanchors(&mut d, t, 3.0, -5.0, &mut re);
    reanchors(&mut d, t, 10.0, -30.0, &mut re);
    assert!(
        re.len() == 1 && (re[0] + 5.0).abs() < 0.5,
        "re-anchored {re:?}"
    );
}

#[test]
fn no_reanchoring_unless_enabled() {
    let mut d = Detector::new(Config::default());
    let mut re = vec![];
    let t = reanchors(&mut d, 0.0, 15.0, 40.0, &mut re);
    reanchors(&mut d, t, 3.0, -10.0, &mut re);
    assert!(re.is_empty(), "re-anchored {re:?}");
}

// The same head movement smooths the same at any sample rate: 0.24 s after a step from 0° to
// 30°, a 25 Hz and a 100 Hz tracker agree.
#[test]
fn smoothing_is_independent_of_sample_rate() {
    let after_step = |hz: f64| {
        let mut d = Detector::new(Config::default());
        d.update(Duration::ZERO, 0.0);
        let steps = (0.24 * hz).round() as u32;
        for i in 1..=steps {
            d.update(s(f64::from(i) / hz), 30.0);
        }
        d.smoothed().unwrap()
    };
    let (slow, fast) = (after_step(25.0), after_step(100.0));
    assert!(
        (slow - fast).abs() < 0.01,
        "25 Hz {slow:.2} vs 100 Hz {fast:.2}"
    );
}

#[test]
fn looking_up_is_not_slouching() {
    let mut d = Detector::new(Config::default());
    let mut cues = vec![];
    run(&mut d, 0.0, 60.0, -40.0, &mut cues);
    assert!(cues.is_empty());
}
