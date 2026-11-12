//! Head orientation from the FreeClip 2 43/89 stream.
//!
//! The earbuds report a quaternion relative to a reference that they move to any pose held still
//! for a few seconds. Every move is a glide: the reported orientation blends to identity over a
//! fixed ~68 samples (at any angle and any sample rate: ≈1.3 s at 52.5 Hz, ≈1.95 s at 35 Hz), ending
//! in an exact identity sample (docs/devices/freeclip2/FINDINGS.md "Re-centre model", "Sample rate
//! varies"). `Rebuilder` turns this into one continuous orientation relative to the stream start:
//!
//! - On an identity sample, the `GLIDE_SAMPLES` before it are the glide: they are dropped, and the
//!   pose just before them becomes the new reference.
//! - So every sample is held until a sample `GLIDE_SAMPLES` newer arrives; after that no identity
//!   sample can make it part of a glide. Counting samples, not seconds, keeps this right at any rate.
//! - When the earbuds go quiet with samples held, only a trailing run that steadily approaches
//!   identity can still be an unfinished glide. Everything before it is released after `QUIET_S`;
//!   the run itself waits up to `GLIDE_STALL_S` for its identity sample.
//! - When the earbuds start a new stream (sequence number goes backwards), the last known
//!   orientation becomes the reference.

use crate::core::quat::Quat;
use std::collections::VecDeque;

/// Samples from the start of a glide to its identity sample. Measured 66–72 (session 6); erring
/// high only drops a few still samples before the glide, and holds samples a little longer.
const GLIDE_SAMPLES: u32 = 70;
/// Silence after which held samples that cannot be part of an unfinished glide are released (the
/// earbuds also go quiet without a re-centre, e.g. a slouch held still:
/// tests/fixtures/freeclip2/stream_restart.csv). Only latency depends on it, not correctness: above
/// the largest gap seen between samples while they flow (74 ms).
const QUIET_S: f64 = 0.2;
/// How long a trailing run that could be an unfinished glide waits for its identity sample. Longer
/// than the longest transport stall seen while the earbuds kept sampling (3.65 s, FINDINGS "Sample
/// rate varies"), so a stall in the middle of a glide cannot let it out.
const GLIDE_STALL_S: f64 = 5.0;

/// The next release while the earbuds are quiet.
enum Pending {
    /// The first `n` held samples, which cannot be part of an unfinished glide.
    BeforeGlide { due: f64, n: usize },
    /// Everything held: all of it could be an unfinished glide, so it waits `GLIDE_STALL_S`.
    All { due: f64 },
}

/// A raw sample as received.
#[derive(Clone, Copy)]
struct Held {
    t: f64,
    seq: u32,
    raw: Quat,
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
}

impl Default for Rebuilder {
    fn default() -> Self {
        Rebuilder {
            reference: Quat::IDENTITY,
            last_abs: Quat::IDENTITY,
            prev: None,
            held: VecDeque::new(),
        }
    }
}

