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
//! While the user walks around, the earbuds also **hold**: an activity detector on the accelerometer
//! (`0x600fab40`) makes them move the reference onto the head at every sample, so the reading jumps
//! to identity with no glide and stays there, the stream pauses, and it resumes relative to
//! wherever the head is once the user has been quiet (FINDINGS "Instant resets while walking"). The
//! head movement in between never reaches the stream, so a hold cannot be undone. The rebuilder
//! reports where one starts ([`Rebuilt::HoldStarted`]); what that means for the session is decided
//! by the link, which keeps the earbuds' state.
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
//! - An identity sample that no hypothesis explains, with nothing held (no glide before it), starts
//!   a hold ([`Rebuilt::HoldStarted`]).

use crate::core::quat::Quat;
use std::collections::VecDeque;
use std::time::Duration;

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
    /// See [`Rebuilt::HoldStarted`].
    HoldStarted(u32),
}

/// What an arriving sample means, given the one before it.
enum Arrival {
    /// The sequence number went backwards: a new stream, measured from the head pose at its start.
    NewStream,
    /// An exact identity sample after a non-identity one: a glide has ended, or a hold started.
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
        let q = if q.w < 0.0 { -q } else { q };
        let v = (q.x * q.x + q.y * q.y + q.z * q.z).sqrt();
        let a = v.atan2(q.w);
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
                x: s * q.x,
                y: s * q.y,
                z: s * q.z,
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
    /// The hypotheses `h` starts. `h` could be the pose a glide starts from. If the `unsent`
    /// samples just before it never arrived, a glide may also have started on one of them, with `h`
    /// as its `k`-th sample (`k <= unsent`): one hypothesis per possible `k`. (A glide that started
    /// on a sample that did arrive already has a hypothesis.) Those start before `h`, so `h` waits
    /// until the glide is decided; the wrong `k`s die at the next sample.
    ///
    /// Nothing starts when `h` is within `FIT_TOL_DEG` of identity: a glide from there moves the
    /// reading by less than the tolerance. It cannot be told from holding still, and it does not
    /// matter whether it comes out.
    fn start(&mut self, h: &Held, unsent: u32) {
        if h.raw.angle_to(Quat::IDENTITY) <= FIT_TOL_DEG {
            return;
        }
        // Every live hypothesis starts before the unsent samples, so pushing the starts from
        // oldest to newest keeps the list in order.
        let ks = (1..=unsent.min(GLIDE_LEN - 1)).rev();
        self.0.extend(ks.filter_map(|k| Glide::through(h, k)));
        self.0.push(Glide {
            seq: h.seq,
            from: h.raw,
        });
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
        // Handle what the sample means, and count the samples the earbuds did not send just
        // before it.
        let unsent = match self.last.map(|prev| (prev, Arrival::of(&prev, &sample))) {
            // The first sample: every earlier one in its stream (streams count from 1).
            None => sample.seq.saturating_sub(1),
            Some((_, Arrival::NewStream)) => {
                // A glide in progress is dropped: the head was still at its start pose.
                if let Some(start) = self.glides.earliest_start() {
                    self.drop_after(start);
                }
                steps.extend(self.release_all());
                steps.push(Step::Rebase);
                sample.seq.saturating_sub(1)
            }
            Some((prev, Arrival::GlideEnd)) => {
                self.end_glide(&prev, &sample, &mut steps);
                sample.seq.saturating_sub(prev.seq + 1)
            }
            Some((prev, Arrival::Sample)) => {
                self.glides.observe(&sample);
                sample.seq.saturating_sub(prev.seq + 1) // 0 for a repeated sequence number
            }
        };
        self.last = Some(sample);
        self.held.push_back(sample);
        self.glides.start(&sample, unsent);
        let n = self.releasable();
        steps.extend(self.release_first(n));
        steps
    }

    /// The identity sample `end`, after `prev`, ends a glide: drop the glide's held samples,
    /// release the rest, and move the reference to the last released pose. With no glide before
    /// it, it starts a hold.
    fn end_glide(&mut self, prev: &Held, end: &Held, steps: &mut Vec<Step>) {
        match self.glides.ending_at(end) {
            // A full glide: the hypothesis that predicts this identity sample started it.
            Some(start) => self.drop_after(start),
            // No hypothesis predicts it: the earbuds cut the glide short because the head moved.
            None => {
                // If nothing is held, no glide came before it: the earbuds started a hold. Unless
                // the last reading was too small for a glide to be tracked (within `FIT_TOL_DEG`
                // of identity).
                if self.held.is_empty() && prev.raw.angle_to(Quat::IDENTITY) > FIT_TOL_DEG {
                    steps.push(Step::HoldStarted(end.seq));
                }
                // Otherwise the glide's hypothesis is still alive and is the oldest: drop what
                // is held after its start.
                if let Some(start) = self.glides.earliest_start() {
                    self.drop_after(start);
                }
            }
        }
        steps.extend(self.release_all());
        steps.push(Step::Rebase);
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
                Step::HoldStarted(seq) => out.push(Rebuilt::HoldStarted { seq }),
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
    /// The identity sample `seq` started a hold (see the module docs). Poses after the hold are
    /// relative to an unknown pose, so the orientation frame before the hold no longer applies.
    HoldStarted { seq: u32 },
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
