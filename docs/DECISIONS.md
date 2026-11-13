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
- Detection: EMA with a 0.123 s time constant (α 0.15 per sample at 50 Hz; per-sample until 2026-11-12). On at
  20°, off below 15°.
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
  head pose at its start. (Superseded 2026-11-12: "Recalibrate after every unavailability".)
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
- (Holding by seconds superseded on 2026-11-12 by "Hold samples by count; poses carry measurement time".)
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
- Superseded 2026-11-12 ("Re-send the stream switch only if it was not accepted"): the idle 43/88 had been refused.

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
- `Recorder` stamps every log line itself: `event(e)`, `pose(angles)` and `status(..)` no longer take `t`. (It
  briefly owned a shared run clock too; superseded by "Wall-clock log time" and "Recorder holds no clock".)
- `src/devices/discovery.rs` holds all BlueZ lookup: `Found`, `find(session, want)` and `ensure_supported` (moved
  out of main). `src/devices/mod.rs` is only the backend registry.
- `session::run` returns `Result<Infallible>`, so main has no "session ended without an error" branch.

## 2026-11-12: Wall-clock log time (user)
- `t` in `--log` and `--raw-log` is wall-clock Unix time in seconds, rounded to the millisecond, read when the line
  is written. Lines from both logs, from different runs appended to one file, and from the journal line up. This
  stays right across suspend, where a monotonic clock stops.
- Session timing (detector, calibration, timeouts) stays monotonic, because the wall clock can jump.
- `DeviceLog::new(file)` again; it no longer shares a clock with `Recorder`. Unix seconds rather than RFC 3339 text:
  no date crate, and analysis needs only subtraction.

## 2026-11-12: Posture logic redesign (user: same principle as devices)
- Smoothing is by time: weight `1 - exp(-dt / smoothing_s)` with `smoothing_s` = 0.123 s, which equals the old
  per-sample α 0.15 at 50 Hz. A tracker at another rate now gets the same smoothing. Test
  `smoothing_is_independent_of_sample_rate` (25 vs 100 Hz) fails with the per-sample weight (18.7° vs 29.4°).
  All FreeClip 2 replay tests pass unchanged. Pose bursts that arrive at one instant (a re-centre releasing held
  samples) now barely move the smoothed value; the next sample after the gap that follows moves it fully.
- The detector no longer knows about trackers: `Config::for_traits` became `session::detector_config`. The
  end-to-end test uses it, so it still runs with the device's real traits.
- `Quat::mean` (sign-aligned normalised sum) moved from the session to quat.rs; the session keeps the p90 check.
- `phrases!` in cues.rs generates `Phrase`, `Phrase::ALL` and the clip table from one list, with an exhaustive
  match (no runtime lookup). Adding a prompt takes one line plus its text in tools/gen_voice.sh.
- Kept: the escalation state (`Posture`, `Episode`), the session's step/calibration/monitor structure, wav.rs, and
  cue playback through files in XDG_RUNTIME_DIR.

## 2026-11-12: Recorder holds no clock (user)
- Console lines no longer carry a time (they were `"  12.3s  Tone"`; now `Tone`, and the status line is
  `pitch 12.3°  ok`). Under systemd the journal stamps each line, and `--log` has wall-clock `t`.
- With no time to show, `Recorder` has no clock. The session owns its monotonic clock again (`Session::start`).
- `AGENTS.md` (repo root) collects the project's working rules for coding agents; it points to these docs rather
  than repeating them.

## 2026-11-12: Hold samples by count; poses carry measurement time; reminders on time (user)
- Why: the earbuds' rate varies (35 Hz in the live check, 52.5 Hz before; FINDINGS "Sample rate varies") while a
  glide stays ~68 samples, so the 2.0 s hold (`DELAY_S`) had only 20–50 ms of margin at 35 Hz.
- `Rebuilder` holds each sample until a sample `GLIDE_SAMPLES` (70) newer arrives, the same window it drops at a
  re-centre, so it is right at any rate. At 52.5 Hz samples now come out after ~1.3 s instead of 2.0 s. If nothing
  arrives for `QUIET_S` = 5 s, everything held is released (the stream stopped without a re-centre). 5 s is longer
  than the longest transport stall seen (3.65 s), so a stall in the middle of a glide cannot let it out. (Refined
  the same day: see "Shape-aware release when the earbuds go quiet".)
- `Event::Pose { at, orientation }`: `at` is when the pose was measured, and the detector smooths and times the
  slouch on it. Found while building the above: releases arrive in bursts (at a re-centre, or at the quiet
  fallback), and with arrival times the time-based smoothing gave a burst almost no weight. The stream-restart replay
  lost its slouch entirely. `Recorder::pose` logs `t` as the measurement time.
- Reminders are due on measured slouch time. During silence the monitor wakes at `Detector::next_due()` instead of
  the next 1 s poll; the live check's tone came 1.0 s late for this reason. (Clock refined in "Reminder timing".) `Episode::pending` lists due times for
  both `tick` and `next_due`.
