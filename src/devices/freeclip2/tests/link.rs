//! The FreeClip 2 protocol state: what the earbuds say, what headup sends, what the session is told.

use super::super::protocol::sample_frame;
use super::*;
use crate::core::quat::Quat;
use std::time::Duration;

fn pc() -> Address {
    "AA:BB:CC:DD:EE:02".parse().unwrap()
}

fn phone() -> Address {
    "12:34:56:78:9A:BC".parse().unwrap()
}

fn link(now: Instant) -> Link {
    let mut link = Link::new(pc(), now);
    link.take_outbox();
    link
}

fn wear(left: u8, right: u8) -> Vec<u8> {
    Frame::new(43, 37, &[(1, &[left]), (2, &[right])]).encode()
}

/// A 43/54 push for `address` with audio state byte `state`.
fn audio(address: Address, state: u8) -> Vec<u8> {
    let mut v = address.0;
    v.reverse();
    Frame::new(43, 54, &[(5, &[&v[..], &[state]].concat())]).encode()
}

/// A 43/88 reply with result `v`.
fn reply(v: u8) -> Vec<u8> {
    Frame::new(43, 88, &[(2, &[v])]).encode()
}

fn events(link: &mut Link) -> Vec<Event> {
    std::iter::from_fn(|| link.next_event()).collect()
}

/// A head turned about 50° to the left, as a raw reading.
fn turned() -> Quat {
    Quat {
        w: 0.9,
        x: 0.0,
        y: 0.0,
        z: 0.19_f64.sqrt(),
    }
}

/// Feed `readings` (sequence number, raw) and return the orientation of the first pose they
/// give.
fn first_orientation(link: &mut Link, readings: impl IntoIterator<Item = (u32, Quat)>) -> Quat {
    let now = Instant::now();
    for (seq, raw) in readings {
        link.on_bytes(now, &sample_frame(seq, raw).encode())
            .unwrap();
    }
    events(link)
        .into_iter()
        .find_map(|e| match e {
            Event::Pose { orientation, .. } => Some(orientation),
            _ => None,
        })
        .expect("no pose")
}

/// Feed reading `raw` with sequence number `seq`, and return the events other than poses.
fn reading(link: &mut Link, seq: u32, raw: Quat) -> Vec<Event> {
    let now = Instant::now();
    link.on_bytes(now, &sample_frame(seq, raw).encode())
        .unwrap();
    events(link)
        .into_iter()
        .filter(|e| !matches!(e, Event::Pose { .. }))
        .collect()
}

#[test]
fn starts_by_querying_and_switching_the_stream_on() {
    let mut link = Link::new(pc(), Instant::now());
    let sent = link.take_outbox();
    assert_eq!(
        sent,
        [wear_query(), capability_query(), stream_switch(true)]
    );
    assert_eq!(link.take_outbox(), []);
    link.stop();
    assert_eq!(link.take_outbox(), [stream_switch(false)]);
}

// Putting the earbuds back on starts a new orientation frame and switches the stream on
// again. The first wear report and repeated reports send nothing.
#[test]
fn rewearing_restarts_the_stream() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &wear(1, 1)).unwrap();
    link.on_bytes(now, &wear(1, 1)).unwrap();
    assert_eq!(events(&mut link), [Event::Worn(true)]);
    assert_eq!(link.take_outbox(), []);
    link.on_bytes(now, &wear(0, 1)).unwrap();
    assert_eq!(events(&mut link), [Event::Worn(false)]);
    link.on_bytes(now, &wear(1, 1)).unwrap();
    assert_eq!(events(&mut link), [Event::Worn(true)]);
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
}

// Re-wearing starts a new stream, so a new frame: the first reading afterwards is its origin,
// whatever frame came before (here one moved to a turned pose by a new stream).
#[test]
fn rewearing_starts_a_new_frame() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &wear(1, 1)).unwrap();
    for seq in 1..=4 {
        reading(&mut link, seq, turned());
    }
    reading(&mut link, 1, Quat::IDENTITY); // a new stream, from the turned pose
    link.on_bytes(now, &wear(0, 1)).unwrap();
    link.on_bytes(now, &wear(1, 1)).unwrap();
    events(&mut link);
    let first = first_orientation(&mut link, [(1, Quat::IDENTITY), (2, Quat::IDENTITY)]);
    assert!(first.angle_to(Quat::IDENTITY) < 1e-9, "{first:?}");
}

