# Decisions

## 2026-11-11: Repository layout
- Rust crate `headup` lives at the repo root (`Cargo.toml`, `src/`) and holds the detector.
- Python research tools live in `tools/`, recordings and logs in `data/`. Both are committed.
- Large or regenerable third-party material goes in `research/` (gitignored): the vendor APK, the jadx output
  and reference clones. Extracted APK assets and the Gadgetbridge clone were deleted.

## 2026-11-11: Head-pose source
- Use MBB 43/88 (enable) → 43/89 (quaternion stream). Rebuild absolute orientation across the
  device's re-centring by setting the reference to the last stationary pose (`src/devices/freeclip2/rebuild.rs`).
- The spatial-audio mode (43/180) has no effect on the stream. Leave the user's setting untouched.

## 2026-11-11: Detector behaviour (user approved design in docs/UX_RESEARCH.md)
- Signal: forward pitch of conj(baseline)·orientation. Only looking down counts.
- Calibration: spoken prompt at every start, then a 3 s capture rejected if p90 wobble ≥ 8° (up to 4 tries).
  The baseline is not persisted, because the stream's frame starts fresh at every enable.
- Detection: EMA α 0.15. On at 20°, off below 15°.
- Cues during a slouch: tone at 10 s, speech "Gently lift your head" at 30 s (repeated every 120 s),
  notify-send at 60 s, tink on recovery (only if a cue was given).
- Silence from the earbuds holds the last state; a 1 s tick keeps timers running. Disconnect or read errors are fatal.
- RFCOMM via `bluer` (rfcomm feature only, no D-Bus).

