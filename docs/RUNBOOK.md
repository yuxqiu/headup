# RUNBOOK

Device MAC used in examples: `C0:DA:5E:F1:A0:A6` (FreeClip 2, connected via BlueZ).

## Nix
- Dev shell with the Rust toolchain, clippy, rustfmt, rust-analyzer, python3, pw-play, espeak-ng and notify-send:
  `nix develop`
- Build the package (runs the tests; the binary is wrapped with its cue tools): `nix build`
- Run: `nix run . -- [C0:DA:5E:F1:A0:A6] [--log data/session.jsonl] [--raw-log data/session_raw.jsonl]`. The address is optional;
  without it, headup auto-detects connected supported earbuds and waits/reconnects.
- The flake only sees git-tracked files, so `git add` new files before `nix build`.

- Probe an MBB command: `python3 -I tools/freeclip2/probe.py C0:DA:5E:F1:A0:A6 '43/90:1='`
- Troubleshoot a device backend: `headup --raw-log FILE` appends every raw sample (time, seq, quaternion as sent)
  and every other frame as JSON lines. Rebuilt orientations are in `--log` (pitch/yaw/roll).
- Log lines in both files start with `t`, wall-clock Unix time in seconds (ms precision). Convert in Python with
  `datetime.fromtimestamp(t)`; subtract the first `t` for seconds since start.
- Stream head pose: `python3 -I tools/freeclip2/headpose.py C0:DA:5E:F1:A0:A6 SECONDS [data/out.csv]`
- Guided axis/drift test (spoken prompts through the buds, ~4 min; run in `nix develop`):
  `python3 -I tools/freeclip2/guided.py C0:DA:5E:F1:A0:A6 tests/fixtures/freeclip2/headpose_axes.csv`
- Spatial mode query/set: `python3 -I tools/freeclip2/probe.py MAC '43/180:1=18,2=,3='` / `'43/180:1=18,2=01'`
  (0 off, 1 follow, 2 fixed).
- Rust detector (inside `nix develop`):
  - Tests: `cargo test`. They include replays of the recordings in `tests/fixtures/`.
  - Run: `cargo run --release -- [C0:DA:5E:F1:A0:A6] [--log data/session.jsonl]`
  - It needs `pw-play` and `notify-send` on PATH, and the BlueZ D-Bus API. Calibration is spoken at start and after
    re-wearing; stop with Ctrl-C, which disables the stream.
- Voice clips: `tools/gen_voice.sh` regenerates `assets/voice/*.wav` (Piper alba). Phrases must match `Phrase` in
  `src/core/cues.rs`.
- Home Manager module: `homeManagerModules.default` in `nix/hm-module.nix`. To verify it evaluates, build a minimal
  `home-manager.lib.homeManagerConfiguration` with `services.headup.enable = true` and read
  `xdg.configFile."systemd/user/headup.service".source`.

## Regenerating research/
- APK: `curl -sSL -A Mozilla/5.0 -o research/smartaudio.apk https://appgallery.cloud.huawei.com/appdl/C114871409`
- Decompile: `nix shell nixpkgs#jadx -c jadx -j 8 --no-res --show-bad-code -d research/jadx_out research/smartaudio.apk`
- Assets: `nix shell nixpkgs#unzip -c unzip -q research/smartaudio.apk 'assets/*' -d research/assets`
- Command catalogue: `python3 -I tools/freeclip2/used_cmds.py research/jadx_out/sources` (service-43 IDs the app sends)
- References: `git clone --depth 1 https://github.com/melianmiko/OpenFreebuds research/openfreebuds`; posture apps in
  `research/ios-refs/` (slouchless, HeadOrbit, dorso, posture-fix, cristdulcey/posture).

## CI and releases
- CI (`.github/workflows/ci.yml`) runs `nix flake check` on x86_64 and aarch64 Linux: the package build plus tests,
  Clippy with `--deny warnings`, and `cargo fmt --check`. Run the same locally with `nix flake check`.
- Release: bump `version` in Cargo.toml, then publish a GitHub release with tag `v<version>`. The `Release` workflow
  checks that the tag matches Cargo.toml, builds and tests on Ubuntu 22.04 (glibc 2.35) for x86_64 and aarch64, and
  uploads `headup-v<version>-<arch>-linux.tar.gz` with its `.sha256`. To rebuild an existing tag, run the workflow
  manually with `tag`.
- Lint the workflows: `nix shell nixpkgs#actionlint -c actionlint .github/workflows/*.yml`.
