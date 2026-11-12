//! Head orientation from the FreeClip 2 43/89 stream.
//!
//! The earbuds report a quaternion relative to a reference that they move to any pose held still
//! for a few seconds. Every move is a glide: from the pose `q_s` held just before it, the `k`-th
//! sample after it reads `slerp(q_s, I, (k + 1) / (GLIDE_LEN + 1))` (its angle falls linearly to
//! zero on a fixed axis; the first sample is already two steps along), and sample `GLIDE_LEN` is an
//! exact identity sample. The length is a sample count at any sample rate
//! (docs/devices/freeclip2/FINDINGS.md "Glide model checked against every recording").
//! `Rebuilder` turns this into one continuous orientation relative to the stream start:
//!
//! - Every sample is held until a sample `GLIDE_SAMPLES` newer arrives: by then no identity sample
//!   can make it part of a glide. This also covers glides during which the head moved, which
//!   don't fit the model.
//! - Glide hypotheses: for each recent sample, "a glide started from this pose". A hypothesis
//!   lives while every later sample fits the model within `FIT_TOL_DEG`.
//! - On an identity sample, the samples of the hypothesis that fits it are dropped (the head was
//!   still at its `q_s`), and the pose before them becomes the new reference.
//! - When the earbuds go quiet for `QUIET`, everything up to the earliest live hypothesis's
//!   start pose is released: nothing before it can be part of an unfinished glide. The rest waits
//!   for its identity sample, however long the silence: the model says the head is still at `q_s`.
//! - When the earbuds start a new stream (sequence number goes backwards), a live glide is dropped
//!   (the head was still) and the last known orientation becomes the reference.

use crate::core::quat::Quat;
use std::collections::VecDeque;
use std::time::Duration;

/// Samples a sample is held for while the stream flows: longer than a glide, with margin.
const GLIDE_SAMPLES: u32 = 70;
/// Samples from the pose held before a glide to its identity sample.
const GLIDE_LEN: u32 = 68;
/// How far a sample may be from the glide model and still fit it. The largest misfit of a clean
/// glide was 0.42°.
const FIT_TOL_DEG: f64 = 0.5;
/// Silence after which the samples that cannot be part of an unfinished glide are released (the
/// earbuds also go quiet without a re-centre, e.g. a slouch held still:
/// tests/fixtures/freeclip2/stream_restart.csv). Only latency depends on it, not correctness: above
/// the largest gap seen between samples while they flow (74 ms).
const QUIET: Duration = Duration::from_millis(200);

/// A raw sample as received, at `t` since the stream's time zero.
#[derive(Clone, Copy)]
struct Held {
    t: Duration,
    seq: u32,
    raw: Quat,
}

/// "A glide started from this pose": the start pose and its sequence number.
#[derive(Clone, Copy)]
struct Glide {
    seq: u32,
    from: Quat,
}

impl Glide {
    /// Whether sample `h` fits this glide.
    fn fits(&self, h: &Held) -> bool {
        let k = h.seq - self.seq;
        k <= GLIDE_LEN
            && h.raw
                .angle_to(self.from.toward_identity(k + 1, GLIDE_LEN + 1))
                <= FIT_TOL_DEG
    }
}

pub struct Rebuilder {
    /// Reference for every held sample (it only changes once the held samples before it are out).
    reference: Quat,
    /// The last released absolute orientation.
    last_abs: Quat,
    /// The last pushed sample.
    prev: Option<Held>,
    /// Samples not yet released, oldest first.
    held: VecDeque<Held>,
    /// Live glide hypotheses, oldest start first.
    glides: Vec<Glide>,
}

impl Default for Rebuilder {
    fn default() -> Self {
        Rebuilder {
            reference: Quat::IDENTITY,
            last_abs: Quat::IDENTITY,
            prev: None,
            held: VecDeque::new(),
            glides: Vec::new(),
        }
    }
}