impl Rebuilder {
    /// Feed one raw sample (time in seconds). Returns the samples released by it, oldest first, as
    /// (sample time, absolute orientation since stream start).
    pub fn push(&mut self, t: f64, seq: u32, raw: Quat) -> Vec<(f64, Quat)> {
        let mut out = Vec::new();
        if let Some(prev) = self.prev {
            if seq < prev.seq {
                // A new stream measures from the head pose at its start; the head was last seen
                // at the last orientation, and streams stop only while the head is still.
                out.extend(self.release_all());
                self.reference = self.last_abs;
            } else if raw == Quat::IDENTITY && prev.raw != Quat::IDENTITY {
                out.extend(self.recentre(seq));
            }
        }
        let sample = Held { t, seq, raw };
        self.prev = Some(sample);
        self.held.push_back(sample);
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

    /// Release what the silence up to time `now` allows: after `QUIET_S`, the samples before a
    /// possible unfinished glide; after `GLIDE_STALL_S`, everything.
    pub fn release(&mut self, now: f64) -> Vec<(f64, Quat)> {
        match self.pending() {
            Some(Pending::BeforeGlide { due, n }) if due <= now => self.release_first(n),
            Some(Pending::All { due }) if due <= now => self.release_all(),
            _ => Vec::new(),
        }
    }

    /// When the next release happens if nothing else arrives, if anything is held.
    pub fn next_release(&self) -> Option<f64> {
        match self.pending()? {
            Pending::BeforeGlide { due, .. } | Pending::All { due } => Some(due),
        }
    }

    /// What the silence will release next, and when. Only a trailing run of consecutive samples
    /// that steadily approach identity can be a glide that has not ended yet (a glide is a straight
    /// ramp to identity; FINDINGS "Re-centre model"); everything before it can go after `QUIET_S`.
    fn pending(&self) -> Option<Pending> {
        let last = self.held.back()?;
        let angle = |h: &Held| h.raw.angle_to(Quat::IDENTITY);
        let mut start = self.held.len() - 1;
        while start > 0 {
            let (a, b) = (&self.held[start - 1], &self.held[start]);
            if b.seq != a.seq + 1 || angle(b) >= angle(a) {
                break;
            }
            start -= 1;
        }
        Some(match start {
            0 => Pending::All {
                due: last.t + GLIDE_STALL_S,
            },
            n => Pending::BeforeGlide {
                due: last.t + QUIET_S,
                n,
            },
        })
    }

    fn release_first(&mut self, n: usize) -> Vec<(f64, Quat)> {
        let first: Vec<Held> = self.held.drain(..n).collect();
        first.into_iter().map(|h| self.emit(h)).collect()
    }

    fn release_all(&mut self) -> Vec<(f64, Quat)> {
        self.release_first(self.held.len())
    }

    fn emit(&mut self, h: Held) -> (f64, Quat) {
        self.last_abs = self.reference * h.raw;
        (h.t, self.last_abs)
    }

    /// The identity sample `seq` ends a glide: drop the glide, release what came before it, and
    /// take the pose at the glide's start as the new reference.
    fn recentre(&mut self, seq: u32) -> Vec<(f64, Quat)> {
        let start = seq.saturating_sub(GLIDE_SAMPLES);
        while self.held.back().is_some_and(|h| h.seq > start) {
            self.held.pop_back();
        }
        let out = self.release_all();
        self.reference = self.last_abs;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quat::{Euler, Quat};
    use crate::devices::freeclip2::tests::replay::{Row, phase_at, phases, read_csv, replay};

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
            t,
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
            assert!((pitch - 30.0).abs() < 0.5, "t={:.2}: pitch {pitch:.1}", o.t);
        }
        // The glide and the still sample just before it. (The one before that, also within
        // `GLIDE_SAMPLES`, was released during the stall: a still pose cannot be the glide.)
        assert_eq!(rows.len() - out.len(), 68, "the glide is dropped");
    }

    // The earbuds can go quiet without a re-centre while the head is held still
    // (stream_restart.csv). A still pose cannot be an unfinished glide, so it comes out right after
    // the stream goes quiet, without waiting `GLIDE_STALL_S`.
    #[test]
    fn releases_still_pose_soon_after_the_stream_goes_quiet() {
        let mut rb = Rebuilder::default();
        let mut t = 0.0;
        for seq in 1..=100 {
            rb.push(t, seq, pitch_quat(30.0));
            t += 0.02;
        }
        let last = t - 0.02;
        let due = rb.next_release().unwrap();
        assert!((due - (last + QUIET_S)).abs() < 1e-9, "due at {due}");
        let out = rb.release(due);
        // All but the newest sample, which on its own could be the first of a glide.
        assert_eq!(out.len(), 69);
        assert!(
            out.iter()
                .all(|(_, q)| (q.euler().pitch - 30.0).abs() < 1e-6)
        );
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
            if o.t - start > 2.0 {
                match name.as_str() {
                    "down" => assert!(
                        pitch > 45.0,
                        "t={:.2} down hold emitted pitch {pitch:.1}",
                        o.t
                    ),
                    "center" => assert!(
                        pitch.abs() < 10.0,
                        "t={:.2} center hold emitted pitch {pitch:.1}",
                        o.t
                    ),
                    _ => {}
                }
            }
        }
        // Five re-centres (one small, while upright at 5.4 s), each preceded by 69 glide samples
        // within `GLIDE_SAMPLES`; exactly those are dropped.
        let dropped = rows.len() - out.len();
        assert_eq!(dropped, 5 * 69, "dropped samples");
    }

    // Live session (2026-11-11): look down at ~15.2 s and hold. The earbuds glide to identity at
    // 19.27–20.49 s (0.67°/sample), only ~2 s after the head settled. The glide must stay hidden.
    #[test]
    fn hides_glide_after_short_hold() {
        for o in replay(&read_csv("short_hold_glide.csv")) {
            let pitch = o.abs.euler().pitch;
            if o.t > 16.0 {
                assert!(
                    pitch > 40.0,
                    "t={:.2}: still looking down, but pitch {pitch:.1}",
                    o.t
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
                t: n as f64 * 0.02,
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
        for o in out.iter().filter(|o| o.t >= restart && o.t < 25.6) {
            let p = o.abs.euler().pitch;
            assert!(
                p > 40.0,
                "t={:.2}: still looking down, but pitch {p:.1}",
                o.t
            );
        }
        let end = out.last().unwrap();
        let p = end.abs.euler().pitch;
        assert!(
            p.abs() < 10.0,
            "t={:.2}: upright again, but pitch {p:.1}",
            end.t
        );
    }
}
