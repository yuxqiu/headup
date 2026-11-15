use anyhow::{Context, Result};
use clap::Parser;
use headup::core::record::{DeviceLog, LogSink, Recorder};
use headup::core::{session, sound::DesktopPlayer, tracker::HeadTracker};
use headup::devices::bluetooth::{Address, Host};
use headup::devices::{self, AnyTracker};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};

/// Head-posture reminders from earbud head tracking.
///
/// Runs until stopped: waits for supported earbuds to be connected, and reconnects when the
/// connection drops.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Bluetooth address of the earbuds, e.g. AA:BB:CC:DD:EE:01. Default: the first connected
    /// supported earbuds.
    device: Option<Address>,
    /// Append posture samples and cue events as JSON lines to this file
    #[arg(long)]
    log: Option<PathBuf>,
    /// Append raw device samples (for debugging the backend) as JSON lines to this file
    #[arg(long)]
    raw_log: Option<PathBuf>,
}

/// How long to wait before looking for the earbuds again, or reconnecting (DECISIONS "Wear pause,
/// auto-detect, daemon, Home Manager").
const RETRY: Duration = Duration::from_secs(5);

/// Resolves when headup is asked to stop: Ctrl-C (SIGINT) in a terminal, or SIGTERM, which
/// systemd sends on `systemctl stop`, on logout and on a Home Manager switch.
type StopSignal = Pin<Box<dyn Future<Output = std::io::Result<()>>>>;

fn stop_signal() -> std::io::Result<StopSignal> {
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(Box::pin(async move {
        tokio::select! {
            r = tokio::signal::ctrl_c() => r,
            _ = terminate.recv() => Ok(()),
        }
    }))
}

fn open_append(p: &Path) -> Result<LogSink> {
    if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("create {dir:?}"))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .with_context(|| format!("open {p:?}"))?;
    Ok(Box::new(file))
}

/// Sleep for `d`; `true` if a stop signal arrived instead.
async fn sleep_or_stop(d: Duration, stop: &mut StopSignal) -> Result<bool> {
    tokio::select! {
        _ = tokio::time::sleep(d) => Ok(false),
        r = stop => { r?; Ok(true) }
    }
}

/// How one attempt ended.
enum Attempt {
    /// A stop signal arrived and headup stops. If a tracker was connected, its stream was switched
    /// off, unless a second stop signal cut that short.
    Stopped,
    /// No supported earbuds are connected yet; try again after RETRY.
    Waiting,
    /// Something failed; try again after RETRY.
    Failed(anyhow::Error),
}

/// One attempt: find the earbuds, connect, and run a session until it ends. Anything that can be
/// retried comes back as an [`Attempt`]. An `Err` is fatal.
async fn attempt(
    bt: &Host,
    args: &Args,
    player: &DesktopPlayer,
    rec: &mut Recorder,
    stop: &mut StopSignal,
) -> Result<Attempt> {
    let found = tokio::select! {
        r = devices::find(bt, args.device) => r,
        r = &mut *stop => { r?; return Ok(Attempt::Stopped); }
    };
    let found = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Ok(Attempt::Waiting),
        Err(e) => return Ok(Attempt::Failed(e.context("looking for earbuds failed"))),
    };
    rec.note(&format!(
        "using {} ({})",
        found.device.name, found.device.address
    ));
    let device_log = DeviceLog::new(args.raw_log.as_deref().map(open_append).transpose()?);
    let connected = tokio::select! {
        r = AnyTracker::connect(bt, &found, device_log) => r,
        r = &mut *stop => { r?; return Ok(Attempt::Stopped); }
    };
    let mut tracker = match connected {
        Ok(t) => t,
        Err(e) => return Ok(Attempt::Failed(e.context("connecting failed"))),
    };
    let Err(e) = tokio::select! {
        r = session::run(&mut tracker, player, rec) => r,
        r = &mut *stop => {
            r?;
            rec.note("stopping (stop again to quit at once)");
            // Switching the stream off waits on the link; a second stop signal gives up on it.
            tokio::select! {
                () = stop_stream(&mut tracker, rec) => {}
                r = stop_signal()? => {
                    r?;
                    rec.warn("quit without switching the earbuds' stream off");
                }
            }
            return Ok(Attempt::Stopped);
        }
    };
    stop_stream(&mut tracker, rec).await;
    // Returning drops `tracker`, which releases the Bluetooth link before the retry wait.
    Ok(Attempt::Failed(e.context("session ended")))
}

/// Switch the earbuds' stream off. This usually fails because the link is already gone; a failure
/// matters only if the link is still up, so it is only a warning.
async fn stop_stream(tracker: &mut AnyTracker, rec: &mut Recorder) {
    if let Err(e) = tracker.stop().await {
        rec.warn(&format!("could not stop the stream: {e:#}"));
    }
}

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    // Dropping the runtime would wait for every blocking thread, including one playing a sound
    // that is stuck in a sound server that stopped answering (see `audio::play_bounded`), possibly
    // forever. The stream is off and the logs are written by now, so exit without waiting.
    runtime.shutdown_background();
    result
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let player = DesktopPlayer::new()?;
    let mut rec = Recorder::new(args.log.as_deref().map(open_append).transpose()?);
    let bt = Host::new().await?;
    if let Some(addr) = args.device {
        devices::ensure_supported(&bt, addr).await?;
    }
    let mut stop = stop_signal()?;

    let mut waiting_noted = false;
    loop {
        match attempt(&bt, &args, &player, &mut rec, &mut stop).await? {
            Attempt::Stopped => return Ok(()),
            Attempt::Waiting => {
                if !waiting_noted {
                    rec.note("waiting for supported earbuds to connect…");
                    waiting_noted = true;
                }
            }
            Attempt::Failed(e) => {
                waiting_noted = false;
                rec.warn(&format!("{e:#}; retrying in {}s", RETRY.as_secs()));
            }
        }
        if sleep_or_stop(RETRY, &mut stop).await? {
            return Ok(());
        }
    }
}
