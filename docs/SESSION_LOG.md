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

## 2026-11-12 02:31 UTC
- Glide model studied on every recording (FINDINGS): 68 samples from the held pose, first sample ~2 steps along,
  145/170 glides fit within 0.5°; every stream's first re-centre ends at seq 279–280; head movement during clean glides
  ≤ 0.8° (not the drift source).
- Built model-based glide release (DECISIONS "Model-based glide release"); removed `GLIDE_STALL_S`. Fixed a
  nanosecond wake-time spin in `Link::on_timer`. 60 tests pass; clippy, fmt, rustdoc clean. Staged, not committed.

## 2026-11-12 02:37 UTC
- Exact time in the timing path (DECISIONS): rebuilder, Link, detector and session timeline on `Duration`; removed the
  `on_timer` patch and the session's 1 ms floor. 60 tests pass; clippy, fmt, rustdoc clean. Staged, not committed.

## 2026-11-12 02:40 UTC
- Exact time extended to the test harness and replay driver (user asked about the script's floats). The replay
  driver had the same float round trip as `Link`. 60 tests pass; clippy, fmt, rustdoc clean. Staged.

## 2026-11-12 02:49 UTC
- Live check of f83fc6d (data/live3*.jsonl): glances ignored, held-still slouch cued 0.2 s after due (quiet release),
  recovery after the 1.33 s hold, sit-up at 8 s gave no reminder, re-anchor corrected 22.6° of re-centre drift.
  Found: a 43/54 without TLV 5 ended the session (v0.2.0 bug); fixed with a test from the real frame. Service stopped
  for the check and restarted.

## 2026-11-12 02:54 UTC
- Bumped to 0.2.1 (Cargo.toml, Cargo.lock); `nix flake check` passes; the built binary reports 0.2.1. Release next.

## 2026-11-12 02:59 UTC
- Released v0.2.1: bump commit cd407c0 (signed), CI green on both architectures, signed tag, GitHub release (notes:
  the dropped-connection fix and robustness to stalls/restarts). Release workflow uploaded both tarballs; x86_64
  tarball verified (sha256, `--version` 0.2.1).

## 2026-11-12 05:06 UTC
- Noted a potential issue: head movement during a re-centre glide (FINDINGS "Head movement during a glide", HANDOFF).
  Guided runs show cut-short ramps with the reference ending at the glide's start pose; the mid-glide movement case
  is untested.

## 2026-11-12 14:11 UTC
- Readability refactor (user approved detector + rebuilder + session): detector `Smoother` + `State` machine;
  rebuilder `GlideFilter` → `Step`s → `Stitch` with `Hypotheses` and `Arrival`; session `TrackerStatus` and
  `CalibrationWindow`. Behaviour unchanged: 61 tests pass; clippy, fmt, rustdoc clean. Typestate considered and
  rejected (runtime-driven state). Staged, not committed.

## 2026-11-12 14:32 UTC
- Live test of moving during a re-centre (new tool `tools/freeclip2/glide_move.py`, fixture `glide_move.csv`): the
  earbuds cut the glide short, the movement does not show in its readings, and they re-centre roughly onto its start
  pose; headup reads the new posture afterwards (within the known ~10° per re-centre). Potential issue resolved;
  corrected the FINDINGS explanation of non-fitting glides. Test `moving_during_a_glide_is_not_lost`; 62 tests pass.
  Service stopped for the test and restarted.

## 2026-11-12 14:36 UTC
- Bumped to 0.2.2 (Cargo.toml, Cargo.lock); `nix flake check` passes; the built binary reports 0.2.2. Release next.

## 2026-11-12 14:42 UTC
- Released v0.2.2: bump commit 27b599d (signed), CI green, signed tag, GitHub release (maintenance, no behaviour
  change). Release workflow uploaded both tarballs; x86_64 tarball verified (sha256, `--version` 0.2.2).

## 2026-11-12 15:17 UTC
- Firmware .526 analysed: not encrypted. Sections are LZMA in a `hwcompress10` block container (64 KiB raw LZMA1
  blocks), or plain (bootloader, SBC prompts, case package with LZMA-alone images). Decoded the package table; every
  section SHA-256 matches. New `tools/freeclip2/fw_unpack.py`; FINDINGS "Firmware image" corrected. Head-tracking
  code is probably in RISC-V image 5 (`rotation_perdict_task`); disassembly not started. Staged, not committed.

## 2026-11-12 15:36 UTC
- Disassembled firmware image 5 (load base 0x70280300; RV32IMFC + HiSilicon xlinx custom ISA, BS25 variant of the
  push/pop and branch-immediate encodings). New `tools/freeclip2/fw_xlinx.py` annotates objdump output. Ghidra
  (stock and the Ghidra_RISCV31 module) could not decode this variant.
- Found the 43/88 set/report, 43/89 sender and 43/90 handlers, the start conditions (enable, status flags from
  internal messages, mode byte, boot-time hardware gate) and that 43/90 records the host in a persisted 3-slot
  list that the head-tracking paths check (FINDINGS "Firmware head-tracking logic"). Staged, not committed.

## 2026-11-12 15:59 UTC
- Grounded the firmware reading. Fixed the method: objdump had decoded the image as RV64 (missing ~4,800 `c.jal`
  calls), now RV32 through an ELF wrapper. Decoded opcode 0x7b (long gp-relative load/store) and 0x2b (`shf`).
  Found the host service-43 dispatch table, which corrected the handler mapping: 43/88 → `0x7039c018`, and the
  earlier "43/88" handler is 43/167.
- 43/88 reply TLV2: 0 accepted, 2 = requester is not the current (last-streaming) device. Matches `2=02` then `2=00`
  in live2/live3. 1 = earbud-to-earbud link down `[inferred]`. 43/89 goes to the current device; retracted the claim
  that the 43/90 list gates it. Start needs a per-device mode R (probably the 43/180 spatial mode, `[inferred]`).
- FINDINGS, HANDOFF and RUNBOOK updated; the RUNBOOK pipeline reproduces the analysis listing exactly.
  Staged, not committed.

## 2026-11-12 17:41 UTC
- Live test (service stopped, then restarted; data/mode_gate_live.jsonl): the stream runs in spatial mode off,
  follow and fixed (138–139 samples per 4 s, 43/88 always `2=00`). The "R = spatial mode" inference is refuted. The
  mode was restored to follow.
- Code: 43/88 reply parsed (`SwitchReply`); a refusal reports `Available(false)` and waits for this computer to
  stream (DECISIONS). New tests `stream_switch_replies` and `refused_stream_switch_waits_for_this_computer_to_stream`
  (the second fails without the change). 64 tests, clippy and fmt pass. Staged, not committed.
- Live smoke run of the new build (25 s, service stopped then restarted; data/smoke_reply*.jsonl): 43/88 `2=00`
  parsed, calibration on the first attempt, 281 samples, clean Ctrl-C stop.
- Live check of the refusal path (phone last active; data/refusal_live*.jsonl): 43/88 `2=02` at 0.09 s →
  TrackingUnavailable and the "play audio on this computer" hint. Audio started at 15 s → 43/54 `…09` →
  TrackingAvailable, re-sent 43/88 `2=00` at 15.53 s, first sample at 15.53 s, calibration on the first attempt,
  280 samples, clean stop. Service restarted.

## 2026-11-12 17:59 UTC
- Earbuds report 7.0.0.526 (MBB 1/7), the disassembled version. Guided live test (tools/freeclip2/grounding_test.py,
  prompts spoken + printed + notifications; data/grounding_test.log): one earbud stops samples and 43/88 does not
  revive them; earbud in closed case still `2=00` (43/37 TLV4 = in case); re-wear needs 43/88; audio gap resumes by
  itself on 43/54 `…09`; stillness pause and seq counting confirmed; 43/88 on restarts seq at 1.
- FINDINGS: grounding table for every device fact headup relies on; stale "unverified"/"re-test" notes resolved.
  README tested firmware → 526.
- Tools: fw_fetch.py (offline-tested against saved 526 files, rejects a corrupt zip), fw_unpack.py reads the zip,
  fw_disasm.py (finds base 0x70280300, listing identical to the analysis), fw_anchors.py (baseline
  docs/devices/freeclip2/firmware/anchors-526.txt). Skill .claude/skills/freeclip2-firmware/ dry-run on 526 passed.
  Staged, not committed.

## 2026-11-12 18:08 UTC
- Grouped tools/freeclip2/ into firmware/ (fw_ prefix dropped), device/ and app/; updated every current reference
  (docs, skill, AGENTS, flake.nix comment, Rust comments). Firmware pipeline re-run: anchors identical.
- Corrected two earlier claims: the 0x600xxxxx code is image 5's RAM-run code (startup copies flash 0x702abf38 to
  0x600d0000), not missing ROM; and the head-tracking computation is in it: task 40 ← 0x600e1be0 ← 0x600e1844 ←
  ops of module `robin_gesture` (tail table 0x7041332c) → library ~0x600f9b30. disasm.py now disassembles the RAM
  regions; anchors.py reports the module (baseline updated). Glide code not yet read.