impl Rebuilder {
    /// Feed one raw sample received at `t` (since the caller's time zero). Returns the samples
    /// released by it, oldest first, as (receive time, absolute orientation since stream start).
    pub fn push(&mut self, t: Duration, seq: u32, raw: Quat) -> Vec<(Duration, Quat)> {
        let sample = Held { t, seq, raw };
        let mut out = Vec::new();
        if let Some(prev) = self.prev {
            if seq < prev.seq {
                // A new stream measures from the head pose at its start; the head was last seen
                // at the last orientation, and streams stop only while the head is still.
                out.extend(self.end_stream());
            } else if raw == Quat::IDENTITY && prev.raw != Quat::IDENTITY {
                out.extend(self.recentre(&sample));
            } else {
                self.glides.retain(|g| g.fits(&sample));
            }
        }
        self.prev = Some(sample);
        self.held.push_back(sample);
        // A glide from within FIT_TOL_DEG of identity moves the reading by less than the tolerance:
        // indistinguishable from holding still, and harmless either way. Only track the others.
        if raw.angle_to(Quat::IDENTITY) > FIT_TOL_DEG {
            self.glides.push(Glide { seq, from: raw });
        }
        // Samples `GLIDE_SAMPLES` older than this one can no longer be part of a glide.
        while let Some(&h) = self.held.front() {
            if seq.saturating_sub(h.seq) < GLIDE_SAMPLES {
                break;
            }
            self.held.pop_front();
            out.push(self.emit(h));
        }
        out
    }

    /// After `QUIET` of silence, release the held samples that cannot be part of an unfinished
    /// glide. Calling it at (or after) `next_release()` always releases them: times are exact.
    pub fn release(&mut self, now: Duration) -> Vec<(Duration, Quat)> {
        match self.next_release() {
            Some(due) if due <= now => {
                let n = self.releasable();
                self.release_first(n)
            }
            _ => Vec::new(),
        }
    }

    /// When the next release happens if nothing else arrives, if anything is releasable.
    pub fn next_release(&self) -> Option<Duration> {
        let last = self.prev?;
        (self.releasable() > 0).then_some(last.t + QUIET)
    }

    /// How many held samples, from the oldest, cannot be part of an unfinished glide: those up to
    /// and including the earliest live hypothesis's start pose.
    fn releasable(&self) -> usize {
        let start = self.glides.first().map_or(u32::MAX, |g| g.seq);
        self.held.iter().take_while(|h| h.seq <= start).count()
    }

    /// The identity sample `end` ends a glide: drop it, release what came before it, and take the
    /// pose at its start as the new reference. The glide is the live hypothesis that predicts
    /// `end` (the one closest to `GLIDE_LEN` long if several do). A glide during which the head
    /// moved fits none, and a small one may have no hypothesis; then the `GLIDE_SAMPLES` before
    /// `end` are dropped.
    fn recentre(&mut self, end: &Held) -> Vec<(Duration, Quat)> {
        let start = self
            .glides
            .iter()
            .filter(|g| g.fits(end))
            .map(|g| g.seq)
            .min_by_key(|&s| (end.seq - s).abs_diff(GLIDE_LEN))
            .unwrap_or(end.seq.saturating_sub(GLIDE_SAMPLES));
        self.drop_after(start);
        let out = self.release_all();
        self.reference = self.last_abs;
        out
    }

    /// The stream ended (a new one starts): drop a live glide (the head was still at its start),
    /// release the rest, and take the last orientation as the new stream's reference.
    fn end_stream(&mut self) -> Vec<(Duration, Quat)> {
        if let Some(g) = self.glides.first() {
            let start = g.seq;
            self.drop_after(start);
        }
        let out = self.release_all();
        self.reference = self.last_abs;
        out
    }

    /// Drop held samples after sequence number `start`, and every hypothesis.
    fn drop_after(&mut self, start: u32) {
        while self.held.back().is_some_and(|h| h.seq > start) {
            self.held.pop_back();
        }
        self.glides.clear();
    }

    fn release_first(&mut self, n: usize) -> Vec<(Duration, Quat)> {
        let first: Vec<Held> = self.held.drain(..n).collect();
        first.into_iter().map(|h| self.emit(h)).collect()
    }

    fn release_all(&mut self) -> Vec<(Duration, Quat)> {
        self.release_first(self.held.len())
    }

