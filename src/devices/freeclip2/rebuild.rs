//! Head orientation from the FreeClip 2 43/89 stream.
//!
//! The earbuds report each sample relative to a reference pose. When the head holds still for 208
//! samples (within 10° on every axis), they move the reference to that pose. The reading does not
//! jump: it glides to identity over the next `GLIDE_LEN` samples. With `q_s` the pose held before
//! the glide, the `k`-th glide sample reads `nlerp(I, q_s, (GLIDE_LEN − k) / (GLIDE_LEN + 1))`, and
//! sample `GLIDE_LEN` is exactly identity. This is the firmware's own formula, and recorded glides
//! match it within 0.0002° (docs/devices/freeclip2/FINDINGS.md "Re-centre and stillness"). It
//! counts samples, so it holds at any sample rate.
//!
//! A glide is not head movement, so it must not come out. `Rebuilder` has two parts:
//!
//! - [`GlideFilter`] decides what happens to each raw sample: it comes out, or it is dropped as
//!   part of a glide. It also says when the earbuds moved their reference. It hands these decisions
//!   on as an ordered list of [`Step`]s.
//! - [`Stitch`] applies the steps. It turns each released sample into an absolute orientation (`raw
//!   × reference`). At each `Step::Rebase` the reference becomes the last absolute orientation,
//!   because that is where the earbuds moved their reference.
//!
//! How the filter decides:
//!
//! - Any sample could be the pose a glide starts from, so each one starts a hypothesis
//!   ([`Hypotheses`]). A hypothesis lives while every later sample matches the glide formula within
//!   `FIT_TOL_DEG`. Head movement does not match, so most hypotheses die at the next sample. A real
//!   glide matches until its identity sample: during a glide the readings do not depend on the
//!   head.
//! - Samples up to the start of the oldest live hypothesis come out at once. Later samples might be
//!   part of a glide, so they wait for it to be decided, however long the stream is silent.
//! - When an identity sample ends a glide, the glide's samples are dropped, the rest come out, and
//!   the reference moves.
//! - The earbuds sometimes do not send the start of a glide: they resume after a gap, or start a
//!   new stream, in the middle of one. So after a gap, a sample also starts hypotheses of the form
//!   "this is step `k` of a glide that started in the gap".
//! - When a new stream starts (the sequence number goes backwards), a glide in progress is dropped
//!   (the head was still), the rest come out, and the reference moves to the last orientation.
//! - An identity sample that no hypothesis explains is reported ([`Rebuilt::MissedGlide`]).

use crate::core::quat::Quat;
use std::collections::VecDeque;
use std::time::Duration;

/// When an identity sample ends a glide the earbuds cut short, the held samples from this many
/// samples before it are dropped. A little more than a glide.
const GLIDE_SAMPLES: u32 = 70;
/// Samples from the pose held before a glide to its identity sample.
const GLIDE_LEN: u32 = 68;
/// How far a sample may be from the glide formula and still match it. The firmware computes glides
/// in single precision, so glide samples are off by about 1e-5°; the worst in the recordings is
/// 0.00015° (FINDINGS "Re-centre and stillness"). Head movement is much farther off.
const FIT_TOL_DEG: f64 = 0.001;

/// A raw sample, received at `t` after the caller's time zero.
#[derive(Clone, Copy)]
struct Held {
    t: Duration,
    seq: u32,
    raw: Quat,
}

/// A decision of the [`GlideFilter`], applied in order by the [`Stitch`].
enum Step {
    /// This sample comes out.
    Release(Held),
    /// The earbuds' reference has moved to where the head was at the last released sample.
    Rebase,
    /// See [`Rebuilt::MissedGlide`].
    MissedGlide(u32),
}

/// What an arriving sample means, given the one before it.
enum Arrival {
    /// The sequence number went backwards: a new stream, measured from the head pose at its start.
    NewStream,
    /// An exact identity sample after a non-identity one: a glide has ended.
    GlideEnd,
    /// Any other sample.
    Sample,
}

