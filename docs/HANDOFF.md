# HANDOFF

Last updated: 2026-11-11 18:05 UTC

## Current state
- The protocol works.
  - 43/88=1 starts a 43/89 relative-quaternion stream (~50 Hz). It re-centres to the held pose
    (identity sample) and pauses while the head is still.
  - The stream is rebuilt to ±5–10°.
  - Axes: pitch = y (down +), yaw = z (left +), roll = x (left tilt −).
- The Rust detector works end to end (live smoke test on 2026-11-11 passed).
  - Modular layout:
    - `src/core/`: quat, the `HeadTracker` trait, detector, session (calibration + monitor), cues.
    - `src/devices/freeclip2/`: mbb framing, rebuild, and the backend with `--raw-log`.
  - `src/core/` also has `record.rs` (console/log output) and `wav.rs` (WAV + 1 s lead-in); `src/devices/mod.rs` has
    the model registry and the `AnyTracker` dispatch.
  - 34 tests pass, including replays of real recordings (guided axes, long holds, stream restart), end-to-end
    cue tests, and scripted session tests on a paused clock (core/session.rs).
  - The design was approved by the user (docs/UX_RESEARCH.md); the parameters are in docs/DECISIONS.md.
- Git: the user makes all commits; changes since their last commit are staged. `data/` and `research/` are
  gitignored; test recordings are in `tests/fixtures/freeclip2/`.
- Nix flake: `nix develop`, `nix build` (runs the tests), `nix run . -- <MAC>`. The binary is `headup`.
- Voice: Piper alba clips embedded (assets/voice). espeak-ng is in the dev shell only.
- Runs as a daemon with BlueZ auto-detect; Home Manager module in nix/hm-module.nix.
- Layout: model-specific files under `freeclip2/` in src/devices, docs/devices, tools, tests/fixtures.
- MIT license. CI: `nix flake check` (build+tests, clippy, fmt; passes locally). Release workflow builds tarballs
  when a `vX.Y.Z` release is published. Neither has run on GitHub yet (repo not pushed).

## Next actions
0. FreeClip 2 tracking needs this computer to stream audio, and goes silent ~5 s into any still hold, even
   during playback (FINDINGS). Implemented `Event::Available` from 43/54; TrackingLost only after 30 s
   unavailable; lookup robustness. 28 tests pass. Live: calibration while audio played, unsupported-address
   check. Still live-untested: audio stop > 30 s, then resume (expect TrackingLost and recalibration).
1. Live checks still owed by the user:
   - Wear pause: take one or both earbuds off, then put both back on. Expect "Paused", then recalibration.
     With only one earbud on, headup stays paused: FreeClip 2 tracking needs both (FINDINGS "Single earbud").
   - Disconnect/reconnect: earbuds into the case and back out. Expect waiting, reconnecting, recalibrating.
   - The 60 s desktop notification (not yet seen live).
   - The Home Manager service in the user's own config (`systemctl --user status headup`).
2. Long session (30+ min, `--log --raw-log`) to measure drift of the rebuilt pitch and how often stream restarts
   happen. Then decide on on-demand recalibration or guarded auto re-baselining.
3. Tune thresholds and timings from real use (20°/15°, 10/30/60 s, 120 s re-nudge).
4. Not built: a manual snooze/pause control (no UI chosen; removal-pause covers the main case). Volume ducking
   and the daily score were dropped by the user.

Confirmed live so far:
- Calibration with the alba voice; the full first prompt is heard after the 1 s lead-in.
- Tone, speech and recovery cues, including warnings after a first recovery (stream-restart fix).
- Glide suppression; auto-detect of the FreeClip 2; clean non-TTY output.

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
