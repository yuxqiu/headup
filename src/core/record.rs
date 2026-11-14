//! Everything a run reports:
//!
//! - messages and events on the console,
//! - an optional JSON-lines session log ([`Recorder`]),
//! - an optional JSON-lines log of raw device data ([`DeviceLog`]).
//!
//! Every log line starts with `t`, the wall-clock Unix time in seconds, to the millisecond. This
//! lets lines from both logs, from different runs and from the journal line up. Console lines
//! carry no time, because under systemd the journal stamps them.
//!
//! The live status line is redrawn in place with `\r`. It is shown only when stdout is a
//! terminal. Under systemd the journal gets one clean line per message instead.

use super::detector::Cue;
use super::quat::Euler;
use anyhow::Result;
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::Instant;

/// Wall-clock Unix time now, in seconds.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_secs_f64()
}

/// Wall-clock Unix time of the monotonic instant `at`, in seconds.
fn unix_time_of(at: Instant) -> f64 {
    unix_now() - at.elapsed().as_secs_f64()
}

/// Where a log goes: a file in use, a buffer in tests.
pub type LogSink = Box<dyn Write + Send>;

/// Append `{"t":<t>, <fields>}` to `file` as one JSON line, in a single write. `t` is Unix time.
fn write_line(file: &mut dyn Write, t: f64, fields: &impl Serialize) -> Result<()> {
    #[derive(Serialize)]
    struct Line<'a, T> {
        t: f64,
        #[serde(flatten)]
        fields: &'a T,
    }
    let mut line = serde_json::to_vec(&Line {
        t: round(t, 3),
        fields,
    })?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}

/// `x` rounded to `places` decimals, so log lines stay short.
fn round(x: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (x * scale).round() / scale
}

/// A session event, logged as `{"t":…,"event":"<variant>", <its fields>}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "event")]
pub enum LogEvent {
    /// The head kept moving on every calibration attempt; headup asks again later.
    CalibrationFailed,
    /// The device can deliver head tracking again.
    TrackingAvailable,
    /// The device can no longer deliver head tracking, for the tracker's `reason`. The posture
    /// reference is dropped, and headup recalibrates when tracking resumes.
    TrackingUnavailable { reason: &'static str },
    /// After a recovery the head settled above upright, so upright was re-anchored to this pitch
    /// (relative to calibration).
    Reanchored {
        #[serde(serialize_with = "two_places")]
        upright: f64,
    },
    /// A reminder cue was played.
    Cue { cue: Cue },
}

/// Serialise an angle rounded to two decimals, so log lines stay short.
fn two_places<S: serde::Serializer>(x: &f64, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_f64(round(*x, 2))
}

impl std::fmt::Display for LogEvent {
    /// The console line for the event. The reason for unavailable tracking is shown in the
    /// session's own note, so it is left out here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogEvent::CalibrationFailed => write!(f, "calibration failed; will ask again later"),
            LogEvent::TrackingAvailable => write!(f, "head tracking available"),
            LogEvent::TrackingUnavailable { .. } => write!(f, "head tracking unavailable"),
            LogEvent::Reanchored { upright } => write!(f, "upright re-anchored to {upright:.1}°"),
            LogEvent::Cue { cue } => write!(f, "cue: {cue:?}"),
        }
    }
}

/// An optional JSON-lines log. Each line is `{"t":…, <fields>}`; without a sink, lines are
/// dropped.
pub struct JsonLog(Option<LogSink>);

impl JsonLog {
    pub fn new(sink: Option<LogSink>) -> Self {
        JsonLog(sink)
    }

    /// Log `fields` at the current wall-clock time.
    pub fn record(&mut self, fields: &impl Serialize) -> Result<()> {
        self.record_at(unix_now(), fields)
    }

    /// Log `fields` at Unix time `t`.
    fn record_at(&mut self, t: f64, fields: &impl Serialize) -> Result<()> {
        match self.0.as_mut() {
            Some(sink) => write_line(sink, t, fields),
            None => Ok(()),
        }
    }
}

/// A backend's log of raw device data for debugging it (`--raw-log`), with fields the backend
/// chooses.
pub type DeviceLog = JsonLog;

pub struct Recorder {
    log: JsonLog,
    live: bool,
    /// A status line is on screen without a trailing newline.
    status_shown: bool,
}