    fn emit(&mut self, h: Held) -> (Duration, Quat) {
        self.last_abs = self.reference * h.raw;
        (h.t, self.last_abs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quat::{Euler, Quat};
    use crate::devices::freeclip2::tests::replay::{Row, phase_at, phases, read_csv, replay, secs};

    /// Replays a guided recording and returns the median (pitch, yaw, roll) of the last 60 samples of
    /// each phase, in order.
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

    // The two real guided recordings; run 1 accumulates more error.
    #[test]
    fn rebuilds_guided_run_fixed_mode() {
        check("headpose_axes_fixed.csv", 8.0);
    }

    #[test]
    fn rebuilds_guided_run_follow_mode() {
        check("headpose_axes.csv", 15.0);
    }

    fn pitch_quat(deg: f64) -> Quat {
        let h = deg.to_radians() / 2.0;
        Quat {
            w: h.cos(),
            x: 0.0,
            y: h.sin(),
            z: 0.0,
        }
    }

    /// A sample of `pitch` with sequence number `seq`, received at `t`.
    fn sample(t: f64, seq: u32, pitch: f64) -> Row {
        let raw = if pitch == 0.0 {
            Quat::IDENTITY
        } else {
            pitch_quat(pitch)
        };
        Row {
            phase: String::new(),
            t: secs(t),
            seq,
            raw,
        }
    }

    // The earbuds' rate varies (35 Hz in data/live_check_raw.jsonl, 52.5 Hz elsewhere) but a glide
    // is always ~68 samples: at 35 Hz it takes ~1.95 s. Here a 3 s stall in transit (seen early in
    // streams) makes the identity sample arrive ~4.9 s after the glide began. Every output must still
    // read 30°, the held pose: no glide sample may come out before the identity sample that marks it.
    #[test]
    fn hides_slow_glide_at_any_sample_rate() {
        let hz = 35.0;
        let mut rows = Vec::new();
        let mut seq = 1;
        let mut t = 0.0;
        let mut add = |rows: &mut Vec<Row>, t: f64, pitch: f64| {
            rows.push(sample(t, seq, pitch));
            seq += 1;
        };
        for _ in 0..105 {
            add(&mut rows, t, 30.0); // 3 s held at 30°
            t += 1.0 / hz;
        }
        for i in 1..=67 {
            add(&mut rows, t, 30.0 * (1.0 - f64::from(i) / 68.0)); // the glide
            t += 1.0 / hz;
            if i == 40 {
                t += 3.0; // transport stall, as seen early in streams
            }
        }
        add(&mut rows, t, 0.0); // identity: the glide ends
        t += 6.0; // head still: the earbuds go quiet
        for _ in 0..100 {
            add(&mut rows, t, 0.0); // relative to the new reference: still 30°
            t += 1.0 / hz;
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
        // Exactly the glide's 67 samples; the still samples before it all come out.
        assert_eq!(rows.len() - out.len(), 67, "the glide is dropped");
    }

    // The earbuds can go quiet without a re-centre while the head is held still
    // (stream_restart.csv). A still pose cannot be an unfinished glide, so it comes out right after
    // the stream goes quiet.
    #[test]
    fn releases_still_pose_soon_after_the_stream_goes_quiet() {
        let mut rb = Rebuilder::default();
        let samples: Vec<(u32, f64)> = (1..=100).map(|seq| (seq, 30.0)).collect();
        let last = push_all(&mut rb, &samples);
        let due = rb.next_release().unwrap();
        assert_eq!(due, last + QUIET);
        let out = rb.release(due);
        // All of it: a still pose far from identity cannot be an unfinished glide.
        assert_eq!(out.len(), 70);
        assert!(
            out.iter()
                .all(|(_, q)| (q.euler().pitch - 30.0).abs() < 1e-6)
        );
    }

    /// Push `(seq, pitch)` samples at 50 Hz from t = 0; returns the time of the last one.
    fn push_all(rb: &mut Rebuilder, samples: &[(u32, f64)]) -> Duration {
        let mut t = Duration::ZERO;
        for (i, &(seq, pitch)) in samples.iter().enumerate() {
            t = Duration::from_millis(20 * i as u64);
            rb.push(t, seq, pitch_quat(pitch));
        }
        t
    }

    /// Release everything the silence after `last` allows; returns how many came out.
    fn drain_quiet(rb: &mut Rebuilder, last: Duration) -> usize {
        let mut n = 0;
        while let Some(due) = rb
            .next_release()
            .filter(|&d| d <= last + Duration::from_secs(1))
        {
            n += rb.release(due).len();
        }
        n
    }

    // A still pose whose readings drift by sensor noise towards the reference (here 0.05° per
    // sample, as in stream_restart.csv) is not a glide: a glide from 50° moves 0.75° per sample.
    // Everything comes out within a second of the earbuds going quiet.
    #[test]
    fn noisy_still_pose_comes_out_when_the_stream_goes_quiet() {
        let mut rb = Rebuilder::default();
        let samples: Vec<(u32, f64)> = (1..=100)
            .map(|seq: u32| (seq, 50.0 - 0.05 * f64::from(seq.saturating_sub(95))))
            .collect();
        let last = push_all(&mut rb, &samples);
        let early = 100 - 70; // released by the sample count while the stream flowed
        assert_eq!(drain_quiet(&mut rb, last) + early, 100);
        assert_eq!(rb.next_release(), None);
    }

    // Sitting back up towards the reference at 2° per sample, then going still and quiet, is not a
    // glide (a glide from 30° moves 0.45° per sample): it comes out at once, not seconds later.
    #[test]
    fn sitting_up_comes_out_when_the_stream_goes_quiet() {
        let mut rb = Rebuilder::default();
        let mut samples: Vec<(u32, f64)> = (1..=80).map(|seq| (seq, 30.0)).collect();
        samples.extend((81..=92).map(|seq| (seq, 30.0 - 2.0 * f64::from(seq - 80))));
        let last = push_all(&mut rb, &samples);
        assert_eq!(drain_quiet(&mut rb, last) + (92 - 70), 92);
        assert_eq!(rb.next_release(), None);
    }

    /// 105 samples held at 30°, then `glide` samples of a 30° glide to identity (67 steps), as
    /// rows at 35 Hz with a `stall` in transit after the 40th glide sample.
    fn held_then_glide(glide: u32, stall: f64) -> (Vec<Row>, f64, u32) {
        let hz = 35.0;
        let (mut rows, mut seq, mut t) = (Vec::new(), 1, 0.0);
        let mut add = |rows: &mut Vec<Row>, t: f64, pitch: f64| {
            rows.push(sample(t, seq, pitch));
            seq += 1;
        };
        for _ in 0..105 {
            add(&mut rows, t, 30.0);
            t += 1.0 / hz;
        }
        for i in 1..=glide {
            add(&mut rows, t, 30.0 * (1.0 - f64::from(i) / 67.0));
            t += 1.0 / hz;
            if i == 40 {
                t += stall;
            }
        }
        let next = rows.last().unwrap().seq + 1;
        (rows, t, next)
    }

    // A transport stall of any length in the middle of a glide cannot let the glide out: the
    // model says the head is still at 30° until the identity sample arrives.
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

    // The earbuds start a new stream in the middle of a glide (the head is still): the unfinished
    // glide must not come out as head movement.
    #[test]
    fn stream_restart_mid_glide_hides_the_glide() {
        let (mut rows, t, _) = held_then_glide(40, 0.0);
        for i in 1..=60 {
            // The new stream measures from the head pose at its start: still at 30°.
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

    // data/hold_test.csv: look down 30 s / center 15 s, twice. While held still the earbuds glide the
    // reading to identity, then go silent. The glide must not reach the output.
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
        // Five re-centres (one small, while upright at 5.4 s): exactly each glide's 67 samples are
        // dropped (the model's hypothesis from the pose held before it predicts each identity).
        let dropped = rows.len() - out.len();
        assert_eq!(dropped, 5 * 67, "dropped samples");
    }

    // Live session (2026-11-11): look down at ~15.2 s and hold. The earbuds glide to identity at
    // 19.27–20.49 s (0.67°/sample), only ~2 s after the head settled. The glide must stay hidden.
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

    // data/session3 (stream_restart.csv): looking down ~51°, head held still, then the earbuds
    // started a new stream (seq 721 -> 235) whose orientation is relative to the pose at that
    // moment; then the user sat up. Before handling restarts this read ~0° while still looking down
    // (a false "recovered") and -55° once upright.
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
}
