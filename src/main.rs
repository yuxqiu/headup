use anyhow::{Context, Result, bail};
use clap::Parser;
use headup::core::{cues::Cues, session, tracker::HeadTracker};
use headup::devices::{self, Found, Model, freeclip2::FreeClip2};
use std::path::{Path, PathBuf};
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

/// The requested (or first) supported earbuds currently connected, if any.
async fn find(bt: &bluer::Session, want: Option<bluer::Address>) -> Result<Option<Found>> {
    let found = devices::connected_supported(bt).await?;
    let Some(addr) = want else {
        return Ok(found.into_iter().next());
    };
    let device = bt.default_adapter().await?.device(addr)?;
    if let Some(name) = device.name().await?
        && Model::from_bluetooth_name(&name).is_none()
    {
        bail!("{addr} ({name}) is not a supported device (see README)");
    }
    Ok(found.into_iter().find(|f| f.address == addr))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cues = Cues::new()?;
    let mut log = args.log.as_deref().map(open_append).transpose()?;
    let bt = bluer::Session::new()
        .await
        .context("connect to BlueZ over D-Bus")?;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    let mut waiting_noted = false;
    loop {
        let found = tokio::select! {
            r = find(&bt, args.device) => r?,
            r = &mut ctrl_c => { r?; return Ok(()); }
        };
        let Some(found) = found else {
            if !waiting_noted {
                println!("waiting for supported earbuds to connect…");
                waiting_noted = true;
            }
            tokio::select! {
                _ = tokio::time::sleep(RETRY) => continue,
                r = &mut ctrl_c => { r?; return Ok(()); }
            }
        };
        waiting_noted = false;
        println!("using {} ({})", found.name, found.address);
        let raw_log = args.raw_log.as_deref().map(open_append).transpose()?;
        let connected = match found.model {
            Model::FreeClip2 => FreeClip2::connect(found.address, raw_log).await,
        };
        let mut tracker = match connected {
            Ok(t) => t,
            Err(e) => {
                eprintln!("connecting failed: {e:#}; retrying in {}s", RETRY.as_secs());
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        let result = tokio::select! {
            r = session::run(&mut tracker, &cues, &mut log) => r,
            r = &mut ctrl_c => {
                r?;
                println!("\nstopping");
                return tracker.stop().await;
            }
        };
        if let Err(e) = result {
            eprintln!(
                "\nsession ended: {e:#}; reconnecting in {}s",
                RETRY.as_secs()
            );
        }
        // Usually fails because the link is gone; it matters only if the link is still up.
        if let Err(e) = tracker.stop().await {
            eprintln!("could not stop the stream: {e:#}");
        }
        drop(tracker);
        tokio::time::sleep(RETRY).await;
    }
}
