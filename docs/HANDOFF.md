# HANDOFF

Last updated: 2026-11-17 16:45 UTC

## Current state
- headup works end to end on HUAWEI FreeClip 2 (firmware 7.0.0.526): calibration, reminders (tone 10 s, speech
  30 s and every 120 s, notification 60 s, tink on recovery), drift re-anchoring, pauses on any loss of tracking.
  Latest release: v0.5.0 (tag on aa149e6). Live checks on 2026-11-15 passed (data/slouch_live*, data/stop_live*, data/hear_live*).
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
- Tests: 147 pass; `nix flake check` (build + tests, clippy, rustdoc with warnings denied, fmt) passes.
- Git: the user commits; work since the last commit is staged. `data/` and `research/` are gitignored.

## Next actions
0. Staged, not committed: FreeClip 2 asks for the device list (43/49) at connect, so an idle computer is reported
   unavailable at start-up (DECISIONS 2026-11-17). Tests (147), `nix flake check` and two live start-ups pass.
   Open report from the user: on 2026-11-17 10:00–11:05 slouches stopped giving reminders while upright kept
   re-anchoring (journal only: upright −28° to −44°, then −0.3° to −8.9° every few seconds to minutes). No pose log
   from that run; the user suspects re-anchoring only pitch instead of the full orientation. Needs a run with
   `--log` and `--raw-log` to replay.
   Recorded 2026-11-17 15:32–16:13 local (data/slouch_retry*.jsonl, console in data/slouch_retry_console.txt);
   headup.service restarted. First look: no reminder 15:46–16:03 while the resting pitch read −11° → −28° (looking up)
   and yaw crept 0° → 48°; no re-anchor in that stretch. User: turned the head a lot 15:45–16:05; afterwards a
   slight head drop gave a slouch. Log: re-anchored at 16:05:43 (yaw ~80°); rest at yaw ~75° read −3.6° at 16:06:12
   and +12.4° at 16:06:55, after head turns with identity re-centres and stream gaps between (16:06:11, :18, :41).
   So the error builds up after the re-anchor, across re-centres during turns; a full-orientation re-anchor would
   not undo it (rough script: 16:07 median 11.0° vs 12.2°). FINDINGS "Upright error after many head turns": the
   frame stays gravity-aligned; lost movement in silent windows re-labels the forward axis. Fix direction undecided.
1. Head-up re-anchoring and the monitor timing fixes are committed (ad68689; DECISIONS 2026-11-16). Live run passed
   (data/headup_live2*: tone on a slouch, re-anchor on the head-up, nothing on a quick glance up or a held look-up,
   re-anchor on a deliberate head-up, tone and tink after). Next: a longer everyday session to see
   whether drift stays bounded.
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