impl Arrival {
    fn of(prev: &Held, now: &Held) -> Arrival {
        if now.seq < prev.seq {
            Arrival::NewStream
        } else if now.raw == Quat::IDENTITY && prev.raw != Quat::IDENTITY {
            Arrival::GlideEnd
        } else {
            Arrival::Sample
        }
    }
}

/// Sample `k` of a glide from the held pose `q`, by the firmware's formula (`0x600faa4e`, FINDINGS
/// "Re-centre and stillness"): `normalize(t·q + (1 − t)·I)` with `t = (GLIDE_LEN − k) /
/// (GLIDE_LEN + 1)`. At `k = GLIDE_LEN` it is identity.
fn glide_point(q: Quat, k: u32) -> Quat {
    assert!(
        k <= GLIDE_LEN,
        "glide step {k} beyond its {GLIDE_LEN} samples"
    );
    let t = f64::from(GLIDE_LEN - k) / f64::from(GLIDE_LEN + 1);
    let s = if q.w < 0.0 { -t } else { t };
    Quat {
        w: s * q.w + (1.0 - t),
        x: s * q.x,
        y: s * q.y,
        z: s * q.z,
    }
    .normalized()
}

/// A hypothesis: a glide started at sample `seq`, from pose `from`.
#[derive(Clone, Copy)]
struct Glide {
    seq: u32,
    from: Quat,
}

impl Glide {
    /// The glide whose `k`-th sample is `h`, if one exists. It starts at sample `h.seq − k`, so
    /// `k < h.seq`. Its start pose turns about the same axis as `h`, only farther. With `a` the
    /// half-angle of `h` and `b` that of the start pose, the glide formula gives
    /// `t·sin(b − a) = (1 − t)·sin a`.
    fn through(h: &Held, k: u32) -> Option<Glide> {
        let q = h.raw.normalized();
        // `q` and `-q` are the same rotation; take the one with `w >= 0`.
        let sign = if q.w < 0.0 { -1.0 } else { 1.0 };
        let (w, x, y, z) = (sign * q.w, sign * q.x, sign * q.y, sign * q.z);
        let v = (x * x + y * y + z * z).sqrt();
        let a = v.atan2(w);
        let t = f64::from(GLIDE_LEN - k) / f64::from(GLIDE_LEN + 1);
        let sin_ba = (1.0 - t) * a.sin() / t;
        if v == 0.0 || sin_ba > 1.0 {
            return None; // no glide passes through `h` at this step
        }
        let b = a + sin_ba.asin();
        // The firmware flips a glide's start pose to `w >= 0` (FINDINGS), so the start pose turns
        // by at most 180°: half-angle at most 90°.
        if b > std::f64::consts::FRAC_PI_2 {
            return None;
        }
        let s = b.sin() / v;
        Some(Glide {
            seq: h.seq - k,
            from: Quat {
                w: b.cos(),
                x: s * x,
                y: s * y,
                z: s * z,
            },
        })
    }

    /// Whether sample `h` matches this glide.
    fn fits(&self, h: &Held) -> bool {
        let k = h.seq - self.seq;
        k <= GLIDE_LEN && h.raw.angle_to(glide_point(self.from, k)) <= FIT_TOL_DEG
    }
}

/// The live hypotheses, oldest start first.
#[derive(Default)]
struct Hypotheses(Vec<Glide>);

impl Hypotheses {
    /// `h` could be the pose a glide starts from. Not when `h` is within `FIT_TOL_DEG` of identity:
    /// a glide from there moves the reading by less than the tolerance. It cannot be told from
    /// holding still, and it does not matter whether it comes out.
    fn start(&mut self, h: &Held) {
        if h.raw.angle_to(Quat::IDENTITY) > FIT_TOL_DEG {
            self.0.push(Glide {
                seq: h.seq,
                from: h.raw,
            });
        }
    }