- Tests: `hides_slow_glide_at_any_sample_rate` (35 Hz, 3 s stall mid-glide; fails with the 2.0 s hold) and
  `tone_is_on_time_while_head_is_still` (fails with the 1 s poll: tone at 15.0 s instead of ~14.1 s), plus
  `next_due_tracks_the_next_reminder` and `live_35hz_session_end_to_end` on the new fixture
  `tests/fixtures/freeclip2/live_35hz.csv` (the live check; the old hold also passes it, narrowly).
- Rejected: a 2.0 s quiet fallback. It was needed only while poses carried arrival time; it reopened the stall gap.

## 2026-11-12: Shape-aware release when the earbuds go quiet (user) — superseded by "Model-based glide release"
- A glide is a straight ramp to identity (FINDINGS "Re-centre model"), so when the earbuds go quiet only a trailing
  run of consecutive samples that steadily approach identity can still be an unfinished glide. After `QUIET_S` =
  0.2 s of silence, everything before that run is released; the run waits up to `GLIDE_STALL_S` = 5 s for its
  identity sample. A slouch held still is flat, so it comes out after 0.2 s instead of 5 s.
- `QUIET_S` affects only latency (anything it releases cannot be an unfinished glide); it is above the largest gap
  seen between flowing samples (74 ms). `GLIDE_STALL_S` is the only value that guards correctness, and now applies
  only to a suspected unfinished glide; it is above the longest stall seen (3.65 s).
- Known limit: a very small glide whose steps are below sensor noise may not look like a steady approach, and part of
  it can be released during a stall. The error is at most that glide's size (the one seen was 2°).
- Test `releases_still_pose_soon_after_the_stream_goes_quiet`; `hides_slow_glide_at_any_sample_rate` still passes
  with a 3 s stall in the middle of a 35 Hz glide.

## 2026-11-12: Reminder timing: measured time, confirmed posture (user chose confirmed)
- First version (same day): reminders played on time, judged on the posture known so far (up to the backend's hold,
  ~1.3–2 s, old). Sitting up in that window just before a reminder was due gave the reminder and then the tink.
- Now: the detector runs on measured time and never ahead of delivered posture. During silence its clock is the last
  pose's measured time plus the silence since that pose arrived (`Delivered::clock` in session.rs), and the monitor
  wakes when that clock reaches `Detector::next_due()`. Reminders come after the hold (~1.3 s at 52.5 Hz, 2 s at
  35 Hz), never after the user has sat up. A threshold nobody can feel (10 s vs 11.3 s) is cheaper than a reminder
  that reads as wrong.
- Tests: `no_reminder_after_sitting_up_when_poses_arrive_late` (fails on the on-time version: Tone then Recovered)
  and `late_poses_delay_the_reminder_by_the_lag`. The fake tracker now honours the wait timeout between samples like
  a real backend, and can deliver poses late (`lag`). The end-to-end replay helper ticks on the same clock.
- Prior art (docs/UX_RESEARCH.md "Reminder timing in the open-source apps"): HeadOrbit times the dwell on sample
  timestamps; the other four use wall-clock time at processing. None buffers samples, so the edge case does not arise
  for them.

## 2026-11-12: Thermo review applied (user approved all, including two new abstractions)
- Review: scratchpad report (two independent reviewers; summary `00_summary.md`). Findings checked against the code
  before acting.
- Session: one `Interrupt { Removed, TrackingLost, Fatal }` carried by `?` replaces `Step`, `MonitorEnd`,
  `Calibration` and their `Removed` arms. `Timeline` holds the session's one clock plus the latest pose's delivery
  lag (`known_until = now - lag`); it replaces the `Delivered` struct. A pose from before the session or out of order
  is an error at this boundary (no silent clamps); the detector asserts monotonic time.
- Calibration windows on measured time: the window opens when the prompt ends (`now`), ignores poses measured before
  it, and closes when the delivered posture covers 3 s. Test `calibration_ignores_poses_measured_during_the_prompt`
  (failed before: a spurious "Moved").
- Detector: reminders are data (`Timer { cue, due, every }`), removing `pending`, `had_cue` and an `unreachable!`.
  `upright()` removed; tests check `Update::reanchored`.
- FreeClip 2: `protocol.rs` parses each frame once into a typed `Message` (a malformed 43/54 is now an error like the
  others; all 10 real 43/54 frames in data/ had TLV 5 — wrong as a rule: see "43/54 has more than one form"); `link.rs` is the protocol without I/O, with one ordered
  event queue (no hand-paired reset of rebuild and pose queue) and frames stamped when read; `mod.rs` is the socket.
  `Link` unit tests cover the startup frames, the re-wear restart, the v0.1.2 resend when this computer starts audio,
  malformed frames and pose stamps. Rebuilder: `Pending { BeforeGlide, All }` replaces a `0` sentinel.
  Rejected: the reviewer's "Rebuilder owns its ready queue" (one ordered queue in `Link` also keeps event order).
  Rebuilder stays in `f64` seconds; `Link` converts at one place.
- Tests: recordings replay through the real `Link` (`Replay`) and the real session (`ReplayTracker`,
  `testing::run_session`); the copies of the backend's timer loop and the session's clock are gone. CSV via the
  `csv` dev-dependency. The session harness records each cue once and parses the log with serde_json.
