//! Head orientation from the FreeClip 2 43/89 stream.
//!
//! The earbuds report a quaternion relative to a reference that they silently move to any pose held
//! still for a few seconds; the move is announced by an exact identity sample once the head moves
//! again. `Rebuilder` turns that into one continuous orientation relative to the stream start by
//! taking "last stationary pose" as the new reference on each identity sample, and the last known
//! orientation when the earbuds start a new stream (sequence number goes backwards)
//! (docs/devices/freeclip2/FINDINGS.md).

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

pub struct Rebuilder {
    reference: Quat,
    last_abs: Quat,
    still: Quat,
    still_since: Option<f64>,
    prev_raw: Option<Quat>,
    prev_seq: Option<u32>,
    window: VecDeque<(f64, Quat)>,
    glide: Option<Glide>,
}

/// A suspected re-centre glide: after the head has been still, the earbuds slew the reported
/// orientation to identity at a constant rate (~1.6–1.9° per sample, data/hold_test.csv).
struct Glide {
    prev_angle: f64,
    step: Option<f64>,
}

/// A pose counts as stationary when every sample in the last `STILL_WINDOW_S` is within `STILL_DEG`.
const STILL_DEG: f64 = 3.0;
const STILL_WINDOW_S: f64 = 0.5;
/// Glides were observed after ~4 s of stillness; only suspect one after this long.
const GLIDE_AFTER_STILL_S: f64 = 2.5;
/// Per-sample decrease of the angle to identity: minimum, and allowed deviation from the first step.
const GLIDE_MIN_STEP_DEG: f64 = 0.3;
const GLIDE_STEP_TOL_DEG: f64 = 0.6;

impl Default for Rebuilder {
    fn default() -> Self {
        Rebuilder {
            reference: Quat::IDENTITY,
            last_abs: Quat::IDENTITY,
            still: Quat::IDENTITY,
            still_since: None,
            prev_raw: None,
            prev_seq: None,
            window: VecDeque::new(),
            glide: None,
        }
    }
}

impl Rebuilder {
    /// The earbuds started a new stream (its sequence number went backwards). A new stream measures
    /// from the head pose at the moment it started; the head was last seen at `last_abs` and the
    /// stream is silent only while the head is still, so that pose becomes the reference.
    fn restart_stream(&mut self) {
        self.reference = self.last_abs;
        self.still = self.last_abs;
        self.still_since = None;
        self.prev_raw = None;
        self.window.clear();
        self.glide = None;
    }

