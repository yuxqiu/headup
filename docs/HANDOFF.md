# HANDOFF

Last updated: 2026-11-12 02:40 UTC

## Current state
- The protocol works.
  - 43/88=1 starts a 43/89 relative-quaternion stream (usually ~52.5 Hz; 35 Hz in the 2026-11-12 live check). It re-centres to the held pose
    (identity sample) and pauses while the head is still.
  - The stream is rebuilt to ±5–10°.
  - Axes: pitch = y (down +), yaw = z (left +), roll = x (left tilt −).
- The Rust detector works end to end (live smoke test on 2026-11-11 passed).
  - Modular layout:
    - `src/core/`: quat, the `HeadTracker` trait, detector, session (calibration + monitor), cues.
    - `src/devices/freeclip2/`: mbb framing, rebuild, and the backend with `--raw-log`.
  - `src/core/` also has `record.rs` (console/log output) and `wav.rs` (WAV parse, encode, tones); `src/devices/mod.rs` has
    the `Backend` trait and the `backends!` list, which generates `Model` and the `AnyTracker` dispatch. Adding a model
    takes a backend module, its `mod` line and one list line.
  - Logging: `core::record::Recorder` (session log, `--log`, and the run clock) and `DeviceLog` (the backend's raw
    data, `--raw-log`), both JSON lines via serde_json; `t` is wall-clock Unix time (session timing stays
    monotonic). BlueZ lookup is
    in `src/devices/discovery.rs`. Test-only backend modules live in `src/devices/<model>/tests/`.
  - 42 tests pass, including replays of real recordings (guided axes, long holds, stream restart), end-to-end
    cue tests, and scripted session tests on a paused clock (core/session.rs).
  - The design was approved by the user (docs/UX_RESEARCH.md); the parameters are in docs/DECISIONS.md.
- Git: the user makes all commits; changes since their last commit are staged. `data/` and `research/` are
  gitignored; test recordings are in `tests/fixtures/freeclip2/`.
- Nix flake: `nix develop`, `nix build` (runs the tests), `nix run . -- <MAC>`. The binary is `headup`.
- Voice: Piper alba clips embedded (assets/voice). espeak-ng is in the dev shell only.
- Runs as a daemon with BlueZ auto-detect; Home Manager module in nix/hm-module.nix.
- Layout: model-specific files under `freeclip2/` in src/devices, docs/devices, tools, tests/fixtures.
- MIT license. CI: `nix flake check` (build+tests, clippy, fmt) passes on GitHub. v0.1.0 is released: the tag is
  signed and points to 71ca177 (moved from 2389515). The first release run failed on a runner DNS outage (Ubuntu
  mirrors unresolvable); a manual rerun (`gh workflow run release.yml -f tag=v0.1.0`) uploaded both tarballs.

