# RUNBOOK

Device MAC used in examples: `C0:DA:5E:F1:A0:A6` (FreeClip 2, connected via BlueZ).

## Nix
- Dev shell with the Rust toolchain, clippy, rustfmt, rust-analyzer, python3, espeak-ng, and pw-play and notify-send
  for the live-test tools:
  `nix develop`
- Build the package (runs the tests): `nix build`
- Run: `nix run . -- [C0:DA:5E:F1:A0:A6] [--log data/session.jsonl] [--raw-log data/session_raw.jsonl]`. The address is optional;
  without it, headup auto-detects connected supported earbuds and waits/reconnects.
- The flake only sees git-tracked files, so `git add` new files before `nix build`.

- Probe an MBB command: `python3 -I tools/freeclip2/device/probe.py C0:DA:5E:F1:A0:A6 '43/90:1='`
- Troubleshoot a device backend: `headup --raw-log FILE` appends every raw sample (time, seq, quaternion as sent)
  and every other frame as JSON lines. Rebuilt orientations are in `--log` (pitch/yaw/roll). A FreeClip 2
  re-centre the rebuilder's glide model did not follow is logged as `{"missed_glide":SEQ}` and printed as a warning
  on stderr (the journal for the service).
- Log lines in both files start with `t`, wall-clock Unix time in seconds (ms precision). Convert in Python with
  `datetime.fromtimestamp(t)`; subtract the first `t` for seconds since start.
- Stream head pose: `python3 -I tools/freeclip2/device/headpose.py C0:DA:5E:F1:A0:A6 SECONDS [data/out.csv]`
- Guided axis/drift test (spoken prompts through the buds, ~4 min; run in `nix develop`):
  `python3 -I tools/freeclip2/device/guided.py C0:DA:5E:F1:A0:A6 tests/fixtures/freeclip2/headpose_axes.csv`
- Move during a re-centre (beeps when a glide starts; look straight up at the beep; audio playing, headup stopped):
  `python3 -I tools/freeclip2/device/glide_move.py C0:DA:5E:F1:A0:A6 data/glide_move.csv [--trials 3]`
- Spatial mode query/set: `python3 -I tools/freeclip2/device/probe.py MAC '43/180:1=18,2=,3='` / `'43/180:1=18,2=01'`
  (0 off, 1 follow, 2 fixed).
- Rust detector (inside `nix develop`):
  - Tests: `cargo test`. They include replays of the recordings in `tests/fixtures/`.
  - Run: `cargo run --release -- [C0:DA:5E:F1:A0:A6] [--log data/session.jsonl]`
  - It needs the BlueZ D-Bus API, a desktop notification server, and a PulseAudio server (PipeWire provides one;
    without it sound goes to ALSA's default device). Calibration is spoken at start and after
    re-wearing; stop with Ctrl-C, which disables the stream.
- Voice clips: `tools/gen_voice.sh` regenerates `assets/voice/*.wav` (Piper alba). Phrases must match the `phrases!` list in
  `src/core/cues.rs`.
- Home Manager module: `homeManagerModules.default` in `nix/hm-module.nix`. To verify it evaluates, build a minimal
  `home-manager.lib.homeManagerConfiguration` with `services.headup.enable = true` and read
  `xdg.configFile."systemd/user/headup.service".source`.

## Regenerating research/
- APK: `curl -sSL -A Mozilla/5.0 -o research/smartaudio.apk https://appgallery.cloud.huawei.com/appdl/C114871409`
- Decompile: `nix shell nixpkgs#jadx -c jadx -j 8 --no-res --show-bad-code -d research/jadx_out research/smartaudio.apk`
- Assets: `nix shell nixpkgs#unzip -c unzip -q research/smartaudio.apk 'assets/*' -d research/assets`
- Firmware re-check (full procedure: skill `.claude/skills/freeclip2-firmware/SKILL.md`):
  - Fetch (sends the serial number to Huawei; ask first): `python3 -I tools/freeclip2/firmware/fetch.py SERIAL "VERSION" research/firmware/<ver>/`
  - Unpack (split, decompress, check every SHA-256): `python3 -I tools/freeclip2/firmware/unpack.py research/firmware/<ver>/update_accessory_fw.zip research/firmware/<ver>/unpacked`
  - Disassemble the app image as RV32 at its load base, custom instructions annotated; also writes `ram_<addr>.asm`
    for each region the startup code copies to RAM (the RAM code holds the OS and the head-tracking library):
    `nix shell nixpkgs#pkgsCross.riscv64-embedded.buildPackages.binutils -c python3 -I tools/freeclip2/firmware/disasm.py research/firmware/<ver>/unpacked/sec05.img research/firmware/<ver>/app`
  - Anchors report, to diff with `docs/devices/freeclip2/firmware/anchors-526.txt` (reads the load base from
    `body.asm`; exits non-zero, after the report, if an invariant headup relies on broke):
    `python3 -I tools/freeclip2/firmware/anchors.py research/firmware/<ver>/app`
  - Other images (e.g. image 3, the Bluetooth core): add `--no-stub` to `disasm.py`.
  - Check recorded re-centres against the firmware's glide and stillness rules (exits non-zero if no glide fits):
    `python3 -I tools/freeclip2/recordings/glide_check.py data/*_raw.jsonl`
- Turn a raw log into a publishable test fixture (samples only, time since the first sample; no addresses, serial
  or wall-clock time): `python3 -I tools/freeclip2/recordings/to_fixture.py data/X_raw.jsonl tests/fixtures/freeclip2/X.csv`.
  Add the name to `RECORDINGS` in `src/devices/freeclip2/rebuild.rs`.
  - Guided live test (headup stopped; prompts are spoken, printed and shown as notifications; run in `nix develop`):
    `python3 -I tools/freeclip2/device/grounding_test.py C0:DA:5E:F1:A0:A6 bluez_output.C0_DA_5E_F1_A0_A6.1 data/grounding_test.log`
    (stops with a non-zero exit if the RFCOMM link drops or the silence player dies)
- Command catalogue: `python3 -I tools/freeclip2/app/used_cmds.py research/jadx_out/sources` (service-43 IDs the app sends)
- Custom-instruction reference (WS63 encodings; this chip differs, see FINDINGS): `git clone --depth 1 https://github.com/hispark-rs/hisi-riscv-qemu research/hisi-riscv-qemu` (`docs/reference/xlinx-isa.md`).
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
