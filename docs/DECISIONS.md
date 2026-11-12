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

## 2026-11-11: Send the 43/90 capability query before enabling the stream (user)
- `FreeClip2::connect` now sends `43/90:1=` before `43/88:1=01`; the reply is ignored.
- Reason: during the tracking outage (FINDINGS "Tracking outage"), tracking returned right after a probe that sent
  43/90. Whether 43/90 arms the stream is unverified. It is a read-only query, so it costs nothing. If an outage
  recurs with it in place, 43/90 is ruled out.

## 2026-11-11: Glide detection after 1.0 s of stillness
- `GLIDE_AFTER_STILL_S` lowered from 2.5 s to 1.0 s (src/devices/freeclip2/rebuild.rs). A live glide began ~2 s after
  the head settled, so headup saw a return to upright and never reminded (FINDINGS "Re-centre glide").
- Test `hides_glide_after_short_hold` replays that session (`tests/fixtures/freeclip2/short_hold_glide.csv`) and
  fails with 2.5 s. All other tests pass with any value from 0.5 to 2.5 s; 1.0 s keeps margin both ways.
- v0.1.0 ships with the 2.5 s threshold, so a slouch held very still can go unnoticed there.

## 2026-11-11: Re-anchor upright after each recovery (user approved as a compromise)
- The FreeClip 2's re-centres are ~10° off (always towards "looking up"), so the summed pose drifts and later slouches
  go undetected (FINDINGS "Ground truth"). No drift-free signal exists on this device, and the user chose not to model
  the error.
- `Detector` (src/core/detector.rs): after a recovery, the first time the head holds still (pitch within 2° over
  ≥0.8 s of a 1 s window), a pitch above upright (< 0) becomes the new upright. Once per recovery; a new slouch cancels
  the wait. Assumption: people sit back up to their upright, not beyond it. Logged as `Reanchored { upright }`.
- Replayed on data/slouch_guided.csv: upright −1…+2°, every look-down 29–48° (without: upright to −41°, last look-down
  +6°). On data/session6_raw.jsonl: upright about −8…+7°, the missed 14:21/14:23 slouches 21–42°, 4 re-anchors.
- Rejected: a continuous clamp after recovery (ratchets upright upward: +19…+28° by the end of session 6, i.e. false
  reminders); re-anchoring at the earbuds' next re-centre (fires mid-movement; uprights −10…−16°); the one-sided
  guard (pitch < −8° for 3 s), which also re-anchors on a plain look up.
- Tests: four detector unit tests, and `repeated_slouches_stay_detectable_end_to_end` on the new fixture
  `tests/fixtures/freeclip2/slouch_guided.csv` (fails without re-anchoring).
- Abstraction (user): re-anchoring is off by default. `TrackerTraits::pose_drifts_up` (default `false`) says whether a
  tracker's pose accumulates error; the session sets `Config::reanchor_after_recovery` from it. FreeClip 2 returns
  `true`, with the reason in its backend. Session test `reanchors_only_for_drifting_trackers` covers both cases.
- Core comments no longer name FreeClip 2 behaviour (wear rule, audio precondition, re-centre numbers); those live in
  `src/devices/freeclip2/mod.rs` and FINDINGS.

## 2026-11-11: Correlated state lives in one type (user, general rule)
- Fields that are only valid together, or exclude each other, are grouped into one enum or struct so illegal states
  cannot compile.
- `Detector` now holds `posture: Posture`, with `Upright(Option<Settling>)` (waiting to re-anchor) and
  `Slouching(Episode { since, toned, notified, last_speak: Option<f64> })`, replacing the loose `slouch_since`,
  `cued`, `last_speak` and `settling` fields. Behaviour unchanged; 42 tests pass.

## 2026-11-11: Device quirks: absorb in the backend, declare the rest in `TrackerTraits` (user approved)
- A quirk crosses into core only if handling it needs session knowledge (messages shown, slouch/recovery state).
  Re-centring, glides, the both-earbuds rule, the audio precondition (as `Available`) and the 43/90 query stay in
  the backend.
- The rest are fields of one struct, `HeadTracker::traits() -> TrackerTraits` (`tracking_hint: Option<&str>`,
  `pose_drifts_up: bool`), each defaulting to "no quirk". It replaces the per-quirk methods `tracking_hint()` and
  `pose_drifts()`, so adding a quirk adds a field, not a trait method. Pattern as in libinput's quirks files
  `[from memory]`.
- Rejected: letting backends tune the detector's `Config` directly (puts posture policy in device code).

## 2026-11-11: Refactor pass after review (4 parallel reviewers: reuse, simplification, efficiency, altitude)
- `Detector::update` returns `Update { cues, reanchored: Option<f64> }`; the session no longer infers a re-anchor by
  diffing `upright()`. The log event is `Reanchored` with the new upright (`"upright":-3.60`), so logs can be replayed
  without re-running the rule. Renamed from `Rebaselined` to match the code.
- `TrackerTraits::pose_drifts` → `pose_drifts_up`: the rule only corrects drift towards "looking up".
- `Config::for_traits()` maps traits to detector settings in one place; `FreeClip2::TRAITS` is an associated const so
  the end-to-end test uses the device's real traits.
- Merged the duplicate spoken-reminder branches in `Detector::tick`.
- Skipped: rebasing the full 3D baseline instead of a pitch offset (would also reset yaw/roll; unvalidated behaviour
  change); removing the glide stillness gate (behaviour change; the deeper fix is the deferred buffered-glide design);
  sharing one stillness window between rebuilder and detector (different layers and metrics); test-only duplication.
  Efficiency review found nothing worth changing at 50 Hz.