- main: one `attempt()` returning `Attempt { Stopped, Waiting, Failed }`; Ctrl-C is checked in three places instead
  of five bool checks.
- Smaller: `LogEvent::parts` (one match), `Quat::dot`, `STATUS_EVERY_S`, documented cue stderr and discovery soft
  fail. The notification text no longer says "for a minute" ("for a while"): the duration is the detector's setting.

## 2026-11-12: Model-based glide release (user)
- The rebuilder now uses the measured glide model (FINDINGS "Glide model checked against every recording"): from the
  pose `P` held before a glide, sample `k` reads `slerp(P, I, (k+1)/69)` and sample 68 is the identity sample
  (`GLIDE_LEN` = 68). It keeps glide hypotheses ("a glide started from this pose"), each alive while every later
  sample fits within `FIT_TOL_DEG` = 0.5° (largest clean misfit: 0.42°). Poses within 0.5° of identity get no
  hypothesis: a glide from there is indistinguishable from holding still.
- Known limit of that rule: a glide from within 0.5° that a transport stall (> `QUIET`) cuts off before its identity
  sample gets partly released, and the next reference is taken from the partly glided pose: an offset of at most
  0.5° until recalibration, in either direction (re-anchoring corrects only offsets towards "looking up"). It needs
  a rare stall to hit a tiny re-centre; none of the 8 stalls in the recordings did. Tracking hypotheses inside the
  noise band instead brought back samples held forever and wrong drops.
- While samples flow, every sample is still held `GLIDE_SAMPLES` = 70 samples: that covers glides the earbuds cut
  short because the head moved (16%; FINDINGS "Head movement during a glide"), which fit no hypothesis.
- After `QUIET_S` = 0.2 s of silence, everything up to the earliest live hypothesis's start pose is released. What
  remains waits for its identity sample, with no time limit: the model says the head is still at `P`. `GLIDE_STALL_S`
  and the "steadily approaching" shape rule are gone.
- On an identity sample, the samples of the hypothesis that predicts it are dropped (67 per clean glide); if none
  does, the 70 before it, as before. On a new stream, a live hypothesis's samples are dropped (the head was still).
- Live recordings: stream_restart.csv's 47 look-down samples all come out 0.2 s after the earbuds go quiet (four used
  to wait 5 s). New tests, each failing on the previous rebuilder: noisy still pose and sit-up both released at once,
  a glide survives an 8 s stall, a stream restart mid-glide hides the glide. Drop counts in the recording tests now
  follow the model (5 × 67 for hold_test, 4 × 67 + 1 for live_35hz).
- Found on the way: `Link::on_timer` called at exactly `next_wake()` could release nothing (seconds → Instant →
  seconds lands up to a nanosecond early), so a caller looping on `next_wake` spun forever; the replay helper did.
  `on_timer` now releases with the due time once its Instant is reached. Test `timer_at_next_wake_makes_progress`
  (fails before). Production was very unlikely to hit it (tokio timers round up to milliseconds).
- Rejected: the reviewer-era "release everything after 5 s"; a minimum-travel threshold for the old shape rule
  (another number, and the model makes it unnecessary).

## 2026-11-12: Exact time in the timing path (user)
- Cause: the rebuilder kept time as `f64` seconds and `Link` converted it to an `Instant` and back. The round trip can
  land up to a nanosecond before the due time, so `on_timer` at exactly `next_wake()` released nothing and a loop on
  `next_wake` spun forever. The session had the same shape, papered over with a 1 ms minimum wait.
- Fix: one exact time type. `Rebuilder`, `Link`, `Detector` (timers, settle window, `next_due`) and the session's
  `Timeline` work in `Duration` since their time zero; `Instant` and `Duration` arithmetic is integer nanoseconds, so
  reaching a due time always fires it. The patch in `Link::on_timer` and the session's 1 ms floor are gone.
  Detector `Config` fields are `Duration`s (`smoothing`, `tone_after`, `speak_after`, `notify_after`, `renudge`).
  Seconds as `f64` remain only inside the smoothing formula and in test scripting.
- Test `timer_at_next_wake_makes_progress` (failed with the float rebuilder) passes without the patch; the pose-stamp
  test now asserts exact equality.
- Tests too: the session tests' `Script`, fake tracker and `Sim`, and `testing::run_session` (prompt length, cue
  times) use `Duration`; script literals go through `secs(x)` (whole milliseconds), and a pose span that is not a
  whole number of 20 ms samples fails loudly. The replay driver (`Replay`, `ReplayTracker`) keeps `Instant`s
  throughout; it had the same float round trip around `next_wake` and only passed by luck. Recording timestamps
  (float seconds in the CSVs) are converted once, when loaded (`Row.t`, `Out.t` and phase starts are `Duration`).
- Floats remaining on purpose: the smoothing formula, wall-clock Unix seconds in the logs, and display.

## 2026-11-12: 43/54 has more than one form (live check)
- The live check (data/live3*.jsonl) got a 43/54 with only TLV 1 = `01 00`, 13.8 s after connecting. v0.2.0 treated
  a 43/54 without TLV 5 as malformed, so the session ended and headup reconnected 5 s later (and recalibrated).
  v0.2.0 has this bug.
