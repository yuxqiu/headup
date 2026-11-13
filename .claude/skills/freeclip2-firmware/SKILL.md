---
name: freeclip2-firmware
description: Re-check headup's FreeClip 2 device logic against new earbud firmware. Use when a new FreeClip 2 firmware is out, when the earbuds report a different version (MBB 1/7 TLV 7), or when the device starts behaving differently. Fetches, unpacks and disassembles the firmware, diffs the protocol anchors against the 7.0.0.526 baseline, runs the guided live test, and updates FINDINGS.
---

# FreeClip 2 firmware re-check

Goal: every row of FINDINGS "Grounding of headup's device logic" is still true on the new firmware, with evidence,
or the backend changes to match. The baseline is 7.0.0.526 (2026-11-12). Background: FINDINGS "Firmware image",
"Firmware head-tracking logic" and "Grounding of headup's device logic".

Work in a scratch directory. Put downloads under `research/firmware/<ver>/` (gitignored), never in git.

## 1. Get the firmware (asks the user)

1. Stop the service (`systemctl --user stop headup`; it holds the RFCOMM link), then read the earbuds'
   version and serial number: `python3 -I tools/freeclip2/device/probe.py MAC '1/7:'`. TLV 7 = version string, TLV 9 =
   serial number (both hex ASCII). Restart the service after.
2. **Ask the user before the next step.** It sends the serial number to Huawei.
   `python3 -I tools/freeclip2/firmware/fetch.py SERIAL "VERSION" research/firmware/<ver>/`. To fetch a version newer
   than the one installed, pass the installed version. The tool checks the zip's size, MD5 and SHA-256.

## 2. Unpack and disassemble

```sh
python3 -I tools/freeclip2/firmware/unpack.py research/firmware/<ver>/update_accessory_fw.zip research/firmware/<ver>/unpacked
nix shell nixpkgs#pkgsCross.riscv64-embedded.buildPackages.binutils -c \
  python3 -I tools/freeclip2/firmware/disasm.py research/firmware/<ver>/unpacked/sec05.img research/firmware/<ver>/app
python3 -I tools/freeclip2/firmware/anchors.py research/firmware/<ver>/app > anchors-<ver>.txt
diff docs/devices/freeclip2/firmware/anchors-526.txt anchors-<ver>.txt
```

- Every tool fails loudly when the layout changed: a section hash mismatch, an unknown section kind, an LZMA
  dictionary size other than 0x2000, an LZMA block that does not end, a code-info hash mismatch, no clear load base,
  no MBB send pattern, or not exactly one host dispatch table. On a failure, fix the tool from evidence (FINDINGS
  "Firmware image" documents each layout) before going on.
- Section 5 is the app core (MBB, head-tracking control). Check that this still holds: the section table may be
  renumbered. Look for the image whose strings include `set_spatial_mode`.
- `anchors.py` reads the load base from `body.asm` and checks the invariants headup relies on: every watched command
  but 43/89 has a host handler, 43/89 has exactly one sender, the 43/88 reply codes are `[0, 1, 2]`, one named
  head-tracking module is found, and the internal messages 0x8402, 0x8406, 0x841a and 0x8424 have handlers. It
  prints one `check:` line for each and exits non-zero after the report if any failed. "none" in other lines is
  information, as in the baseline.
- Addresses move between versions; the diff to read is in the *values*.

## 3. Read what changed

Open `research/firmware/<ver>/app/body.asm` at the handlers `tools/freeclip2/firmware/anchors.py` printed. Re-read, in this order (526
addresses in FINDINGS "Firmware head-tracking logic"):
1. The 43/88 handler: the reply codes and the "requester is the current device" check.
2. The 43/89 sender: TLV layout and destination.
3. The sample loop: its send conditions, and when it clears `enable`.
4. The start decision (`0x603`/`0x602`/`0x600`) and the auto re-enable on audio-state messages.
5. The 43/90 handler.
6. The head-tracking module (`anchors.py` names it; FINDINGS "Where the head-tracking computation is",
   "Re-centre and stillness"): re-read `0x600fa53c`-style logic (stillness counter, countdown, nlerp step) and the
   parameter setter; then check new recordings with
   `python3 -I tools/freeclip2/recordings/glide_check.py data/*_raw.jsonl` and replay recordings with `cargo test`.
   Pass: the glide length mode is 68 (`GLIDE_LEN` in `tools/freeclip2/quat.py` and `rebuild.rs`) and the clean-glide
   misfit max stays below 0.001° (`FIT_TOL_DEG` in `rebuild.rs`); the still-run median should stay near the firmware
   threshold (208 on 526). The share of re-centres at 68 is not a criterion: movement cuts many glides short. 526
   data (2026-11-13): "re-centres: 156; glide length mode 68 (91 of 156)", clean glides 57, misfit max 0.00014°,
   still run median 214. The tool exits non-zero
   when no glide fits the formula.