## 2026-11-11: Buffered glide handling (user)
- `Rebuilder` holds every sample for `DELAY_S` = 2.0 s. On an identity sample it drops the `GLIDE_SAMPLES` = 70
  samples before it (the glide; measured 66–72, session 6) and takes the last released pose as the new reference.
  Replaces the stillness gate and step-size heuristic (`GLIDE_AFTER_STILL_S`, `GLIDE_MIN_STEP_DEG`,
  `GLIDE_STEP_TOL_DEG`, `STILL_*`), which missed small glides. Re-anchoring is unchanged.
- The backend queues released poses and wakes at `Rebuilder::next_release()` so held samples come out on time during
  silences. Cost: every pose, and so every cue, is ~2 s later.
- hold_test.csv: exactly 5 × 69 samples dropped (the old rule missed the small re-centre at 5.4 s). All fixture tests
  pass, including both guided-axis runs and the stream restart.
- `--raw-log` now writes raw samples only (`t`, `seq`, `raw`); rebuilt orientations are in `--log`.

## 2026-11-11: No silent lead-in before sounds (user)
- Removed `LEAD_IN_S` and `Pcm::with_lead_in`. Cues only play while tracking is available, i.e. while this computer is
  already streaming audio, so the Bluetooth output is awake. Supersedes "Silent lead-in for every sound".
- Edge case accepted: during a short audio pause (< 30 s, slouch state kept) a reminder can start on a suspended output
  and lose its first moment.

## 2026-11-11: Re-send the stream switch when this computer starts streaming
- `FreeClip2` sends 43/88=1 again on every 43/54 transition of this computer to streaming. A 43/88 sent while idle
  left tracking off for an hour after audio started (FINDINGS "Stream switch sent while idle"). The extra 43/88 is
  harmless during playback: earlier restarts of the stream were acked and the rebuilder handles stream restarts.

## 2026-11-11: Backend registry generated from one list (user approved)
- Each backend implements `devices::Backend` (`recognises(bluetooth_name)`, `connect(&Found, raw_log)`) on top of
  `HeadTracker`, so the backend owns its name rule and its connect arguments.
- A `backends!` macro in src/devices/mod.rs generates `Model`, `Model::from_bluetooth_name` and the `AnyTracker`
  enum with its dispatch from one list of `Variant => type` pairs. Adding a model takes a `mod` line and one list
  line (checked by compiling a throwaway second backend). Before, it took ~7 edits in the registry. When several
  backends recognise a name, the first listed wins.
- Rejected: a `&[&dyn Backend]` slice. It needs boxed futures and an object-safe twin of `HeadTracker`.
- FreeClip 2 cleanups with no change in behaviour: `stream_switch(on)` builds the 43/88 frame (4 copies before);
  the backend uses `tokio::time::Instant` throughout instead of converting from `std`.

## 2026-11-11: Shared logging for backends; serde_json for both logs (user approved)
- `core::record::DeviceLog` replaces the bare `Option<File>` that backends got for `--raw-log`. It owns the file
  and the clock, and `record(&fields)` writes `{"t":…, <fields>}` for any `Serialize` value. Without a file,
  records are dropped. `Backend::connect` takes a `DeviceLog`. For what `t` means, see "Wall-clock log time"
  below.
- FreeClip 2 logs `RawSample { seq, raw }` (components as the f32 the earbuds send) and `RawFrame { svc, cmd, tlvs }`
  (TLVs in hex, ordered by tag). Its hand-built JSON (`raw_log_line`, `raw_frame_line`) was removed.
- The session log (`--log`) is also written with serde_json (user's choice: one serializer). Values are rounded
  as before (t to 3 decimals, angles to 2) but printed in shortest form, e.g. `"t":1.0` instead of `"t":1.000`.
  Keys and their order are unchanged.
- New dependencies: serde (derive) and serde_json.

## 2026-11-11: Test-only modules live in a `tests/` directory (user)
- Test-only Rust modules sit apart from production modules: a backend's inline `mod tests` declares them, and their
  files live in `<backend>/tests/` (FreeClip 2: `tests/replay.rs` for fixture replay, `tests/end_to_end.rs` for
  rebuilder + detector runs). They cannot go in the crate's top-level `tests/` because they use private types
  (`Rebuilder`).

## 2026-11-11: One run clock; discovery module; `session::run` cannot return `Ok` (user: redesign freely)
- `Recorder` owns the run's monotonic clock (`Recorder::now`), used for session timing and console messages, and
  stamps every log line itself: `event(e)`, `pose(angles)` and `status(..)` no longer take `t`. The session reads
  time from the recorder. (Log timestamps: superseded by "Wall-clock log time".)
- `src/devices/discovery.rs` holds all BlueZ lookup: `Found`, `find(session, want)` and `ensure_supported` (moved
  out of main). `src/devices/mod.rs` is only the backend registry.
- `session::run` returns `Result<Infallible>`, so main has no "session ended without an error" branch.

## 2026-11-12: Wall-clock log time (user)
- `t` in `--log` and `--raw-log` is wall-clock Unix time in seconds, rounded to the millisecond, read when the line
  is written. Lines from both logs, from different runs appended to one file, and from the journal line up. This
  stays right across suspend, where a monotonic clock stops.
- Session timing (detector, calibration, timeouts) stays monotonic (`Recorder::now`), because the wall clock can
  jump. Console messages keep seconds since start; journald adds the date and time.
- `DeviceLog::new(file)` again; it no longer shares a clock with `Recorder`. Unix seconds rather than RFC 3339 text:
  no date crate, and analysis needs only subtraction.
