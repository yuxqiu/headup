//! The head-up on pitches and times.

use super::*;
use crate::testing::secs;

/// The travel a slouch needs, as the monitor sets it.
const TRAVEL_DEG: f64 = 20.0;

/// Feed `pitch` at 50 Hz from `t0` for `span` seconds, checking after every sample as the monitor
/// does, and collect the pitches head-ups ended at. Returns the time after the last sample.
fn hold(h: &mut HeadUp, t0: f64, span: f64, pitch: f64, ended: &mut Vec<f64>) -> f64 {
    let mut t = t0;
    while t < t0 + span {
        h.pose(secs(t), pitch);
        ended.extend(h.check(secs(t)).map(|(_, held)| held));
        t += 0.02;
    }
    t
}

/// The head at rest at 0° for 3 s from time 0, so a head-up has a level to start from. Returns the
/// time after the last sample.
fn at_rest(h: &mut HeadUp) -> f64 {
    hold(h, 0.0, 3.0, 0.0, &mut vec![])
}

// Down to 40°, up past where it was to -10°, held: a head-up ending at -10°.
#[test]
fn down_then_up_then_still_is_a_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    hold(&mut h, t, 3.0, -10.0, &mut ended);
    assert_eq!(ended, [-10.0]);
}

// Coming up short of where the head was still counts, once it rises the travel: 40° to 10°.
#[test]
fn a_head_up_can_end_below_where_it_started() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    hold(&mut h, t, 3.0, 10.0, &mut ended);
    assert_eq!(ended, [10.0]);
}

// The movement alone counts, wherever it happens: from 25°, a glance down to 50° and back.
#[test]
fn a_head_up_from_any_level_counts() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = hold(&mut h, 0.0, 3.0, 25.0, &mut ended);
    let t = hold(&mut h, t, 1.0, 50.0, &mut ended);
    hold(&mut h, t, 3.0, 25.0, &mut ended);
    assert_eq!(ended, [25.0]);
}

// Holding still while down, even with the tracker silent, does not cancel the head-up.
#[test]
fn holding_still_while_down_keeps_the_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 1.0, 40.0, &mut ended);
    for i in 0..40 {
        ended.extend(h.check(secs(t + f64::from(i) * 0.5)).map(|(_, held)| held));
    }
    hold(&mut h, t + 20.0, 3.0, 0.0, &mut ended);
    assert_eq!(ended, [0.0]);
}

// Holding so still after coming up that the tracker goes quiet still ends the head-up: silence
// means the pitch is unchanged, so the head has settled once a second passes.
#[test]
fn a_head_up_settles_in_silence() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let up = hold(&mut h, t, 15.0, 40.0, &mut ended);
    let t = hold(&mut h, up, 0.3, -10.0, &mut ended);
    assert!(ended.is_empty(), "ended before settling: {ended:?}");
    // It settled a second after the first raised sample.
    assert_eq!(h.check(secs(t + 1.0)), Some((secs(up) + SETTLE, -10.0)));
}

// The same when the tracker goes quiet after the first raised sample: that sample alone says
// where the head holds.
#[test]
fn one_raised_sample_then_silence_ends_the_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    h.pose(secs(t), -10.0);
    assert_eq!(h.check(secs(t)), None);
    assert_eq!(h.check(secs(t + 1.0)), Some((secs(t) + SETTLE, -10.0)));
}

// Looking up from rest is no head-up: no descent comes first.
#[test]
fn looking_up_from_rest_is_no_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    hold(&mut h, t, 30.0, -30.0, &mut ended);
    assert!(ended.is_empty(), "{ended:?}");
}

// A descent short of the travel (15°) is no head-up, even when the head then rises further.
#[test]
fn a_short_descent_is_no_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 5.0, 15.0, &mut ended);
    hold(&mut h, t, 5.0, -10.0, &mut ended);
    assert!(ended.is_empty(), "{ended:?}");
}