impl Recorder {
    pub fn new(log: Option<LogSink>) -> Self {
        Recorder {
            log: JsonLog::new(log),
            live: std::io::stdout().is_terminal(),
            status_shown: false,
        }
    }

    /// End an on-screen status line so the next message starts on its own line.
    fn break_status(&mut self) {
        if self.status_shown {
            println!();
            self.status_shown = false;
        }
    }

    /// A message for the person running headup (stdout).
    pub fn note(&mut self, msg: &str) {
        self.break_status();
        println!("{msg}");
    }

    /// A problem that does not stop headup (stderr).
    pub fn warn(&mut self, msg: &str) {
        self.break_status();
        eprintln!("{msg}");
    }

    /// A session event that happens now. It is shown and logged.
    pub fn event(&mut self, event: LogEvent) -> Result<()> {
        self.note(&event.to_string());
        self.log.record(&event)
    }

    /// Head orientation relative to the calibrated upright pose, measured at `at`. It is logged,
    /// not shown. The line's `t` is the time of measurement.
    pub fn pose(&mut self, at: Instant, e: Euler) -> Result<()> {
        #[derive(Serialize)]
        struct Fields {
            pitch: f64,
            roll: f64,
            yaw: f64,
        }
        let fields = Fields {
            pitch: round(e.pitch, 2),
            roll: round(e.roll, 2),
            yaw: round(e.yaw, 2),
        };
        self.log.record_at(unix_time_of(at), &fields)
    }

    /// Overwrite the live status line (terminals only).
    pub fn status(&mut self, smoothed_pitch: Option<f64>, slouching: bool) -> Result<()> {
        if !self.live {
            return Ok(());
        }
        let pitch = smoothed_pitch.map_or("  --  ".into(), |p| format!("{p:5.1}°"));
        let state = if slouching { "SLOUCHING" } else { "ok       " };
        print!("\rpitch {pitch}  {state}   ");
        std::io::stdout().flush()?;
        self.status_shown = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::SharedBuf;
    use serde_json::{Value, json};

    /// Runs `write`, then checks every line it wrote starts with a wall-clock `t` taken during the
    /// write, and returns the lines without `t`.
    fn lines_without_t(write: impl FnOnce(LogSink)) -> Vec<Value> {
        let buf = SharedBuf::default();
        let before = unix_now();
        write(Box::new(buf.clone()));
        let after = unix_now();
        buf.text()
            .lines()
            .map(|line| {
                assert!(line.starts_with(r#"{"t":"#), "t is not first: {line}");
                let mut v: Value = serde_json::from_str(line).unwrap();
                let t = v.as_object_mut().unwrap().remove("t").unwrap();
                let t = t.as_f64().unwrap();
                assert!(
                    before - 0.001 <= t && t <= after + 0.001,
                    "t {t} not in [{before}, {after}]"
                );
                v
            })
            .collect()
    }

    #[test]
    fn session_log_lines() {
        let lines = lines_without_t(|f| {
            let mut rec = Recorder::new(Some(f));
            rec.pose(
                Instant::now(),
                Euler {
                    roll: 1.0,
                    pitch: 25.504,
                    yaw: -3.0,
                },
            )
            .unwrap();
            rec.event(LogEvent::Cue { cue: Cue::Tone }).unwrap();
            rec.event(LogEvent::Reanchored { upright: -3.6049 })
                .unwrap();
            rec.event(LogEvent::TrackingUnavailable {
                reason: "play audio",
            })
            .unwrap();
        });
        assert_eq!(
            lines,
            [
                json!({"pitch": 25.5, "roll": 1.0, "yaw": -3.0}),
                json!({"event": "Cue", "cue": "Tone"}),
                json!({"event": "Reanchored", "upright": -3.6}),
                json!({"event": "TrackingUnavailable", "reason": "play audio"}),
            ]
        );
    }

    #[test]
    fn device_log_records_backend_fields() {
        #[derive(Serialize)]
        struct Sample {
            seq: u32,
            name: &'static str,
        }
        let lines = lines_without_t(|f| {
            DeviceLog::new(Some(f))
                .record(&Sample {
                    seq: 7,
                    name: "a \"quoted\" name",
                })
                .unwrap();
        });
        assert_eq!(lines, [json!({"seq": 7, "name": "a \"quoted\" name"})]);
        DeviceLog::new(None)
            .record(&Sample { seq: 8, name: "" })
            .unwrap(); // dropped
    }
}
