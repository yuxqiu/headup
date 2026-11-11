//! Head orientation from the FreeClip 2 43/89 stream.
//!
//! The earbuds report a quaternion relative to a reference that they move to any pose held still
//! for a few seconds. Every move is a glide: the reported orientation blends to identity over a
//! fixed ~68 samples (≈1.3 s, at any angle), ending in an exact identity sample
//! (docs/devices/freeclip2/FINDINGS.md "Re-centre model"). `Rebuilder` turns this into one
//! continuous orientation relative to the stream start:
//!
//! - Every sample is held back for `DELAY_S`, longer than a glide.
//! - On an identity sample, the `GLIDE_SAMPLES` before it are the glide: they are dropped, and the
//!   pose just before them becomes the new reference.
//! - When the earbuds start a new stream (sequence number goes backwards), the last known
//!   orientation becomes the reference.

use crate::core::quat::Quat;
use std::collections::VecDeque;

/// Decode a 43/89 payload: TLV1 = u32 LE sequence, TLV2 = 4 x f32 LE (w, x, y, z).
pub fn decode_sample(seq: &[u8], quat: &[u8]) -> Option<(u32, Quat)> {
    let seq = u32::from_le_bytes(seq.try_into().ok()?);
    if quat.len() != 16 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(quat[i * 4..i * 4 + 4].try_into().unwrap()) as f64;
    Some((
        seq,
        Quat {
            w: f(0),
            x: f(1),
            y: f(2),
            z: f(3),
        },
    ))
}

/// How long each sample is held back; must exceed a glide (~1.3 s) plus Bluetooth jitter.
pub const DELAY_S: f64 = 2.0;
/// Samples from the start of a glide to its identity sample. Measured 66–72 (session 6); erring
/// high only drops a few still samples before the glide.
const GLIDE_SAMPLES: u32 = 70;