// Re-wearing during a hold starts a new stream: the hold is over and the frame is new, so the
// first reading afterwards is the new frame's origin.
#[test]
fn rewearing_during_a_hold_ends_it_and_starts_a_new_frame() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &wear(1, 1)).unwrap();
    assert_eq!(
        reading(&mut link, 1, turned()),
        [Event::Worn(true), Event::Available]
    );
    let walking = Event::Unavailable { reason: WALKING };
    assert_eq!(reading(&mut link, 2, Quat::IDENTITY), [walking]);
    link.on_bytes(now, &wear(0, 1)).unwrap();
    link.on_bytes(now, &wear(1, 1)).unwrap();
    let events = events(&mut link);
    assert_eq!(
        events,
        [Event::Worn(false), Event::Worn(true), Event::Available]
    );
    let first = first_orientation(&mut link, [(1, turned()), (2, turned())]);
    assert!(first.angle_to(turned()) < 1e-4, "{first:?}");
}

// When this computer starts streaming, switch the stream on again.
#[test]
fn this_computer_starting_audio_rearms_the_stream() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &audio(phone(), 0x09)).unwrap();
    assert_eq!(events(&mut link), []);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(events(&mut link), [Event::Available]);
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(events(&mut link), []);
    link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
    assert_eq!(
        events(&mut link),
        [Event::Unavailable {
            reason: NOT_STREAMING
        }]
    );
    assert_eq!(link.take_outbox(), []);
}

// headup connected while the phone was the earbuds' current device. The 43/88 is refused with
// `02`, and tracking is unavailable. Once this computer streams, the stream is switched on
// again (data/live2_raw.jsonl: `2=02`, 43/54 `…09`, re-sent 43/88 `2=00`, then samples).
#[test]
fn refused_stream_switch_waits_for_this_computer_to_stream() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &reply(2)).unwrap();
    assert_eq!(
        events(&mut link),
        [Event::Unavailable {
            reason: NOT_STREAMING
        }]
    );
    link.on_bytes(now, &reply(2)).unwrap();
    assert_eq!(events(&mut link), []);
    assert_eq!(link.take_outbox(), []);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(events(&mut link), [Event::Available]);
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
    link.on_bytes(now, &reply(0)).unwrap();
    assert_eq!(events(&mut link), []);
}

// An accepted switch survives an audio gap. When this computer streams again, the earbuds
// resume the stream by themselves (data/grounding_test.log, "audio_gap": samples 0.3 s after
// 43/54 `…09` with no 43/88). Switching on again would restart the stream, and each new stream
// adds a re-centre, so the switch is not sent.
#[test]
fn audio_resuming_after_an_accepted_switch_keeps_the_stream() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &reply(0)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(
        events(&mut link),
        [
            Event::Available,
            Event::Unavailable {
                reason: NOT_STREAMING
            },
            Event::Available
        ]
    );
    assert_eq!(link.take_outbox(), []);
}

// Until the earbuds accept a switch, this computer starting to stream switches the stream on
// again (the reply may not have arrived yet).
#[test]
fn audio_starting_before_any_reply_switches_on_again() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
}

// Another device (the phone) started streaming after the switch was accepted. The switch does
// not survive the earbuds changing their current device (data/grounding_test3.log
// "phone_takeover"), so it is sent again when this computer streams again.
#[test]
fn switch_is_resent_after_another_device_streamed() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &reply(0)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x03)).unwrap();
    link.on_bytes(now, &audio(phone(), 0x09)).unwrap();
    assert_eq!(link.take_outbox(), []);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
}

// The switch is refused while this computer streams, because the earbud-to-earbud link was
// down. Putting the earbuds back on sends the switch again, so tracking is available again.
#[test]
fn rewearing_after_a_refusal_makes_tracking_available_again() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &wear(1, 1)).unwrap();
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    link.on_bytes(now, &reply(1)).unwrap();
    assert_eq!(
        events(&mut link),
        [
            Event::Worn(true),
            Event::Available,
            Event::Unavailable {
                reason: NOT_STREAMING
            }
        ]
    );
    link.take_outbox();
    link.on_bytes(now, &wear(0, 1)).unwrap();
    link.on_bytes(now, &wear(1, 1)).unwrap();
    assert_eq!(
        events(&mut link),
        [Event::Worn(false), Event::Worn(true), Event::Available]
    );
    assert_eq!(link.take_outbox(), [stream_switch(true)]);
    link.on_bytes(now, &reply(0)).unwrap();
    assert_eq!(events(&mut link), []);
}

