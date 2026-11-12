# Session log

## 2026-11-10 23:10 UTC
- Cloned Gadgetbridge and OpenFreebuds. Both support FreeClip 2 over SPP port 1, with no sensor features.
- Downloaded the official Audio Connect APK from the AppGallery CDN and decompiled it with jadx.
- Found the MBB command catalogue (`Mbb.java`) and the motion-related commands. Confirmed the app never
  sends them (`tools/freeclip2/used_cmds.py`). Wrote `docs/devices/freeclip2/FINDINGS.md`.

## 2026-11-10 23:40 UTC
- Hardware test via BlueZ RFCOMM ch1. 43/90 reports spatial support. 43/88=1 starts a 43/89 quaternion
  stream at about 52 Hz (uint32 seq + 4×f32 LE wxyz). 43/88=0 stops it. Neck-health and sensor commands
  return 100003. Added `tools/freeclip2/probe.py` and `tools/freeclip2/headpose.py`.

## 2026-11-11 00:20 UTC
- Guided test (follow mode): the stream re-centers to the held pose and pauses while still. Axis mapping
  derived via the Python rebuild prototype (now `src/devices/freeclip2/rebuild.rs`). Found the spatial mode command 43/180 (0 off, 1 follow, 2 fixed) and set
  it to fixed. Scripts now tolerate stream pauses.

## 2026-11-11 00:45 UTC
- Fixed-mode guided run: the stream behaves the same as in follow mode. Rebuild with the Python prototype
  gives ±7° centers. Spatial mode restored to follow.

## 2026-11-11 01:20 UTC
- Restructured the repo:
  - tools/, data/, docs/ are committed; research/ is gitignored.
  - Deleted the extracted assets and the Gadgetbridge clone.
  - Merged chain.py into the Python rebuild prototype (since replaced by `src/devices/freeclip2/rebuild.rs`).
- Ran git init and cargo init (`headup`); the crate builds. No commit yet.
- Researched posture-app UX: Posture Pal plus slouchless, HeadOrbit, dorso, posture-fix and cristdulcey/posture.
  Wrote docs/UX_RESEARCH.md.

## 2026-11-11 02:10 UTC
- Implemented the Rust detector:
  - `src/mbb.rs`, `src/pose.rs`, `src/detector.rs`, with 11 tests passing, including a replay of both guided
    recordings.
  - `src/main.rs`: bluer RFCOMM, spoken calibration, earbud tone/speech, notify-send, JSONL log.
- Live smoke test passed: calibration p90 1.8°, the stream disabled on exit was verified.

## 2026-11-11 03:30 UTC
- Modular refactor: `src/core/` plus `src/devices/freeclip2/`, and the `HeadTracker` trait. Added `--raw-log`.
- Long-hold recording (tests/fixtures/freeclip2/hold_test.csv) showed that the earbuds glide to identity after ~4–5 s still. This
  caused session 1's false recoveries and offsets. Added glide suppression to the rebuild.
- 15 tests pass. The end-to-end replay test fails if the fix is disabled.

## 2026-11-11 04:00 UTC
- Live session 2 confirmed the glide fix. The user reports it "looks good". Logs: Tone at 25.0 s, Speak at 45.1 s,
  Recovered at 48.3 s; 134 glide samples were held back.

## 2026-11-11 04:40 UTC
- Data policy: `data/` is gitignored; test recordings moved to `tests/fixtures/`.
- Added the Nix flake (dev shell and package; `nix build` runs the 15 tests). Fixed a clippy warning by
  implementing `Mul` for `Quat`.
- Repository is staged for the user's first commit. The user makes all commits.
- Added `tools/freeclip2/wear_test.py` to check wear detection and disconnect behaviour.

## 2026-11-11 05:20 UTC
- Wear test: removal does not disconnect; 43/37 reports per-earbud wear state. Implemented the wear pause and
  recalibration on re-wear (`Event::Worn` in the HeadTracker trait).
- Piper voices: picked CC BY 4.0 candidates (alba, libritts_r) and generated samples.
- Rewrote the README for users, with a supported-devices table.

## 2026-11-11 06:30 UTC
- Voice: alba clips (tools/gen_voice.sh, assets/voice/CREDITS.md) embedded; espeak-ng removed from runtime.
- Device registry plus BlueZ auto-detect (bluer `bluetoothd`; flake adds dbus and pkg-config). headup is now a
  daemon (optional address, waits and reconnects). The live smoke test auto-detected the FreeClip 2.
