# HANDOFF

Last updated: 2026-11-13 18:30 UTC

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
  - `src/core/` also has `record.rs` (console/log output) and `audio.rs` (play to the end) and `wav.rs` (WAV parse, tones); `src/devices/mod.rs` has
    the `Backend` trait and the `backends!` list, which generates `Model` and the `AnyTracker` dispatch. Adding a model
    takes a backend module, its `mod` line and one list line.
  - Logging: `core::record::Recorder` (session log, `--log`, and the run clock) and `DeviceLog` (the backend's raw
    data, `--raw-log`), both JSON lines via serde_json; `t` is wall-clock Unix time (session timing stays
    monotonic). Bluetooth is in `src/devices/bluetooth/` (BlueZ: `bluez.rs`), discovery in
    `src/devices/discovery.rs`. Test-only backend modules live in `src/devices/<model>/tests/`.
  - 92 tests pass, including replays of all 15 recordings in `tests/fixtures/freeclip2/`, end-to-end cue tests,
    and scripted session tests on a paused clock (core/tests/session.rs); calibration decisions are a state machine in core/calibration.rs. The FreeClip 2 backend reads
    its socket in its own task, so poses are timed on arrival even while a prompt plays.
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
- 2026-11-13: cues play in-process (`core/audio.rs`, `cpal`; notifications via `notify-rust`); DECISIONS "Cues play
  in-process with cpal". Next: a live run to hear tone, tink and prompts on the earbuds (start and end intact).
  Bluetooth is behind `src/devices/bluetooth/` (DECISIONS "Bluetooth behind a per-OS module"); Windows and macOS
  backends wait for a machine to test on.
- 2026-11-13: the rebuilder releases each sample as soon as no glide hypothesis covers it (no 70-sample hold, no
  `QUIET` timer), with an exact glide model: precise `Quat::angle_to` (the old acos was the whole "0.126° misfit"),
  `FIT_TOL_DEG` = 0.001°, hypotheses for glides whose start was not sent, and a `missed_glide` report (no
  recalibration). DECISIONS "Exact glide model, immediate release". Six recordings published as fixtures
  (DECISIONS "Publish selected recordings as fixtures"). Core wording made device-neutral; `Available(false)`
  obligation documented. 78 tests, clippy, fmt, `nix flake check` pass; staged, not committed. Next:
  1. Live run (headup service stopped): look down and hold, sit up, turn then slouch, an audio gap. Expect the tone
     about 10 s after looking down and the recovery tink right after sitting up (no 1.3–2 s hold any more), and no
     `missed_glide` warning on stderr.
  2. The user commits; then release (patch bump) and update the installed service.
  3. Open: why the earbuds skip a new stream's first seconds, and what makes an identity sample without a glide
     (live4 seq 34): both `[unverified]` in FINDINGS "Re-centre and stillness".
- After v0.3.0: any tracking unavailability now ends monitoring and headup recalibrates when tracking resumes
  (DECISIONS "Recalibrate after every unavailability"; also fixes long phone takeovers keeping the old reference).
  Calibration asks a still head to move (new voice prompt) and only uses poses measured after the prompt
  (DECISIONS "Calibration asks a still head to move"); live-checked (data/live7*). 73 tests pass; staged. Next:
  the user commits and releases 0.3.1.
  Live: gaps up to 4 s never interrupt; 7 s and 15 s do (data/live6*). Next: optional 1-minute nodding run to see
  whether short gaps restart the stream; then the user commits and releases 0.3.1.
- 2026-11-12 21:30: Link restructured as state + view (DECISIONS "Link as state, view and reported changes");
  firmware re-centre fully read (reference = current pose, silent re-centres while paused explain the ~10° error);
  Stitch order fixed to `raw × reference` (DECISIONS "Stitch composes on the right"). 70 tests pass; staged. The
  live test should include turning the head and then slouching (the fix changes readings after turns).
- v0.3.0 released (https://github.com/yuxqiu/headup/releases/tag/v0.3.0, tag on 027d65d) after live tests passed
  (DECISIONS "Live grounding test 3", "Live headup run 4"). CI green on both architectures for main and the tag;
  Release workflow uploaded both tarballs; x86_64 tarball verified (sha256 OK, `--version` 0.3.0 with libdbus from
  nixpkgs). Next: update the installed service (`nix flake update headup`, then switch).
- Link state refactor and thermo review fixes done (DECISIONS "Link state: one switch enum, derived availability",
  "Research tools: shared modules, checks in code"); 69 tests, clippy, fmt, flake check pass; firmware tools
  re-verified on 526 (anchors output = baseline, all checks ok); device scripts verified offline only. Staged, not
  committed. Next: the user's live test (skill freeclip2-firmware, `grounding_test.py`, then a
  normal headup run), then release 0.2.3 and update the service.
