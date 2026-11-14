//! Rebuilder tests: synthetic glides built with the firmware's formula, and the real recordings.

use super::super::rebuild::{Rebuilder, Rebuilt};
use crate::core::quat::{Euler, Quat};
use crate::devices::freeclip2::tests::replay::{Row, phase_at, phases, read_csv, replay, secs};
use std::time::Duration;
use tokio::time::Instant;

/// Replays a guided recording and returns the median (pitch, yaw, roll) of the last 60 samples
/// of each phase, in order.
fn phase_medians(name: &str) -> Vec<(String, f64, f64, f64)> {
    let rows = read_csv(name);
    let ph = phases(&rows);
    let mut per: Vec<Vec<Quat>> = vec![Vec::new(); ph.len()];
    for o in replay(&rows) {
        per[phase_at(&ph, o.t)].push(o.abs);
    }
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    ph.into_iter()
        .zip(per)
        .filter(|(_, qs)| !qs.is_empty())
        .map(|((name, _), qs)| {
            let tail = &qs[qs.len().saturating_sub(60)..];
            let e: Vec<_> = tail.iter().map(|q| q.euler()).collect();
            let m = |k: fn(&Euler) -> f64| median(e.iter().map(k).collect());
            (name, m(|e| e.pitch), m(|e| e.yaw), m(|e| e.roll))
        })
        .collect()
}

fn check(path: &str, center_tol: f64) {
    let ph = phase_medians(path);
    for (name, pitch, yaw, roll) in &ph {
        match name.as_str() {
            "upright" | "center" => assert!(
                pitch.abs() < center_tol && yaw.abs() < center_tol && roll.abs() < center_tol,
                "{path}: center off: {pitch:.1}/{yaw:.1}/{roll:.1}"
            ),
            "down" => assert!(*pitch > 45.0, "{path}: down pitch {pitch:.1}"),
            "up" => assert!(*pitch < -25.0, "{path}: up pitch {pitch:.1}"),
            "turn_left" => assert!(*yaw > 50.0, "{path}: left yaw {yaw:.1}"),
            "turn_right" => assert!(*yaw < -50.0, "{path}: right yaw {yaw:.1}"),
            "tilt_left" => assert!(*roll < -15.0, "{path}: tilt left roll {roll:.1}"),
            "tilt_right" => assert!(*roll > 15.0, "{path}: tilt right roll {roll:.1}"),
            _ => {}
        }
    }
}

// The two guided recordings. Run 1 (follow mode) accumulates more error.
#[test]
fn rebuilds_guided_run_fixed_mode() {
    check("headpose_axes_fixed.csv", 9.0);
}

#[test]
fn rebuilds_guided_run_follow_mode() {
    check("headpose_axes.csv", 15.0);
}

/// A rotation of `deg` about the unit axis `(x, y, z)`.
fn rotation(deg: f64, (x, y, z): (f64, f64, f64)) -> Quat {
    let (s, c) = (deg.to_radians() / 2.0).sin_cos();
    Quat {
        w: c,
        x: s * x,
        y: s * y,
        z: s * z,
    }
}

fn pitch_quat(deg: f64) -> Quat {
    rotation(deg, (0.0, 1.0, 0.0))
}

/// Raw sample `raw` with sequence number `seq`, received at `t` seconds.
fn row(t: f64, seq: u32, raw: Quat) -> Row {
    Row {
        phase: String::new(),
        t: secs(t),
        seq,
        raw,
    }
}

/// A sample of `pitch` with sequence number `seq`, received at `t`. A pitch of 0 is exact identity.
fn sample(t: f64, seq: u32, pitch: f64) -> Row {
    row(t, seq, pitch_quat(pitch))
}

/// Row `seq` of a stream at 52.5 Hz (the earbuds' usual rate), reading `raw`.
fn row_52hz(seq: u32, raw: Quat) -> Row {
    row(f64::from(seq) / 52.5, seq, raw)
}