// While the user walks around, the earbuds hold: the reading jumps to identity with no glide
// and stays there while their reference follows the head. Head tracking is unavailable until
// the reading moves again (FINDINGS "Instant resets while walking").
#[test]
fn a_hold_makes_tracking_unavailable_until_the_reading_moves() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    assert_eq!(events(&mut link), [Event::Available]);
    assert_eq!(reading(&mut link, 1, turned()), []);
    let walking = Event::Unavailable { reason: WALKING };
    assert_eq!(reading(&mut link, 2, Quat::IDENTITY), [walking]);
    assert_eq!(reading(&mut link, 3, Quat::IDENTITY), []);
    assert_eq!(reading(&mut link, 400, turned()), [Event::Available]);
}

// The earbuds send no 43/54 on connect while audio already plays (13 of 22 raw logs in
// data/). Readings arrive only at the current device, so the first one makes tracking
// available, and a hold's end makes it available again.
#[test]
fn readings_show_this_computer_streams_when_no_audio_push_came() {
    let mut link = link(Instant::now());
    assert_eq!(reading(&mut link, 1, turned()), [Event::Available]);
    let walking = Event::Unavailable { reason: WALKING };
    assert_eq!(reading(&mut link, 2, Quat::IDENTITY), [walking]);
    assert_eq!(reading(&mut link, 400, turned()), [Event::Available]);
}

// Readings that trail in while tracking is unavailable are not rebuilt, and do not leave a frame
// behind for the next stretch: here they include a new stream, which would rebase a frame.
#[test]
fn readings_while_unavailable_leave_no_frame() {
    let now = Instant::now();
    let mut link = link(now);
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap();
    reading(&mut link, 1, turned());
    link.on_bytes(now, &audio(pc(), 0x03)).unwrap(); // paused: unavailable
    for seq in 2..=5 {
        reading(&mut link, seq, turned());
    }
    reading(&mut link, 1, Quat::IDENTITY); // a new stream, from the turned pose
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap(); // playing again
    events(&mut link);
    let first = first_orientation(&mut link, [(50, Quat::IDENTITY), (51, Quat::IDENTITY)]);
    assert!(first.angle_to(Quat::IDENTITY) < 1e-9, "{first:?}");
}

// While tracking was unavailable the earbuds kept re-centring where headup could not see it, so
// when it resumes a new orientation frame starts: the first reading afterwards is the frame's
// origin, whatever frame came before. Here the frame before had moved its reference to a turned
// pose (a new stream starts from the last pose).
#[test]
fn tracking_resuming_starts_a_new_frame() {
    let now = Instant::now();
    let mut link = link(now);
    let orientations = |link: &mut Link, seq, raw| -> Vec<Quat> {
        link.on_bytes(now, &sample_frame(seq, raw).encode())
            .unwrap();
        events(link)
            .into_iter()
            .filter_map(|e| match e {
                Event::Pose { orientation, .. } => Some(orientation),
                _ => None,
            })
            .collect()
    };
    orientations(&mut link, 1, turned());
    orientations(&mut link, 2, turned());
    orientations(&mut link, 1, Quat::IDENTITY); // a new stream, from the turned pose
    link.on_bytes(now, &audio(pc(), 0x03)).unwrap(); // audio paused: unavailable
    link.on_bytes(now, &audio(pc(), 0x09)).unwrap(); // playing again: available
    orientations(&mut link, 50, Quat::IDENTITY);
    let after = orientations(&mut link, 51, Quat::IDENTITY);
    assert_eq!(
        after.first().map(|q| q.angle_to(Quat::IDENTITY) < 1e-9),
        Some(true)
    );
}

#[test]
fn malformed_frames_are_errors() {
    let now = Instant::now();
    let mut link = link(now);
    let short = Frame::new(43, 54, &[(5, &[1, 2])]).encode();
    assert!(link.on_bytes(now, &short).is_err());
}

// Poses carry the time their sample was read, not when they are handed out.
#[test]
fn poses_carry_their_read_time() {
    let start = Instant::now();
    let mut link = link(start);
    let at = |i: u32| start + Duration::from_millis(20 * u64::from(i));
    for seq in 1..=80 {
        let raw = Quat::IDENTITY;
        link.on_bytes(at(seq), &sample_frame(seq, raw).encode())
            .unwrap();
    }
    let Some(Event::Pose { at: first, .. }) = link.next_event() else {
        panic!("no pose");
    };
    assert_eq!(first, at(1));
}
