//! Everything a run reports: messages and events on the console, and an optional JSON-lines log.
//!
//! The live status line (`\r`-updated) is shown only when stdout is a terminal; under systemd the
//! journal gets one clean line per message instead.

use super::quat::Euler;
use anyhow::Result;
use std::fs::File;
use std::io::{IsTerminal, Write};

pub struct Recorder {
    log: Option<File>,
    live: bool,
    /// A status line is on screen without a trailing newline.
    status_shown: bool,
}

impl Recorder {
    pub fn new(log: Option<File>) -> Self {
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

    /// A session event (cue, pause, ...) at time `t`: shown and logged.
    pub fn event(&mut self, t: f64, name: &str) -> Result<()> {
        self.note(&format!("{t:8.1}s  {name}"));
        if let Some(f) = self.log.as_mut() {
            writeln!(f, r#"{{"t":{t:.3},"event":"{name}"}}"#)?;
        }
        Ok(())
    }

    /// Head orientation relative to the calibrated upright pose at time `t`: logged only.
    pub fn pose(&mut self, t: f64, e: Euler) -> Result<()> {
        if let Some(f) = self.log.as_mut() {
            writeln!(
                f,
                r#"{{"t":{t:.3},"pitch":{:.2},"roll":{:.2},"yaw":{:.2}}}"#,
                e.pitch, e.roll, e.yaw
            )?;
        }
        Ok(())
    }

    /// Overwrite the live status line (terminals only).
    pub fn status(&mut self, t: f64, smoothed_pitch: Option<f64>, slouching: bool) -> Result<()> {
        if !self.live {
            return Ok(());
        }
        let pitch = smoothed_pitch.map_or("  --  ".into(), |p| format!("{p:5.1}°"));
        let state = if slouching { "SLOUCHING" } else { "ok       " };
        print!("\r{t:8.1}s  pitch {pitch}  {state}   ");
        std::io::stdout().flush()?;
        self.status_shown = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_are_json() {
        let path =
            std::env::temp_dir().join(format!("headup-record-test-{}.jsonl", std::process::id()));
        let mut rec = Recorder::new(Some(File::create(&path).unwrap()));
        rec.pose(
            1.0,
            Euler {
                roll: 1.0,
                pitch: 25.5,
                yaw: -3.0,
            },
        )
        .unwrap();
        rec.event(2.0, "Tone").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            text,
            "{\"t\":1.000,\"pitch\":25.50,\"roll\":1.00,\"yaw\":-3.00}\n{\"t\":2.000,\"event\":\"Tone\"}\n"
        );
    }
}
