# AGENTS.md

headup is a Linux daemon in Rust. It reads head tracking from supported earbuds over Bluetooth and reminds the
user, through the earbuds, when they look down for too long. Everything runs locally.

## Read first

1. `README.md`: what the user sees.
2. `docs/HANDOFF.md`: current state, next actions, blockers.
3. `docs/DECISIONS.md`: why the code is shaped as it is. Check here before you change a design.
4. `docs/RUNBOOK.md`: every command.
5. `docs/devices/<model>/FINDINGS.md`: what is known about a device's protocol, with evidence.

## Layout

- `src/core/`: device-agnostic logic.
  - `tracker.rs`: the `HeadTracker` interface that backends implement.
  - `session.rs`: calibration and monitoring.
  - `detector.rs`: slouch detection.
  - `cues.rs`, `wav.rs`: sounds and notifications.
  - `record.rs`: console output and logs.
  - `quat.rs`: orientation math.
- `src/devices/`: the backend registry (`mod.rs`) and BlueZ discovery (`discovery.rs`).
- `src/devices/<model>/`: one backend. FreeClip 2: `protocol.rs` (wire messages), `link.rs` (protocol state, no
  I/O), `rebuild.rs` (orientation rebuild), `mbb.rs` (framing), `mod.rs` (the socket). Its test-only modules are in
  `src/devices/<model>/tests/`.
- `src/testing.rs`: test support shared across modules (run the real session on any tracker).
- Per model, outside `src/`: `docs/devices/<model>/`, `tools/<model>/` (Python research scripts) and
  `tests/fixtures/<model>/` (recordings the tests replay).
- `assets/voice/`: embedded voice clips. `tools/gen_voice.sh` generates them.
- `nix/hm-module.nix`: the Home Manager service.
- `data/` and `research/` are gitignored: local recordings, logs, the vendor APK and decompiled code.

## Commands

Run everything inside `nix develop`.

- Tests: `cargo test`
- Lint: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
- Same checks as CI: `nix flake check`. The flake sees only git-tracked files, so `git add` new files first.
- Run: `cargo run --release -- [MAC] [--log FILE] [--raw-log FILE]`
- Before a live run, ask the user. The installed `headup.service` holds the earbuds' RFCOMM link, so a second
  instance competes with it. The user can stop it with `systemctl --user stop headup`.
- If a tool is missing, use `nix shell nixpkgs#<pkg>`.

## Design rules

- **`src/core/` never names a device.** Code and comments in core describe generic behaviour. Device facts
  belong in the backend and in `docs/devices/<model>/`.
- **Device quirks.** Absorb a quirk in the backend: re-centring, wear rules, the precondition behind
  `Event::Available`. Declare a quirk in `TrackerTraits` only when handling it needs session knowledge. Each new
  field defaults to "no quirk".
- **Adding a backend.** Write `src/devices/<model>/`, implement `HeadTracker` and `Backend`, declare the module, and
  add one line to `backends!` in `src/devices/mod.rs`. Do not add parallel `match` arms anywhere.
- **Protocol logic without I/O.** Keep a backend's decisions in a synchronous type that takes bytes and time and
  returns events and frames to send (FreeClip 2: `Link`). The `HeadTracker` impl only moves bytes, so every
  decision can be unit-tested without earbuds.
- **One list per registry.** `backends!` (devices) and `phrases!` (voice prompts) generate their enums and dispatch.
  Extend the list instead of adding a lookup that can miss at runtime.
- **Independent of sample rate.** Detection and session logic use seconds: timers, windows, and a smoothing time
  constant. Sample counts are allowed only inside a backend, where the protocol defines them.
- **Two kinds of time.** Timing logic uses the monotonic `tokio::time::Instant`, so tests can run on a paused
  clock, and spans of it as `Duration` (exact integer nanoseconds), never `f64` seconds: a float round trip can land
  a nanosecond early and make a timer loop spin. Convert to seconds only inside formulas (e.g. smoothing). Log lines use wall-clock Unix seconds (`t`). Console lines carry no time.
- **Poses carry their measurement time** (`Event::Pose { at, .. }`). A backend that buffers reports when each pose
  was measured; smoothing and slouch timers run on it, never on arrival time. Reminders fire only on delivered
  posture: the detector's clock never runs ahead of the latest delivered pose (plus silence).
- **Logging.** Log lines are JSON written with serde_json; never build JSON by hand. Session output goes through
  `Recorder` (event names in `LogEvent`). A backend's raw data goes to its `DeviceLog` (`--raw-log`).
- **Make illegal states unrepresentable.** Fields that are valid only together, or that exclude each other, go into
  one enum or struct (for example `Posture`, `Episode`).
- **Fail loudly.** A malformed frame or a broken precondition is an error, not a silent fallback.
- **Keep changes small.** Change only what the task needs. Mention unrelated problems instead of fixing them.
  Before you add a new abstraction, propose it to the user in 2–3 sentences.

## Tests

- Test behaviour that users can observe: cues, events, rebuilt angles. Do not test internals.
- For a bug fix, add a test that fails without the fix, and check that it does.
- Replay real recordings from `tests/fixtures/<model>/` where you can, through the real protocol and the real
  session (FreeClip 2: `Replay`, `ReplayTracker`). Do not copy production loops into test helpers. Session tests use
  `FakeTracker` and `testing::run_session` on tokio's paused clock.
- Test-only modules go in a `tests/` directory next to the module. Declare them from its inline `mod tests`, not in
  the production module list.

## Docs and workflow

- Record each durable decision in `docs/DECISIONS.md`: date, what, why, and the alternatives you rejected. When
  a decision changes, update every place that states the old one.
- Put command changes in `docs/RUNBOOK.md`. Keep `docs/HANDOFF.md` current, and append to `docs/SESSION_LOG.md`
  at the end of a session.
- A claim about device behaviour needs evidence: a FINDINGS section, a `data/` file or a fixture. Otherwise mark
  it `[unverified]` or `[inferred]`.
- The user makes all commits, tags and releases. Stage changes; do not commit.
- Release notes describe user-visible effects ("reminders are no longer missed when you hold still"). They never
  describe one device's quirks.
- Release steps: bump `version` in `Cargo.toml`, wait for green CI, then the user publishes a `v<version>` GitHub
  release. The `Release` workflow builds the tarballs (see `docs/RUNBOOK.md`).
- Voice prompts: add a line to `phrases!` in `src/core/cues.rs` and the text to `tools/gen_voice.sh`, then run
  the script.