/// A sample waiting out `DELAY_S`.
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
    /// Sequence number and raw orientation of the last pushed sample.
    prev: Option<(u32, Quat)>,
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
        if let Some((prev_seq, prev_raw)) = self.prev {
            if seq < prev_seq {
                // A new stream measures from the head pose at its start; the head was last seen
                // at the last orientation, and streams stop only while the head is still.
                out.extend(self.release_all());
                self.reference = self.last_abs;
            } else if raw == Quat::IDENTITY && prev_raw != Quat::IDENTITY {
                out.extend(self.recentre(seq));
            }
        }
        self.prev = Some((seq, raw));
        self.held.push_back(Held { t, seq, raw });
        out.extend(self.release(t));
        out
    }

    /// Release the samples whose delay has passed by time `now`.
    pub fn release(&mut self, now: f64) -> Vec<(f64, Quat)> {
        let mut out = Vec::new();
        while let Some(h) = self.held.front() {
            if h.t + DELAY_S > now {
                break; // same expression as `next_release`, so a due sample always comes out
            }
            let h = self.held.pop_front().unwrap();
            out.push(self.emit(h));
        }
        out
    }

    /// When the oldest held sample is due, if any.
    pub fn next_release(&self) -> Option<f64> {
        self.held.front().map(|h| h.t + DELAY_S)
    }

    fn release_all(&mut self) -> Vec<(f64, Quat)> {
        let held: Vec<Held> = self.held.drain(..).collect();
        held.into_iter().map(|h| self.emit(h)).collect()
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

    /// One recorded 43/89 sample; `phase` is the guided-test step ("" if the recording has none).
    struct Row {
        phase: String,
        t: f64,
        seq: u32,
        raw: Quat,
    }

    fn read_csv(name: &str) -> Vec<Row> {
        let path = format!(
            "{}/tests/fixtures/freeclip2/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        let col = |n: &str| header.iter().position(|h| *h == n);
        let c = |n: &str| col(n).unwrap_or_else(|| panic!("{path}: no column {n}"));
        let (ct, cs, cw, cx, cy, cz, cp) = (
            c("t"),
            c("seq"),
            c("w"),
            c("x"),
            c("y"),
            c("z"),
            col("phase"),
        );
        lines
            .map(|line| {
                let f: Vec<&str> = line.split(',').collect();
                let p = |i: usize| f[i].parse::<f64>().unwrap();
                Row {
                    phase: cp.map_or(String::new(), |i| f[i].to_string()),
                    t: p(ct),
                    seq: f[cs].parse().unwrap(),
                    raw: Quat {
                        w: p(cw),
                        x: p(cx),
                        y: p(cy),
                        z: p(cz),
                    },
                }
            })
            .collect()
    }

    /// A released sample: measured at `t`, released at `now` (the delay later, or at a re-centre).
    struct Out {
        t: f64,
        now: f64,
        abs: Quat,
    }

    /// Replay a recording through a `Rebuilder`, then release everything still held.
    fn replay(rows: &[Row]) -> Vec<Out> {
        let mut rb = Rebuilder::default();
        let mut out = Vec::new();
        for r in rows {
            // Held samples come out on time even when nothing arrives (the backend's timer).
            while let Some(due) = rb.next_release().filter(|&due| due <= r.t) {
                out.extend(
                    rb.release(due)
                        .into_iter()
                        .map(|(t, abs)| Out { t, now: due, abs }),
                );
            }
            out.extend(rb.push(r.t, r.seq, r.raw).into_iter().map(|(t, abs)| Out {
                t,
                now: r.t,
                abs,
            }));
        }
        let end = rows.last().map_or(0.0, |r| r.t) + DELAY_S;
        out.extend(
            rb.release(end)
                .into_iter()
                .map(|(t, abs)| Out { t, now: end, abs }),
        );
        out
    }

    /// The guided phases of a recording, in order, with their start times.
    fn phases(rows: &[Row]) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = Vec::new();
        for r in rows {
            if v.last().is_none_or(|p| p.0 != r.phase) {
                v.push((r.phase.clone(), r.t));
            }
        }
        v
    }

    /// Index into `phases` of the phase a sample measured at `t` belongs to.
    fn phase_at(phases: &[(String, f64)], t: f64) -> usize {
        phases.partition_point(|p| p.1 <= t).saturating_sub(1)
    }

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

    // Ground truth (guided.py --slouch): five upright → look-down cycles back to the same screen
    // point. The earbuds' re-centres add ~10° towards "looking up" per cycle; without the detector's
    // re-anchoring, upright drifts to about -40° and the last look-down never counts as a slouch.
    #[test]
    fn repeated_slouches_stay_detectable_end_to_end() {
        use crate::core::detector::{Config, Detector};
        let rows = read_csv("slouch_guided.csv");
        let ph = phases(&rows);
        let mut det = Detector::new(Config::for_traits(super::super::FreeClip2::TRAITS));
        let mut state = vec![(false, 0.0); ph.len()]; // (slouched, last smoothed) per phase
        for o in replay(&rows) {
            det.update(o.now, o.abs.euler().pitch); // baseline = stream start (upright)
            let s = &mut state[phase_at(&ph, o.t)];
            s.0 |= det.slouching();
            s.1 = det.smoothed().unwrap_or(0.0);
        }
        for ((name, _), (slouched, end)) in ph.iter().zip(&state) {
            if name.starts_with("down") {
                assert!(slouched, "{name}: look-down not detected; {state:?}");
            } else {
                assert!(
                    end.abs() < 6.0,
                    "{name}: upright ends at {end:.1}; {state:?}"
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

    /// Replay through a detector the way the session drives it: poses at their release time and
    /// 1 s ticks in between. Returns (cue, measurement time of the last pose before it).
    fn cues_end_to_end(rows: &[Row]) -> Vec<(crate::core::detector::Cue, f64)> {
        use crate::core::detector::{Config, Detector};
        let mut det = Detector::new(Config::default());
        let (mut cues, mut tick, mut cur) = (Vec::new(), 0.0, 0.0);
        for o in replay(rows) {
            while tick < o.now {
                cues.extend(det.tick(tick).into_iter().map(|c| (c, cur))); // silence: the last pose holds
                tick += 1.0;
            }
            cur = o.t;
            // baseline = stream start (upright)
            cues.extend(
                det.update(o.now, o.abs.euler().pitch)
                    .cues
                    .into_iter()
                    .map(|c| (c, cur)),
            );
        }
        cues
    }

    // End to end on the real long-hold recording: each 30 s look-down hold must cue, and sitting
    // back up must register as recovery (session 1 failed this before glides were suppressed).
    #[test]
    fn long_holds_trigger_cues_end_to_end() {
        use crate::core::detector::Cue;
        let rows = read_csv("hold_test.csv");
        let ph = phases(&rows);
        let cues = cues_end_to_end(&rows);
        // Speech (at 30 s) depends on exactly how long each ~30 s hold lasted, so it is not asserted.
        let key: Vec<(Cue, &str)> = cues
            .iter()
            .filter(|c| c.0 != Cue::Speak)
            .map(|&(c, t)| (c, ph[phase_at(&ph, t)].0.as_str()))
            .collect();
        let want = [
            (Cue::Tone, "down"),
            (Cue::Recovered, "center"),
            (Cue::Tone, "down"),
            (Cue::Recovered, "center"),
        ];
        assert_eq!(key, want, "cues: {cues:?}");
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

    // End to end on the stream-restart recording: one slouch, so exactly one tone and one recovery,
    // and the recovery only once the user actually sat up (~25.6 s), not at the restart.
    #[test]
    fn stream_restart_end_to_end() {
        use crate::core::detector::Cue;
        let cues = cues_end_to_end(&read_csv("stream_restart.csv"));
        let kinds: Vec<Cue> = cues
            .iter()
            .map(|c| c.0)
            .filter(|c| *c != Cue::Speak)
            .collect();
        assert_eq!(kinds, [Cue::Tone, Cue::Recovered], "cues: {cues:?}");
        let recovered_at = cues.iter().find(|c| c.0 == Cue::Recovered).unwrap().1;
        assert!(
            recovered_at >= 25.6,
            "recovered at {recovered_at:.2}, before sitting up"
        );
    }

    #[test]
    fn decodes_real_sample() {
        let seq = [0x0b, 0, 0, 0];
        let q = [
            0xf2, 0xff, 0x7f, 0x3f, 0x6a, 0xfc, 0xef, 0xb9, 0x7b, 0x6e, 0xed, 0xb8, 0x85, 0x90,
            0x9d, 0xba,
        ];
        let (s, q) = decode_sample(&seq, &q).unwrap();
        assert_eq!(s, 11);
        assert!((q.w - 1.0).abs() < 1e-5 && q.x.abs() < 1e-3);
    }
}
