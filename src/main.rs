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

/// Sleep for `d`; `true` if Ctrl-C arrived instead.
async fn sleep_or_ctrl_c(d: Duration, ctrl_c: &mut CtrlC) -> Result<bool> {
    tokio::select! {
        _ = tokio::time::sleep(d) => Ok(false),
        r = ctrl_c => { r?; Ok(true) }
    }
}

/// How one attempt ended.
enum Attempt {
    /// Ctrl-C arrived and headup stops. If a tracker was connected, its stream was switched off.
    Stopped,
    /// No supported earbuds are connected yet; try again after RETRY.
    Waiting,
    /// Something failed (the message says what); try again after RETRY.
    Failed(String),
}

/// One attempt: find the earbuds, connect, and run a session until it ends. Anything that can be
/// retried comes back as an [`Attempt`]. An `Err` is fatal.
async fn attempt(
    bt: &bluer::Session,
    args: &Args,
    cues: &Cues,
    rec: &mut Recorder,
    ctrl_c: &mut CtrlC,
) -> Result<Attempt> {
    let found = tokio::select! {
        r = devices::find(bt, args.device) => r,
        r = &mut *ctrl_c => { r?; return Ok(Attempt::Stopped); }
    };
    let found = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Ok(Attempt::Waiting),
        Err(e) => {
            return Ok(Attempt::Failed(format!(
                "looking for earbuds failed: {e:#}"
            )));
        }
    };
    rec.note(&format!("using {} ({})", found.name, found.address));
    let device_log = DeviceLog::new(args.raw_log.as_deref().map(open_append).transpose()?);
    let mut tracker = match AnyTracker::connect(&found, device_log).await {
        Ok(t) => t,
        Err(e) => return Ok(Attempt::Failed(format!("connecting failed: {e:#}"))),
    };
    let Err(e) = tokio::select! {
        r = session::run(&mut tracker, cues, rec) => r,
        r = &mut *ctrl_c => {
            r?;
            rec.note("stopping");
            tracker.stop().await?;
            return Ok(Attempt::Stopped);
        }
    };
    // Stopping usually fails because the link is already gone. A failure matters only if the
    // link is still up, so it is only a warning.
    if let Err(e) = tracker.stop().await {
        rec.warn(&format!("could not stop the stream: {e:#}"));
    }
    // Returning drops `tracker`, which releases the Bluetooth link before the retry wait.
    Ok(Attempt::Failed(format!("session ended: {e:#}")))
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
        match attempt(&bt, &args, &cues, &mut rec, &mut ctrl_c).await? {
            Attempt::Stopped => return Ok(()),
            Attempt::Waiting => {
                if !waiting_noted {
                    rec.note("waiting for supported earbuds to connect…");
                    waiting_noted = true;
                }
            }
            Attempt::Failed(what) => {
                waiting_noted = false;
                rec.warn(&format!("{what}; retrying in {}s", RETRY.as_secs()));
            }
        }
        if sleep_or_ctrl_c(RETRY, &mut ctrl_c).await? {
            return Ok(());
        }
    }
}