## 2026-11-12 18:27 UTC
- Firmware: gp = 0x20007c48 (startup stub run at 0x20000; its copies chain), confirmed by 380/388 gp-relative
  float loads landing on sensible constants. Stub also copies code to 0x201c0, so 0x46b90 (log), 0x45d12 (memcpy)
  etc. are in the image, not ROM. disasm.py annotates gp-relative loads with initial values and disassembles all
  copied regions; anchors.py made region-agnostic (baseline unchanged). Glide code located, not read.
- Code: Switch {Unconfirmed, Accepted, Refused} in Link; 43/54 streaming re-sends 43/88 only when not accepted;
  another device streaming unconfirms; Link.streaming → current; reply 1 → OtherRefusal; 43/54 doc states the
  evidence. Tests: audio_resuming_after_an_accepted_switch_keeps_the_stream (fails before),
  audio_starting_before_any_reply_switches_on_again, another_device_streaming_unconfirms_the_switch. 67 pass.
- Live: data/gap_live*.jsonl (no re-send after an audio gap; samples resume 0.03 s after 43/54 …09);
  data/grounding_test2.log with a phone-takeover phase (phone takes over, this computer's audio does not preempt,
  43/88 → 02). Docs updated (FINDINGS grounding table, DECISIONS, skill, HANDOFF). Staged, not committed.