- The vendor app names 43/54 "connection management: paired-device-list change report"; the TLV 5 form (address +
  audio state) is one variant. A 43/54 without TLV 5 is now `Message::Other` (still in `--raw-log`); a TLV 5 that is
  too short stays an error. Test `other_device_list_reports_are_not_errors` uses the real frame (fails before).
- Lesson: ten samples of a message do not define its format. "Fail loudly" applies to frames that break the format
  headup relies on, not to variants of a command it only partly uses.

## 2026-11-12: Readability refactor of detector, rebuilder and session (user)
- No behaviour change: the 61 tests, including every recording replay, pass unchanged.
- Detector: `Smoother` (time-based EMA, owns its state) smooths pitch relative to calibration; upright is a plain
  offset subtracted afterwards, so re-anchoring no longer shifts the smoothed value (EMA is linear: identical
  results). Posture is one `State` enum {Upright, Settling, Slouching} (no `Upright(Option<Settling>)`), with all
  threshold transitions in `State::step(self, …) -> (State, cues)` and the settle-and-re-anchor transition in
  `State::settle`. `update` is three visible steps: smooth, settle, step.
- Rebuilder: `GlideFilter` (raw samples only: held queue, `Hypotheses`, the last sample; `Arrival` classifies each
  sample as NewStream / GlideEnd / Sample) emits an ordered list of `Step` {Release, Rebase}; `Stitch` applies them
  (`absolute = reference × raw`, reversed to `raw × reference` on 2026-11-12, see "Stitch composes on the right";
  `Rebase` sets reference = last absolute). The "reference changes only after the held
  samples are out" rule is now the order of the steps, not the order of three calls.
- Session: `TrackerStatus` (worn, unavailable-since, and their rules: a pose proves tracking; only worn → removed
  interrupts; lost after TRACKING_LOST) and `CalibrationWindow` (opens when the prompt ends, counts poses measured
  inside it, closes when the delivered posture covers CALIBRATION, gives mean and p90).
- Rejected: typestate (states as types). It fits lifecycles the code controls (open before read, builders); here
  the state is decided by sensor data at runtime, so it would need a boxed or enum wrapper around every state type
  anyway. Explicit enums with transition functions say the same thing without that ceremony.

## 2026-11-12: Read the 43/88 reply; a refusal makes tracking unavailable
- The firmware's 43/88 handler answers TLV 2 = 0 (accepted), 2 (refused: this computer is not the earbuds' current
  device, the last one to start streaming) or 1 (refused while the earbud-to-earbud link is down). A
  refusal leaves the stream off (FINDINGS "43/88 (switch the data report on or off)"). Seen live: `2=02` at connect
  while the phone was last active, then `2=00` after 43/54 reported this computer streaming (data/live2_raw.jsonl).
- headup parsed no reply before, so after a refusal the session took the silence for a still head. Now
  `Message::StreamSwitch(SwitchReply)` parses it; any other value is a malformed frame. A refusal marks this computer
  as not streaming and reports `Available(false)`, so the session says tracking stopped. The existing 43/54 path
  switches the stream on again (and reports `Available(true)`) when this computer starts streaming, and re-wearing
  does too. `Accepted` reports nothing: the stream still needs the earbuds' own conditions.
- Test `refused_stream_switch_waits_for_this_computer_to_stream` (fails without the change) uses the reply values from
  the recording.
- Rejected: retrying 43/88 on a timer after a refusal. The firmware would refuse it again until this computer
  streams, and the 43/54 push already marks that moment.

## 2026-11-12: Firmware re-check is a skill with tools, gated by a live test
- New firmware can change the protocol under headup. The re-check is `.claude/skills/freeclip2-firmware/`:
  fetch (`tools/freeclip2/firmware/fetch.py`, after asking the user), unpack (`tools/freeclip2/firmware/unpack.py`), disassemble (`tools/freeclip2/firmware/disasm.py`), diff anchors
  (`tools/freeclip2/firmware/anchors.py`, baseline `docs/devices/freeclip2/firmware/anchors-526.txt`), then the guided live test
  (`grounding_test.py`) against FINDINGS "Grounding of headup's device logic".
- Each tool fails loudly on a layout it does not recognise (hashes, load base, one dispatch table), so a changed
  firmware stops the procedure instead of producing a wrong reading.
