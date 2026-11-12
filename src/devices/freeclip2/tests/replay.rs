//! Replays recorded 43/89 samples (tests/fixtures/freeclip2/*.csv) through a [`Rebuilder`], for
//! tests.

use crate::core::quat::Quat;
use crate::devices::freeclip2::rebuild::{DELAY_S, Rebuilder};

/// One recorded 43/89 sample; `phase` is the guided-test step ("" if the recording has none).
pub struct Row {
    pub phase: String,
    pub t: f64,
    pub seq: u32,
    pub raw: Quat,
}

pub fn read_csv(name: &str) -> Vec<Row> {
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
pub struct Out {
    pub t: f64,
    pub now: f64,
    pub abs: Quat,
}

/// Replay a recording through a `Rebuilder`, then release everything still held.
pub fn replay(rows: &[Row]) -> Vec<Out> {
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
        out.extend(
            rb.push(r.t, r.seq, r.raw)
                .into_iter()
                .map(|(t, abs)| Out { t, now: r.t, abs }),
        );
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
pub fn phases(rows: &[Row]) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = Vec::new();
    for r in rows {
        if v.last().is_none_or(|p| p.0 != r.phase) {
            v.push((r.phase.clone(), r.t));
        }
    }
    v
}

/// Index into `phases` of the phase a sample measured at `t` belongs to.
pub fn phase_at(phases: &[(String, f64)], t: f64) -> usize {
    phases.partition_point(|p| p.1 <= t).saturating_sub(1)
}