7. The 43/54 state-byte builder (`connected | audio << 1`) and the 0x8406 peer-link message behind reply 1.

Label every conclusion as read from code, confirmed live, or inferred.

Reading the code:
- Decoded custom instructions end in `; X`; a `?` marks a guess. The encodings are in FINDINGS ("How to read the
  image"). An undecoded `.insn` inside a function you need means: decode it from usage (compare many instances,
  check branch targets and register use), add it to `tools/freeclip2/firmware/xlinx.py`, and record the check in FINDINGS.
- Find code by structure, not by text. Logs are numeric IDs (`0x46b90(level, id)`). Useful anchors:
  - MBB sends: `li a0,43` plus `li a1,<cmd>` before a call.
  - Dispatch tables: `(u32 id, u32 handler)` pairs.
  - Getters: `lui; addi; ret` returning a global.
  - Stack canary: `lw ...,-1748(gp)`.

## 4. Live test (asks the user; they wear the earbuds)

The test needs the user, so ask first and show them the steps. Spoken prompts alone were too hard to hear: the tool
also prints them and shows desktop notifications. Then:

```sh
systemctl --user stop headup
nix develop -c python3 -I tools/freeclip2/device/grounding_test.py MAC bluez_output.<MAC_with_underscores>.1 data/grounding_test_<ver>.log
systemctl --user start headup
```

The test stops with a non-zero exit if the RFCOMM link drops, the frame stream is invalid, or the silence player
exits; the log says which. Fix the cause and rerun the whole test: later phases depend on the earlier ones.

Compare each phase with the 526 log (`data/grounding_test.log`; expectations in the FINDINGS grounding table):

| Phase | 7.0.0.526 result |
|---|---|
| baseline | 43/88 → `00`, sequence restarts at 1, ~52/s |
| one_earbud | samples stop within ~0.5 s of 43/37 `2=00`; 43/88 → `00` but no samples |
| in_case | 43/37 `4=01`; 43/88 → `00`, no samples |
| rewear | no samples after both are worn until 43/88 is sent |
| audio_gap | 43/54 `…03` ~5 s after audio stops, samples stop; samples resume ~0.3 s after `…09` without 43/88 |
| phone_takeover | phone `…09`, poses stop. Either this computer's audio does not take back over and 43/88 → `02` (`data/grounding_test2.log`), or it does (`…09` for this computer) with no poses until 43/88 → `00`, seq from 1 (`data/grounding_test3.log`) |
| still / move | samples stop ~6 s into the hold and resume on movement; the sequence keeps counting |

Also run a live check of the refusal path. The phone plays last, then headup starts; expect 43/88 `2=02` and
`TrackingUnavailable`, then audio on this computer gives 43/54 `…09` → 43/88 `2=00` → samples
(`data/refusal_live*.jsonl` for 526).

## 5. Record

- Add a dated section to FINDINGS for the version. Update each grounding-table row whose evidence changed, and
  every other place that states a changed fact.
- Save the anchors report as `docs/devices/freeclip2/firmware/anchors-<ver>.txt`.
- If behaviour changed, change the backend (`src/devices/freeclip2/`) with a test that reproduces the new behaviour
  from the recording, and record the decision in DECISIONS.
- Update README's "Tested firmware" and the HANDOFF / SESSION_LOG.

## Pitfalls already hit

- objdump on a raw binary decodes **RV64**: `c.jal` shows as `addiw`, and about 4,800 calls were missing.
  `tools/freeclip2/firmware/disasm.py` wraps the image in an rv32imfc ELF.
- Read dispatch tables as `(id, handler)` pairs. An off-by-one reading paired 43/88 with the 43/167 handler.
- There are two 43/88 handlers. The host one is in the 88-entry table; the small tables are earbud-to-earbud sync.
- This chip is not WS63. The WS63 custom-instruction encodings (hisi-riscv-qemu, Ghidra_RISCV31) differ for
  push/pop, 0x0b, 0x2b and 0x7b. Stock Ghidra cannot decode this image.
- Part of image 5 runs from RAM (copied at boot to `0x600d0000`: OS, libc, the head-tracking library). Read
  `ram_<addr>.asm` from `disasm.py`, not only `body.asm`. The flash copy of that code also appears in `body.asm`,
  but at the wrong addresses.
- An earlier reading wrongly concluded that the head-tracking math was not in this image, because the sample loop
  only receives buffers. Follow data to its producer (who sends to the task, who fills the buffer) before
  concluding that code is absent.
- `disasm.py` derives `gp` from the startup stub and annotates gp-relative loads with their initial values
  (526: `gp = 0x20007c48`). Image 3's stub has another shape: use `--no-stub` there.
- Code alone can mislead in both directions: "R = spatial mode" fit the code and failed live; "reply 1 is not the
  earbud link" was concluded from one live case and the code later showed it is. Check a claim against both.
- A hypothesis that only fits the code is not grounded. "R = spatial mode" fit the code and failed the live test
  (`data/mode_gate_live.jsonl`).