- The live test is required, not optional: the code reading alone produced a wrong conclusion ("R = spatial mode",
  refuted by `data/mode_gate_live.jsonl`), and the re-centre math has not been read from the firmware (it is in
  image 5's RAM code; FINDINGS "Where the head-tracking computation is"), so recordings remain its only evidence.
- Rejected: a Ghidra processor module for this custom-instruction variant. objdump plus `tools/freeclip2/firmware/xlinx.py` was enough
  to read the needed functions, and the module would be a large unverified artefact.

## 2026-11-12: Research tools grouped by purpose
- `tools/freeclip2/` mixed firmware analysis with scripts that talk to the earbuds. Now: `firmware/` (fetch, unpack,
  disasm, anchors, xlinx; the `fw_` prefix dropped), `device/` (probe and the live tests; they share `device/mbb.py`,
  so they stay in one directory), `app/` (vendor app analysis). All current references were updated; SESSION_LOG
  keeps the old paths as history.

## 2026-11-12: Re-send the stream switch only if it was not accepted
- Replaces "always re-send on 43/54 streaming" (2026-11-11). The firmware answers a 43/88 from a device that is
  not its current one with `02` and keeps its switch off; an accepted switch survives audio gaps (the earbuds
  resume the stream by themselves on 43/54 `…09`; data/grounding_test.log "audio_gap", firmware `0x7039b5ae`).
  A 43/88 "on" always restarts the stream (seq from 1), and every new stream brings an extra re-centre (~10° of
  error each, FINDINGS "Re-centre model"), so the unconditional re-send cost accuracy for nothing.
- `Link` now keeps whether the switch is accepted (`accepted`): 43/54 streaming re-sends only when not accepted;
  putting the earbuds back on always re-sends (re-wearing cancels the switch: data/grounding_test.log "rewear").
- Live check (data/gap_live*.jsonl, second run from 84 s): audio stopped → `TrackingUnavailable`; audio back →
  `TrackingAvailable`, samples 0.03 s later, no 43/88 sent. Tests `audio_resuming_after_an_accepted_switch_keeps_the_stream`
  (fails before) and `audio_starting_before_any_reply_switches_on_again`.
- 43/88 reply value 1 was briefly renamed `OtherRefusal`; the firmware later showed it is the earbud-to-earbud link
  being down, so it is `PeerLinkDown` again ("Firmware-exact glide model, 43/54 decoding, reply 1" below).
- Rejected: dropping the re-send entirely. A refused switch needs it (data/refusal_live*.jsonl).
- When another device starts streaming, an accepted switch becomes `Unconfirmed` and is re-sent once this computer
  streams again: whether it survives a phone takeover could not be observed (data/grounding_test2.log), so this case
  keeps the earlier behaviour. Test `switch_is_resent_after_another_device_streamed`.

## 2026-11-12: Firmware-exact glide model, 43/54 decoding, reply 1
- Glide model: the firmware's re-centre (`0x600faa4e`) outputs `normalize(t·q + (1−t)·I)` with `t = (n−1)/69`
  (nlerp), not slerp. `rebuild.rs` now uses that exact formula (`glide_point`); on 50 clean recorded glides it fits
  within 0.126° (slerp: 0.50°, right at `FIT_TOL_DEG`), so the 0.5° tolerance has 4× margin. Tolerance and
  `GLIDE_LEN` unchanged (both grounded). The FreeClip-specific `Quat::toward_identity` left `src/core/` (core names
  no device behaviour). Test `hides_glide_from_a_large_angle` builds a 90° glide from the firmware formula; it
  also passes with the slerp model (other hypotheses absorb the gap), so it guards behaviour, not the change.
- 43/54: the firmware builds the state byte as `connected | audio << 1` (audio of the current device; the
  Bluetooth core folds 5 and 6 into 3, other values pass through). `protocol.rs` reads `streaming = audio == 4`
  instead of "bit 0x08", and any other value as not streaming (a variant, not a broken frame); same
  result on every reachable value, but it states the encoding.
- Reply value 1 is `SwitchReply::PeerLinkDown` again: the firmware answers 1 while message 0x8406 reports the link
  to the stored peer-earbud address disconnected.
- Kept: re-sending 43/88 when this computer streams after another device took over. The code suggested the
  firmware re-derives `enable` itself on a device change; live, it does not resume the stream, so the re-send is
  required (data/grounding_test3.log; "Live grounding test 3" below).
- Rejected: tightening `FIT_TOL_DEG` to match the new fit. No recording shows a false glide at 0.5°, and the
  tolerance also absorbs f32 rounding of real samples.

## 2026-11-12: Review of the switch and glide code (independent code-quality review)
- (Superseded by "Link state: one switch enum, derived availability" below.) `Link`: the three-state switch had a
  `Refused` variant that changed nothing; it is now `accepted: bool`, cleared
  when an earbud comes off (where the earbuds cancel it) as well as when another device streams. `current` became
  `available` (the last reported availability), since a `PeerLinkDown` refusal says nothing about which device is
  current. Reply handling is an exhaustive `match`.
- `SwitchReply::NotCurrentSource` → `NotCurrentDevice` (one term throughout); variants in wire order; firmware
  internals moved out of the variant docs into FINDINGS.
- `glide_point` asserts `k ≤ GLIDE_LEN` instead of saturating (an impossible case fails loudly).
- `hides_glide_from_a_large_angle` now holds the pose for 220 samples with 6.6° of drift (inside the firmware's
  208-sample, 10° stillness rule) and selects outputs by time; its oracle uses literals only.
- Not taken: erroring on 43/54 `audio` values other than those seen. The firmware passes them through, so they are
  valid variants; erroring would turn an unseen state into a session reset.

## 2026-11-12: Link state: one switch enum, derived availability (user: same strategy as session, detector, rebuilder)
- `Link` kept `available: Option<bool>` with two jobs: the last reported `Event::Available` and a fact that a 43/88
  refusal overwrote. Each handler compared and pushed its own event. Now the facts are separate: `streaming` (this
  computer, per 43/54) and `switch: Switch` {`Unconfirmed`, `Accepted`, `Refused`}. Availability is derived
  (`Refused` → false, otherwise `streaming`), and one step after each frame (`report_availability`) emits the event
  when it changes. Handlers only update facts and send frames.
- Transitions: sending "on" → `Unconfirmed`; reply 0 → `Accepted`; reply 1 or 2 → `Refused`; an earbud off or another
  device streaming → `Accepted` becomes `Unconfirmed` (a refusal stands). 43/54 for this computer re-sends when
  `Refused` and streaming, or `Unconfirmed` and streaming just started; never when `Accepted`. Same as before on every
  existing test.
- Behaviour fix the old fields hid: after a refusal while this computer streams (reply 1, earbud-to-earbud link down),
  putting the earbuds back on re-sent the switch, but availability stayed false even after it was accepted, until
  this computer's audio stopped and restarted. Now it becomes available when the switch is re-sent, as on the 43/54
  path. Test `rewearing_after_a_refusal_makes_tracking_available_again` (fails on the old code). The session's
  "a pose proves tracking" rule already recovered once poses flowed, so the user-visible effect was limited to the
  status. [inferred: reply 1 has not been seen live.]
- Rejected: a separate status type returning actions (like the session's `TrackerStatus`). `Link` is already the
  I/O-free type; a second layer would only forward its fields.
- Review follow-up: kept three switch states. A split of `Unconfirmed` into `Pending` (sent, unanswered) and
  `Lapsed` (accepted, then cancelled) was proposed so the "streaming just started" check could go. Rejected: a
  pending switch must still be re-sent when this computer starts streaming, because a refusal can be in transit
  (sent while another device was current; the 43/54 push can arrive first), and a lapsed one is re-sent at the same
  moment in practice, so the split changes nothing reachable and keeps the check. No retry timer: the firmware's
  43/88 handler (`0x7039c018`) answers every well-formed request (silent only on internal copy/store or allocation
  failures), and 13 of 13 live requests were answered within 80 ms (data/grounding_test*.log).
- Also from the review: `Link::new` sends its first switch through `switch_on`; `Quat::normalized` (generic, in
  core) replaces three copies of the normalise step.

## 2026-11-12: Research tools: shared modules, checks in code
- The FreeClip 2 Python tools had copies where one answer belongs, and several clean exits that hid a failure. Now:
  - `device/mbb.py` holds framing (one CRC-checking frame reader that raises on desync or a bad CRC), connection,
    `say()` and tool checks; probe, headpose, guided, glide_move and the phase tests import it. Before, five
    scripts carried their own reader, three of which skipped the CRC and dropped bytes on desync.
  - `device/runner.py` runs a scripted phase test. `grounding_test.py`, `single_bud_test.py` and `wear_test.py` are
    schedules of it, with their CLIs and log line format unchanged so old `data/` logs still compare. Every test
    stops with a non-zero exit when the link drops; unknown action names fail before connecting; grounding_test
    checks that its silence player still runs.
  - `quat.py` holds `GLIDE_LEN` and the firmware's nlerp glide for both glide scripts. `glide_move.py` used slerp
    before; on every recording both detect the same 31 glides.
  - `firmware/listing.py` is the one parser of disasm listings (address, raw word, mnemonic, operands, objdump's
    resolved address, our annotation), used by disasm.py and anchors.py. `xlinx.py` exposes `annotate()`;
    disasm.py imports it instead of running it as a subprocess and keeps its temporary ELF files out of OUT_DIR.
  - `anchors.py` reads the load base from `body.asm` (a wrong base given by hand produced a plausible report and
    exit 0) and checks the invariants the skill listed for the operator to read off the diff; it exits non-zero
    after the report if one breaks. The baseline `anchors-526.txt` gained the `check:` lines and says "none"
    instead of "NOT FOUND"/"NONE", which had meant "fine" on some lines and "broken" on others.
  - `fetch.py` fails if the file list does not name the zip; `unpack.py` reads the LZMA dictionary size from the
    header and fails unless it is 0x2000 (FINDINGS); `glide_check.py` fails instead of omitting a line when no
    glide fits.
- Checked on 7.0.0.526: the disassembly is byte-identical apart from objdump's header line (the ELF path), the
  anchors report differs only in the format changes above, and glide_check prints the same numbers. The load-base
  vote count moved from 1160 to 1165: the old regex skipped the line after every instruction without operands.
- Rejected: one register-constant tracker for the whole image. The load-base vote counts a lui at its first addi;
  tracking registers through the image changed the counts (1160 → 1178), so straight-line code (startup stub and
  copy loops) uses `listing.constants` and the vote keeps its own rule.
- Rejected: a `--schedule` flag on one test script. Separate scripts keep the commands in FINDINGS and old notes
  valid.

## 2026-11-12: Link as state, view and reported changes (user: Link was hard to read)
- `Link` mixed three jobs in one set of fields: what the earbuds said (`worn`, `streaming`, `switch`), what the
  session had been told (`reported`, and `worn` again for deduplication), and when to send. Each handler updated
  facts, sent frames and pushed events in its own way.
- Now: `Earbuds` holds only what the earbuds said and the switch's fate. Each message handler updates it and may
  return a `Request` (`SwitchOn`, or `Restart` = fresh orientation frame + switch on); `Earbuds::view` derives what
  the session sees (`View { worn, available }`). `Link::on_frame` is three steps: the earbuds take the message, the
  link carries out the request, `report` tells the session each view field that changed (`changed`, one rule for
  both events). No test changed; all 69 pass.
- The pattern is a reducer with a derived view (as in the Elm architecture): state transitions in one place, outputs
  computed from state rather than stored, events as differences between views.
- Rejected: level-triggered sending (send whenever the view says the stream should be on and is not). A refusal
  while this computer streams (reply 1) would be re-sent at once and refused again in a loop; the send rules need the
  message that triggered them, so they stay edge-triggered in the handlers.
- Rejected: one enum for the whole link state. Wear, audio and the switch change independently; their product
  (3 × 3 × 3 states) would be harder to read than three fields.


## 2026-11-12: Stitch composes on the right (firmware)
- The earbuds report `raw = q ⊗ conj(ref)` (re-centre stage `0x600fa53c` calls `0x600ff6b0` with the input first
  and the reference at `S+40` second; `0x600ff626` computes `a ⊗ conj(b)`). So `q = raw ⊗ ref`, and the rebuild is
  `absolute = raw × reference`; `Rebase` (reference = last absolute) is unchanged. headup had `reference × raw`,
  which equals it only for rotations about one axis: a nod after a turn read about the wrong axis.
- Effect, through the real `Rebuilder`: guided run 1 (headpose_axes.csv) upright pitch after the turns and tilts
  8.5–9.3° → 0.9–4.4°; run 2 about the same. Test `nod_after_a_turn_reads_as_pitch` (60° turn, re-centre, 30° nod:
  old order 14.5° pitch and −26.6° roll; new 30°). `rebuilds_guided_run_fixed_mode` tolerance 8° → 9°: its last
  center phase reads roll −8.3° (was −7.6°), pitch −1.3° (was −2.4°).
- Not fixed by this: the ~10° per re-centre "toward looking up". It is movement the earbuds absorb into their
  reference without sending it (silent re-centres every 209 samples while paused, slow drift during a countdown;
  FINDINGS "Re-centre and stillness", "Ground truth"), so it is not in the stream. Re-anchoring after recoveries
  stays.

## 2026-11-12: Live grounding test 3 (refactored tools, phone takeover returned)
- data/grounding_test3.log, the first run of the refactored device tools (runner.py, mbb.py): exit 0, every phase
  matches the 526 baseline run (data/grounding_test.log) in pose flow and replies.
- New: the phone took over and then paused, and this computer became current again (43/54 `…09`). The earbuds did
  not resume the stream for 9 s, until a new 43/88 (accepted, seq from 1). So an accepted switch does not survive a
  takeover; `Link` cancels it when another device streams and re-sends when this computer streams, which is now
  grounded. This overrides the code reading of the device-change path (`0x7039be0c`).
- Still not seen live: 43/88 reply 1 (a closed case with one earbud in it still gave `00`).

## 2026-11-12: Live headup run 4 (Link restructure, stitch order)
- data/live4*.jsonl, release build with the restructured `Link` and `raw × reference`: calibration first try
  (p90 0.4°); look down → tone, recovery, re-anchor −4.8°; turned ~50° left then looked down → tone, recovery,
  re-anchor −5.2°; an earbud off → Paused, on → Resumed and recalibration; clean Ctrl-C (exit 0).
- Turned left and looking down, the rebuilt pose read pitch 31.9°, yaw 49.0°, roll 2.2°. The same samples stitched
  in the old order read roll −25.2°, pitch 27.8°, yaw 36.9° (a tilt that did not happen).
- Phone takeover: this computer's audio restarted while the phone still played, so when the phone paused the
  earbuds did not make this computer current (no `…09` for it); headup stayed unavailable, correctly. Getting
  tracking back then needs audio started on this computer after the phone stops (device rule: the last device to
  start streaming is current).

## 2026-11-12: Recalibrate after every unavailability (user; fixes a missed tracking loss)
- Bug (user report; service log 22:24:57–22:29:49, ~5 min unavailable, and data/live4.jsonl, ~75 s): no
  `TrackingLost` and no recalibration after a long phone takeover. Cause: every pose cleared the session's
  unavailability ("a pose proves tracking works"), and poses still arrive after `Available(false)`: the rebuilder
  releases samples it was holding, and the earbuds can send a few more (5 after this computer's `…03` in
  data/grounding_test3.log). Once they stopped, the silence was taken for a still head indefinitely.
- The 30 s grace itself was a judgement call with no data behind it, and it rests on an assumption nothing checks:
  every resumed stream measures from the head pose at its start, so carrying the posture over assumes the head did
  not move during the gap. Using the phone usually breaks that, whatever the gap's length.
- Now (user's choice): `Available(false)` ends the cycle at once (`Interrupt::Unavailable`): the slouch state is
  dropped and headup calibrates again once tracking resumes. The calibration waits for `Available(true)`; poses
  that arrive while unavailable do not start it. `TRACKING_LOST`, the unavailability timer and the `TrackingLost`
  log event are gone (`TrackingUnavailable` now means it).
- Track skips do not trigger it: the earbuds report this computer's audio stop ~5 s after it stops
  (data/grounding_test3.log, two cases at 5.07 s); the shortest such gap in all recordings is 6.6 s.
- Tests changed (behaviour change): `unavailability_drops_the_slouch_and_recalibrates` (fails on v0.3.0: a tone from
  stale posture, no recalibration) replaces the short/long unavailability tests;
  `poses_after_unavailability_do_not_start_calibration` (fails without the wait for `Available(true)`);
  `poses_end_unavailability_without_announcement` removed: a tracker must announce `Available(true)`.
- Rejected: decide by what happened during the gap (recalibrate after another device took over, carry over short
  laptop pauses). Re-starting playback after stopping a video usually takes longer than a short limit anyway, so it
  would behave like this rule with an extra tracker event.
- Rejected: keeping 30 s and only fixing the timer (an intermediate fix this session): it keeps the unchecked
  assumption.
- Live check (data/live6*.jsonl, silence played by the test, gaps made by stopping it): gaps of 0.5, 1, 2, 3 and 4 s
  gave no 43/54 for this computer; 7 s and 15 s gave `…03` at +4.90 s and +4.93 s, then `…09` on resume. So a stop
  is reported only after ~4.9 s without audio, and short gaps such as a track skip never interrupt. The head was
  still, so whether a short gap restarts the pose stream was not observable (the 15 s gap did: seq 279 → 227).
- Found in that run: after an interruption, the restarted cycle waited for a first pose before prompting, and a
  still head sends none, so no prompt came until the user moved (v0.3.0 had the same wait after a 30 s loss: service
  log 22:31:22 available → prompt ~33 s later). Fixed by "Calibration asks a still head to move" below.

## 2026-11-12: Calibration asks a still head to move (user)
- A calibration must measure the head after the prompt. A briefly tried fix calibrated from the latest pose when
  none arrived; the user rejected it: it defeats the purpose of recalibrating. Relying on the tracker to send poses
  by itself (FreeClip 2's burst at a new stream) is a device habit, not a guarantee.
- Now: once tracking has started (a pose or `Available(true)`), a calibration prompts as soon as it is available (after a
  pause, after re-wearing, after a failed calibration's back-off). Only poses measured after the prompt count. With
  none 5 s after it, a new prompt asks "Move your head a little, then hold still, looking straight ahead."
  (`Phrase::MoveHead`), repeated every 15 s while none arrive.
- `CalibrationAttempt` replaces the fixed 3 s window: it succeeds as soon as the delivered posture shows the head
  steady for 3 s (the latest pose measured at least 3 s before the known end, and every pose after it, p90 under
  8°; silence after a pose counts as holding it). So moving when asked, then holding still, calibrates without
  "You moved". An attempt fails 5 s after its first pose without a steady 3 s; four attempts (~20 s), then the 60 s
  back-off as before. The empty-window fallback (use the last pose) is gone.
- At startup, before any pose or `Available(true)`, headup still waits silently ("waiting for head tracking…"
  note): silence may mean tracking has not started (no audio), so asking the user to move would be wrong.
- The tracker's state is one enum in `TrackerStatus` (user: it does not belong loose in the session):
  `Tracking { Starting, Available, Unavailable }`. A pose ends `Starting`; `Available(true)`/`(false)` set the
  other two; a pose during `Unavailable` changes nothing; only leaving `Available` interrupts a cycle. It replaces
  `available: bool` plus `tracking_seen: bool`, which encoded the same three states with two flags.
- Tests: `still_head_is_asked_to_move_when_tracking_resumes`, `still_head_is_asked_to_move_after_rewearing`,
  `nudge_repeats_while_no_pose_arrives` (fail on the previous code); `startup_waits_silently_for_tracking` (fails if
  the session prompts before the first pose). Changed: `failed_calibration_retries_later` (the head now moves for 30 s
  to fail four 5 s attempts; the retry is nudged because the scripted head is still).
- Voice: `assets/voice/move_head.wav` generated alone with the `tools/gen_voice.sh` command and voice, so the other
  clips are unchanged.
- Rejected: re-sending 43/88 to force a burst of poses (device-specific, adds a re-centre error each time).
- Live check (data/live7*.jsonl, automated, silence played by the test): after an 8 s audio pause with a still head,
  tracking was available again at 43.0 s; the prompt ran to 45.5 s; no pose came, so the nudge played from ~50.5 s
  (4.2 s clip); the first poses arrived at 54.9 s and calibration succeeded (39 samples, p90 7.2°, no "You moved").
  Re-wearing calibrated on the new stream's poses (93 samples, p90 0.2°) without a nudge. The 8 s pause kept the
  same stream (seq continued at 2866), so not every gap restarts it.
