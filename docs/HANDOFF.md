# HANDOFF

Last updated: 2026-11-16 21:29 UTC

## Current state
- headup works end to end on HUAWEI FreeClip 2 (firmware 7.0.0.526): calibration, reminders (tone 10 s, speech
  30 s and every 120 s, notification 60 s, tink on recovery), drift re-anchoring, pauses on any loss of tracking.
  Latest release: v0.4.6 (tag on 2dd465e). Live checks on 2026-11-15 passed (data/slouch_live*, data/stop_live*, data/hear_live*).
- Layout (AGENTS.md has the full map):
  - `src/core/`: `tracker.rs` (the `HeadTracker` contract, incl. "Late poses"), `calibration.rs` (decision type),
    `monitor/` (`Monitor`; private `posture.rs`, `head_up.rs`, `reminders.rs`), `session/` (the loop; private `tracking.rs`,
    `timeline.rs`, `sounds.rs`), `sound/` (`Sound`, `Cue`, `Phrase`, `Player`; private `desktop.rs`, `audio.rs`,
    `wav.rs`, the `voices!` registry), `record.rs` (`Recorder` for the console and `--log`, `JsonLog`), `steady.rs`,
    `quat.rs`.
  - `src/devices/`: `backends!` registry, discovery, `bluetooth/` (BlueZ); `freeclip2/` (`protocol`, `mbb`, `link`,
    `rebuild`, `mod.rs` socket task). Recordings replayed by the tests: `tests/fixtures/freeclip2/`.
- Decision types (`Calibration`, `Monitor`, `Tracking`) share one rhythm: pass poses, then
  `check(now)`. A calibration waiting for a prompt is a separate `Paused` value; the link records the stream switch
  where it sends it (`Link::send_switch_on`).
- Tests: 141 pass; `nix flake check` (build + tests, clippy, rustdoc with warnings denied, fmt) passes.
- Git: the user commits; work since the last commit is staged. `data/` and `research/` are gitignored.

## Next actions
1. Head-up re-anchoring (DECISIONS 2026-11-16, uncommitted): a look down to a slouch and back up re-anchors upright
   in either direction, under `TrackerTraits::pose_drifts`. Replays show identical reminders; next is a live run
   with the user (`systemctl --user stop headup` first) to confirm the head-up gesture fixes a drifted upright.
   The firmware has no host lever (FINDINGS: the mode without re-centres needs NearLink).
2. Optional: find why the hold after a re-wear falls at seq 34 or 69 (the activity detector's state across a stream
   start; FINDINGS "A hold after putting the earbuds back in").
3. Waiting on a need or a machine: `Event::FrameChanged` (a backend whose frame changes while available),
   Windows/macOS Bluetooth backends.
4. If a 43/88 reply 1 is ever recorded, check the peer-link sentence and the resend rule against it.

## Open questions
- 43/88 reply 1 (peer link down) has never been recorded; its sentence and the resend rule are `[unverified]`. If
  audio is already playing when it happens, nothing resends the switch until a 43/54 push or a re-wear.
- Why the earbuds skip a new stream's first seconds, and what makes an identity sample without a glide (live4 seq
  34): FINDINGS "Re-centre and stillness".

## Blockers
- 2026-11-11 tracking outage (~2 h, no 43/89 while everything looked right): cause unknown, sensor-side and not
  visible over MBB (FINDINGS "Tracking outage"). headup sends 43/90 before 43/88 since; if an outage recurs with
  that in place, 43/90 is ruled out as the cure.