- Home Manager module `services.headup` (nix/hm-module.nix). Evaluated in a real home-manager config; the unit
  was checked. The log file's parent dir is created automatically.
- README covers auto-detect, the HM service and credits. 17 tests pass and clippy is clean.

## 2026-11-11 07:20 UTC
- Added the MIT LICENSE and Cargo metadata. Added flake `checks` (build/test, clippy deny-warnings, fmt); `nix flake
  check` passes, and the fmt check was shown to fail on misformatted code.
- Added `.github/workflows/ci.yml` and `release.yml`, mirroring ../quillway; actionlint is clean.
- README install section: Nix + Home Manager from github:yuxqiu/headup, plus release tarballs.

## 2026-11-11 07:50 UTC
- Repo cleanup: removed the obsolete tools/rebuild.py. Moved FreeClip 2–specific docs, tools and test fixtures
  under `freeclip2/` directories and updated all references. `nix flake check` passes.

## 2026-11-11 08:40 UTC
- Refactor:
  - `Euler` struct; FreeClip2 frame handlers split.
  - `AnyTracker` dispatch enum; `Recorder` (TTY-only status line).
  - Calibration failure retries in-session; Ctrl-C-aware retry waits.
- Tests 17 → 20 (raw-log format, sign-flipped calibration mean, JSON log format); clippy clean; flake check passes.
- Live: the non-TTY run printed clean lines with no `\r`. The earbuds were disconnected, so only the waiting path
  was exercised.

## 2026-11-11 09:20 UTC
- User report: no warnings after the first recovery. session3 logs showed the 43/89 stream restarting (seq 721 → 235),
  measured from the still-looking-down pose. Rebuilder now treats a backwards seq as a new stream with
  reference = last known orientation.
- New fixture `stream_restart.csv` with 2 tests (both fail without the fix). `--raw-log` now records non-pose
  frames. 23 tests pass; `nix flake check` passes.

## 2026-11-11 09:50 UTC
- The user confirmed the stream-restart fix works. They reported the 10 s tone was inaudible and the first prompt
  truncated. The session4 log shows the tones fired, so the cause is the Bluetooth output waking up.
- Added `core::wav` (parse/encode mono 16-bit PCM, tones, lead-in) and a 1 s silent lead-in on every sound. A
  test checks that the embedded clips parse. 27 tests pass; `nix flake check` passes.

## 2026-11-11 10:00 UTC
- The user confirmed the 1 s lead-in works: the tone is audible and the first prompt is complete.

## 2026-11-11 10:40 UTC
- User saw "no head pose from the earbuds during calibration" after putting on one earbud.
  tools/freeclip2/single_bud_test.py showed the pose stream stops whenever either earbud is off.
- "Worn" now means both earbuds. The session waits for the first pose before calibrating (no error or reconnect).
  README and FINDINGS updated. 27 tests pass.

## 2026-11-11 11:30 UTC
- headup was stuck at "waiting for head tracking". Cause: the iPhone was connected/active (multipoint). The
  controlled reclaim test plus session 4 show tracking only while this computer streams audio. New streams are
  relative to the head pose at their start.
- The user chose to monitor only during laptop playback and to recalibrate after gaps over 30 s. Next: a hold test
  under continuous music.

## 2026-11-11 12:00 UTC
- Implemented the audio-dependent tracking behaviour:
  - `HeadTracker::tracking_hint()`; the waiting message says to play audio on this computer.
  - `TrackingLost` after 30 s without poses (no stale reminders); recalibration when tracking resumes.
  - README explains that monitoring runs while the computer plays audio.
- Live: the waiting message is shown with no audio playing. 27 tests pass.