// A re-centre after a ~90° turn (e.g. to a side monitor), with glide samples from the
// firmware's formula. The head settles slowly for 220 samples (0.03° per sample, 6.6° in all).
// That stays inside the firmware's 10° stillness box, so the earbuds re-centre after 208
// samples. Only the last pose starts the glide that follows. The rebuilt yaw must stay where
// the head settled.
#[test]
fn hides_glide_from_a_large_angle() {
    let yaw = |deg: f64| rotation(deg, (0.0, 0.0, 1.0));
    let settle = |s: u32| 83.0 + 0.03 * f64::from(s);
    let mut rows: Vec<Row> = (1..=220).map(|s| row_52hz(s, yaw(settle(s)))).collect();
    let held = yaw(settle(220));
    rows.extend((1..=68).map(|k| row_52hz(220 + k, firmware_glide(held, k))));
    // After the re-centre the head stays where it settled, so the earbuds read identity.
    rows.extend((1..=80).map(|i| row_52hz(220 + 68 + i, Quat::IDENTITY)));
    let held_at = secs(220.0 / 52.5);
    for o in replay(&rows).into_iter().filter(|o| o.t >= held_at) {
        let got = o.abs.euler().yaw;
        assert!(
            (got - settle(220)).abs() < 0.5,
            "t={:.2}: yaw {got:.1}",
            o.t.as_secs_f64()
        );
    }
}

// Turn left 60° and hold until the earbuds re-centre there, then nod down 30°. The earbuds
// report the nod relative to the turned reference (`raw = q ⊗ conj(ref)`). Rebuilt, it must
// read 30° of pitch at 60° of yaw.
#[test]
fn nod_after_a_turn_reads_as_pitch() {
    let yaw = |deg: f64| rotation(deg, (0.0, 0.0, 1.0));
    let pitch = pitch_quat;
    let turned = yaw(60.0);
    let mut rows: Vec<Row> = (1..=60).map(|s| row_52hz(s, yaw(f64::from(s)))).collect();
    rows.extend((61..=280).map(|s| row_52hz(s, turned)));
    rows.extend((1..=68).map(|k| row_52hz(280 + k, firmware_glide(turned, k))));
    // The head is still at the new reference, so the earbuds send nothing for a while. The
    // sequence number keeps counting.
    let nod = |deg: f64| turned * pitch(deg) * turned.conj();
    rows.extend((1..=30).map(|i| row_52hz(400 + i, nod(f64::from(i)))));
    rows.extend((1..=100).map(|i| row_52hz(430 + i, nod(30.0))));
    let out = replay(&rows);
    let end = out.last().expect("poses").abs.euler();
    assert!(
        (end.pitch - 30.0).abs() < 0.5 && (end.yaw - 60.0).abs() < 0.5 && end.roll.abs() < 0.5,
        "nod after a turn read {end:?}"
    );
}

/// Sample `k` of a firmware glide from the held pose `q`: `normalize(t·q + (1−t)·I)` with `t =
/// (68 − k) / 69`, exactly identity at `k = 68`. Written from FINDINGS with literal numbers, so
/// it shares nothing with `rebuild`'s code. Assumes `q.w >= 0`.
fn firmware_glide(q: Quat, k: u32) -> Quat {
    if k == 68 {
        return Quat::IDENTITY;
    }
    let t = f64::from(68 - k) / 69.0;
    Quat {
        w: t * q.w + (1.0 - t),
        x: t * q.x,
        y: t * q.y,
        z: t * q.z,
    }
    .normalized()
}

// The sample rate varies (35 Hz in data/live_check_raw.jsonl, 52.5 Hz elsewhere), but a glide
// is always 68 samples: about 1.95 s at 35 Hz. Here a 3 s stall in transit (seen early in
// streams) delays the identity sample to ~4.9 s after the glide began. Every output must still
// read 30°, the held pose.
#[test]
fn hides_slow_glide_at_any_sample_rate() {
    let (mut rows, mut t, seq) = held_then_glide(67, 3.0);
    rows.push(row(t, seq, Quat::IDENTITY)); // identity: the glide ends
    t += 6.0; // head still: the earbuds go quiet
    for i in 1..=100 {
        rows.push(row(t, seq + i, Quat::IDENTITY)); // relative to the new reference: still 30°
        t += 1.0 / 35.0;
    }
    let out = replay(&rows);
    for o in &out {
        let pitch = o.abs.euler().pitch;
        assert!(
            (pitch - 30.0).abs() < 0.5,
            "t={:.2}: pitch {pitch:.1}",
            o.t.as_secs_f64()
        );
    }
    // Exactly the glide's 67 samples are dropped. Every still sample before it comes out.
    assert_eq!(rows.len() - out.len(), 67, "the glide is dropped");
}