    /// Feed one raw sample (time in seconds). Returns the absolute orientation since stream start
    /// and whether this sample was a device re-centre, or `None` while the sample is held back as a
    /// possible re-centre glide (the head is then still, so "no new pose" is accurate).
    pub fn push(&mut self, t: f64, seq: u32, raw: Quat) -> Option<(Quat, bool)> {
        if self.prev_seq.is_some_and(|p| seq < p) {
            self.restart_stream();
        }
        self.prev_seq = Some(seq);
        let reset = raw == Quat::IDENTITY && self.prev_raw.is_some_and(|p| p != raw);
        let angle = raw.angle_to(Quat::IDENTITY);
        let prev_angle = self.prev_raw.map(|p| p.angle_to(Quat::IDENTITY));
        self.prev_raw = Some(raw);
        if reset {
            self.glide = None;
            self.reference = self.still;
        } else if let Some(g) = self.glide.as_mut() {
            let step = g.prev_angle - angle;
            let consistent = step > GLIDE_MIN_STEP_DEG
                && g.step.is_none_or(|s| (step - s).abs() < GLIDE_STEP_TOL_DEG);
            if consistent {
                g.step.get_or_insert(step);
                g.prev_angle = angle;
                return None;
            }
            self.glide = None; // not a glide: real movement, report it
        } else if self
            .still_since
            .is_some_and(|s| t - s >= GLIDE_AFTER_STILL_S)
            && prev_angle.is_some_and(|p| p - angle > GLIDE_MIN_STEP_DEG)
        {
            // First step of a glide can be larger than the rest, so it sets no step size.
            self.glide = Some(Glide {
                prev_angle: angle,
                step: None,
            });
            return None;
        }
        self.last_abs = self.reference * raw;
        self.window.push_back((t, self.last_abs));
        while self
            .window
            .front()
            .is_some_and(|(t0, _)| t - t0 > STILL_WINDOW_S)
        {
            self.window.pop_front();
        }
        let front = self.window.front().unwrap().0;
        if t - front >= 0.8 * STILL_WINDOW_S
            && self
                .window
                .iter()
                .all(|(_, q)| q.angle_to(self.last_abs) < STILL_DEG)
        {
            self.still = self.last_abs;
            self.still_since.get_or_insert(front);
        } else {
            self.still_since = None;
        }
        Some((self.last_abs, reset))
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

    /// Replays a guided recording and returns the median (pitch, yaw, roll) of the last 60 samples of
    /// each phase, in order.
    fn phase_medians(name: &str) -> Vec<(String, f64, f64, f64)> {
        let mut rb = Rebuilder::default();
        let mut phases: Vec<(String, Vec<Quat>)> = Vec::new();
        for r in read_csv(name) {
            let Some((abs, _)) = rb.push(r.t, r.seq, r.raw) else {
                continue;
            };
            match phases.last_mut() {
                Some((name, v)) if *name == r.phase => v.push(abs),
                _ => phases.push((r.phase, vec![abs])),
            }
        }
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        phases
            .into_iter()
            .map(|(name, qs)| {
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
    // reading to identity (~45°/s, even steps), then go silent. The glide must not reach the output.
    #[test]
    fn hides_recentre_glide_during_long_holds() {
        let mut rb = Rebuilder::default();
        let mut phase_start = 0.0;
        let mut prev_phase = String::new();
        let mut held_back = 0;
        for Row { phase, t, seq, raw } in read_csv("hold_test.csv") {
            if phase != prev_phase {
                (phase_start, prev_phase) = (t, phase.clone());
            }
            let Some((abs, _)) = rb.push(t, seq, raw) else {
                held_back += 1;
                continue;
            };
            let pitch = abs.euler().pitch;
            if t - phase_start > 2.0 {
                match phase.as_str() {
                    "down" => assert!(pitch > 45.0, "t={t:.2} down hold emitted pitch {pitch:.1}"),
                    "center" => assert!(
                        pitch.abs() < 10.0,
                        "t={t:.2} center hold emitted pitch {pitch:.1}"
                    ),
                    _ => {}
                }
            }
        }
        // Four glides of ~1.25 s (~67 samples) each; nothing else should be held back.
        assert!(
            (220..=300).contains(&held_back),
            "held back {held_back} samples"
        );
    }

    // A real head lift after a still hold must still come through (possibly a little late).
    #[test]
    fn real_lift_after_hold_is_not_swallowed() {
        let mut rb = Rebuilder::default();
        let (mut t, mut seq) = (0.0, 0);
        let mut last = None;
        let mut push = |rb: &mut Rebuilder, t: &mut f64, q: Quat| {
            seq += 1;
            *t += 0.02;
            rb.push(*t, seq, q)
        };
        for _ in 0..200 {
            last = push(&mut rb, &mut t, pitch_quat(30.0)).or(last); // 4 s still at 30°
        }
        let mut p = 30.0;
        for step in [0.3, 0.8, 1.5, 2.5, 3.5, 4.0, 3.0, 2.0, 1.0, 0.5] {
            p -= step; // accelerating then decelerating lift, stops at 10.9°
            last = push(&mut rb, &mut t, pitch_quat(p)).or(last);
        }
        for _ in 0..50 {
            last = push(&mut rb, &mut t, pitch_quat(p)).or(last);
        }
        let pitch = last.unwrap().0.euler().pitch;
        assert!(
            (pitch - p).abs() < 0.5,
            "lift to {p:.1} reported as {pitch:.1}"
        );
    }

    // End to end on the real long-hold recording: each 30 s look-down hold must cue, and sitting
    // back up must register as recovery (session 1 failed this before glides were suppressed).
    #[test]
    fn long_holds_trigger_cues_end_to_end() {
        use crate::core::detector::{Config, Cue, Detector};
        let mut rb = Rebuilder::default();
        let mut det = Detector::new(Config::default());
        let mut cues: Vec<(Cue, String)> = Vec::new();
        let mut tick = 0.0;
        let mut cur = String::new(); // phase of the last sample: silence belongs to the pose before it
        for Row { phase, t, seq, raw } in read_csv("hold_test.csv") {
            while tick < t {
                cues.extend(det.tick(tick).into_iter().map(|c| (c, cur.clone()))); // 1 s ticks during silence
                tick += 1.0;
            }
            cur = phase.clone();
            if let Some((abs, _)) = rb.push(t, seq, raw) {
                // baseline = stream start (upright)
                cues.extend(
                    det.update(t, abs.euler().pitch)
                        .into_iter()
                        .map(|c| (c, phase.clone())),
                );
            }
        }
        // Speech (at 30 s) depends on exactly how long each ~30 s hold lasted, so it is not asserted.
        let key: Vec<(Cue, &str)> = cues
            .iter()
            .filter(|c| c.0 != Cue::Speak)
            .map(|(c, p)| (*c, p.as_str()))
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
        let mut rb = Rebuilder::default();
        let mut pitches = Vec::new();
        for r in read_csv("stream_restart.csv") {
            if let Some((abs, _)) = rb.push(r.t, r.seq, r.raw) {
                pitches.push((r.t, r.seq, abs.euler().pitch));
            }
        }
        let restart = pitches
            .windows(2)
            .position(|w| w[1].1 < w[0].1)
            .expect("recording has a restart")
            + 1;
        // Still looking down right after the restart, until the user starts to sit up (~25.6 s).
        for &(t, _, p) in pitches[restart..].iter().take_while(|(t, _, _)| *t < 25.6) {
            assert!(p > 40.0, "t={t:.2}: still looking down, but pitch {p:.1}");
        }
        let (t, _, end) = *pitches.last().unwrap();
        assert!(
            end.abs() < 10.0,
            "t={t:.2}: upright again, but pitch {end:.1}"
        );
    }

    // End to end on the stream-restart recording: one slouch, so exactly one tone and one recovery,
    // and the recovery only once the user actually sat up (~25.6 s), not at the restart.
    #[test]
    fn stream_restart_end_to_end() {
        use crate::core::detector::{Config, Cue, Detector};
        let mut rb = Rebuilder::default();
        let mut det = Detector::new(Config::default());
        let (mut cues, mut tick) = (Vec::new(), 0.0);
        for r in read_csv("stream_restart.csv") {
            while tick < r.t {
                cues.extend(det.tick(tick).into_iter().map(|c| (c, tick)));
                tick += 1.0;
            }
            if let Some((abs, _)) = rb.push(r.t, r.seq, r.raw) {
                cues.extend(
                    det.update(r.t, abs.euler().pitch)
                        .into_iter()
                        .map(|c| (c, r.t)),
                );
            }
        }
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