## 2026-11-11 12:50 UTC
- The music hold test showed the stillness glide/silence also happens during playback. Replaced the "no pose 30 s"
  rule with `Event::Available` (43/54 streaming bit for this adapter's address).
- Fixed a crash on BLE devices vanishing during lookup (skip, retry); the unsupported-address check is now
  startup-only. 28 tests pass.

## 2026-11-11 13:40 UTC
- Second refactor (DECISIONS "Second refactor"):
  - Fixed the TrackingLost loop bug.
  - CueSink trait; scripted session tests on a paused tokio clock (6 scenarios).
  - LogEvent enum; one session clock; FreeClip2 impl merge; `retry_later`.
- 34 tests pass, clippy clean, `nix flake check` passes. Live: the waiting message shows correctly.

## 2026-11-11 16:25 UTC
- Drafted v0.1.0 release notes (no FreeClip-specific quirks, per the user). Comment-only fixes in core/detector.rs and
  core/tracker.rs so core docs don't depend on FreeClip 2. 34 tests pass.
- Service live check: no head tracking at all while the earbuds report pc as playing. Details and ruled-out causes in
  HANDOFF "Blockers".

## 2026-11-11 18:05 UTC
- Tracking outage (~2 h, no 43/89): ruled out PipeWire 1.6.11 (old generation also failed), codec, reboot, case reset,
  firmware, headup code. Tracking returned right after a 43/90/43/44/43/180 query probe; cause unknown (FINDINGS
  "Tracking outage").
- `FreeClip2::connect` now sends the 43/90 query before 43/88. 34 tests pass, clippy and fmt clean; live: reply
  logged, 552 samples in 20 s.
- Started a read-only search for the firmware OTA endpoint in the decompiled app.
- OTA endpoint found in the app (subagent, spot-checked). With the user's approval, sent the version check with the
  SN: the newest firmware is 7.0.0.526 (earbuds run .516). Downloaded and unpacked it into research/firmware/526/:
  HiSilicon BS25 `[inferred]`, readable bootloader, the rest encrypted or compressed. FINDINGS "Firmware image".

## 2026-11-11 18:15 UTC
- The v0.1.0 Release run had failed in "Install build dependencies": the GitHub runners could not resolve the Ubuntu
  mirrors (transient, not a workflow bug). CI on main (71ca177) passes.
- Moved v0.1.0 to 71ca177 as a signed tag (user's tag.gpgsign) and reran the release via workflow_dispatch: both
  builds passed; 4 assets uploaded. The x86_64 tarball's sha256 checks out and `headup --help` runs (with libdbus).

## 2026-11-11 19:00 UTC
- User: no reminders while slouching. A diagnostic run showed tracking intact but a re-centre glide ~2 s into the
  hold passed as movement back to upright (threshold 2.5 s). Lowered `GLIDE_AFTER_STILL_S` to 1.0 s; new fixture
  and test `hides_glide_after_short_hold` (fails before the fix). 35 tests pass, clippy and fmt clean.
- Live with the fixed build: Tone at 22.2 s, Speak at 42.2 s, Recovered at 46.4 s; pitch held at ~51° through the
  glide and the silence after it.

## 2026-11-11 19:55 UTC
- Missed slouches at 14:21 and 14:23 (session 6): glides were held correctly, but upright had drifted to −14…−45°.
  Every re-centre is a ~68-sample blend; small ones happen 3–9×/min.
- Guided ground truth (`guided.py --slouch`, new; prompts are now printed too): the earbuds report the return from a
  look down 10–16° larger than the look down itself. Rebuild order and glide blending can't fix that. A guarded
  re-baseline (pitch < −8° for 3 s) fixes both recordings in replay. Awaiting the user's go-ahead.

## 2026-11-11 20:30 UTC
- Researched drift-free alternatives: no gravity or raw-IMU command on the FreeClip 2 (43/151 no reply, 43/152
  unsupported, 43/88 modes 2/3 no stream); Galaxy Buds expose gravity, so their projects have no drift problem.
- With the user, chose re-anchoring upright after each recovery over the 3 s guard. Replays compared four variants;
  the stillness-triggered one won (DECISIONS). Implemented in `Detector`, logged as `Rebaselined`; 40 tests pass.
- User flagged leaked device details. Re-anchoring is now opt-in via `HeadTracker::pose_drifts()` (FreeClip 2: true);
  core comments made generic (FreeClip specifics moved to its backend). 42 tests pass, clippy and fmt clean.
- Detector refactored to a `Posture` state enum (user rule: correlated fields in one type). 42 tests pass.
- Device quirks: `tracking_hint()` and `pose_drifts()` replaced by one `HeadTracker::traits() -> TrackerTraits`
  (hint is now `Option`). 42 tests pass, clippy and fmt clean. The waiting-message hint was not re-checked live.

## 2026-11-11 21:30 UTC
- Live test (data/session7*.jsonl, 200 s): 6/6 slouches got a tone; 5 `Rebaselined` events matching the offline rule
  exactly; corrected slouches 38–63°, upright −1…+5°; a ceiling look did not re-anchor. The earbuds' error had moved
  real upright to −31° (relative to calibration) by the end.

## 2026-11-11 21:50 UTC
- Refactor pass (DECISIONS "Refactor pass after review"): `Update` return value, `Reanchored { upright }` log event,
  `pose_drifts_up`, `Config::for_traits`, `FreeClip2::TRAITS`, merged speak branches. 42 tests pass, clippy and fmt
  clean. v0.1.1 release is waiting on CI (stuck in GitHub's queue).

## 2026-11-11 22:30 UTC
- Buffered glide handling: 2 s delay, drop the 70 samples before each identity sample. Fixed a test-helper infinite
  loop (release vs next_release used different float expressions). 41 tests pass.
- Removed the 1 s sound lead-in (user: cues only play while audio already streams).
- Live (data/session8*.jsonl): 3/3 slouches cued ~12 s in, 1 re-anchor, 9 re-centres absorbed; user confirmed the
  calibration prompt is complete without the lead-in. User will commit; v0.1.1 release next.

## 2026-11-11 23:05 UTC
- Bumped to 0.1.1 (6de8e88, signed); flake version now derived from Cargo.toml. Local `nix flake check` passes.
- GitHub Actions major outage: CI runs queued with no jobs. Release waits for green CI on 6de8e88.

## 2026-11-11 23:30 UTC
- GitHub Actions recovered; CI green on 6de8e88 (x86_64, aarch64). Released v0.1.1: signed tag, notes, release
  workflow uploaded both tarballs and checksums; x86_64 tarball verified (sha256, `--version` 0.1.1).

## 2026-11-11 22:10 UTC
- No calibration again: the v0.1.1 service connected at 17:15 local while the computer was idle and sent 43/90 + 43/88.
  Audio started at 17:17:58 (TrackingAvailable), but no 43/89 arrived. Restarting during playback tracked at once
  (data/outage2_raw.jsonl). Fix: re-send 43/88 whenever this computer starts streaming. Builds; 41 tests pass;
  live repro not yet done.
- Live check of the fix (data/repro_fix.jsonl): started while idle; the 43/88 re-sent when audio began (18.1 s)
  started tracking 30 ms later and calibration passed.
- Correction: a 43/88 sent while idle can work (data/reclaim_test.log), and the morning outage had 43/88 sent during
  playback, so the re-send does not explain it. Recorded the open hypothesis (after a long power-off: 43/90 plus a
  43/88 sent while streaming) in FINDINGS; the user chose not to test further.

## 2026-11-11 22:28 UTC
- Fix committed by the user (c7e0bda). Bumped to 0.1.2 (Cargo.toml, Cargo.lock); `nix flake check` passes and the
  built binary reports 0.1.2. Drafted v0.1.2 release notes.

## 2026-11-11 22:38 UTC
- Released v0.1.2: CI green on b01263b; the user created the signed tag and the release (the auto-mode classifier
  blocked it for the agent); the release workflow uploaded both tarballs; x86_64 tarball verified (sha256, `--version`
  0.1.2, run with nixpkgs dbus on LD_LIBRARY_PATH).

## 2026-11-11 23:48 UTC
- Restructured src/devices (DECISIONS "Backend registry generated from one list"): `Backend` trait plus a
  `backends!` macro, so adding a model takes 2 lines in the registry instead of ~7 edits. FreeClip 2 has a
  `stream_switch` helper and uses tokio `Instant` throughout. No change in behaviour; 41 tests pass; clippy and fmt
  clean. A throwaway second backend compiled with only a `mod` line and a list line, then was removed. Uncommitted;
  the user commits.

## 2026-11-11 23:54 UTC
- Fixed the two items left alone earlier: the rustdoc warning (private `rebuild` link), and the end-to-end tests that
  reached `super::super::FreeClip2::TRAITS`. They moved to `src/devices/freeclip2/tests/{replay,end_to_end}.rs`,
  declared from the inline `mod tests` (user: test-only modules stay out of the production module list).
- Logging (DECISIONS "Shared logging for backends"): `DeviceLog` in core; FreeClip 2 logs serde records; the session
  log also uses serde_json (shorter number formatting, same keys). Added serde + serde_json. 41 tests pass;
  `nix flake check` passes. Changes staged, not committed. A live run with `--raw-log` has not been done yet.

## 2026-11-12 00:01 UTC
- Redesign pass (user: no backward compatibility needed). One run clock owned by `Recorder` and shared with
  `DeviceLog` (logs line up; `t` no longer resets on reconnect). BlueZ discovery moved to `devices/discovery.rs`.
  `session::run` returns `Result<Infallible>`. 41 tests pass; `nix flake check` passes. Staged, not committed.
  Not run live (would compete with the installed service for the earbuds' RFCOMM channel).

## 2026-11-12 00:03 UTC
- Log `t` is now wall-clock Unix time (user). Timing logic stays monotonic. `DeviceLog::new` is back;
  `Recorder::device_log` removed. 41 tests pass; `nix flake check` passes. Staged, not committed; not run live.

## 2026-11-12 00:08 UTC
- Posture logic redesign (DECISIONS "Posture logic redesign"): time-based smoothing (rate independent, identical at
  50 Hz), detector free of tracker types, `Quat::mean`, `phrases!` list. 42 tests pass (new rate test proven to
  fail with the old weight); `nix flake check` passes. Staged, not committed; not run live.

## 2026-11-12 00:12 UTC
- `Recorder` holds no clock: console lines carry no time; the session owns its monotonic clock again.
- Drafted `AGENTS.md` (layout, commands, design rules, tests, docs workflow). 42 tests pass. Staged, not committed.

## 2026-11-12 00:23 UTC
- Live check of the refactors (data/live_check*.jsonl) passed: wall-clock `t` in both logs, raw samples and frames
  logged, the stream restart after 43/88 handled, a quick glance ignored, a held slouch toned, recovery and re-anchor.
  Smoothing replayed over the log gives the same slouch onset with the old and new weights (+8.49 vs +8.53 s).
- Tone came 1.0 s after it was due: the slouch was held still (no samples) and silence ticks run every `POLL` = 1 s;
  the tick 10 ms before the due time missed it. Behaviour from before today; harmless at a 10 s dwell.
- Finding: this session ran at 35 Hz (all earlier ones 52.5 Hz); glides stay 68 samples, so they took 1.95 s against
  the 2.0 s delay. Recorded in FINDINGS; fix proposed in HANDOFF, not built.

## 2026-11-12 00:31 UTC
- Built both fixes (DECISIONS "Hold samples by count; poses carry measurement time; reminders on time"). Found on the
  way that bursts of released poses defeated time-based smoothing with arrival times; poses now carry measurement time.
- New fixture `live_35hz.csv` from the live check. 46 tests pass; clippy, fmt, rustdoc clean. Not run live yet.

## 2026-11-12 00:49 UTC
- Shape-aware release (DECISIONS): after 0.2 s of silence, release everything but a trailing run approaching identity;
  that run waits up to 5 s. 47 tests pass.
- Surveyed reminder timing in the five reference apps (UX_RESEARCH "Reminder timing"); spot-checked the key
  citations against the source. Reminder timing kept as on-time pending the user's choice.

## 2026-11-12 00:53 UTC
- Reminder timing switched to confirmed (user): detector clock = last delivered pose's measured time + silence since.
  Two new session tests (one fails on the on-time version); the fake tracker honours timeouts and can lag. 49 tests
  pass; clippy, fmt, rustdoc clean. Staged; not run live.

## 2026-11-12 01:40 UTC
- Thermo-nuclear code quality review (two independent reviewers; findings verified), then applied all findings
  with the user's approval (DECISIONS "Thermo review applied"). Session `Interrupt` + `Timeline`; calibration on
  measured time; detector timers as data; FreeClip 2 split into protocol / sans-IO `Link` / socket; replays drive the
  real Link and session; main `attempt()`. 55 tests pass; clippy, fmt, rustdoc clean; binary smoke-run (waiting note,
  clean exit on Ctrl-C). Not run live with earbuds.

## 2026-11-12 01:52 UTC
- Live check of the reviewed build (data/live2*.jsonl): everything as designed at 35 Hz; tone and recovery each
  exactly the 2.0 s hold after the measured events. Ctrl-C stop path exits 0 (the background task's exit 1 was the
  wrapper shell, killed by an over-broad `pkill -f`). New finding: the 43/88 ack is `02` when sent while idle and `00`
  once streaming (FINDINGS). headup.service was already inactive and was left so.

## 2026-11-12 01:55 UTC
- Bumped to 0.2.0 (Cargo.toml, Cargo.lock); `nix flake check` passes; the built binary reports 0.2.0. Release next.

## 2026-11-12 02:00 UTC
- Released v0.2.0: bump commit a2eb9fc (signed), CI green on both architectures, signed tag v0.2.0, GitHub release
  with notes (user-visible changes; log format as a breaking change). Release workflow uploaded both tarballs; x86_64
  tarball verified (sha256, `--version` 0.2.0, run with nixpkgs dbus on LD_LIBRARY_PATH).