/// Push `(seq, pitch)` samples at 50 Hz from now; returns, for each, how many samples it
/// released.
fn push_all(rb: &mut Rebuilder, samples: &[(u32, f64)]) -> Vec<usize> {
    let start = Instant::now();
    samples
        .iter()
        .enumerate()
        .map(|(i, &(seq, pitch))| {
            let t = start + Duration::from_millis(20 * i as u64);
            rb.push(t, seq, pitch_quat(pitch))
                .iter()
                .filter(|out| matches!(out, Rebuilt::Pose { .. }))
                .count()
        })
        .collect()
}

// A still pose is not a glide (a glide from 30° moves 0.45° per sample). So each sample comes
// out as it arrives, including the last one before the earbuds go quiet without a re-centre
// (as in stream_restart.csv).
#[test]
fn still_pose_comes_out_at_once() {
    let mut rb = Rebuilder::default();
    let samples: Vec<(u32, f64)> = (1..=100).map(|seq| (seq, 30.0)).collect();
    assert_eq!(push_all(&mut rb, &samples), vec![1; 100]);
}

// A still pose whose readings drift towards the reference by sensor noise (0.05° per sample,
// as in stream_restart.csv) is not a glide either: a glide from 50° moves 0.75° per sample.
// Every sample comes out as it arrives.
#[test]
fn noisy_still_pose_comes_out_at_once() {
    let mut rb = Rebuilder::default();
    let samples: Vec<(u32, f64)> = (1..=100)
        .map(|seq: u32| (seq, 50.0 - 0.05 * f64::from(seq.saturating_sub(95))))
        .collect();
    assert_eq!(push_all(&mut rb, &samples), vec![1; 100]);
}

// Sitting back up towards the reference at 2° per sample is not a glide (a glide from 30° moves
// 0.45° per sample). It comes out as it happens.
#[test]
fn sitting_up_comes_out_at_once() {
    let mut rb = Rebuilder::default();
    let mut samples: Vec<(u32, f64)> = (1..=80).map(|seq| (seq, 30.0)).collect();
    samples.extend((81..=92).map(|seq| (seq, 30.0 - 2.0 * f64::from(seq - 80))));
    assert_eq!(push_all(&mut rb, &samples), vec![1; 92]);
}

// As in data/session6_raw.jsonl at seq 71552: held at ~53°, then a glide the earbuds cut short
// after 43 samples. The held samples are real head data and must come out. Only the glide is
// dropped.
#[test]
fn cut_short_glide_keeps_the_pose_before_it() {
    let held = pitch_quat(53.0);
    let mut rows: Vec<Row> = (1..=100).map(|s| row_52hz(s, held)).collect();
    rows.extend((1..=43).map(|k| row_52hz(100 + k, firmware_glide(held, k))));
    rows.push(row_52hz(144, Quat::IDENTITY)); // cut short
    let out = replay(&rows);
    assert_eq!(rows.len() - out.len(), 43, "only the glide is dropped");
    assert!(out.iter().all(|o| (o.abs.euler().pitch - 53.0).abs() < 0.5));
}

/// The holds started while pushing `rows`, by the sequence numbers of their first samples.
fn holds(rows: &[Row]) -> Vec<u32> {
    let start = Instant::now();
    let mut rb = Rebuilder::default();
    rows.iter()
        .flat_map(|r| rb.push(start + r.t, r.seq, r.raw))
        .filter_map(|out| match out {
            Rebuilt::HoldStarted { seq } => Some(seq),
            Rebuilt::Pose { .. } => None,
        })
        .collect()
}