## 2026-11-11: Speech voice
- espeak-ng was too robotic for real reminders. Spoken prompts are now pre-generated with Piper's en_GB
  "alba" voice (user's pick; CC BY 4.0, credits in assets/voice/CREDITS.md), embedded with `include_bytes!` and
  played with pw-play. espeak-ng is not a runtime dependency; it stays in the dev shell for tools/*.py.

## 2026-11-11: Scope trim (user)
- Dropped: volume ducking and the daily score. The user considers them too much and not needed.
- Kept for later: snooze/pause, and pausing when an earbud is removed.
- The user prefers a modular design so that other earbuds can plug into the framework. The architecture
  proposal is pending approval.

## 2026-11-11: Modular architecture (user approved)
- The library is split into `core/` (device-agnostic: quat, the `HeadTracker` trait, detector, session, cues)
  and `devices/<model>/` (backends).
- `HeadTracker::next_pose(timeout) -> Option<Quat>` returns an absolute orientation in the framework
  convention (pitch + down, yaw + left, roll + right). Silence is not an error. `stop()` restores the device.
- Device quirks (FreeClip 2 re-centring) live in the backend.
- Earbud in/out events will be added to the trait when the removal-pause feature is built. They are not
  added speculatively.

## 2026-11-11: Nix and data policy (user)
- The flake provides `devShells.default` (Rust toolchain plus the research and cue tools) and `packages.default`
  (`buildRustPackage`, which runs the tests and wraps the binary with pw-play, espeak-ng and notify-send on PATH).
  nixpkgs is nixos-unstable, pinned by flake.lock.
- Recordings and logs are not committed (`data/` is gitignored). The three recordings the tests replay live in
  `tests/fixtures/`. The command catalogue moved to `docs/devices/freeclip2/mbb_commands.txt`.

## 2026-11-11: Wear pause, auto-detect, daemon, Home Manager (user approved)
- `HeadTracker::next_event` yields `Event::Pose` or `Event::Worn`. "Worn" means at least one earbud is worn.
  While not worn, monitoring pauses silently; on re-wear the session recalibrates.
- Device registry: `devices::Model`, which recognises earbuds by Bluetooth name (FreeClip 2: prefix
  "HUAWEI FreeClip 2", as in the vendor whitelist). BlueZ lookup uses bluer's `bluetoothd` feature, so the build
  needs dbus and pkg-config.
- `headup [DEVICE]` runs as a daemon: it waits for supported earbuds, and on connection loss logs, waits 5 s and
  reconnects. Ctrl-C works in every state.
- `homeManagerModules.default` defines a systemd user service on graphical-session.target, after
  pipewire.service, with Restart=on-failure. Options: enable, package, device, logFile.

## 2026-11-11: License, CI and releases (user)
- License: MIT, in `LICENSE`. The GitHub repo is github.com/yuxqiu/headup.
- CI and releases mirror the user's quillway project. CI is a single `nix flake check` (flake `checks`: build with
  tests, clippy with warnings denied, rustfmt; written with buildRustPackage overrides rather than crane).
  Releases are triggered by publishing a `vX.Y.Z` GitHub release and produce glibc 2.35 tarballs with sha256.

## 2026-11-11: Model-specific layout
- Everything that only applies to one earbud model lives under a `freeclip2/` (model) directory:
  `src/devices/freeclip2/`, `docs/devices/freeclip2/` (protocol findings and command catalogue),
  `tools/freeclip2/` (MBB research scripts) and `tests/fixtures/freeclip2/` (recordings).
  Generic code, docs and tools stay at the top level.
- The Python rebuild prototype was removed; `src/devices/freeclip2/rebuild.rs` is the only implementation.

## 2026-11-11: Refactor (user approved)
- `Quat::euler()` returns `Euler { roll, pitch, yaw }` instead of a positional tuple.
- `devices::AnyTracker` (an enum over backends implementing `HeadTracker`) keeps `main` model-agnostic. Adding a
  model touches only `devices/` and its backend.
- `core::record::Recorder` owns all output: console messages and events, the JSON-lines log, and the `\r` live
  status line. The status line is shown only on a TTY, so systemd's journal gets clean lines.
- A failed calibration (head kept moving) is a normal outcome. headup logs `CalibrationFailed`, waits 60 s (or until
  the earbuds are removed) and asks again, without dropping the Bluetooth connection.
- Every retry wait in `main` responds to Ctrl-C.

## 2026-11-11: Silent lead-in for every sound
- Every played sound starts with 1 s of silence (`LEAD_IN_S` in src/core/cues.rs, applied with `core::wav`).
- Reason: PipeWire suspends idle Bluetooth outputs, and the start of playback was lost while the link woke. The
  0.43 s tone was inaudible even though it was logged (session4: Tone at 19.8 s and 56.4 s), and "Sit upright…"
  lost its start (the user heard "…top right"). The wake time of ~0.5–1.5 s is an estimate `[unverified]`; if
  sounds are still clipped, raise `LEAD_IN_S`. 1 s was confirmed sufficient by the user on 2026-11-11.
- Rejected: keeping the Bluetooth output awake by streaming silence. It costs battery and may interfere with the
  earbuds switching to another device.

## 2026-11-11: "Worn" means both earbuds (FreeClip 2)
- FreeClip 2 head tracking stops when either earbud is removed (FINDINGS "Single earbud"). `worn_from` requires both,
  so with one earbud headup pauses ("paused until both earbuds are worn") and recalibrates once both are back.
- The session waits for the first pose before calibrating, with no deadline, noting "waiting for head tracking"
  after 5 s. A missing pose is no longer an error that drops the connection.

## 2026-11-11: Monitor only while the laptop plays audio (user)
- FreeClip 2 head tracking is only available while this computer streams audio to the earbuds (FINDINGS "Tracking
  runs only while this computer streams audio"). The user chose to monitor only while listening on the laptop.
  No silent keep-alive stream: it would cost battery and break switching audio to the phone.
- When tracking resumes after a pause: short gaps (under ~30 s, e.g. skipping tracks) carry the last posture over;
  after a longer gap headup recalibrates (spoken prompt over the music), because a new stream measures from the
  head pose at its start.
- Implemented:
  - `Event::Available(bool)` in the HeadTracker interface. FreeClip 2 derives it from 43/54 pushes for this
    computer's adapter address (`Found.adapter`, little-endian in the frame); bit 0x08 = streaming.
  - Silence while available = head still (posture carried over indefinitely). Unavailable for
    `TRACKING_LOST_S` = 30 s ends monitoring (`TrackingLost`; slouch state dropped, so no stale reminders), and
    calibration repeats when tracking resumes. Availability changes are logged as
    `TrackingAvailable`/`TrackingUnavailable`.
  - `HeadTracker::tracking_hint()` (FreeClip 2: play audio on this computer) appears in the waiting message.
    Calibration waits for the first pose before prompting.
  - The first version used "no pose for 30 s" as tracking lost. It was replaced because the music hold test
    showed silence also means a still head during playback.

## 2026-11-11: Robust device lookup
- `connected_supported` skips devices that vanish while being queried (BlueZ drops nearby BLE devices at any
  time; this caused "target object was either not present or removed" and a crash). `main` logs and retries a
  failed lookup. An explicitly given but unsupported address is rejected once at startup.

## 2026-11-11: Second refactor (user approved)
- Fixed: a pose now clears "unavailable" (`Session::next`). Before, tracking that resumed without an
  `Available(true)` push would trigger `TrackingLost` again on the first quiet second, looping forever. The test
  `poses_end_unavailability_without_announcement` fails without the fix.
- `core::cues::CueSink` trait (`play`, `say`), implemented by `Cues`. The session is generic over it so tests can
  record cues instead of playing them.
- The session uses `tokio::time::Instant`, so scripted session tests run on tokio's paused clock (dev-dependency
  `tokio/test-util`). They cover: cues while the head is still; wear pause and recalibration; short vs long
  unavailability; resume without an announcement; failed calibration and retry.
- `core::record::LogEvent` defines all `"event"` names in the JSON log (strings unchanged).
- One clock in the session (seconds since start); `CALIBRATION_RETRY_S`.
- FreeClip 2 backend: one `impl` block; the module comment covers 43/88, 43/89, 43/37, 43/54.
- `main`: one `retry_later` helper instead of three copies.
