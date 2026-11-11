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