// A rise short of the travel is no head-up: from 40° to 25°.
#[test]
fn a_short_rise_is_no_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    hold(&mut h, t, 5.0, 25.0, &mut ended);
    assert!(ended.is_empty(), "{ended:?}");
}

// Rising and going down again before the head holds still: what counts is the rise from the
// deepest point to where the head holds (40° to 30°), which is too small.
#[test]
fn a_rise_undone_before_holding_still_is_no_head_up() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    let t = hold(&mut h, t, 0.3, 5.0, &mut ended);
    hold(&mut h, t, 5.0, 30.0, &mut ended);
    assert!(ended.is_empty(), "{ended:?}");
}

// One head-up counts once; a later look up from where it ended needs a new descent first.
#[test]
fn one_head_up_counts_once() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    let t = hold(&mut h, t, 3.0, -5.0, &mut ended);
    hold(&mut h, t, 10.0, -30.0, &mut ended);
    assert_eq!(ended, [-5.0]);
}

// Head-ups in a row each count: one ending high (-10°), then one ending lower (5°).
#[test]
fn head_ups_in_a_row_each_count() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let mut t = at_rest(&mut h);
    t = hold(&mut h, t, 15.0, 40.0, &mut ended);
    t = hold(&mut h, t, 3.0, -10.0, &mut ended);
    t = hold(&mut h, t, 5.0, 30.0, &mut ended);
    hold(&mut h, t, 3.0, 5.0, &mut ended);
    assert_eq!(ended, [-10.0, 5.0]);
}

const SAMPLE: Duration = Duration::from_millis(20);

/// A head moving between 0° and 30° on every sample.
fn wobble(t: Duration) -> f64 {
    if (t.as_millis() / 20).is_multiple_of(2) {
        0.0
    } else {
        30.0
    }
}

/// Feed `pitch` to `settle` at 50 Hz from `t0` for `span` seconds, and return the time after the
/// last sample with the pitch the head held at it, if it had settled by then.
fn settle_on(
    settle: &mut Settle,
    t0: Duration,
    span: Duration,
    pitch: impl Fn(Duration) -> f64,
) -> (Duration, Option<f64>) {
    let mut t = t0;
    let mut held = None;
    while t < t0 + span {
        settle.pitch(t, pitch(t));
        held = held_at(settle, t);
        t += SAMPLE;
    }
    (t, held)
}

/// The pitch the head holds at `t`, if it has settled by then.
fn held_at(settle: &Settle, t: Duration) -> Option<f64> {
    let (at, held) = settle.settles()?;
    (at <= t).then_some(held)
}

// A moving head has not settled; once its pitch stays within 2° for 1 s, it holds the last pitch.
#[test]
fn head_settles_after_a_second_within_two_degrees() {
    let mut settle = Settle::default();
    let (t, held) = settle_on(&mut settle, Duration::ZERO, secs(3.0), wobble);
    assert_eq!(held, None);
    let (t, held) = settle_on(&mut settle, t, secs(0.9), |_| 10.0);
    assert_eq!(held, None, "settled before a second at {t:?}");
    let (_, held) = settle_on(&mut settle, t, secs(0.2), |_| 11.0);
    assert_eq!(held, Some(11.0));
}

// Silence counts as unchanged: one sample, and the head settles a second later.
#[test]
fn silence_after_a_sample_settles() {
    let mut settle = Settle::default();
    settle.pitch(secs(5.0), 12.0);
    assert_eq!(settle.settles(), Some((secs(6.0), 12.0)));
}

// A quick glance up, with no hold, is no starting point: coming back from it is no descent, so a
// later held look-up is no head-up.
#[test]
fn a_quick_glance_up_starts_nothing() {
    let mut h = HeadUp::new(TRAVEL_DEG, 0.0);
    let mut ended = vec![];
    let t = at_rest(&mut h);
    let t = hold(&mut h, t, 0.3, -25.0, &mut ended);
    let t = hold(&mut h, t, 5.0, 0.0, &mut ended);
    hold(&mut h, t, 3.0, -25.0, &mut ended);
    assert!(ended.is_empty(), "{ended:?}");
}