// A glide whose readings leave the formula partway (not something the firmware does: its glide
// does not depend on the head) is indistinguishable from head movement, so its identity sample is
// taken for the start of a hold, whatever the glide's size.
#[test]
fn glide_off_the_model_is_taken_for_a_hold() {
    for deg in [10.0, 20.0, 30.0, 40.0] {
        let held = pitch_quat(deg);
        let mut rows: Vec<Row> = (1..=100).map(|s| row_52hz(s, held)).collect();
        rows.extend((1..=67).map(|k| {
            let q = firmware_glide(held, k);
            row_52hz(100 + k, if k == 30 { q * pitch_quat(0.2) } else { q })
        }));
        rows.push(row_52hz(168, Quat::IDENTITY));
        assert_eq!(holds(&rows), [168], "glide from {deg}°");
    }
}

// The head turns, then the reading jumps to identity with no glide: the earbuds start a hold.
// Identity samples after it start nothing more.
#[test]
fn identity_with_no_glide_before_it_starts_a_hold() {
    let mut rows: Vec<Row> = (1..=50)
        .map(|s| row_52hz(s, pitch_quat(f64::from(s))))
        .collect();
    rows.extend((51..=260).map(|s| row_52hz(s, Quat::IDENTITY)));
    rows.push(row_52hz(900, pitch_quat(5.0)));
    assert_eq!(holds(&rows), [51]);
}

// A glide is under way when the earbuds stop sending; the next sample they send is its
// identity sample, more than a glide's length later. The held glide samples must still be
// dropped, not released as head movement.
#[test]
fn glide_cut_off_by_a_gap_stays_hidden() {
    let held = pitch_quat(30.0);
    let mut rows: Vec<Row> = (1..=100).map(|s| row_52hz(s, held)).collect();
    rows.extend((1..=20).map(|k| row_52hz(100 + k, firmware_glide(held, k))));
    rows.extend((80..=90).map(|k| row_52hz(100 + k, Quat::IDENTITY))); // 59 samples not sent
    for o in replay(&rows) {
        let pitch = o.abs.euler().pitch;
        assert!((pitch - 30.0).abs() < 0.01, "pitch {pitch:.2}");
    }
}

/// The head held still at 30° for 100 samples. Then a glide from `start` (relative to the
/// earbuds' reference), of which the earbuds send only step 16 onwards, numbered from
/// `first_seq`. The glide ends with its identity sample, and the head stays still.
fn glide_sent_from_its_middle(start: Quat, first_seq: u32) -> Vec<Row> {
    let mut rows: Vec<Row> = (1..=100).map(|s| row_52hz(s, pitch_quat(30.0))).collect();
    let k0 = 16;
    rows.extend((k0..=68).map(|k| row_52hz(first_seq + k - k0, firmware_glide(start, k))));
    let end = first_seq + 68 - k0;
    rows.extend((1..=30).map(|i| row_52hz(end + i, Quat::IDENTITY)));
    rows
}

// The earbuds can start sending in the middle of a glide: after a gap in the same stream, or in
// a new stream whose first samples never arrived (data/live6_raw.jsonl, seq 279 -> 227;
// stream_restart.csv, 721 -> 235). The glide's start is never seen, yet the glide must stay
// hidden.
#[test]
fn glide_with_an_unseen_start_is_hidden() {
    for (what, rows) in [
        (
            "same stream",
            glide_sent_from_its_middle(pitch_quat(30.0), 116),
        ),
        (
            "new stream",
            glide_sent_from_its_middle(pitch_quat(1.5), 227),
        ),
    ] {
        let out = replay(&rows);
        assert_eq!(
            rows.len() - out.len(),
            52,
            "{what}: the glide's 52 samples are dropped"
        );
        for o in &out {
            let pitch = o.abs.euler().pitch;
            assert!((pitch - 30.0).abs() < 0.01, "{what}: pitch {pitch:.3}");
        }
        assert_eq!(holds(&rows), Vec::<u32>::new(), "{what}");
    }
}