## 2026-11-12 18:46 UTC
- User away (no live tests). Firmware grounding completed:
  - Glide/stillness read from code (`0x600fa53c`, `0x600faa4e`, setter `0x600fa29e`): still = every Euler axis within
    10° of the run's start, trigger after 208 samples, glide = nlerp over N = 69 ending in exact identity, cut short
    when an angle exceeds 2.4°. Recordings agree (new tools/freeclip2/recordings/glide_check.py: 138 re-centres,
    length 68, nlerp misfit ≤ 0.126°, median still run 213.5).
  - 43/54 state byte = connected | audio<<1 (`0x7037a828`; audio from message 0x8400, Bluetooth core folds 5/6 → 3).
  - Reply 1 = earbud-to-earbud link down (message 0x8406 from the Bluetooth core's peer-address link events).
  - Device change re-derives enable (`0x7039be0c`); headup's re-send there kept (grounded live in refusal_live/live2/live3).
- Tools: xlinx.py decodes load/store-multiple, muliadd-imm and 16-bit zero stores; disasm.py gets --no-stub (image 3).
- Code: rebuild.rs uses the firmware's nlerp glide (`glide_point`; Quat::toward_identity removed from core);
  protocol.rs decodes 43/54 audio == 4; SwitchReply::PeerLinkDown restored. Test hides_glide_from_a_large_angle.
  68 tests pass. FINDINGS grounding table: every row grounded. Staged, not committed.

## 2026-11-12 18:51 UTC
- Outage re-examined with the firmware: none of the probe's queries (43/90, 43/44 with an empty value, 43/180) can
  reach the start/stop logic (empty TLV values are dropped by the parser), so the probe did not cause the recovery;
  with `2=00` the failed condition was sensor-side and not observable over MBB.
- Replayed all data/*_raw.jsonl through Rebuilder (temporary test, removed): 67 full re-centres, rebuilt orientation
  moves ≤ 3.0° (one, boundary) and ≤ 0.33° in the other 66; slerp vs nlerp give identical outcomes. First re-centre at
  seq ~279 explained (208 + 69). FINDINGS updated. 68 tests pass.

## 2026-11-12 18:55 UTC
- Independent code-quality review of the src changes; applied: Switch enum → accepted bool (cleared on earbud
  removal), current → available, exhaustive match on replies, NotCurrentDevice, assert in glide_point, test fixed to
  the firmware's stillness rule (220 samples, 6.6°) with time-based selection and a literal oracle, docs reworded.
  Kept 43/54 unseen values as variants (firmware passes them through). 68 tests, clippy, fmt pass.

## 2026-11-12 19:10 UTC
- Refactored `Link` state (user): `Switch` enum {Unconfirmed, Accepted, Refused} plus `streaming`; availability derived
  and reported in one step after each frame. Fixes availability staying false after a refusal followed by re-wearing
  (new test fails on the old code). Rebuilder, detector and session checked: no correlated loose fields left.
  69 tests, clippy, fmt pass. Staged, not committed.

## 2026-11-12 19:55 UTC
- Thermo code-quality review (two reviewers; report in the session scratchpad) and all fixes applied:
  - Rust: `Quat::normalized` in core; `Link::new` via `switch_on`; `reported` doc and let-chain; test helpers
    (`phone()`, `rotation`, `row`); `AUDIO_STREAMING` doc. Kept three `Switch` states (DECISIONS): the firmware
    answers every well-formed 43/88 (`0x7039c018`; 13/13 live within 80 ms), and the pending re-send covers a refusal
    in transit.
  - Tools: shared `tools/freeclip2/quat.py`, `device/mbb.py` (CRC-checking reader), `device/runner.py` (schedules;
    single_bud_test and wear_test are schedules now), `firmware/listing.py` (one listing parser); anchors.py reads the
    base itself and asserts five invariants; clean-exit fixes in fetch, unpack, glide_check, grounding_test;
    glide_move uses nlerp. Same anchors/disasm/glide_check results on 526 and data/; device scripts offline only.

## 2026-11-12 21:30 UTC
- Link restructured: `Earbuds` (facts + switch, handlers return a `Request`), `View` derived, `report` emits changed
  view fields. No test changed.
- Firmware re-centre read (agent, key points re-checked by hand): reference = current pose (two samples behind
  during a countdown; at a cut, the pose two samples before it); while paused the stage keeps running and re-centres
  silently every 209 samples (25/26 identity resumes in the predicted window; all 121 normal resumes at >= 4.00°).
  The ~10° error is absorbed movement, not a computational bias. Fusion is Mahony; latency compensation predicts 27
  samples ahead weighted by yaw rate. FINDINGS updated.
- Stitch order fixed (`raw × reference`, firmware `0x600ff6b0`/`0x600ff626`); new test fails on the old order.

## 2026-11-12 22:30 UTC
- Live grounding test 3 (data/grounding_test3.log): refactored tools work; all phases match 526; after a phone
  takeover the stream does not resume until a new 43/88 (re-send grounded; FINDINGS, DECISIONS updated).
- Live headup run 4 (data/live4*.jsonl): all steps behaved; the turned slouch read pitch 31.9° roll 2.2° (old order
  would give roll −25.2°). Service restarted.

## 2026-11-12 22:15 UTC
- v0.3.0 released by the user (tag on 027d65d). CI green (main and tag, x86_64 and aarch64); Release workflow
  uploaded both tarballs; x86_64 sha256 OK and `--version` prints 0.3.0. Next: update the service.

## 2026-11-12 22:50 UTC
- User noticed no recalibration after using the phone for 30 s or more. The service log confirmed it (unavailable
  ~5 min, no TrackingLost), as did data/live4.jsonl. Cause: poses released or sent after `Available(false)` cleared
  the session's unavailability. First fixed in `TrackerStatus`; then, by the user's choice, replaced by "recalibrate
  after every unavailability" (no 30 s grace). 69 tests pass. Staged.

## 2026-11-12 23:20 UTC
- Automated gap test (data/live6*): stop reported only after ~4.9 s without audio; gaps <= 4 s never interrupt.
  Found and fixed: after an interruption the session waited for a pose a still head never sends; it now calibrates
  from the last pose. 70 tests pass. Service restarted. Staged.

## 2026-11-12 23:45 UTC
- User rejected calibrating from the last pose. Calibration now prompts as soon as tracking is available, asks a
  still head to move ("Move your head a little, then hold still…", new clip), and succeeds on 3 s of steady posture
  measured after the prompt (sliding). Startup still waits silently for the first pose. 73 tests pass. Staged.

## 2026-11-13 00:05 UTC
- `Tracking { Starting, Available, Unavailable }` in `TrackerStatus` replaces `available` + `tracking_seen` (user).
- Live nudge test (data/live7*): still head after a pause got the nudge, moved, calibrated; re-wear calibrated on the
  new stream. Service restarted. Staged.

## 2026-11-13 16:00 UTC
- Rebuilder: dropped the `QUIET` timer and the 70-sample hold; each sample comes out as soon as no glide hypothesis
  covers it. A thermo review of the first version (0.5° tolerance) found three regressions (small glides lost the
  pose, off-model glides wiped it with a blind miss report, a stream resuming mid-glide leaked).
- Research: the "0.126° glide misfit" was an `acos` precision artifact (Rust `Quat::angle_to`, `tools/freeclip2/quat.py`);
  the firmware's glide routine is exact f32 nlerp (listing read: `0x600fa45a`, `0x600ff572`), and recorded glides fit
  within 0.00015°. Two streams start mid-glide (live6, stream_restart): their first ~230 samples were never sent.
- Final design (user chose over the 70-sample hold): precise `angle_to`, `FIT_TOL_DEG` = 0.001°, unseen-start glide
  hypotheses bounded by the number of unsent samples, `MissedGlide` report without recalibration. Measured on all 17
  raw logs: no glide sample released, 561 real samples kept that the old fallback dropped.
- Core review: wording fixes and the `Available(false)` obligation in `tracker.rs`.
- Privacy check of `data/` (serial number, phone address, wall-clock times); six recordings converted to fixtures
  with the new `to_fixture.py`. 78 tests, `nix flake check` pass. All staged, not committed.

## 2026-11-13 18:30 UTC
- Design review of `Link`, the session and the tracker interface. Found that poses measured while a prompt
  plays were stamped when it ended (raw-log bursts of 136–172 samples within 20 ms).
- FreeClip 2 now reads its socket in its own task; `Link` returns raw-log records instead of writing them;
  session tests moved to `src/core/tests/session.rs`. 87 tests, clippy and fmt pass.
- Removed the session's delivery-lag model (held poses now ≤49 ms in every fixture): the session drains the
  tracker, then judges posture up to now. FakeTracker plays its script on the clock.
- Calibration decisions moved into `core/calibration.rs` as a state machine with direct tests. 91 tests pass.
- Live test (data/live8*.jsonl, 210 s, silence played to the earbuds): calibrated on the first attempt (107
  samples, p90 0.6°). Two held slouches: tone exactly 10.00 s and the spoken reminder 30.00 s after the smoothed
  pitch crossed 20° (the reminder during 24 s of silence from a still head), recovery on the same pose as the
  crossing below 15°, re-anchored to -6.1° and -10.7°. Six later glances (≤5 s) cued nothing. Raw log: at most 3
  samples within any 20 ms (prompt bursts gone), no missed glide. The user heard every prompt and cue.

## 2026-11-13 22:20 UTC
- Investigated a "stuck" service: walking around breaks the FreeClip 2 reference. Firmware 7.0.0.526: an
  accelerometer activity detector makes the earbuds hold (reference follows the head, readings identity, stream
  paused) while the user walks (FINDINGS "Instant resets while walking"). Rejected along the way: recalibrating on
  every reset (prompts mid-walk), the weak-link theory, correcting the reference across a reset.
- Now: `Earbuds::holding` makes tracking unavailable during a hold; a hold starts a new orientation frame; the
  session's `Tracking::Resuming` waits for a still head before "sit upright"; `Event::Unavailable { reason }`
  carries the backend's own wording. Fixed: with audio playing at connect (no 43/54), tracking never came back
  after a hold.
- Live check (data/walk3*.jsonl, audio playing before connect): no prompt mid-walk, "sit upright" after sitting
  down, tone and tink on a look-down (user heard all three). 99 tests pass. Fixtures: turn_walk, walk_reconnect,
  walk_near_far, walk_slouch.

## 2026-11-14 02:20 UTC
- Released 0.4.1 (walking holds, resume wait, backend-worded reasons, the no-43/54-at-connect fix, nudge re-measure).
- Since: any interruption starts a new orientation frame (`PoseFrame`, one stretch of tracking), because the earbuds
  keep tracking and re-centring unseen while headup gets nothing; `Link` split into `Earbuds`, `PoseFrame`,
  `reported: View` and `Outputs`. README made device-neutral; FreeClip 2 user page added.
- Live checks: sitting perfectly still after a walk keeps the stream silent until a head move (data/sitstill*);
  pause, walk, sit, play, look down cues correctly (data/pausewalk*). 105 tests pass.

## 2026-11-14 06:20 UTC
- `record.rs`: `LogEvent` derives `Serialize` (`tag = "event"`); cues logged as `Cue { cue }`; console lines from
  `Display`; `JsonLog(Option<LogSink>)` shared by `Recorder` and `DeviceLog` (alias). 111 tests pass.
- Live check (data/rewear*; default output switched to the earbuds, silence playing): calibration, Tone/Recovered,
  earbuds out → "put both earbuds in", back in → brief 0.2 s walking hold, recalibration, Tone/Recovered,
  Reanchored −3.34°.
- Released v0.4.4 (CI and Release workflow green; x86_64 and aarch64 tarballs).