    /// The `unsent` samples just before `h` never arrived. A glide may have started on one of them,
    /// with `h` as its `k`-th sample (`k <= unsent`), so add one hypothesis per possible `k`. (A
    /// glide that started on a sample that did arrive already has a hypothesis.) These hypotheses
    /// start before `h`, so `h` waits until the glide is decided. The wrong `k`s die at the next
    /// sample.
    fn start_unseen(&mut self, h: &Held, unsent: u32) {
        if h.raw.angle_to(Quat::IDENTITY) > FIT_TOL_DEG {
            let ks = 1..=unsent.min(GLIDE_LEN - 1);
            self.0.extend(ks.filter_map(|k| Glide::through(h, k)));
            self.0.sort_by_key(|g| g.seq); // oldest start first
        }
    }

    /// Keep only the hypotheses that `h` fits.
    fn observe(&mut self, h: &Held) {
        self.0.retain(|g| g.fits(h));
    }

    /// Where the oldest live hypothesis starts (a sequence number).
    fn earliest_start(&self) -> Option<u32> {
        self.0.first().map(|g| g.seq)
    }

    /// The start of the full glide that the identity sample `end` ends: the live hypothesis that
    /// predicts identity at `end`. If several do, the one closest to `GLIDE_LEN` long.
    fn ending_at(&self, end: &Held) -> Option<u32> {
        self.0
            .iter()
            .filter(|g| g.fits(end))
            .map(|g| g.seq)
            .min_by_key(|&s| (end.seq - s).abs_diff(GLIDE_LEN))
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Decides which raw samples come out, which are dropped, and where the reference moves.
#[derive(Default)]
struct GlideFilter {
    /// The last arrived sample.
    last: Option<Held>,
    /// Samples not yet out, oldest first.
    held: VecDeque<Held>,
    glides: Hypotheses,
}

impl GlideFilter {
    fn push(&mut self, sample: Held) -> Vec<Step> {
        let mut steps = Vec::new();
        match self.last.map(|prev| Arrival::of(&prev, &sample)) {
            Some(Arrival::NewStream) => {
                // A glide in progress is dropped: the head was still at its start pose.
                if let Some(start) = self.glides.earliest_start() {
                    self.drop_after(start);
                }
                steps.extend(self.release_all());
                steps.push(Step::Rebase);
            }
            Some(Arrival::GlideEnd) => {
                // The identity sample ends a glide: drop the glide's held samples, release the
                // rest, and move the reference to the last released pose.
                let start = match self.glides.ending_at(&sample) {
                    // A full glide: the hypothesis that predicts this identity sample started it.
                    Some(start) => start,
                    // No hypothesis predicts it: the earbuds cut the glide short because the head
                    // moved. The glide's hypothesis is still alive and holds its samples, so drop
                    // everything held from the last `GLIDE_SAMPLES`.
                    None => {
                        // If nothing is held, the glide's samples have already come out: the model
                        // missed the glide. Unless it was too small to track (the last sample
                        // within `FIT_TOL_DEG` of identity).
                        let prev = self.last.expect("a glide end follows a sample");
                        if self.held.is_empty() && prev.raw.angle_to(Quat::IDENTITY) > FIT_TOL_DEG {
                            steps.push(Step::MissedGlide(sample.seq));
                        }
                        sample.seq.saturating_sub(GLIDE_SAMPLES)
                    }
                };
                self.drop_after(start);
                steps.extend(self.release_all());
                steps.push(Step::Rebase);
            }
            Some(Arrival::Sample) => self.glides.observe(&sample),
            None => {}
        }
        // How many samples the earbuds did not send just before this one. In the same stream: the
        // gap in sequence numbers. At the first sample or a new stream: all earlier ones (streams
        // count from 1).
        let unsent = match self.last {
            Some(prev) if sample.seq >= prev.seq => sample.seq.saturating_sub(prev.seq + 1),
            _ => sample.seq.saturating_sub(1),
        };
        self.last = Some(sample);
        self.held.push_back(sample);
        if unsent > 0 {
            self.glides.start_unseen(&sample, unsent);
        }
        self.glides.start(&sample);
        let n = self.releasable();
        steps.extend(self.release_first(n));
        steps
    }

    /// How many held samples, from the oldest, can come out: those up to and including the start
    /// of the oldest live hypothesis. None of them can be part of an unfinished glide.
    fn releasable(&self) -> usize {
        let start = self.glides.earliest_start().unwrap_or(u32::MAX);
        self.held.iter().take_while(|h| h.seq <= start).count()
    }

    /// Drop the held samples after sequence number `start` (the glide's start), and forget every
    /// hypothesis.
    fn drop_after(&mut self, start: u32) {
        while self.held.back().is_some_and(|h| h.seq > start) {
            self.held.pop_back();
        }
        self.glides.clear();
    }

    fn release_first(&mut self, n: usize) -> Vec<Step> {
        self.held.drain(..n).map(Step::Release).collect()
    }

    fn release_all(&mut self) -> Vec<Step> {
        self.release_first(self.held.len())
    }
}

/// Turns released samples into absolute orientations: `absolute = raw × reference`. The earbuds
/// report `raw = q ⊗ conj(ref)` (firmware `0x600fa53c`, FINDINGS "Re-centre and stillness"), so
/// the reference goes on the right. `reference` is the earbuds' reference pose, in the frame of the
/// stream start. At a `Step::Rebase` it becomes the last absolute orientation, because that is
/// where the earbuds just moved it.
struct Stitch {
    reference: Quat,
    last_abs: Quat,
}

impl Stitch {
    /// Apply the filter's steps in order.
    fn apply(&mut self, steps: Vec<Step>) -> Vec<Rebuilt> {
        let mut out = Vec::new();
        for step in steps {
            match step {
                Step::Release(h) => {
                    self.last_abs = h.raw * self.reference;
                    out.push(Rebuilt::Pose {
                        at: h.t,
                        orientation: self.last_abs,
                    });
                }
                Step::Rebase => self.reference = self.last_abs,
                Step::MissedGlide(seq) => out.push(Rebuilt::MissedGlide { seq }),
            }
        }
        out
    }
}

/// What the rebuilder hands out, in order.
#[derive(Debug, PartialEq)]
pub enum Rebuilt {
    /// A released sample: received at `at`, with its absolute orientation (relative to the stream
    /// start).
    Pose { at: Duration, orientation: Quat },
    /// The identity sample `seq` ended a glide that no hypothesis predicted, and none of its
    /// samples were held. The model missed it, so that glide's samples came out as head movement.
    MissedGlide { seq: u32 },
}

/// See the module docs.
pub struct Rebuilder {
    filter: GlideFilter,
    stitch: Stitch,
}

impl Default for Rebuilder {
    fn default() -> Self {
        Rebuilder {
            filter: GlideFilter::default(),
            stitch: Stitch {
                reference: Quat::IDENTITY,
                last_abs: Quat::IDENTITY,
            },
        }
    }
}

impl Rebuilder {
    /// Feed one raw sample, received at `t` after the caller's time zero. Returns what it releases,
    /// in order.
    pub fn push(&mut self, t: Duration, seq: u32, raw: Quat) -> Vec<Rebuilt> {
        let steps = self.filter.push(Held { t, seq, raw });
        self.stitch.apply(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quat::{Euler, Quat};
    use crate::devices::freeclip2::tests::replay::{Row, phase_at, phases, read_csv, replay, secs};

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

    /// A sample of `pitch` with sequence number `seq`, received at `t`.
    fn sample(t: f64, seq: u32, pitch: f64) -> Row {
        let raw = if pitch == 0.0 {
            Quat::IDENTITY
        } else {
            pitch_quat(pitch)
        };
        row(t, seq, raw)
    }

    // A re-centre after a ~90° turn (e.g. to a side monitor), with glide samples from the
    // firmware's formula. The head settles slowly for 220 samples (0.03° per sample, 6.6° in all).
    // That stays inside the firmware's 10° stillness box, so the earbuds re-centre after 208
    // samples. Only the last pose starts the glide that follows. The rebuilt yaw must stay where
    // the head settled.
    #[test]
    fn hides_glide_from_a_large_angle() {
        let yaw = |deg: f64| rotation(deg, (0.0, 0.0, 1.0));
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        let settle = |s: u32| 83.0 + 0.03 * f64::from(s);
        let mut rows: Vec<Row> = (1..=220).map(|s| row(s, yaw(settle(s)))).collect();
        let held = yaw(settle(220));
        rows.extend((1..=68).map(|k| row(220 + k, firmware_glide(held, k))));
        // After the re-centre the head stays where it settled, so the earbuds read identity.
        rows.extend((1..=80).map(|i| row(220 + 68 + i, Quat::IDENTITY)));
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
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        let turned = yaw(60.0);
        let mut rows: Vec<Row> = (1..=60).map(|s| row(s, yaw(f64::from(s)))).collect();
        rows.extend((61..=280).map(|s| row(s, turned)));
        rows.extend((1..=68).map(|k| row(280 + k, firmware_glide(turned, k))));
        // The head is still at the new reference, so the earbuds send nothing for a while. The
        // sequence number keeps counting.
        let nod = |deg: f64| turned * pitch(deg) * turned.conj();
        rows.extend((1..=30).map(|i| row(400 + i, nod(f64::from(i)))));
        rows.extend((1..=100).map(|i| row(430 + i, nod(30.0))));
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
        let hz = 35.0;
        let mut rows = Vec::new();
        let mut seq = 1;
        let mut t = 0.0;
        let mut add = |rows: &mut Vec<Row>, t: f64, raw: Quat| {
            rows.push(row(t, seq, raw));
            seq += 1;
        };
        let held = pitch_quat(30.0);
        for _ in 0..105 {
            add(&mut rows, t, held); // 3 s held at 30°
            t += 1.0 / hz;
        }
        for k in 1..=67 {
            add(&mut rows, t, firmware_glide(held, k)); // the glide
            t += 1.0 / hz;
            if k == 40 {
                t += 3.0; // transport stall, as seen early in streams
            }
        }
        add(&mut rows, t, Quat::IDENTITY); // identity: the glide ends
        t += 6.0; // head still: the earbuds go quiet
        for _ in 0..100 {
            add(&mut rows, t, Quat::IDENTITY); // relative to the new reference: still 30°
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
        // Exactly the glide's 67 samples are dropped. Every still sample before it comes out.
        assert_eq!(rows.len() - out.len(), 67, "the glide is dropped");
    }

    /// Push `(seq, pitch)` samples at 50 Hz from t = 0; returns, for each, how many samples it
    /// released.
    fn push_all(rb: &mut Rebuilder, samples: &[(u32, f64)]) -> Vec<usize> {
        samples
            .iter()
            .enumerate()
            .map(|(i, &(seq, pitch))| {
                let t = Duration::from_millis(20 * i as u64);
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
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        let held = pitch_quat(53.0);
        let mut rows: Vec<Row> = (1..=100).map(|s| row(s, held)).collect();
        rows.extend((1..=43).map(|k| row(100 + k, firmware_glide(held, k))));
        rows.push(row(144, Quat::IDENTITY)); // cut short
        let out = replay(&rows);
        assert_eq!(rows.len() - out.len(), 43, "only the glide is dropped");
        assert!(out.iter().all(|o| (o.abs.euler().pitch - 53.0).abs() < 0.5));
    }

    /// The missed glides reported while pushing `rows`.
    fn missed(rows: &[Row]) -> Vec<u32> {
        let mut rb = Rebuilder::default();
        rows.iter()
            .flat_map(|r| rb.push(r.t, r.seq, r.raw))
            .filter_map(|out| match out {
                Rebuilt::MissedGlide { seq } => Some(seq),
                Rebuilt::Pose { .. } => None,
            })
            .collect()
    }

    // A glide whose readings leave the formula partway through is reported as missed, whatever its
    // size.
    #[test]
    fn glide_off_the_model_is_reported() {
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        for deg in [10.0, 20.0, 30.0, 40.0] {
            let held = pitch_quat(deg);
            let mut rows: Vec<Row> = (1..=100).map(|s| row(s, held)).collect();
            rows.extend((1..=67).map(|k| {
                let q = firmware_glide(held, k);
                row(100 + k, if k == 30 { q * pitch_quat(0.2) } else { q })
            }));
            rows.push(row(168, Quat::IDENTITY));
            assert_eq!(missed(&rows), [168], "glide from {deg}°");
        }
    }

    /// The head held still at 30° for 100 samples. Then a glide from `start` (relative to the
    /// earbuds' reference), of which the earbuds send only step 16 onwards, numbered from
    /// `first_seq`. The glide ends with its identity sample, and the head stays still.
    fn glide_sent_from_its_middle(start: Quat, first_seq: u32) -> Vec<Row> {
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        let mut rows: Vec<Row> = (1..=100).map(|s| row(s, pitch_quat(30.0))).collect();
        let k0 = 16;
        rows.extend((k0..=68).map(|k| row(first_seq + k - k0, firmware_glide(start, k))));
        let end = first_seq + 68 - k0;
        rows.extend((1..=30).map(|i| row(end + i, Quat::IDENTITY)));
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
            assert_eq!(missed(&rows), Vec::<u32>::new(), "{what}");
        }
    }

    // A slow slouch: the head moves 0.2° from the reference and holds still until the earbuds
    // re-centre (209 samples, then the glide), ten times over. Small glides are hidden like large
    // ones, so the ten steps add up to 2°.
    #[test]
    fn small_recentres_keep_the_pose() {
        let row = |seq: u32, raw: Quat| row(f64::from(seq) / 52.5, seq, raw);
        let (mut rows, mut seq) = (Vec::new(), 0);
        for _ in 0..10 {
            let step = pitch_quat(0.2); // the head moved 0.2° from the earbuds' reference
            for _ in 0..209 {
                seq += 1;
                rows.push(row(seq, step));
            }
            for k in 1..=68 {
                seq += 1;
                rows.push(row(seq, firmware_glide(step, k)));
            }
        }
        let pitch = replay(&rows).last().expect("poses").abs.euler().pitch;
        assert!(
            (pitch - 2.0).abs() < 0.01,
            "ten 0.2° steps read {pitch:.3}°"
        );
    }

    /// Every real recording in tests/fixtures/freeclip2/.
    const RECORDINGS: [&str; 14] = [
        "headpose_axes.csv",
        "headpose_axes_fixed.csv",
        "hold_test.csv",
        "short_hold_glide.csv",
        "stream_restart.csv",
        "glide_move.csv",
        "live_35hz.csv",
        "slouch_guided.csv",
        "session4.csv",
        "session6.csv",
        "session7.csv",
        "live4.csv",
        "live6.csv",
        "gap_live.csv",
    ];

    // Every real recording follows the glide formula, including glides cut short. The one report is
    // live4.csv seq 34: an identity sample with no glide before it, after a wear flicker (FINDINGS
    // "Re-centre and stillness").
    #[test]
    fn real_glides_are_not_reported_as_missed() {
        for name in RECORDINGS {
            let want: &[u32] = if name == "live4.csv" { &[34] } else { &[] };
            assert_eq!(missed(&read_csv(name)), want, "{name}");
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
                let p = if p.w < 0.0 {
                    p * Quat {
                        w: -1.0,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    }
                } else {
                    p
                };
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
        for name in RECORDINGS {
            // Make receive times unique (the row index), to see which rows came out.
            let rows: Vec<Row> = read_csv(name)
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
}