// A slow slouch: the head moves 0.2° from the reference and holds still until the earbuds
// re-centre (209 samples, then the glide), ten times over. Small glides are hidden like large
// ones, so the ten steps add up to 2°.
#[test]
fn small_recentres_keep_the_pose() {
    let (mut rows, mut seq) = (Vec::new(), 0);
    for _ in 0..10 {
        let step = pitch_quat(0.2); // the head moved 0.2° from the earbuds' reference
        for _ in 0..209 {
            seq += 1;
            rows.push(row_52hz(seq, step));
        }
        for k in 1..=68 {
            seq += 1;
            rows.push(row_52hz(seq, firmware_glide(step, k)));
        }
    }
    let pitch = replay(&rows).last().expect("poses").abs.euler().pitch;
    assert!(
        (pitch - 2.0).abs() < 0.01,
        "ten 0.2° steps read {pitch:.3}°"
    );
}

/// The file names of every real recording in tests/fixtures/freeclip2/.
fn recordings() -> Vec<String> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/freeclip2");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".csv"))
        .collect();
    names.sort();
    assert!(names.len() >= 19, "recordings missing from {dir}");
    names
}

// Every real recording follows the glide formula, including glides cut short, so no glide is
// taken for a hold. The holds are identity samples with no glide before them: live4.csv seq 34,
// after a wear flicker, and the walks in turn_walk.csv, walk_reconnect.csv, walk_near_far.csv and
// walk_slouch.csv (FINDINGS "Re-centre and stillness", "Instant resets while walking").
#[test]
fn holds_are_found_only_where_recorded() {
    for name in recordings() {
        let want: &[u32] = match name.as_str() {
            "live4.csv" => &[34],
            "turn_walk.csv" => &[8197, 11137],
            "walk_reconnect.csv" => &[984, 3414],
            "walk_near_far.csv" => &[2122, 3622, 7552],
            "walk_slouch.csv" => &[2161, 2821],
            _ => &[],
        };
        assert_eq!(holds(&read_csv(&name)), want, "{name}");
    }
}

/// Indices of the rows of `rows` that are glide samples, found with the firmware formula alone.
/// For each identity sample after a non-identity one, look for the earliest row before it (at
/// most 68 sequence numbers back, in the same stream) such that every row in between matches a
/// glide from it (`firmware_glide`). The rows in between are that glide.
fn recorded_glide_rows(rows: &[Row]) -> Vec<usize> {
    let mut glide = Vec::new();
    for e in 1..rows.len() {
        if rows[e].raw != Quat::IDENTITY || rows[e - 1].raw == Quat::IDENTITY {
            continue;
        }
        let start = (e.saturating_sub(70)..e).find(|&s| {
            let p = rows[s].raw;
            let p = if p.w < 0.0 { -p } else { p };
            let same_stream = (s..e).all(|m| rows[m].seq < rows[m + 1].seq);
            same_stream
                && rows[e].seq - rows[s].seq <= 68
                && p.angle_to(Quat::IDENTITY) > 0.001
                && (s + 1..e).all(|m| {
                    let k = rows[m].seq - rows[s].seq;
                    rows[m].raw.angle_to(firmware_glide(p, k)) <= 0.001
                })
        });
        if let Some(s) = start {
            glide.extend(s + 1..e);
        }
    }
    glide
}

// No sample of a recorded glide comes out, in any recording.
#[test]
fn no_recorded_glide_comes_out() {
    for name in recordings() {
        // Make receive times unique (the row index), to see which rows came out.
        let rows: Vec<Row> = read_csv(&name)
            .into_iter()
            .enumerate()
            .map(|(i, r)| Row {
                t: Duration::from_nanos(i as u64),
                ..r
            })
            .collect();
        let out: std::collections::HashSet<u128> =
            replay(&rows).iter().map(|o| o.t.as_nanos()).collect();
        let glide = recorded_glide_rows(&rows);
        let leaked: Vec<u32> = glide
            .iter()
            .filter(|&&i| out.contains(&(i as u128)))
            .map(|&i| rows[i].seq)
            .collect();
        assert!(leaked.is_empty(), "{name}: glide samples out: {leaked:?}");
        println!("{name}: {} glide samples, none out", glide.len());
    }
}

