# HANDOFF

Last updated: 2026-11-15 15:17 UTC

## Current state
- headup works end to end on HUAWEI FreeClip 2 (firmware 7.0.0.526): calibration, reminders (tone 10 s, speech
  30 s and every 120 s, notification 60 s, tink on recovery), drift re-anchoring, pauses on any loss of tracking.
  Latest release: v0.4.5. Live checks on 2026-11-15 passed (data/slouch_live*, data/stop_live*, data/hear_live*).
- Layout (AGENTS.md has the full map):
  - `src/core/`: `tracker.rs` (the `HeadTracker` contract, incl. "Late poses"), `calibration.rs` (decision type),
    `monitor/` (`Monitor`; private `detector.rs`, `reminders.rs`), `session/` (the loop; private `tracking.rs`,
    `timeline.rs`, `sounds.rs`), `sound/` (`Sound`, `Cue`, `Phrase`, `Player`; private `desktop.rs`, `audio.rs`,
    `wav.rs`, the `voices!` registry), `record.rs` (`Recorder` for the console and `--log`, `JsonLog`), `steady.rs`,
    `quat.rs`.
  - `src/devices/`: `backends!` registry, discovery, `bluetooth/` (BlueZ); `freeclip2/` (`protocol`, `mbb`, `link`,
    `rebuild`, `mod.rs` socket task). Recordings replayed by the tests: `tests/fixtures/freeclip2/`.
- Decision types (`Calibration`, `Monitor`, the detector, `Tracking`) share one rhythm: pass poses, then
  `check(now)`. A calibration waiting for a prompt is a separate `Paused` value; the link records the stream switch
  where it sends it (`Link::send_switch_on`).
- Tests: 130 pass; `nix flake check` (build + tests, clippy, rustdoc with warnings denied, fmt) passes.
- Git: the user commits; work since the last commit is staged. `data/` and `research/` are gitignored.

## Next actions
1. User commits the staged work; then bump to 0.4.6, wait for CI, publish the release (notes draft: reminders that
   cannot play are reported; a peer-link refusal says so instead of "play audio"; sitting up as a reminder falls
   due plays it, then the tink).
2. Optional: record in FINDINGS the 0.2 s "walking" hold seen right after putting the earbuds back in
   (data/rewear.jsonl).
3. Waiting on a need or a machine: `Event::FrameChanged` (a backend whose frame changes while available),
   Windows/macOS Bluetooth backends.

## Open questions
- 43/88 reply 1 (peer link down) has never been recorded; its sentence and the resend rule are `[unverified]`. If
  audio is already playing when it happens, nothing resends the switch until a 43/54 push or a re-wear.
- Why the earbuds skip a new stream's first seconds, and what makes an identity sample without a glide (live4 seq
  34): FINDINGS "Re-centre and stillness".

## Blockers
- 2026-11-11 tracking outage (~2 h, no 43/89 while everything looked right): cause unknown, sensor-side and not
  visible over MBB (FINDINGS "Tracking outage"). headup sends 43/90 before 43/88 since; if an outage recurs with
  that in place, 43/90 is ruled out as the cure.