## Next actions
- Thermo review applied (DECISIONS "Thermo review applied"); 55 tests pass. Live check passed 2026-11-12
  (data/live2*.jsonl, 35 Hz): calibration first try, glance ignored, tone 10 s + 2.0 s hold after the measured slouch
  start, recovery after the hold, re-anchor, resend on streaming, clean Ctrl-C (exit 0). Not yet seen live: no tone
  when sitting up just before it is due. v0.2.0 released (https://github.com/yuxqiu/headup/releases/tag/v0.2.0, signed tag on a2eb9fc).
  CI green on x86_64 and aarch64; release workflow uploaded both tarballs; x86_64 tarball verified (sha256,
  `--version` 0.2.0). Next: update the installed service (`nix flake update headup`, then switch), pointing
  `services.headup.logFile` at a new file (the log format changed).
- Model-based glide release and exact time in the timing path (DECISIONS) built after v0.2.0: 60 tests pass;
  not yet run live, not released.
- v0.1.1 is released (https://github.com/yuxqiu/headup/releases/tag/v0.1.1, signed tag on 6de8e88): buffered glide
  handling, re-anchoring upright after recoveries (`TrackerTraits::pose_drifts_up`), `HeadTracker::traits()`, no sound
  lead-in. Live-verified in data/session7 and data/session8. 41 tests pass; CI green on x86_64 and aarch64.
0. v0.1.2 released (https://github.com/yuxqiu/headup/releases/tag/v0.1.2, signed tag on b01263b): re-sends the
   stream switch when this computer starts streaming. CI green on x86_64 and aarch64; release workflow uploaded both
   tarballs; x86_64 tarball verified (sha256, `--version` 0.1.2).
1. Live check of the rebuilder and timing changes (DECISIONS "Hold samples by count"): the same steps as the last live
   check (RUNBOOK). Expect the tone ~11.3 s after you look down at 52.5 Hz (~12 s at 35 Hz), and no tone if you sit
   up just before it.
2. Then release (minor bump: the log format changed) and update the installed service (`nix flake update headup`,
   then switch). Point `logFile` at a new file first: the old one holds relative `t`. The live check of the
   2026-11-12 refactors passed (data/live_check*.jsonl).
2. Watch a normal day's use with `services.headup.logFile` set: count `Reanchored` events and any missed slouch.
3. Not built: a manual snooze/pause control (no UI chosen). The 43/90-before-43/88 query is still unverified as the
   cure for the 2026-11-11 tracking outage (see Blockers).

Older open items:
0. FreeClip 2 tracking needs this computer to stream audio, and goes silent ~5 s into any still hold, even
   during playback (FINDINGS). Implemented `Event::Available` from 43/54; TrackingLost only after 30 s
   unavailable; lookup robustness. 28 tests pass. Live: calibration while audio played, unsupported-address
   check. Still live-untested: audio stop > 30 s, then resume (expect TrackingLost and recalibration).
1. Live checks still owed by the user:
   - Wear pause: take one or both earbuds off, then put both back on. Expect "Paused", then recalibration.
     With only one earbud on, headup stays paused: FreeClip 2 tracking needs both (FINDINGS "Single earbud").
   - The 60 s desktop notification (not yet seen live).
3. Tune thresholds and timings from real use (20°/15°, 10/30/60 s, 120 s re-nudge).
   Volume ducking and the daily score were dropped by the user.

Confirmed live so far:
- Calibration with the alba voice; the full first prompt is heard (the 1 s lead-in was later removed: cues only play while audio is already streaming).
- Tone, speech and recovery cues, including warnings after a first recovery (stream-restart fix).
- Glide suppression; auto-detect of the FreeClip 2; clean non-TTY output.
- 2026-11-11: case reset → disconnect, reconnect, recalibration; the Home Manager service runs; 30 min session
  (session6) measured the drift that led to re-anchoring.

## Blockers
- 2026-11-11: tracking stopped completely for ~2 h (no 43/89 while the earbuds listed pc as playing, both worn,
  43/88 acked, spatial mode follow). Ruled out: PipeWire 1.6.11 (old generation 636 also failed), codec, reboot,
  case reset, firmware change, headup code (the Python tool failed too). It started working at 12:53 local right after
  a probe sent only queries: 43/90 (spatial capability), 43/44, 43/180. After a later case reset it kept working
  without any 43/90. Cause unknown. Open hypothesis: 43/90 arms tracking and the arming survives a case reset but
  not a long power-off (first night: 43/90 was also sent just before the first working 43/88). Next time it
  fails: send only `43/90:1=` and see whether tracking returns.
  - Since then `FreeClip2::connect` sends 43/90 before 43/88 (DECISIONS). The installed service still runs the
    old build until the user rebuilds Home Manager. If an outage recurs with it in place, 43/90 is ruled out.
  - Firmware .526 downloaded via the OTA check (user approved sending the SN). The main sections are encrypted or
    compressed; only the bootloader is readable (FINDINGS "Firmware image"). The firmware does not explain 43/88 yet.