/// 105 samples held at 30°, then the first `glide` samples of the firmware's glide from there.
/// Rows at 35 Hz, with a `stall` in transit after the 40th glide sample.
fn held_then_glide(glide: u32, stall: f64) -> (Vec<Row>, f64, u32) {
    let hz = 35.0;
    let (mut rows, mut seq, mut t) = (Vec::new(), 1, 0.0);
    let mut add = |rows: &mut Vec<Row>, t: f64, raw: Quat| {
        rows.push(row(t, seq, raw));
        seq += 1;
    };
    let held = pitch_quat(30.0);
    for _ in 0..105 {
        add(&mut rows, t, held);
        t += 1.0 / hz;
    }
    for k in 1..=glide {
        add(&mut rows, t, firmware_glide(held, k));
        t += 1.0 / hz;
        if k == 40 {
            t += stall;
        }
    }
    let next = rows.last().unwrap().seq + 1;
    (rows, t, next)
}

// A transport stall of any length in the middle of a glide does not let the glide out. Until
// the identity sample arrives, the head is taken to be still at 30°.
#[test]
fn glide_survives_a_long_stall() {
    let (mut rows, t, seq) = held_then_glide(66, 8.0);
    rows.push(sample(t, seq, 0.0)); // identity: the glide ends
    for i in 1..=60 {
        rows.push(sample(t + 6.0 + f64::from(i) / 35.0, seq + 60 + i, 0.0)); // still at 30°
    }
    for o in replay(&rows) {
        let pitch = o.abs.euler().pitch;
        assert!(
            (pitch - 30.0).abs() < 0.5,
            "t={:.2}: pitch {pitch:.1}",
            o.t.as_secs_f64()
        );
    }
}

// The earbuds start a new stream in the middle of a glide, while the head is still. The
// unfinished glide must not come out as head movement.
#[test]
fn stream_restart_mid_glide_hides_the_glide() {
    let (mut rows, t, _) = held_then_glide(40, 0.0);
    for i in 1..=60 {
        // The new stream measures from the head pose at its start, so the head still reads 30°.
        rows.push(sample(t + 1.0 + f64::from(i) / 35.0, i, 0.0));
    }
    for o in replay(&rows) {
        let pitch = o.abs.euler().pitch;
        assert!(
            (pitch - 30.0).abs() < 0.5,
            "t={:.2}: pitch {pitch:.1}",
            o.t.as_secs_f64()
        );
    }
}

// tools/freeclip2/device/glide_move.py (2026-11-12): look down; at a beep, the moment a
// re-centre starts, look straight back up. The earbuds cut the glide short, and the movement
// does not show in its readings. Their reference is then the pose two samples before the cut
// (FINDINGS "Re-centre and stillness"), so the movement before it is lost. Afterwards the
// rebuilt pose must be near upright (within the earbuds' ~10° error per re-centre), not stuck
// at the ~50° slouch. Trial 2 got no beep (no re-centre in time).
#[test]
fn moving_during_a_glide_is_not_lost() {
    let rows = read_csv("glide_move.csv");
    let ph = phases(&rows);
    let mut after: Vec<Vec<f64>> = vec![Vec::new(); ph.len()];
    for o in replay(&rows) {
        after[phase_at(&ph, o.t)].push(o.abs.euler().pitch);
    }
    let mut checked = 0;
    for ((name, start), pitches) in ph.iter().zip(&after) {
        if name != "after" {
            continue;
        }
        let end = &pitches[pitches.len().saturating_sub(40)..];
        let mean = end.iter().sum::<f64>() / end.len() as f64;
        assert!(
            (-25.0..5.0).contains(&mean),
            "after the move at {:.1} s: pitch {mean:.1}°",
            start.as_secs_f64()
        );
        checked += 1;
    }
    assert_eq!(checked, 2, "two trials moved during a glide");
}

