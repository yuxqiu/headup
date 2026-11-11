# HANDOFF

Last updated: 2026-11-11 07:50 UTC

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
  - 15 tests pass, including replays of the recordings and an end-to-end cue test on tests/fixtures/freeclip2/hold_test.csv.
  - The design was approved by the user (docs/UX_RESEARCH.md); the parameters are in docs/DECISIONS.md.
- Git: no commits yet. Everything is staged for the user's first commit; the user makes all commits.
  `data/` and `research/` are gitignored; test recordings are in `tests/fixtures/`.
- Nix flake: `nix develop`, `nix build` (runs the tests), `nix run . -- <MAC>`. The binary is `headup`.
- Voice: Piper alba clips embedded (assets/voice). espeak-ng is in the dev shell only.
- Runs as a daemon with BlueZ auto-detect; Home Manager module in nix/hm-module.nix.
- Layout: model-specific files under `freeclip2/` in src/devices, docs/devices, tools, tests/fixtures.
- MIT license. CI: `nix flake check` (build+tests, clippy, fmt; passes locally). Release workflow builds tarballs
  when a `vX.Y.Z` release is published. Neither has run on GitHub yet (repo not pushed).

## Next actions
1. Tune thresholds and timings with real use. The tone, speech and recovery cues are confirmed live; the 60 s
   notification is not yet seen live.
2. Long session (30+ min, `--log`) to measure drift of the rebuilt pitch. Decide whether to add on-demand
   recalibration (signal/keypress) or guarded auto re-baselining.
0. Needs a live test by the user:
   - Wear pause: take both earbuds off, put them back on, expect recalibration.
   - Disconnect/reconnect: put the earbuds in the case, then take them out.
   - The Home Manager service on their own config.
   Done and verified so far:
   - Alba voice (heard in the smoke test).
   - Auto-detect (live smoke test found the FreeClip 2).
   - The HM module (evaluated in a real home-manager config; the generated unit was checked).
1. Tune thresholds and timings with real use. The tone, speech and recovery cues are confirmed live; the 60 s
   notification is not yet seen live.
2. Long session (30+ min, `--log`) to measure drift of the rebuilt pitch. Decide whether to add on-demand
   recalibration (signal/keypress) or guarded auto re-baselining.
0. In progress:
   (a) Piper voice: DONE. The user picked alba. Clips are in assets/voice/ (tools/gen_voice.sh, CREDITS.md),
       embedded and played with pw-play. espeak-ng was removed from runtime (dev shell only, for tools).
       Not yet heard live in headup.
   (b) Wear pause: implemented (`Event::Worn`, FreeClip 43/37 query/push, recalibrate on re-wear). 16 tests pass;
       not yet tested live.
   (c) README rewritten for users, with a supported-devices table.
   (d) APPROVED, in progress: device registry plus BlueZ auto-detect (bluer D-Bus), a daemon loop that
       waits/reconnects, and a home-manager module (`homeManagerModules.default`, `services.headup`).
4. Not built yet: a manual snooze/pause control (the user did not pick a UI; removal-pause covers the main case).
   Ducking and the daily score were dropped by the user.
5. Session 1 issue: FIXED and confirmed live in session 2 (data/session2*.jsonl).
   - Cues: Tone at 25.0 s, Speak at 45.1 s, Recovered at 48.3 s.
   - 134 glide samples were held back, with no false recovery.
6. Modular backend architecture: done (see DECISIONS).

## Blockers
- None.
