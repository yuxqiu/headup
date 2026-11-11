# RUNBOOK

Device MAC used in examples: `C0:DA:5E:F1:A0:A6` (FreeClip 2, connected via BlueZ).

## Nix
- Dev shell with the Rust toolchain, clippy, rustfmt, rust-analyzer, python3, pw-play, espeak-ng and notify-send:
  `nix develop`
- Build the package (runs the tests; the binary is wrapped with its cue tools): `nix build`
- Run: `nix run . -- [C0:DA:5E:F1:A0:A6] [--log data/session.jsonl] [--raw-log data/session_raw.jsonl]`. The address is optional;
  without it, headup auto-detects connected supported earbuds and waits/reconnects.
- The flake only sees git-tracked files, so `git add` new files before `nix build`.

- Probe an MBB command: `python3 -I tools/probe.py C0:DA:5E:F1:A0:A6 '43/90:1='`
- Troubleshoot a device backend: `headup --raw-log FILE` appends every raw sample (seq, raw and rebuilt quaternion,
  whether it was re-centred or held back as a glide) as JSON lines.
- Stream head pose: `python3 -I tools/headpose.py C0:DA:5E:F1:A0:A6 SECONDS [data/out.csv]`
- Guided axis/drift test (spoken prompts through the buds, ~4 min; run in `nix develop`):
  `python3 -I tools/guided.py C0:DA:5E:F1:A0:A6 tests/fixtures/headpose_axes.csv`
- Rebuild orientation from a recording: `python3 -I tools/rebuild.py tests/fixtures/headpose_axes_fixed.csv`
- Spatial mode query/set: `python3 -I tools/probe.py MAC '43/180:1=18,2=,3='` / `'43/180:1=18,2=01'`
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
- Command catalogue: `python3 -I tools/used_cmds.py research/jadx_out/sources` (service-43 IDs the app sends)
- References: `git clone --depth 1 https://github.com/melianmiko/OpenFreebuds research/openfreebuds`; posture apps in
  `research/ios-refs/` (slouchless, HeadOrbit, dorso, posture-fix, cristdulcey/posture).