// data/hold_test.csv: look down 30 s, center 15 s, twice. While the head is held still, the
// earbuds glide the reading to identity, then go silent. The glide must not reach the output.
#[test]
fn hides_recentre_glide_during_long_holds() {
    let rows = read_csv("hold_test.csv");
    let ph = phases(&rows);
    let out = replay(&rows);
    for o in &out {
        let (name, start) = &ph[phase_at(&ph, o.t)];
        let pitch = o.abs.euler().pitch;
        if o.t - *start > secs(2.0) {
            match name.as_str() {
                "down" => assert!(
                    pitch > 45.0,
                    "t={:.2} down hold emitted pitch {pitch:.1}",
                    o.t.as_secs_f64()
                ),
                "center" => assert!(
                    pitch.abs() < 10.0,
                    "t={:.2} center hold emitted pitch {pitch:.1}",
                    o.t.as_secs_f64()
                ),
                _ => {}
            }
        }
    }
    // Five re-centres (one small, while upright at 5.4 s). Each drops exactly its glide's 67
    // samples: the hypothesis from the pose held before each glide predicts its identity
    // sample.
    let dropped = rows.len() - out.len();
    assert_eq!(dropped, 5 * 67, "dropped samples");
}

// Live session (2026-11-11): look down at ~15.2 s and hold. The earbuds glide to identity at
// 19.27–20.49 s (0.67° per sample), only ~2 s after the head settled. The glide must stay
// hidden.
#[test]
fn hides_glide_after_short_hold() {
    for o in replay(&read_csv("short_hold_glide.csv")) {
        let pitch = o.abs.euler().pitch;
        if o.t > secs(16.0) {
            assert!(
                pitch > 40.0,
                "t={:.2}: still looking down, but pitch {pitch:.1}",
                o.t.as_secs_f64()
            );
        }
    }
}

// A real head lift after a still hold must come through.
#[test]
fn real_lift_after_hold_is_not_swallowed() {
    let mut rows = Vec::new();
    let mut add = |p: f64| {
        let n = rows.len() as u32 + 1;
        rows.push(Row {
            phase: String::new(),
            t: Duration::from_millis(20 * u64::from(n)),
            seq: n,
            raw: pitch_quat(p),
        });
    };
    (0..200).for_each(|_| add(30.0)); // 4 s still at 30°
    let mut p = 30.0;
    for step in [0.3, 0.8, 1.5, 2.5, 3.5, 4.0, 3.0, 2.0, 1.0, 0.5] {
        p -= step; // accelerating then decelerating lift, stops at 10.9°
        add(p);
    }
    (0..50).for_each(|_| add(p));
    let pitch = replay(&rows).last().unwrap().abs.euler().pitch;
    assert!(
        (pitch - p).abs() < 0.5,
        "lift to {p:.1} reported as {pitch:.1}"
    );
}

// data/session3 (stream_restart.csv): while the user looked down ~51° with the head still, the
// earbuds started a new stream (seq 721 -> 235), measured from the pose at that moment. Then
// the user sat up. The rebuilt pose must stay down until the user sits up, and read upright
// after.
#[test]
fn continues_across_stream_restart() {
    let rows = read_csv("stream_restart.csv");
    let restart = rows
        .windows(2)
        .find(|w| w[1].seq < w[0].seq)
        .expect("recording has a restart")[1]
        .t;
    let out = replay(&rows);
    // Still looking down right after the restart, until the user starts to sit up (~25.6 s).
    for o in out.iter().filter(|o| o.t >= restart && o.t < secs(25.6)) {
        let p = o.abs.euler().pitch;
        assert!(
            p > 40.0,
            "t={:.2}: still looking down, but pitch {p:.1}",
            o.t.as_secs_f64()
        );
    }
    let end = out.last().unwrap();
    let p = end.abs.euler().pitch;
    assert!(
        p.abs() < 10.0,
        "t={:.2}: upright again, but pitch {p:.1}",
        end.t.as_secs_f64()
    );
}
