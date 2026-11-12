use anyhow::{Context, Result};
use clap::Parser;
use headup::core::record::{DeviceLog, Recorder};
use headup::core::{cues::Cues, session, tracker::HeadTracker};
use headup::devices::{self, AnyTracker};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

/// Head-posture reminders from earbud head tracking.
///
/// Runs until stopped: waits for supported earbuds to be connected, and reconnects when the
/// connection drops.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Bluetooth address of the earbuds, e.g. C0:DA:5E:F1:A0:A6. Default: the first connected
    /// supported earbuds.
    device: Option<bluer::Address>,
    /// Append posture samples and cue events as JSON lines to this file
    #[arg(long)]
    log: Option<PathBuf>,
    /// Append raw device samples (for debugging the backend) as JSON lines to this file
    #[arg(long)]
    raw_log: Option<PathBuf>,
}

const RETRY: Duration = Duration::from_secs(5);

type CtrlC = Pin<Box<dyn Future<Output = std::io::Result<()>>>>;

fn open_append(p: &Path) -> Result<std::fs::File> {
    if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("create {dir:?}"))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .with_context(|| format!("open {p:?}"))
}

/// Warn, then wait RETRY before the next attempt; `true` if Ctrl-C arrived instead.
async fn retry_later(rec: &mut Recorder, what: &str, ctrl_c: &mut CtrlC) -> Result<bool> {
    rec.warn(&format!("{what}; retrying in {}s", RETRY.as_secs()));
    sleep_or_ctrl_c(RETRY, ctrl_c).await
}

/// Sleep for `d`; `true` if Ctrl-C arrived instead.
async fn sleep_or_ctrl_c(d: Duration, ctrl_c: &mut CtrlC) -> Result<bool> {
    tokio::select! {
        _ = tokio::time::sleep(d) => Ok(false),
        r = ctrl_c => { r?; Ok(true) }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cues = Cues::new()?;
    let mut rec = Recorder::new(args.log.as_deref().map(open_append).transpose()?);
    let bt = bluer::Session::new()
        .await
        .context("connect to BlueZ over D-Bus")?;
    if let Some(addr) = args.device {
        devices::ensure_supported(&bt, addr).await?;
    }
    let mut ctrl_c: CtrlC = Box::pin(tokio::signal::ctrl_c());

    let mut waiting_noted = false;
    loop {
        let found = tokio::select! {
            r = devices::find(&bt, args.device) => r,
            r = &mut ctrl_c => { r?; return Ok(()); }
        };
        let found = match found {
            Ok(found) => found,
            Err(e) => {
                let what = format!("looking for earbuds failed: {e:#}");
                if retry_later(&mut rec, &what, &mut ctrl_c).await? {
                    return Ok(());
                }
                continue;
            }
        };
        let Some(found) = found else {
            if !waiting_noted {
                rec.note("waiting for supported earbuds to connect…");
                waiting_noted = true;
            }
            if sleep_or_ctrl_c(RETRY, &mut ctrl_c).await? {
                return Ok(());
            }
            continue;
        };
        waiting_noted = false;
        rec.note(&format!("using {} ({})", found.name, found.address));

        let device_log = DeviceLog::new(args.raw_log.as_deref().map(open_append).transpose()?);
        let mut tracker = match AnyTracker::connect(&found, device_log).await {
            Ok(t) => t,
            Err(e) => {
                if retry_later(&mut rec, &format!("connecting failed: {e:#}"), &mut ctrl_c).await? {
                    return Ok(());
                }
                continue;
            }
        };
        let Err(e) = tokio::select! {
            r = session::run(&mut tracker, &cues, &mut rec) => r,
            r = &mut ctrl_c => {
                r?;
                rec.note("stopping");
                return tracker.stop().await;
            }
        };
        // Usually fails because the link is gone; it matters only if the link is still up.
        if let Err(e) = tracker.stop().await {
            rec.warn(&format!("could not stop the stream: {e:#}"));
        }
        drop(tracker);
        if retry_later(&mut rec, &format!("session ended: {e:#}"), &mut ctrl_c).await? {
            return Ok(());
        }
    }
}
