//! [`FreeClip2`] on an in-memory socket: what reaches the session, and what reaches the earbuds.

use super::replay::{Replay, read_csv, secs};
use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker};
use crate::devices::freeclip2::FreeClip2;
use crate::devices::freeclip2::mbb::Decoder;
use crate::devices::freeclip2::protocol::{sample_frame, stream_switch};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::Instant;

fn connect() -> (FreeClip2, DuplexStream) {
    let (ours, earbuds) = tokio::io::duplex(1 << 16);
    let adapter = "00:00:00:00:00:01".parse().unwrap();
    (
        FreeClip2::start(ours, adapter, DeviceLog::new(None)),
        earbuds,
    )
}

// The session reads nothing while it speaks a prompt. Samples that arrive meanwhile still carry
// their arrival time, the same as when every sample is read at once.
#[tokio::test(start_paused = true)]
async fn poses_keep_their_arrival_time_while_nothing_reads() {
    let start = Instant::now();
    let rows: Vec<_> = read_csv("live4.csv")
        .into_iter()
        .filter(|r| r.t < secs(5.0))
        .collect();
    let mut want = Vec::new();
    let mut replay = Replay::new(&rows, start);
    while let Some((_, event)) = replay.next() {
        if let Event::Pose { at, .. } = event {
            want.push(at - start);
        }
    }
    assert!(want.len() > 100, "{} poses", want.len());

    let (mut tracker, mut earbuds) = connect();
    for r in &rows {
        tokio::time::sleep_until(start + r.t).await;
        earbuds
            .write_all(&sample_frame(r.seq, r.raw).encode())
            .await
            .unwrap();
    }
    let mut got = Vec::new();
    while let Some(event) = tracker.next_event(Duration::ZERO).await.unwrap() {
        match event {
            Event::Pose { at, .. } => got.push(at - start),
            other => panic!("samples produced {other:?}"),
        }
    }
    assert_eq!(got, want);
}

// Stopping switches the pose stream off before the tracker reports that it has stopped.
#[tokio::test(start_paused = true)]
async fn stop_switches_the_stream_off() {
    let (mut tracker, mut earbuds) = connect();
    tracker.stop().await.unwrap();
    let mut bytes = Vec::new();
    earbuds.read_to_end(&mut bytes).await.unwrap();
    let mut decoder = Decoder::default();
    decoder.push(&bytes);
    let last = std::iter::from_fn(|| decoder.next_frame().unwrap()).last();
    assert_eq!(last, Some(stream_switch(false)));
}
