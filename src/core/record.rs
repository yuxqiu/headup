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

/// Everything that appears as `{"t":…,"event":"<name>"}` in the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogEvent {
    /// Earbuds no longer worn well enough for tracking: monitoring paused.
    Paused,
    /// Earbuds worn again: monitoring resumes after recalibration.
    Resumed,
    /// The head kept moving on every calibration attempt; headup asks again later.
    CalibrationFailed,
    /// The device can deliver head tracking again.
    TrackingAvailable,
    /// The device can no longer deliver head tracking. The posture reference is dropped, and
    /// headup recalibrates when tracking resumes.
    TrackingUnavailable,
    /// After a recovery the head settled above upright, so upright was re-anchored to this pitch
    /// (relative to calibration).
    Reanchored { upright: f64 },
    /// A reminder cue was played.
    Cue(Cue),
}

impl LogEvent {
    /// The event's name and, for `Reanchored`, the new upright.
    fn parts(self) -> (&'static str, Option<f64>) {
        let name = match self {
            LogEvent::Paused => "Paused",
            LogEvent::Resumed => "Resumed",
            LogEvent::CalibrationFailed => "CalibrationFailed",
            LogEvent::TrackingAvailable => "TrackingAvailable",
            LogEvent::TrackingUnavailable => "TrackingUnavailable",
            LogEvent::Reanchored { upright } => return ("Reanchored", Some(upright)),
            LogEvent::Cue(Cue::Tone) => "Tone",
            LogEvent::Cue(Cue::Speak) => "Speak",
            LogEvent::Cue(Cue::Notify) => "Notify",
            LogEvent::Cue(Cue::Recovered) => "Recovered",
        };
        (name, None)
    }
}

pub struct Recorder {
    log: Option<LogSink>,
    live: bool,
    /// A status line is on screen without a trailing newline.
    status_shown: bool,
}

impl Recorder {
    pub fn new(log: Option<LogSink>) -> Self {
        Recorder {
            log,
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
        #[derive(Serialize)]
        struct Fields {
            event: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            upright: Option<f64>,
        }
        let (name, upright) = event.parts();
        let shown = upright.map_or(String::new(), |u| format!(" (upright {u:.1}°)"));
        self.note(&format!("{name}{shown}"));
        if let Some(f) = self.log.as_mut() {
            let fields = Fields {
                event: name,
                upright: upright.map(|u| round(u, 2)),
            };
            write_line(f, unix_now(), &fields)?;
        }
        Ok(())
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
        if let Some(f) = self.log.as_mut() {
            let fields = Fields {
                pitch: round(e.pitch, 2),
                roll: round(e.roll, 2),
                yaw: round(e.yaw, 2),
            };
            write_line(f, unix_time_of(at), &fields)?;
        }
        Ok(())
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

/// A backend's log of raw device data for debugging it (`--raw-log`). Each record is one JSON line,
/// `{"t":…, <fields>}`, with fields the backend chooses. Without a file, records are dropped.
pub struct DeviceLog {
    file: Option<LogSink>,
}

impl DeviceLog {
    pub fn new(file: Option<LogSink>) -> Self {
        DeviceLog { file }
    }

    pub fn record(&mut self, fields: &impl Serialize) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            write_line(f, unix_now(), fields)?;
        }
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
            rec.event(LogEvent::Cue(Cue::Tone)).unwrap();
            rec.event(LogEvent::Reanchored { upright: -3.6049 })
                .unwrap();
        });
        assert_eq!(
            lines,
            [
                json!({"pitch": 25.5, "roll": 1.0, "yaw": -3.0}),
                json!({"event": "Tone"}),
                json!({"event": "Reanchored", "upright": -3.6}),
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