- v0.2.2 released (https://github.com/yuxqiu/headup/releases/tag/v0.2.2, signed tag on 27b599d): maintenance
  release, no behaviour change. CI green on both architectures; tarballs uploaded; x86_64 tarball verified (sha256,
  `--version` 0.2.2). Optional for the installed service (`nix flake update headup`, then switch).
- Readability refactor of detector, rebuilder and session done (DECISIONS); 61 tests pass; staged, not committed.
- Thermo review applied (DECISIONS "Thermo review applied"); 55 tests pass. Live check passed 2026-11-12
  (data/live2*.jsonl, 35 Hz): calibration first try, glance ignored, tone 10 s + 2.0 s hold after the measured slouch
  start, recovery after the hold, re-anchor, resend on streaming, clean Ctrl-C (exit 0). Not yet seen live: no tone
  when sitting up just before it is due. v0.2.0 released (https://github.com/yuxqiu/headup/releases/tag/v0.2.0, signed tag on a2eb9fc).
  CI green on x86_64 and aarch64; release workflow uploaded both tarballs; x86_64 tarball verified (sha256,
  `--version` 0.2.0). Next: update the installed service (`nix flake update headup`, then switch), pointing
  `services.headup.logFile` at a new file (the log format changed).
- After v0.2.0: model-based glide release, exact time in the timing path, and a fix for a session-ending bug in
  v0.2.0 (a 43/54 without TLV 5 was treated as malformed; DECISIONS "43/54 has more than one form"). Live check
  passed 2026-11-12 (data/live3*.jsonl). 61 tests pass. v0.2.1 released (https://github.com/yuxqiu/headup/releases/tag/v0.2.1, signed tag on cd407c0); CI green on
  both architectures; tarballs uploaded; x86_64 tarball verified (sha256, `--version` 0.2.1). Next: update the
  installed service (`nix flake update headup`, then switch); it still runs v0.2.0, which has the 43/54 bug.
- v0.1.1 is released (https://github.com/yuxqiu/headup/releases/tag/v0.1.1, signed tag on 6de8e88): buffered glide
  handling, re-anchoring upright after recoveries (`TrackerTraits::pose_drifts_up`), `HeadTracker::traits()`, no sound
  lead-in. Live-verified in data/session7 and data/session8. 41 tests pass; CI green on x86_64 and aarch64.
0. v0.1.2 released (https://github.com/yuxqiu/headup/releases/tag/v0.1.2, signed tag on b01263b): re-sends the
   stream switch when this computer starts streaming. CI green on x86_64 and aarch64; release workflow uploaded both
   tarballs; x86_64 tarball verified (sha256, `--version` 0.1.2).
1. (Done 2026-11-12; the hold it describes was replaced on 2026-11-13 by "Exact glide model, immediate release".)
2. Then release (minor bump: the log format changed) and update the installed service (`nix flake update headup`,
   then switch). Point `logFile` at a new file first: the old one holds relative `t`. The live check of the
   2026-11-12 refactors passed (data/live_check*.jsonl).
2. Watch a normal day's use with `services.headup.logFile` set: count `Reanchored` events and any missed slouch.
3. Not built: a manual snooze/pause control (no UI chosen). The 43/90-before-43/88 query is still unverified as the
   cure for the 2026-11-11 tracking outage (see Blockers).

- Resolved 2026-11-12: moving the head during a re-centre glide is not an issue. The earbuds cut the glide short and
  keep as reference the head pose two samples before the cut (firmware `0x600fa53c`); headup reads the new posture afterwards (FINDINGS "Head movement during a
  glide", test `moving_during_a_glide_is_not_lost`).

Older open items:
0. FreeClip 2 tracking needs this computer to stream audio, and goes silent ~5 s into any still hold, even
   during playback (FINDINGS). Implemented `Event::Available` from 43/54; monitoring ends when tracking is
   unavailable; lookup robustness. 28 tests pass. Live: calibration while audio played, unsupported-address
   check. Audio stop then resume: recalibration expected (see Next actions).
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
- 2026-11-11 outage (see FINDINGS): the firmware rules out the probe as the cure (none of its queries reaches the start logic;
  FINDINGS "Tracking outage"); the failed condition is sensor-side and not visible over MBB.
- 2026-11-11: tracking stopped completely for ~2 h (no 43/89 while the earbuds listed pc as playing, both worn,
  43/88 acked, spatial mode follow). Ruled out: PipeWire 1.6.11 (old generation 636 also failed), codec, reboot,
  case reset, firmware change, headup code (the Python tool failed too). It started working at 12:53 local right after
  a probe sent only queries: 43/90 (spatial capability), 43/44, 43/180. After a later case reset it kept working
  without any 43/90. Cause unknown. Open hypothesis: 43/90 arms tracking and the arming survives a case reset but
  not a long power-off (first night: 43/90 was also sent just before the first working 43/88). Next time it
  fails: send only `43/90:1=` and see whether tracking returns.
  - Since then `FreeClip2::connect` sends 43/90 before 43/88 (DECISIONS). The installed service still runs the
    old build until the user rebuilds Home Manager. If an outage recurs with it in place, 43/90 is ruled out.
  - Firmware .526 downloaded via the OTA check (user approved sending the SN). It is not encrypted: the sections are
    LZMA (`hwcompress10`) and unpack with `tools/freeclip2/firmware/unpack.py`, all SHA-256s verified (FINDINGS "Firmware
    image"). Image 5 disassembled (FINDINGS "Firmware head-tracking logic"). Grounded: the 43/88 reply TLV2 is
    0 = accepted, 2 = refused because the requester is not the current (last-streaming) device (matches live2/live3),
    1 = refused while the earbud-to-earbud link is down `[inferred]`. 43/89 goes to the current device; the 43/90
    list does not gate it (earlier eviction idea retracted). Start also needs a per-device value `R`; live test showed it is
    not the 43/180 spatial mode (stream runs with mode off; data/mode_gate_live.jsonl). headup now parses the 43/88
    reply and reports tracking unavailable on a refusal (DECISIONS); 64 tests pass; refusal path verified live
    (data/refusal_live*.jsonl). Not yet released.
  - Grounding complete for 7.0.0.526 (the earbuds' version): FINDINGS "Grounding of headup's device logic" lists
    every device fact headup uses with its evidence; guided live test data/grounding_test.log confirmed one-earbud
    stop, re-wear needs 43/88, audio-gap auto resume, stillness pause, seq counting. headup's logic matches.
  - Firmware re-check skill: .claude/skills/freeclip2-firmware/ (tools tools/freeclip2/firmware/{fetch,unpack,disasm,anchors}.py,
    grounding_test.py; baseline docs/devices/freeclip2/firmware/anchors-526.txt). Dry run on 526: anchors identical.
  - The head-tracking computation is in image 5's RAM code (module `robin_gesture`, library ~0x600f9b30; FINDINGS
    "Where the head-tracking computation is"). Not yet read: reading it would ground the glide and stillness rules
    from code (today: recordings only).
  - tools/freeclip2/ grouped into firmware/, device/, app/.
  - Code improved from the groundings: an accepted 43/88 is no longer re-sent when this computer resumes audio
    (each needless re-send restarted the stream and added a re-centre); refused/unconfirmed switches still are;
    another device streaming clears the accepted switch. Reply 1 = `PeerLinkDown` (grounded in firmware: message 0x8406).
    Live-verified (data/gap_live*.jsonl, data/grounding_test2.log). 67 tests, `nix flake check` pass.
  - Firmware: gp = 0x20007c48, the startup stub's copies (incl. code to 0x201c0) and the RAM code are decoded by
    tools/freeclip2/firmware/disasm.py with gp-relative values annotated; nothing the app calls is missing ROM.
  - Grounded in firmware code and recordings (FINDINGS "Re-centre and stillness", "43/54 state byte", "After a
    device change", reply 1): glide = nlerp over N = 69 steps, stillness = 208 samples within 10°, 43/54 state =
    connected | audio<<1, reply 1 = earbud link down. Code now uses the exact glide formula and 43/54 decoding.
    The phone-takeover return headup relies on (re-sent 43/88 accepted once this computer streams) is grounded
    live (refusal_live, live2, live3); the firmware's own re-enable on that path is code-only.
  - Next: release 0.2.3 (43/88 reply handling), then update the installed service.
