# FreeClip 2 sensor / posture investigation

Analysed: HUAWEI Audio Connect for Android (`com.huawei.smartaudio`), official APK from
`https://appgallery.cloud.huawei.com/appdl/C114871409` (downloaded 2026-11-10, `research/smartaudio.apk`),
decompiled with jadx 1.5.6 into `research/jadx_out/`. Open-source reference: OpenFreebuds, cloned in `research/openfreebuds`
(Gadgetbridge was also reviewed, then removed).

## Transport (already public)

The transport is Huawei's "MBB" SPP protocol on RFCOMM port 1. Each frame is
`0x5A | len(2, BE) | 0x00 | serviceId | commandId | TLVs... | CRC16`.
Both OpenFreebuds and Gadgetbridge implement it for FreeClip 2. The framing also
matches the app: `com/huawei/audiodevicekit/audiobluetooth/layer/data/mbb/e.java`.

## Device identity in app config

`assets/000167.json`: FreeClip 2 has modelId `000167`, productId `ZAAS`, internal name `Robin`
and BT model `T0027`.
- `hardwareAttributes: ["dualConnect", "imuFollow", "l2hc"]`. `imuFollow` refers to the IMU
  head-follow mode of spatial audio.
- `softwareFeatures: ["eqAdjust", "highQuality", "spatialAudio"]`.

## Command catalogue (the closest thing to a protocol spec)

`com/huawei/audiodevicekit/datarouter/base/collector/mbb/Mbb.java` lists 164
(service, command) pairs, each with a Chinese description. The raw list is in
`docs/devices/freeclip2/mbb_commands.txt`. Motion-relevant entries:

| svc | cmd | description (translated) |
|---|---|---|
| 43 | 44 | ambient-sound & motion(sport)-state query/report |
| 43 | 48 | sensor data collection report |
| 43 | 88 | spatial-audio data report: switch set |
| 43 | 89 | spatial-audio data report |
| 43 | 90 | spatial-audio capability query |
| 43 | 113 | cervical health: real-time preview mode switch |
| 43 | 114 | cervical health: real-time preview heartbeat |
| 43 | 115 | cervical health: real-time **head pose** data report |
| 43 | 116/117 | cervical health: stored head-pose data query/report |
| 43 | 118–123 | cervical health: reminder switch, notify, thresholds, calibration, clear |
| 43 | 124 | cervical health: device capability query |
| 43 | 151/152 | SPP data-capture switch / data report (debug) |

Payload beans exist for the cervical data:
- `GlassAngleDataInfo`: int16 pitch ("itch"), yaw ("deflection") and roll.
- `GlassHealthBean`: collectTime, pitch, yaw and status.

Both beans are named "Glass". The cervical-health feature ships on Huawei smart glasses.

## What the Android app actually uses

`tools/freeclip2/app/used_cmds.py` resolves every `{43, X, ...}` byte array in the decompiled source.
The app sends service-43 command IDs
0-2,4,5,7-11,14-17,22-25,30-35,37,38,42-47,49-51,53,55-57,65,66,69-74,81,83,87,93-97,100,106,
108,111,112,127,128,135,136,143,144,146,147,148,149,153,154,160,162,163,168-170,172-174,177,179-182.

The app never sends 48, 88–90, 113–126, 130–133 or 151/152, and no `case` handler parses them.
The glass beans are orphaned, with no callers. Spatial audio in this app only toggles a mode
(off / fixed / follow) through the `INDEPENDENT_SPATIAL_AUDIO` feature. On non-Huawei phones,
head tracking runs inside the earbuds.

Head control (`NODDED_SHAKE`) only produces discrete nod/shake events, used for call answer/reject.
43/44 returns a discrete ambient-scene code that adaptive ANC uses.

## Conclusion

- The app exposes no posture or IMU stream to the user. Huawei advertises no posture feature
  for FreeClip 2.
- The IMU exists and drives spatial-audio "follow" mode, nod/shake detection and drop detection.
- The protocol defines raw-ish streams: 43/88→89 spatial-audio data and 43/115 head pose.
  Huawei phones most likely consume 43/88/89 at system level for phone-side spatial rendering
  `[unverified]`. 43/113–124 belong to the glasses.
- Superseded by the hardware test below: 43/90 and 43/88 work, 43/124 is unsupported. (Originally: whether FreeClip 2 firmware answers 43/90, 43/88 or 43/124 was **unverified**. Testing it
  needs the hardware.)

## iOS app

Not analysed. App Store IPAs are FairPlay-encrypted. Decrypting one needs an Apple ID plus a
jailbroken device or a decrypted dump. The iOS app presumably uses the same MBB protocol,
because the catalogue contains iOS-only entries 43/67–68 ("device authentication (iOS APP)").

## Hardware test (2026-11-10, FreeClip 2 fw "HarmonyOS 7.0.0.516(F003H003C00)", Linux BlueZ, RFCOMM ch 1)

Scripts: `tools/freeclip2/device/probe.py` (generic MBB send/log), `tools/freeclip2/device/headpose.py` (stream + decode);
the framing they share is `tools/freeclip2/device/mbb.py`.
Logs: `data/probe_0*.log`, `data/headpose_run1.csv`.

| query | reply |
|---|---|
| 43/90 (spatial capability) `1=` | `1=01 2=01 3=01` (supported) |
| 43/124, 43/48, 43/115, 43/116, 43/119, 43/89 `1=` | `127=000186a3`: error 100003 (100000 = success; 100003 most likely means unsupported `[unverified]`) |
| 43/130 | no reply |
| 43/44 | `1=00 0=e4` |

**43/88 `1=01` enables a head-orientation stream. `1=00` disables it** (ack `2=00`).
The stream arrives as unsolicited 43/89 frames at about 52 Hz:
- TLV 1: uint32 LE sequence counter.
- TLV 2: 4 × float32 LE, a unit quaternion (w, x, y, z). |q| = 1.0000 on every sample.
- The quaternion starts at identity when the stream is enabled, so it is relative to the head pose at
  that moment. Angles changed by tens of degrees with head movement.

Not yet established:
- How the quaternion axes map to anatomical pitch, yaw and roll.
- Whether pitch and roll are gravity-referenced or drift over time.
- Which earbud is the source.
- Behaviour while a phone is also connected.

## Guided axis test, run 1 (spatial mode = follow)

Data: `tests/fixtures/freeclip2/headpose_axes.csv`. Analysis: `src/devices/freeclip2/rebuild.rs`.

- **The stream is relative and re-centers itself.** While the head is held still for a few seconds,
  the earbuds silently adopt the held pose as the new reference. On the next movement they emit an
  exact identity quaternion (w=1, x=y=z=0) and continue relative to the held pose.
- While still and re-centered, the stream also pauses. Sequence numbers keep counting, so frames are
  dropped, not delayed.
- Rebuilding orientation with "on identity, reference = last stationary pose" gives these axes:
  - pitch (y): look down +56°, look up −36°
  - yaw (z): turn left +62°, turn right −67°
  - roll (x): tilt left −25°, tilt right +30–36°
- Each reset adds roughly 10° of error to the rebuilt orientation.

## Spatial-audio mode command

The app sets the mode with 43/180 (`0xB4`): TLV1 = `0x18` (feature 24, INDEPENDENT_SPATIAL_AUDIO),
TLV2 = mode (0 = off, 1 = follow, 2 = fixed). To query, send TLV1 = `0x18` with empty TLV2 and TLV3.
The reply is `1=18 2=<mode> 3=00`.

The device was in follow (1) originally. It was switched to fixed (2) on 2026-11-10 to test whether
re-centering stops. To restore follow, send `43/180:1=18,2=01`.

## Guided axis test, run 2 (spatial mode = fixed)

Data: `tests/fixtures/freeclip2/headpose_axes_fixed.csv`.

- **The spatial mode does not change the 43/89 stream.** The same identity resets occurred at the same
  phase boundaries, and the stream again stopped during the final still phase.
- Rebuilding with `src/devices/freeclip2/rebuild.rs` (reference = last stationary pose, stationary = <3° over 0.5 s):
  - Every "center" lands within about ±7° on all axes. The final center is −1.8° pitch.
  - Run 1 under the same rebuild had a +9° to +14° pitch offset after the "up" step.
  - Expected error is roughly 5–10°. That is adequate for head-down alerts at 20–30°+, but not for fine posture angles.
- Whether pitch is gravity-referenced or gyro-only is still unknown. Long sessions may drift; needs a long recording.

The mode was restored to follow (`2=01`, verified by readback).

## Re-centre glide (long-hold test, 2026-11-11)

Data: `tests/fixtures/freeclip2/hold_test.csv`, recorded with `tools/freeclip2/device/guided.py --hold`: look down 30 s, center 15 s, twice.

- A pose held still is reported steadily for about 4–5 s. Then the earbuds **glide** the reported quaternion to
  identity:
  - The glide is a straight, constant-rate ramp of about 1.6–1.9° per sample (≈45°/s), lasting about 1.2–1.3 s
    (about 67 samples).
  - It ends with an exact identity sample, after which the stream goes silent until the head moves.
  - The first glide step can be larger (e.g. 2.6°).
  - Examples: 62.6° → 0 over t = 12.42–13.68 s, and −64.2° → 0 over t = 42.28–43.58 s.
- The earlier "snap to identity" seen in the short-hold runs is the end of this glide.
- **Effect on session 1:** the glide was reported as head movement. The smoothed pitch fell, a false
  "recovered" followed, and the silence afterwards froze the detector on the glide's end value.
- **Fix:** `src/devices/freeclip2/rebuild.rs` holds back samples once the head has been still ≥ 2.5 s and the
  raw angle to identity starts decreasing in steps consistent within ±0.6°.
  - If the run ends at identity, the samples are dropped and the reference becomes the held pose.
  - If the steps break pattern, the movement is real and reporting resumes.
  - Tests replay this recording (no glide reaches the output; tones fire in "down" and recoveries in "center").
    They fail if glide detection is disabled.
- **Glides can start after only ~2 s of stillness** (live session 2026-11-11, `tests/fixtures/freeclip2/short_hold_glide.csv`):
  the head settled at ~51° by ~17 s, and the glide ran 19.27–20.49 s at a slower 0.67°/sample, ending at identity.
  With the 2.5 s threshold the glide passed as movement back to upright, so no reminders fired while slouching.
  The threshold is now 1.0 s; real movement still breaks the constant-step pattern and is reported.

## Wear detection (tools/freeclip2/device/wear_test.py, 2026-11-11)

- Taking the earbuds off does **not** disconnect Bluetooth (RFCOMM stayed up with both off for 15 s+).
- The earbuds push **43/37** on every wear change: TLV1 = left, TLV2 = right (1 = worn, 0 = off),
  TLV3/4 = 0. Each change was reported about 3–4 s after the prompt.
- Querying 43/37 with empty TLV1/TLV2 returns the current state, e.g. `1=01 2=01 3=00 4=00`.
- OpenFreebuds uses 43/3 TLV 8/9 for in-ear state on other models. FreeClip 2 sent no 43/3 during the test.

## Stream restart (session 3, 2026-11-11)

Data: `tests/fixtures/freeclip2/stream_restart.csv` (from `--raw-log`).

- After about 10 s of silence while the user held a look-down pose (~51°), the 43/89 stream resumed with its
  **sequence number lower than before** (721 → 235). The values were near identity and followed the same pattern
  as a stream start (slow creep to 0, then the identity re-centre at seq 279).
- A restarted stream's orientation is relative to the head pose at the moment it started. Treating it as a
  continuation caused a false "recovered" while still looking down, and then a −55° offset once upright. So
  after the first recovery, later slouches never reached the threshold.
- Why the earbuds restart the stream is unknown. A primary/secondary earbud handover (each bud has its own
  IMU) is a guess `[unverified]`. `--raw-log` now also records non-pose frames, so the next occurrence shows
  what came before it.
- **Fix:** `Rebuilder` treats `seq < previous seq` as a new stream and takes the last known absolute orientation
  as its reference. On this recording that reads ~51° until the user moves, then −3° to −7° when upright. Two
  tests (rebuild-only and end-to-end with the detector) fail without the fix.

## Single earbud (tools/freeclip2/device/single_bud_test.py, 2026-11-11)

- With both earbuds worn the pose stream runs at ~52 samples/s, pausing only while the head is still.
- **As soon as either earbud is removed** (43/37 `1=01 2=00` or `1=00 2=01`), the stream stops within about 1 s
  and stays stopped while the user keeps nodding. This held for left only, right only, and putting one earbud
  back on after both were off.
- That test never re-sent 43/88 while one earbud was worn. But headup re-sent it after a one-earbud wear event
  and still got no samples. So a re-enable most likely does not revive the stream with one earbud.
  Confirmed 2026-11-12 ("Grounding of headup's device logic").
- Consequence: the backend reports "worn" only when both earbuds are worn. The session waits for the first pose
  before prompting calibration (previously it failed with "no head pose … during calibration" and reconnected).
- Other frames seen around wear changes: 43/54, 43/49 (paired-device list: "pc", "iPhone"), 43/94, 43/180,
  1/39 (battery). None of them is needed for tracking.

## Tracking runs only while this computer streams audio (2026-11-11) — supersedes parts of the above

Evidence: `data/reclaim_test.log` (controlled) and session 4's raw log (`--raw-log` now includes 43/54 pushes).

- The earbuds connect to two devices at once (43/49 lists `pc` and `iPhone`, both connected). **43/89 head tracking
  is sent only while this computer is actively streaming audio to the earbuds.**
  - **Controlled test:** with the phone active, there were 0 samples/s. The laptop played a 3.5 s clip; 43/54
    reported the PC's state `…09` at +0.44 s, tracking started about 2.5 s after the clip began at 52/s, then
    43/54 `…03` and tracking stopped about 5 s after the clip ended. The ~5 s tail matches PipeWire suspending
    the idle Bluetooth output `[inference]`.
  - **Session 4:** every tracking window lies inside a PC `…09` → `…03` window, and each window opened at a
    headup sound (calibration prompts, Tone, Speak).
  - **43/54** TLV5 = 6-byte address + state. Observed states: `09` = streaming audio from this device,
    `03` = connected and idle, `01` = connected (seen at connection) `[meanings inferred]`.
- **Each new stream measures from the head pose at the moment it starts.** At stream restarts (seq reset) during
  a ~53–58° slouch, the first raw samples were ≈ 0°. A resumed stream after a gap can start from a different pose
  than before the gap. Carrying the last pose over is only right if the head did not move.
- **Earlier conclusions that need re-checking under continuous playback:**
  - "Stream pauses while the head is still" was probably mostly the audio stopping. The guided tests had prompts
    every few seconds.
  - The "re-centre glide" about 5 s into a hold coincided with audio ending, so it may be the spatial engine
    winding down rather than a follow-mode re-centre.
  - "Stream restart" (seq backwards) is plausibly a new audio session.
  - "Single earbud stops tracking" was observed while the iPhone had just connected (35.65 s), so it may be
    partly confounded. Re-tested 2026-11-12 without that confound: confirmed ("Grounding of headup's device logic").

## Hold test under continuous music (data/hold_music.csv, 2026-11-11)

- With music playing on the laptop throughout, holds behaved exactly as in tests/fixtures/freeclip2/hold_test.csv:
  ~5–6 s of samples after each move, then the glide to identity (resets at 14.7, 44.7, 58.8, 88.6 s), then
  silence until the head moved again (silences of 24 s and 23 s). The sequence never went backwards.
- So **both** effects are real and independent:
  1. Tracking only flows while this computer streams audio (43/54 state bit 0x08).
  2. During playback, a still head produces the glide and then silence.
  The glide suppression and the "silence = still" rule stay.
- Consequence: silence cannot signal "audio stopped". The backend reports 43/54 for this computer's address as
  `Event::Available`, and headup ends monitoring when tracking becomes unavailable (DECISIONS "Recalibrate after
  every unavailability"). The earbuds report this computer's audio stop ~5 s after it stops
  (data/grounding_test3.log: audio off 75.11 s → `…03` 80.18 s; 116.79 → 121.86 s), so a track skip does not
  show as one.

## Tracking outage (2026-11-11, cause not observable over MBB)

- For ~2 h, 43/88 was acked (`2=00`) but no 43/89 arrived, even with the head moving and music playing. At the same
  time: both earbuds worn (43/37), spatial mode follow (43/180 `2=01`), 43/49 listed pc with state 09 (playing).
- No effect: restarting the stream during playback, pausing music, forcing an A2DP suspend/resume (43/54 03→09), SBC
  instead of AAC, a case reset, a reboot, and an older NixOS generation (PipeWire 1.6.9). Firmware unchanged
  (7.0.0.516). tools/freeclip2/device/headpose.py failed too, so headup's code was not the cause.
- Tracking returned right after a probe that sent only queries: `43/90:1=`, `43/44:1=`, `43/180:1=18,2=,3=`. It
  then survived a case reset with no 43/90 sent.
- Hypothesis `[unverified]`: the 43/90 capability query arms the stream, and the arming survives a case reset but not
  a longer power-off. On the first night, 43/90 (probe_02) was also sent right before the first working 43/88
  (probe_03). The vendor app never sends 43/88, so no app code confirms this.
- Firmware reading (2026-11-12) refutes both hypotheses for the recovery **[code]**: none of the probe's queries
  can reach the stream's start/stop logic. 43/90 only records the host; the 43/180 handler reaches none of the
  start/stop functions; and `43/44:1=` (an empty value) is dropped: the TLV parser stores no value pointer for a
  zero-length value (`0x7034a152`), the getter then returns null (`0x7034a44a`), and the 43/44 setter
  (`0x70395996`, "ambient sound and motion state") takes its error path. So tracking came back at the same time as
  the probe, not because of it.
- With `2=00`, this computer was the current device and `enable` was set, so a sensor-side condition failed (status
  flags from messages 0x841a/event 10, the mode byte from 0x8402, or the sensor pipeline not delivering samples).
  None of these is visible over MBB, so the cause cannot be determined from the recordings. headup now logs every
  43/88 reply, so a recurrence will show at least which side refused.

## Stream switch sent while idle (2026-11-11, evening)

- 17:15:39 local: after the earbuds had been away for about an hour, headup connected and sent 43/90 then 43/88 (acked)
  while this computer was idle. Audio started at 17:17:58 (43/54 → streaming), but no 43/89 arrived in the next hour.
- 17:19: the same binary, restarted during playback, got 43/89 at 0.10 s and 280 samples in 25 s
  (`data/outage2_raw.jsonl`).
- A 43/88 sent while idle *can* work: in `data/reclaim_test.log` (first night) it was sent while the phone was active,
  and tracking started once a clip played.
- headup now sends 43/88 again whenever 43/54 reports this computer starting to stream. Live check
  (`data/repro_fix.jsonl`): the 43/88 sent while idle at 0.19 s gave no samples; audio started at 18.05 s, and the
  re-sent 43/88 was followed by 43/89 at 18.13 s. The earbuds had already been tracking at 17:19, so this run does not
  show that the re-send was needed.
- Hypothesis `[unverified]`, consistent with every case today and the first night: after a long power-off, the earbuds
  need 43/90 *and* a 43/88 sent while this computer streams.
- The firmware explains the 17:15 case **[code + confirmed]**: a 43/88 from a device that is not the current
  (last-streaming) device is answered `2=02` and leaves `enable` unset, so nothing starts when audio later begins.
  A 43/88 sent after this computer becomes current is answered `2=00` and starts the stream (`data/live2_raw.jsonl`,
  `data/live3_raw.jsonl`). This is why a refused switch is re-sent on 43/54; an accepted one is not (DECISIONS
  "Re-send the stream switch only if it was not accepted").

  | Case (local time) | Long power-off before | 43/90 sent | 43/88 sent while streaming | Tracked |
  |---|---|---|---|---|
  | First night, reclaim test | no | earlier | no | yes |
  | Morning outage, 11:13–12:51 | overnight | no | yes | no |
  | 12:53, after the query probe | overnight | yes | yes | yes |
  | 17:15 service | ~1 h | yes | no | no |
  | 17:19 manual run | ~1 h | yes | yes | yes |

- The user chose not to test this further: the backend now does both, so it covers the hypothesis either way.

## Firmware image (2026-11-11)

How the app gets firmware (decompiled app, `research/jadx_out/sources`):
- Version check: `POST https://update-dre.platform.hicloud.com/smartAccessory/v2/Check.action?latest=true&defenceHijack=true`
  (`in/j.java:468`, `in/j.java:862-875`). The host comes from GRS service `com.huawei.cloud.hota`; the US is in group DR3
  (`update-dre`), China is `update-drcn`. Both returned the same answer.
- Body: `{"rules":{"deviceName":"HuaweiBTFT0027","packageName":"com.huawei.BTFT0027","deviceId":<SN, MBB 1/7 TLV 9>,
  "packageType":"full","packageVersionCode":<1/7 TLV 7>,"firmware":<1/7 TLV 7>,"os":"audioapp"}}`. No auth or
  signing. The optional `extra_info` (phone IDs) was left out.
- Download: unauthenticated GET of `{components[0].url}full/filelist.xml`, then the zip it lists (md5, sha256, size).
- The app unzips, takes the first `.bin`, and sends it unchanged over MBB service 9 (`in/j.java:965-990`,
  `nn/m.java:392-427`). Any decryption or signature check happens on the earbuds.

What we got (files in `research/firmware/526/`, gitignored):
- Newest version: **HarmonyOS 7.0.0.526(F003H003C00)**, created 2026-10-21. The test earbuds run .516.
- `update_accessory_fw.zip` (7,484,023 bytes, sha256 matches filelist.xml) is signed with SignApk
  (`META-INF/CERT.*`, `hotakey_v2`). It holds `updatepkg.bin` (7,709,002 bytes) plus metadata
  (`version.txt`: type=BTFT0027, subtype=generic).
- `updatepkg.bin` header names the image "robin" (the app's internal name for productId ZAAS is also "Robin"),
  version `f1.0.0.526`, build 2026-10-15, chip string **`FHISI_BS25`** (HiSilicon BS25 `[inferred]`).
- **The image is not encrypted** (2026-11-12). The 8.00 bits/byte sections are LZMA. Every section's SHA-256 in the
  header matches, which proves the layout below. `tools/freeclip2/firmware/unpack.py` splits, decompresses and checks it.
- Package layout (all integers big-endian):
  - `0x00`: u16 1, u16 0x88, u32 header length (0x38e), u32 0x180; name `robin` at 0x0c; version at 0x4c; build
    date/time at 0x90.
  - `0xb0`: u16 5, u16 table length (0x1e0), then 10 entries of 48 bytes: u16 section id, u16 0, u32 kind,
    u32 stored size, u32 image size, 32-byte SHA-256. Kind `0xac` = plain (stored size field 0, image size = size
    in the file). Kind `0xca` = compressed (sizes = compressed and decompressed).
  - `0x294`: TLV 0x0008 (`HISI_BS25` and other fields), then TLV 0x1005: for each compressed section, u16 id and the
    SHA-256 of its compressed payload.
  - Sections follow back to back from the header length, in table order. The file ends with 0x200 bytes that are
    probably a signature `[inferred]`.
- Each section: 0x200-byte header (0x180 bytes of high-entropy data, probably a signature `[inferred]`, then zeros),
  the payload, and a u16 trailer equal to the section id. The table SHA-256 covers the plain payload, or the
  decompressed payload for kind `0xca`.
- Compressed payload (`hwcompress10`): 32 bytes (the TLV 0x1005 digest is computed over this payload, so these are
  not that digest; meaning unknown), `hwcompress10`, `0x40`, u32 decompressed size, `0x40`, LZMA props `0x5d` (lc 3,
  lp 0, pb 2), u32 LE dictionary size 0x2000. Then blocks: u32 length and a raw LZMA1 stream that decodes to
  64 KiB (the last one shorter).
- Contents (strings and byte counts in the decompressed images; roles `[inferred]` unless stated):

  | id | kind | image | content |
  |---|---|---|---|
  | 1 | plain | 120 KiB | Second-stage bootloader (`SSB|...`), RISC-V. |
  | 3 | lzma | 1.7 MiB | RISC-V image, LiteOS (`LOS_*`). |
  | 4 | lzma | 2.9 MiB | DSP image `HIF:`, `525(DSP)` (audio codecs, LDAC; not RISC-V). |
  | 5 | lzma | 1.6 MiB | RISC-V application and Bluetooth image: AT test commands (`set_spatial_mode`, sensor calibration), `LiteAIRotationPredict`, `rotation_perdict_task`. |
  | 6 | plain | 524 KiB | `MBBSUBCOMPONENT` package with two LZMA-alone RISC-V images; the second has `fotaSleep` and `BLETESTMODE` (probably the charging case). |
  | 7 | plain | 1 MiB | SBC audio prompts: 42-byte frames, sync 0x9c, 48 kHz mono, bitpool 17. |
  | 10 | lzma | 132 KiB | Second DSP image `525(DSP)`. |
  | 12 | plain | 2 KiB | Key area / code info block (magic 0x4ba5c31e). |
  | 13 | plain | 21 KiB | `RTB:` table. |
  | 14 | lzma | 3.1 MiB | `IMOD` neural-network model (Ascend `te_*` kernels, GRU/UNet; probably noise or voice processing). |

- The RISC-V images (1, 3, 5) start with a HiSilicon key area (magic 0x4b1e3c1e family) and a code-info block at
  +0x100. The code info holds the body size and its SHA-256; the body starts at +0x300 (checked for 3 and 5).
- Few log strings survive: logs go through a ROM function (`0x46b90(level, id)`) with numeric IDs `0xff0xxxxx`, so
  nothing names MBB 43/88–90 in text. The head-tracking code is in image 5 (next section).

## Firmware head-tracking logic (image 5, .526; 2026-11-12)

Read by disassembly (`tools/freeclip2/firmware/xlinx.py`; RUNBOOK). Addresses are runtime addresses in image 5. Labels:
**[code]** = read directly from the instructions; **[confirmed]** = the code's prediction matches our recordings;
`[inferred]` = meaning guessed from context. Firmware globals have no names; the names below are ours.

### How to read the image
- Image 5 runs in place from flash: the body (section offset 0x500) loads at **0x70280300**. 1,167 string
  references land on string starts at this base, against ≤632 for any other.
- Disassemble it as **RV32**. objdump on a raw binary silently decodes RV64 (`c.jal` shows as `addiw`, losing
  ~4,800 calls). Wrap the binary in an ELF assembled with `-march=rv32imfc` (RUNBOOK).
- HiSilicon "xlinx" custom instructions. WS63 encodings are in hisi-riscv-qemu `docs/reference/xlinx-isa.md`;
  this chip differs. Decoded and checked:
  - **push/pop/popret** (16-bit): bits[15:11]=10011, rlist=bits[10:7] = {ra, s0..s(rlist−1)}, bit6 push,
    bit5 return, bits[4:2] = extra ×16 bytes. Checked: every function starts with a push and ends in popret.
  - **Opcode 0x0b, funct3 0/1/4/5 = branch on immediate** (`bnei a0,1,…`, WS63's 0x3b field layout). Checked:
    targets land on instruction starts and labels. Funct3 2 is also a compare-and-branch whose exact condition is
    not pinned down (shown as `b?i`).
  - **Opcode 0x0b, funct3 6 = load/store multiple**: base = rs1, bit 8 = store, register bitmap in bits 20–31
    (bit k = register x(k−13)). Checked: `0x078` = {a0–a3} and `0x0d8` = {a0,a1,a3,a4} match the following
    single loads and stores exactly (the quaternion history in `0x600fa53c`).
  - **Opcode 0x2b = WS63 "shf"** (`rd = rs1 OP (rs2 shifted)`, WS63's 0x1b layout), funct3 0–4. Checked:
    `andshf a2,a2,s0,sll,2` inserts bit 2; `addshf a3,a1,a0,sll,1` indexes a halfword jump table.
  - **Opcode 0x2b, funct3 7 = multiply-add with immediate**: `rd = rs1 + rs2 · 2·imm`, imm = bits 31–25.
    Checked: 6-byte address slots (`s0 + a0·6`) and the setter's `6·rate`.
  - **Opcode 0x7b = long gp-relative load/store**: funct3 bit2 = store, bits[1:0] = width. Load: rd=bits[11:7],
    offset=bits[31:15]. Store: rs=bits[24:20], offset = bits[31:25]:bits[11:7]:bits[19:15]. Checked: with this layout
    228 of 400 store offsets are also load offsets (20 for the next-best layout); only 66 of 1,513 offsets fall in
    the ±2 KB that ordinary `gp` loads cover.
  - **16-bit zero stores** in the RV32-invalid `c.slli` space (bit 12 set): base = x(8 + rd&7); shift bit 4 = 0
    → `sb zero, (sh&7)·2 + bit3`, = 1 → `sw zero, (sh&7)·2 + 16·(rd>>3)`. Checked: identity quaternions written as
    `1.0` then three zero words, and the status flags `[2]`/`[3]` cleared in the `0x841a` handler.
  - Compressed `lbu/lhu/sb/sh` in the C.FLD/C.FSD/C.FLDSP/C.FSDSP slots, as on WS63.
  - Not decoded: other 0x2b/0x5b funct3 values and the `c.srli/c.srai` forms with shift ≥32.
- **`gp` = `0x20007c48`.** The startup stub at the image start runs at `0x20000` (its copy lengths are run address +
  constant, and each source starts where the previous copy ends, which fixes the run address). It copies flash
  `0x702a7588`, `0x702a7e00` and `0x702aabc0` to `0x20003000`, `0x20003878` and `0x20006638` (data and the
  constants read gp-relative), and flash `0x70280510–0x702a7588` (code) to `0x201c0`. Checked: 380 of 388
  gp-relative float loads land on sensible constants, against ≤347 for gp ± 4.
- **Nothing the app calls is outside the package.** Besides the stub's code copy (`0x201c0`: the log function
  `0x46b90(level, id)`, `memcpy` `0x45d12`, the allocator `0x47080`), startup copy loops copy flash `0x702abf38` to
  `0x600d0000–0x601070ea` (the OS, libc and the head-tracking library; 929 functions) and two initialised-data blocks
  to `0x601070f0` and `0x60116308`. `tools/freeclip2/firmware/disasm.py` disassembles every region at its run
  address and annotates gp-relative loads with their initial values. (Corrected 2026-11-12: these were earlier
  taken for missing ROM.) Logs are numeric IDs, so almost no text names anything.
- Stock Ghidra and the Ghidra_RISCV31 module (WS63 encodings) cannot decode this variant.

### Command tables [code]
- Host MBB service 43 dispatch at `0x7030e0d4`: 88 `(cmd, handler)` pairs. 43/88 → `0x7039c018`,
  43/90 → `0x7039c82e`, 43/167 → `0x7039c36a`, 43/49–54 → `0x703795aa`, 43/180 → `0x7039fce2`.
- Two smaller tables (`0x7030df5c` set, `0x7030e01c` query) hold 43/88 and 43/167 again (`0x7039c2ca`,
  `0x7039c598`); `[inferred]` sync between the two earbuds.
- Internal messages from the Bluetooth core, `(id, handler)` table around `0x7030db08`: 0x8402 → `0x70370498`,
  0x8406 → `0x70370b62`, 0x841a → `0x70370d10`, 0x8424 → `0x70370e6c`.
- MBB send: `0x703942e6(service, cmd, tlvs, n_tlvs, &len)`.

### 43/88 (switch the data report on or off) — handler `0x7039c018` [code]
1. If `link[0] == 2` (byte set by message 0x8406): reply **43/88 TLV2 = 1** and stop.
2. Read TLV1 (`v`).
3. Compare the **current device** address (`0x70378420`, from `0x601428c0`) with the requesting link's address.
   If they differ: reply **TLV2 = 2** and stop. `enable` is unchanged.
4. Otherwise: `enable` (`0x6014a87d`) = `v`, record the requester as owner (`0x6014a87c+12`), reply **TLV2 = 0**,
   relay 43/88 to the other earbud (`0x70390d20`) and re-evaluate (`0x7039b5f0`, `0x7039b558`).

**[confirmed]** In `data/live2_raw.jsonl` and `data/live3_raw.jsonl`, headup's first 43/88 (computer idle,
phone last active) got `2=02`. After 43/54 reported this computer streaming, the re-sent 43/88 got `2=00` and
samples began within 0.03 s. Every other recording that streamed has `2=00`. So the "current device" follows audio:
at least, a device becomes current when it starts streaming. In `data/reclaim_test.log` the computer still got
`2=00` while the phone was connected but idle, so "current" is the last device that streamed, not only one
streaming now `[inferred]`.

Reply value 1 [code]: returned when `link[0] == 2`. `link[0]` is set by internal message 0x8406, which the
Bluetooth core (image 3, `0x1017ad88`) sends when a link to the **stored peer-earbud address** changes
(`0x100f2910` compares the event address with it): value 0 on connect (the app core then prints the trace
`"Tws connected"`), value 2 on disconnect with the HCI reason (e.g. 22 = 0x16, terminated by local host). So 1 =
refused while the earbud-to-earbud link is down (`SwitchReply::PeerLinkDown`). No recording has it; an earbud in
its closed case kept the link up and got `00` (data/grounding_test.log).

### 43/89 (samples) [code]
- Sample loop `0x7039c994` waits on queue 40. For each sample:
  - If `status[1] != 1` or `status[3] != 1`: overwrite the sample with the identity quaternion, set `enable = 0`,
    and stop and re-evaluate. Tracking then needs `enable` set again.
  - It sends only if `enable == 1`, `link[1] == 0`, `mode[2] == 0` and `mode[0] == 2` (`mode` = `0x60149724`).
- Sender `0x7039bb80`: TLV1 = 4 bytes (sequence), TLV2 = 16 bytes (quaternion). `0x7039b9d0` addresses the frame to
  the **current device** (internal message 0x8307 with its address, or 0x8300).
- The frame goes to the current device, **not** to the 43/90 list. (Earlier note retracted: the 43/90 list does
  not gate the stream on this core.)

### Start and stop (`0x7039c510`, `0x7039b5f0`) [code]
- `R` = `0x6014a87c`, 0/1/2. Its only writers: the host 43/167 handler (when the requester is current), the peer
  43/167 handler, and the device-change event (message 0x8424, which passes a per-device value into `R` via
  `0x7039be0c`).
- R = 1: command 0x603 if `enable == 1`, `mode[0] == 2` and `link[1] == 0`; otherwise 0x600 (stop).
- R = 2: command 0x602 if `mode[0] == 2`; otherwise 0x600.
- R = 0: 0x600, unless an internal tracker is running.
- `enable` is also set to 1 automatically (`0x7039b5ae`) when `status[1] == 1`, `status[3]` is set and `R != 0`.
- `status` = `0x6014281c`. Message 0x841a with state 1/2/3 sets `status[2]`/`status[3]` to 1/0, 0/1, 0/0.
  `status[1]` is set by entry 10 of an event table (`0x702f323c`).
- **R is not the 43/180 spatial mode [confirmed, live 2026-11-12, `data/mode_gate_live.jsonl`].** With this
  computer streaming silence, each step switched the stream off and on and counted samples for 4 s: follow 139,
  **off 138**, fixed 139, follow again 138, every 43/88 answered `2=00`. The mode was restored to follow. So the
  spatial mode does not gate the stream. What sets R for a host that never sends 43/167 is unknown (the per-device
  value in message 0x8424).
- `[inferred]` status[3] = "music playing" (0x841a state 2). Consistent with the grounding test: after a 20 s
  audio stop the stream stopped with 43/54 `…03` and came back by itself 0.3 s after `…09`, which is what the
  automatic re-enable `0x7039b5ae` (run on audio-state messages) predicts. A 0.4 s stop (`data/single_bud.log`)
  did not interrupt the stream.

### 43/90 (spatial-audio capability query) — `0x7039c82e` [code]
- `0x7039c6f0` stores the requesting link's 6-byte address in a 3-slot round-robin list (`0x6014a894`, cursor
  at +42) if it is not there yet, and schedules a save (`0x7039c6c2`: 19 bytes, key 73). The list is reloaded at boot.
- Reply: TLV 1, 2, 3 = 1 each.
- `0x703a6eae` returns whether the current device is in the list. Its users are the internal (on-device) path:
  for a device that is not in the list, with status flags set and `mode[0] == 2`, the earbuds start
  `0x703a6f92`/`0x703a6fc2` and forward mode changes to another queue (`0x7039bf60`). For a listed device they skip
  it. `[inferred]`: on-device head tracking for the earbuds' own spatial audio when the host does not handle it.
- `[inferred]` For headup the list probably does not matter (see 43/89 above). The 2026-11-11 outage is not
  explained by it.

### 43/54 state byte [code + recordings]
- Built in `0x7037a828` (per device, also used for 43/49 TLV 5): `state = connected | (audio << 1)`, where
  `connected` = `0x7037a464(i)` (0/1) and `audio` = `0x7037883a(address)`: the audio state of the earbuds'
  **current device** (`0x601428c0[16]`), or 0 for any other device. The same builder sends 43/54
  (`0x7037aa5c`: `43, 54`).
- `audio` comes from the Bluetooth core in internal message 0x8400 (`[state, address]`, handler `0x70370044` →
  `0x7037851a`); the core sends it from `0x1017af5a` with callers passing 0 and 4, and folds states 5 and 6 into 3.
- Recordings show 0x01 (`audio` 0), 0x03 (1) and 0x09 (4, streaming). headup reads streaming as `audio == 4`;
  other values (the core passes values ≥ 7 through) count as not streaming.

### After a device change [code]
- Message 0x8424 (`0x70370e6c` → `0x70378d0c`) moves the current device and ends in `0x7039be0c(x)` with the new
  device's stored value `x`: `R := x`, `enable := (x == 1)`, then start/stop is re-evaluated. Start needs
  `R == 1 && enable` (command 0x603) or `R == 2` (0x602, which does not check `enable`).
- From this code alone, a device that tracked before would track again after another device (a phone) had the
  earbuds, without a new 43/88. **Live, it does not** (data/grounding_test3.log "phone_takeover"): the phone took
  over at 123.4 s (`…09`) and poses stopped; this computer was current again at 134.9 s (its `…09`), yet no pose
  came for 9 s, until the 43/88 at 144 s (`2=00`, seq from 1). So the stored value `x` does not restore the
  stream here `[inferred: x is not 1 for this computer after the change, or the start needs more]`. headup's
  re-send when this computer streams again is required.

## Where the head-tracking computation is (image 5, 7.0.0.526) [code]

The orientation is computed in image 5's RAM code, so the re-centre glide and the stillness pause can be read from
the firmware; they are read below ("Re-centre and stillness"). The chain, from the sample sent back to its source:
- Task 40 (the sample loop, `0x7039c994`) receives `{buffer 0x6012de8c, type}` from `0x600e1be0`.
- `0x600e1844` fills that buffer: the sequence number (`+0`, incremented per sample), the 16-byte quaternion
  (`+4`) copied from the algorithm's output, and a flag (`+20`). It also logs three more floats as integers
  (probably angles `[inferred]`).
- Its caller (around `0x600e17b0`) feeds sensor triples (12 bytes each) to the algorithm through function
  pointers at `ops+32`, asks `ops+36` whether output is ready, fetches it with `ops+40` (40 bytes), and times the
  call. `ops` is the global `0x6012de58`, set at `0x600e1e96` from the image's tail table (`0x7041332c`).
- That pointer is the descriptor of the algorithm module **`robin_gesture`** (`0x601072a8`, one of a chain:
  `psensor_static`, `logical_ear_side`, `head_monitor`, `robin_gesture`; names in initialised data). Its ops:
  init `0x600df9e4` → `0x600f9b30`; feed `0x600dfa3e`, which negates and swaps the axes for the ear the earbud is
  in (property `persist.device.ear_side` or the logical ear side) → `0x600f9d56`; output `0x600dfa98` →
  `0x600f9e5e` (quaternion), `0x600fa080` and `0x600fa0fe` (two 3-vectors).
- The library around `0x600f9b30–0x600fexxx` is the fusion. Output path (`0x600f9e5e`, per sample): a linear
  regression over the last 4 fused orientations predicts ahead (latency compensation), then the re-centre stage
  `0x600fa53c`. Details in "Fusion and latency compensation" below.

### Fusion and latency compensation [code]
- **The fusion is a Mahony filter with gravity correction.** Feed `0x600f9d56`: type 0 = accelerometer (int16
  triples, stored as floats at `0x6015b904`), type 1 = gyroscope (int32 triples) → `0x600fa2d0`: gyro − bias
  (bias estimator `0x600ffa94`, state `0x6015b9c4`) × 0.001 / 57.2958 → rad/s, then `0x600ff812(accel, gyro, q)`:
  gravity estimate `v = (2(xz − wy), 2(wx + yz), w² − x² − y² + z²)`, error `e = â × v`, `ω += Kp·e`,
  `q += dt/2 · q ⊗ (0, ω)`, normalise. So `q` maps body to a gravity-aligned frame (Hamilton, body rates
  multiplied on the right); tilt is anchored to the accelerometer, heading is gyro-only. `Kp = 1.0`; for the first
  20 samples after init (`S+32 < 20`) the gyro is zeroed and `Kp = 50` (snap to gravity), and the re-centre stage
  outputs identity with `ref := q`. `q` is stored split: `w` at `0x6011c698` (`P+0`), `x, y, z` at `0x6015b944–4c`
  (`S+8..16`); read by `0x600fa438`.
- **Latency compensation** (`0x600f9e5e`): a 16-entry history of `q` (`0x6015b7a8`, shifted by `0x600fa1f2`);
  per component, least squares over the last 4 entries at x = 1..4 (`0x60109588`), evaluated at x = `[0x6011c694]`
  + 4 (static 27 + 4, i.e. 27 samples ahead); normalised; then `nlerp(current q, prediction, t)` (`0x600fa45a`) with
  `t = min(1, |ω_z|/2)` (`0x600fa3b8`, stored at `0x6015b8d8`). Still head → `t ≈ 0` → the stage gets the current
  `q`.
- Decoding notes for this code `[inferred from usage]`: `uxt? rd [sz=1]` acts as logical not (`0x600fa490` feeds
  `andn` so that `0 ≤ t ≤ 1` selects the blend); `b?i` (opcode 0x0b, funct3 2) branches when `rs1 < imm`
  (`0x600fa822`/`0x600fa8ec` skip the countdown when it is 0, and the `== 1` test inside it is reachable); opcode
  0x5b funct3 2 is a float load/store with base update (`0x600fa18c` stores at `a1+72` and leaves `a1 = s3+72`, which
  makes the following `4(a1)`, `a1−12`, `a1−56` the gyro vector, the accel at `+60` and `t` at `+16`; `0x600f9f0a`
  loads with a 16-byte stride over the history).

### Re-centre and stillness (`0x600fa53c`, parameters `P` = `0x6011c698`) [code + recordings]
- **`rel = q ⊗ conj(ref)`** (`0x600ff6b0` → `0x600ff626`, which computes `a ⊗ conj(b)`; checked term by term,
  Hamilton product, `w,x,y,z`). With the fusion above this is the rotation from `ref` to `q` **in the gravity-aligned
  frame**, not in the head frame. (Corrected 2026-11-12: this entry said `conj(ref) ⊗ q`.) Stillness:
  `Δ = rel ⊗ conj(anchor)` (`0x600ff626`), Euler angles in degrees (`0x600ff718`: pitch = asin(2(wy − zx)), roll =
  atan2(2(wx + yz), 1 − 2(x² + y²)), yaw = atan2(2(wz + xy), 1 − 2(y² + z²)), the same as `tools/freeclip2/quat.py`
  `euler`, × 57.2958); still while **every axis is within 10°** of the anchor (`0x600ff98e`). The anchor is `rel` at
  the first sample of the still run; it is used only for this test. A still counter counts samples and resets on
  movement.
- State. `P` (`0x6011c698`): `+0` `q.w`, `+4` countdown (init 1), `+8` still threshold, `+12` `N`, `+16` mode
  (1). `S` (`0x6015b93c`): byte `+0` re-centred, byte `+4` countdown finished, byte `+5` log pending, `+8..16`
  `q.x,y,z`, `+20` still counter, `+24` glide length latched at the first glide step, `+28` forced-identity flag
  (from `0x600fab40`, role unknown), `+32` warm-up counter, `+36` disables the forced-identity path, `+40` `ref`,
  `+56` `G0`, `+72` anchor. Input history `H0..H2` at `0x6015b994` (`H0` = input two samples back).
- Per sample, `0x600fa53c(out, q_in, …, &pause)` (`q_in` is the latency-compensated `q`):
  ```
  if every |q_in component| < 1e-6: q_in = I
  H0, H1, H2 = H1, H2, q_in                              # ldm/stm at 0x600fa64c-0x600fa678
  rel = normalize(q_in ⊗ conj(ref))
  moving = 2 <= P.countdown < N and any |euler(rel)| > 2.4      # 0x600fa6c4-0x600fa750
  if S.still == 0: S.anchor = rel
  S.still = S.still + 1 if all |euler(rel ⊗ conj(S.anchor))| < 10 else 0
  trigger = S.still > P.thr (P.thr + N if mode 0);  if trigger: S.still = 0
  o = rel
  if P.countdown >= 1:                                   # 0x600fa822-0x600fa854
      ref = q_in if P.countdown == 1 else H0             # the reference follows the input, 2 samples behind
      o = I;  P.countdown -= 1
  out = o
  if trigger: P.countdown = N; S.recentred = 1           # 0x600fa882-0x600fa89c
  if S.recentred and P.countdown == 0: S.done = 1        # no glide call
  elif moving and P.countdown >= 2:                      # cut short, 0x600fa8b4-0x600fa8e6
      out = glide(o, 1) = I;  P.countdown = 0            # ref stays H0 = input two samples before
  elif P.countdown >= 1: out = glide(o, P.countdown)
  pause = S.recentred and S.done and |pitch(out)| < 4 and |yaw(out)| < 4 and not moving   # 0x600fa986-0x600faa34
  if not pause and (moving or out not small): S.recentred = S.done = 0
  ```
  `glide(b, n)` (`0x600faa4e`): on the first call `G0 = b` (negated if `w < 0`) and the length is latched; output
  `normalize(t·G0 + (1−t)·b)` with `t = (n−1)/latched` (`0x600fa45a`: scale `0x600ff68e`, add `0x600ff5f8`, normalize
  `0x600ff572`). `b` is `rel` on the trigger sample and `I` afterwards, so the glide is `nlerp(I, G0, (68−k)/69)`
  whatever the head does: **the output during a countdown never depends on the input.** `pause` goes out with the
  sample (buffer `+20`, `0x600e18b8`); task 40 drops samples whose flag is 1 (`0x7039c9c0`).
- **The glide arithmetic is exact nlerp in f32** (read 2026-11-13). `0x600fa45a(out, a, b, t)`: if `0 ≤ t ≤ 1` (the
  `1` is the f32 constant at `0x20007804`, bytes `0000803f`), `out = normalize(t·a + (1 − t)·b)`, otherwise
  identity. Scale `0x600ff68e` and add `0x600ff5f8` are plain `fmul.s`/`fadd.s` per component; normalize
  `0x600ff572` sums the squares in f32 (`fmadd.s`), widens it to double (`0x3dcec`), returns identity if it is
  below `1e-6`, else passes it to `0x60105fc4`, narrows the result (`0x3e4ee`) and divides each component by it
  (`fdiv.s`). `0x60105fc4` is a soft-float double routine shaped like a square root (halves the exponent by its
  parity, starts from a table at `0x60116208`) `[inferred: sqrt]`; even an approximate double result is far below
  f32 precision once narrowed. So the routine itself differs from the f64 formula in `rebuild.rs` by f32 rounding
  only (order of 1e-4°).
- **Recorded glides fit the formula to f32 precision** (2026-11-13). The 0.126° "worst misfit" reported before and
  in DECISIONS was an artifact of measuring angles as `2·acos(|a·b|)` (in `tools/freeclip2/quat.py` and Rust
  `Quat::angle_to`): near 1 the result is dominated by the inputs' norm error (f32 samples: |norm − 1| up to
  1e-7, giving ~0.05°; the 6-decimal logs of 2026-11-11: up to 7e-7, giving ~0.12°). Measured as
  `2·atan2(|vec(a·b̄)|, |scalar(a·b̄)|)` on normalised inputs, every clean glide ≥ 5° in `data/*_raw.jsonl` (76) fits
  `nlerp(I, P, (68−k)/69)` within 0.00015° (6-decimal logs) and 0.00001° (f32 logs). The first re-centre of a
  stream follows the same formula exactly.
- **A stream can resume in the middle of a glide** (data/live6_raw.jsonl, second stream): after an audio gap the
  sequence restarted at 227 (162 s later) with samples already gliding, from 1.25° to the identity sample at seq 279. The pose the
  glide started from is not in the stream, so no hypothesis can follow it. The same happened in
  tests/fixtures/freeclip2/stream_restart.csv (721 → 235, a 0.46° glide). In both, the glide's start is seq 211,
  where every stream's first re-centre starts: the new stream's first ~230 samples were never sent. In live6 the
  43/49 and 1/39 frames 4.4 s before the first sample match the stream starting then (226 samples at 52.5 Hz);
  why its samples were not sent is `[unverified]` (the sample loop sends only while `enable`, `link[1]` and `mode`
  allow; "43/89 (samples)"). Solving for the glide's start from the first sample after the gap (each step `k`
  gives one start pose) explains both resumes exactly and matches none of the other 109 resumes in `data/`.
- **An identity sample without a glide** (data/live4_raw.jsonl, third stream): after a wear flicker (43/37 off and
  on at the same instant, then 43/88), the stream read identity for seq 1–11, rose to 3.34° by seq 33 and read
  identity from seq 34 on, with no glide before it. Which firmware path does this is `[unverified]` (candidates:
  the stage's forced-identity flag `S+28` or its warm-up counter `S+32`). headup's rebuilder reports it as a missed
  glide and moves its reference to the last pose, the same as for a glide cut short.
- **Which pose becomes the reference.** During the countdown, the input two samples back; on the step after the
  identity sample (countdown 1), the current input. With a still head that is the held pose. It is not the glide's
  start pose, not the still run's anchor, and not a prediction: the input is the latency-compensated `q`, but its
  weight on the prediction is `min(1, |ω_z|/2)`, ~0 when still.
- Timeline of a re-centre (trigger sample = the held pose): the 68 next samples are the glide, the 68th is the
  identity (countdown 2 → 1, `t = 0`); the next sample sets `ref := q_in`, finishes the countdown and pauses (output
  identity, small), so the identity sample is the last one sent.
- **Cut short** (head turning faster than 2.4° on one Euler axis over 3 samples, i.e. ~0.8°/sample): output identity,
  countdown 0, and the reference stays the input two samples before the identity sample (not the glide's start
  pose). Movement slower than that is absorbed for the whole countdown. Recordings: the first sample after a cut
  short of a silent re-centre (below) reads 2.3–8.0° (26 cases), i.e. three samples of fast movement.
- **The stage keeps running while the stream is paused, so it re-centres silently.** The pause ends only when the
  output reaches 4° in pitch or yaw (roll is not checked) or a countdown is cut short. Meanwhile the still counter
  keeps counting; it was reset at the visible trigger, so every 209 samples of stillness another countdown runs,
  `ref` moves to the current pose, and the output (a glide from below 4°) stays paused. Recordings (all
  `data/*_raw.jsonl` and fixtures, 147 pauses after an identity sample; scratchpad `recentre/resume.py`,
  `pauses.py`):
  - 121 pauses end with a normal sample: max(|pitch|, |yaw|) of the first sample is 4.00–8.58° (median 4.16°),
    never below 4; |roll| reaches 11.1°.
  - 26 pauses end with an identity sample (a silent countdown cut short by movement). With `gap` = seq of the resume
    − seq of the visible identity sample, these have `gap mod 209` = 145–207 (25 of 26; one at 66), the silent
    countdown window (142–208). The 99 normal resumes have `gap mod 209` = 3–141, never inside the window. By chance
    a third of them would fall inside.
- **Consequence: the reference can move without the stream showing it.** Head (or earbud) movement is lost from the
  stream when it happens (1) during any countdown, visible or silent, slower than ~0.8°/sample, or (2) during a
  pause, below 4° pitch and yaw per 209 samples (any roll within the 10° still box), because each silent re-centre
  moves `ref` to it. With a perfectly still head nothing is lost: a Python model of this stage
  (`recentre/fwsim.py`) fed a still out-and-back cycle rebuilds the upright exactly (0.000°).
- Values: static `P[8] = 200`, `P[12] = 67`; the runtime setter (`0x600fa29e`, from `0x600f9c5c`) overwrites them
  with `6·rate/dec` and `2·rate/dec`. The sensor mode selects `rate`/`dec`: mode 9 → 104 Hz, dec 3 (15 → 120 Hz,
  dec 4; else 100 Hz, dec 3), so **`P[8] = 208`, `N = P[12] = 69`**.
- Recordings agree (`tools/freeclip2/recordings/glide_check.py`, all `data/*_raw.jsonl`, rerun 2026-11-13): 156
  re-centres, glide length 68 samples from the held pose to the identity sample at every rate seen (39–62 Hz); the
  formula `nlerp(I, held, (68−k)/69)` fits 57 clean glides within 0.00014° (single-precision rounding; the 0.126°
  reported before was the `acos` artifact described below); the still run before a glide has a median of 214
  samples (threshold 208). So the rule is a sample count, not a duration: ~4 s at 52 Hz, ~6 s at
  35 Hz.
- This explains "every stream's first re-centre ends at seq 279–280" (Re-centre model): a new stream's still counter
  starts with the stream, so the first trigger comes after 208 samples and the 69-step glide ends at ~279.
- headup's rebuilder against every recording (all `data/*_raw.jsonl`, 67 full re-centres, replayed through
  `Rebuilder`): the rebuilt orientation moves at most 3.0° during a re-centre (one, at a glide boundary next to real
  movement) and at most 0.33° in the other 66. The slerp and nlerp glide models give identical results there; nlerp
  is kept because it is the firmware's formula (fit margin 4× larger).
- `head_monitor` has the same shape (axis remap → `0x600f948e` → library near `0x600fd000`) but its ops global
  (`0x6012d2cc`) does not reach task 40. It is a separate feature `[inferred: Huawei's own head-posture
  monitoring]`. The blob after the name `LiteAIRotationPredict` is passed to a small neural-network setup at
  `0x600f9936` (role unknown).
- `tools/freeclip2/firmware/anchors.py` checks this chain. It finds the module whose ops global is called at
  +32/+36/+40 by a function that reaches the task-40 sender.

## Grounding of headup's device logic (firmware 7.0.0.526, 2026-11-12)

The earbuds run 7.0.0.526 (MBB 1/7 TLV 7, hardware `HL1SAKM_Ver.A`), the version disassembled above. Live test:
`tools/freeclip2/device/grounding_test.py` → `data/grounding_test.log`, repeated with a phone-takeover phase in
`data/grounding_test2.log` (both earbuds, this computer streaming silence;
43/88 sent only where noted). Re-check every row on new firmware (skill `freeclip2-firmware`).

| headup relies on (where) | Evidence | Status |
|---|---|---|
| MBB framing: `Z`, u16 BE length, 0, svc, cmd, TLVs, CRC16-XMODEM (`mbb.rs`) | every frame in every recording passes the CRC | grounded (recordings) |
| 43/89: TLV1 = u32 LE seq, TLV2 = 4 × f32 LE `w,x,y,z` (`protocol.rs`) | sender `0x7039bb80` copies 4 + 16 bytes; the loop writes identity as `1.0` then three zeros | grounded (code + recordings) |
| 43/88 TLV1 = 1/0 switches the stream; reply TLV2 0/2 (/1) (`protocol.rs`, `link.rs`) | handler `0x7039c018`; `2=02` → `2=00` in live2/live3 and `data/refusal_live*.jsonl` | grounded (code + live); value 1 = earbud-to-earbud link down (code: message 0x8406); never seen live |
| A refused 43/88 leaves the stream off until this computer streams (`link.rs` refusal) | handler leaves `enable` unchanged on 2; `data/refusal_live*.jsonl` | grounded (code + live) |
| Samples go only to the earbuds' current device (`tracking_hint`, `Available`) | `0x7039b9d0` addresses frames to the current device; reclaim test; refusal test | grounded (code + live) |
| 43/88 "on" restarts the stream: sequence from 1 (`rebuild.rs` new stream) | grounding test 9.9 s, 63 s, 104 s | grounded (live) |
| A stream can also restart with a lower, non-1 sequence (`rebuild.rs`) | grounding test 94.7 s (901 → 437) after an audio gap; stream_restart fixture | grounded (live) |
| 43/37: TLV1 left, TLV2 right worn; TLV3/TLV4 = left/right in the case (`protocol.rs` reads 1–2) | grounding test 37 s: `4=01` with the right earbud in its case | grounded (live); TLV3/4 meaning new |
| One earbud: no samples, and 43/88 does not revive them (`Worn` = both) | grounding test 18.6 s: stopped within 0.5 s; 43/88 at 23 s and 41 s answered `00`, no samples | grounded (live, no iPhone confound) |
| Re-wearing needs 43/88 again (`link.rs` `on_wear`) | grounding test: both worn at 54.1 s, no samples while nodding until the 43/88 at 63 s | grounded (live) |
| Audio stop/start: tracking resumes when this computer streams again (`Available`, 43/54) | grounding test: 43/54 `…03` 5 s after audio stopped, samples stopped; audio back → 43/54 `…09` → samples 0.3 s later **without** 43/88 (firmware auto re-enable `0x7039b5ae` runs on audio-state messages) | grounded (code + live); headup no longer re-sends an accepted switch here (`data/gap_live*.jsonl`) |
| 43/54 TLV5 = address (LE) + state; streaming = `(state >> 1) == 4` (`protocol.rs`) | encoder `0x7037a828`: `connected \| audio << 1`, audio of the current device ("43/54 state byte"); `01`/`03`/`09` in all recordings | grounded (code + recordings) |
| Stillness: re-centre after 208 samples within 10° on every axis (~4 s at 52 Hz), then the stream pauses; resumes on movement without 43/88 | `0x600fa53c` (P[8] = 208); recordings: median still run 213.5 samples; grounding test 109–129 s; resume when pitch or yaw of the output reaches 4° (roll not checked), 121 of 121 normal resumes ≥ 4.00° | grounded (code + recordings + live); while paused the earbuds re-centre silently every 209 still samples ("Re-centre and stillness") |
| Stitch `absolute = raw × reference` (`rebuild.rs` `Stitch`) | `0x600ff6b0`: `raw = q ⊗ conj(ref)`, `q` in a gravity-aligned frame (Mahony `0x600ff812`), so a rebase composes on the right | grounded (code); fixed 2026-11-12 (was `reference × raw`); test `nod_after_a_turn_reads_as_pitch`, guided run 1 upright pitch after turns 8.5–9.3° → 0.9–4.4° |
| Sequence keeps counting through silences | grounding test: 559 → 1300 over 14 s of silence (~53/s) | grounded (live) |
| Re-centre = 68-sample glide, `nlerp(I, held, (68−k)/69)`, ending in an exact identity sample (`rebuild.rs`) | `0x600fa53c`/`0x600faa4e` (N = 69, nlerp); 156 recorded re-centres, 57 clean glides within 0.00014° (`recordings/glide_check.py`) | grounded (code + recordings) |
| 43/90 is harmless; only records the host (`protocol.rs` connect) | handler `0x7039c82e`; mode test and every session sent it | grounded (code); not needed for the stream on this core |
| Spatial mode (43/180) does not matter | `data/mode_gate_live.jsonl`: off/follow/fixed all stream | grounded (live) |
| Another device (phone) streaming takes the earbuds over; this computer's audio does not preempt it, and a 43/88 from this computer is then refused (`link.rs` cancels an accepted switch when another device streams) | `data/grounding_test2.log` "phone_takeover": phone `…09` at 124 s; this computer's audio from 134 s gave no `…09`; 43/88 at 144 s → `2=02` | grounded (live) |
| After a takeover, the stream does not resume when this computer is current again; a new 43/88 is needed (`link.rs` re-sends it when this computer starts streaming) | `data/grounding_test3.log` "phone_takeover": phone paused, this computer `…09` at 134.9 s, no poses for 9 s; 43/88 at 144 s → `2=00`, seq from 1 | grounded (live) |
| After another device (phone) had the earbuds, a 43/88 sent once this computer streams again is accepted and starts the stream (`link.rs`: switch no longer accepted → re-sent on 43/54 `…09`) | live: phone last active, this computer `…09` → 43/88 `2=00` → samples within 0.03 s (`data/refusal_live*.jsonl`, `data/live2_raw.jsonl`, `data/live3_raw.jsonl`); code: `0x7039be0c` would also re-enable by itself ("After a device change") | grounded (live + code) |

## Prior art for re-centring (2026-11-11)

- **Android spatial audio** (AOSP `frameworks/av/media/libheadtracking`, read from `refs/heads/main`): the host
  re-centres automatically. `StillnessDetector` checks that every pose in a window stays near the newest one; after
  `kAutoRecenterWindowDuration` = 6 s within `kAutoRecenterRotationThreshold` = 10.5°, `HeadTrackingProcessor`
  calls `recenter()` and enables `PoseRateLimiter`, which moves the output to the new target at
  `kMaxRotationalVelocity` = 0.8 rad/s ≈ 46°/s (`services/audiopolicy/service/SpatializerPoseController.cpp:47-76`).
  That is the same shape as the FreeClip 2 glide (stillness, then a constant-rate slew to the new centre, ≈45°/s in
  hold_test.csv). The firmware may use a similar pipeline `[inferred]`. The live glide in short_hold_glide.csv was
  slower (≈38°/s), so the rate is not exactly fixed.
- Android's head-tracker sensor reports re-centres explicitly: `head_tracker_event_t.discontinuity_count`
  (`hardware/libhardware/include_all/hardware/sensors.h:312`). The FreeClip 2's exact identity sample plays the same
  role.
- **HeadOrbit** (`research/ios-refs`, `QuietRecovery.swift:189-194`) flags an unannounced reference reset when the
  attitude jumps more than 35° while the gyroscope rate is under 8°/s. It needs a separate rotation-rate signal,
  which AirPods give and the FreeClip 2 stream does not.
- **cristdulcey/posture** measures tilt from the gravity vector, which re-centring cannot move, and uses the
  attitude only for yaw (`SetupFlow.swift:269-272`). The FreeClip 2 stream has no gravity vector.
- The other posture apps (slouchless, posture-fix, dorso) read `attitude.pitch` directly and do not handle resets.

## Re-centre model and reference drift (data/session6*.jsonl, 2026-11-11, 30 min of normal work)

- **Every re-centre is a fixed-length blend.** Of 74 identity samples, 32 are preceded by a straight ramp (fit residual
  ≈0.1°) of 66–72 samples (≈1.3 s), from starting angles of 1.5° to 56°. The rest are ramps run into earlier movement
  by the measuring script, or short irregular ramps where the head moved during the blend. So the earbuds blend over
  a fixed ~68 samples, not at a fixed speed. Small re-centres (a few degrees) happen 3–9 times a minute during normal
  work and are far below the rebuilder's 0.3°/sample glide threshold, so they reach the output as movement.
- **The sequence counter keeps counting through silences** (≈53 per second), so the earbuds keep sampling and only
  stop sending.
- **headup's upright reference drifts.** Upright pitch went from ≈0° (minutes 0–14) to −14° (16–20) and −45° (24),
  and roll went to ≈25°. The two missed slouches at 14:21 and 14:22:50–14:24 (t ≈ 1300 s and 1362 s) peaked at only
  +7.6° and +13° rebuilt pitch while the raw angle reached 49–66°.
- **Offline replays** (scratchpad scripts, not committed): blending the reference over the 68 samples before each
  identity sample did not stop the drift with `abs = reference * raw`. With the opposite order, `abs = raw *
  reference` (new reference = raw at glide start × old reference), the drift roughly halves (upright −10° to −14° at
  minute 16–27 instead of −21° to −39°) and the missed slouches reach 37–46°. The error still grows by 5–10° at each
  big slouch → glide → recovery, and stays flat during normal work. Without ground truth this cannot be separated
  from real posture changes.
- **Resolved (2026-11-12, code):** `raw × reference` is the right order (`raw = q ⊗ conj(ref)` in a gravity-aligned
  frame, "Re-centre and stillness"). Re-measured with a plain stitch (rebase to the sample 68 before each identity,
  scratchpad `recentre/stitch.py`): session 6 upright pitch median −8.7° to −16.3° over minutes 8–29 with
  `raw × reference`, −6.2° to −43.0° with `reference × raw`; the slouches near t = 1300–1362 s peak at 45.9° vs
  24.6°. Over all recordings the two orders differ by a median 4.4° (p95 43°), mostly in session 6. The remaining
  growth at big slouches is movement hidden by the earbuds ("Ground truth").

## Ground truth: the earbuds' re-centre is ~10° off (data/slouch_guided.csv, `guided.py --slouch`, 2026-11-11)

- Five cycles of upright (same screen point) 20 s → look down 30 s (two of them turned left/right).
- Raw pitch holds steady within each pose; nothing creeps during a hold. But the re-centre itself is inconsistent:
  looking down read 39.9°, 45.1° and 30.6° at glide start, and the return to the same screen point then read −50.1°,
  −56.1° and −46.7° (10–16° larger). Every reconstruction therefore ends each cycle 10–16° "above" upright.
- Replays: the current rebuilder gives upright 0 → −10 → −11 → −22 → −24 → −41° and the last look down only +5.9°.
  Neither the composition order (`reference * raw` vs `raw * reference`) nor blending over the 68 glide samples changes
  this by more than a few degrees. The error is in what the earbuds report, so no rebuild order can remove it.
- **Where it comes from (2026-11-12, code + recordings; the movement itself `[inferred]`).** The magnitudes differ,
  so it is not a frame or composition problem: out 40.1° → back 50.3°, 63.7° → 64.2°, 45.4° → 56.3°,
  60.0° → 67.5°, 31.0° → 47.0° (rotation angle of the held raw pose; frame-independent). The code rules out the
  other candidates: `ref` is never set from the still run's anchor; the latency prediction has ~0 weight on a still
  head; and with a still head the reference is exactly the held pose (model `recentre/fwsim.py`: 0.000° error).
  What is left is movement during the windows where the earbuds move `ref` without reporting it ("Re-centre and
  stillness": countdowns, and silent re-centres every 209 samples during a pause). In this recording each down
  pause lasted 340–1214 samples (1–6 silent re-centres), and three of the five down movements began inside a silent
  countdown (identity resume; the third cycle's began 59 samples into it, so up to ~1.1 s of a slow start was
  absorbed). The first cycle had neither a cut-short start nor a cut-short return (+10.2°), so its loss happened
  during the 1181-sample pause: a slow drift of the head or the clip while looking down `[inferred]`. Outside
  countdowns a pause hides at most ~4° of pitch per silent re-centre; inside a countdown any movement slower than
  ~0.8°/sample is hidden.
- Size of the effect in the model (pitch only, 52.5 Hz, 30 s hold at 45°): a drift of 0.1/0.3/0.6/1.0°/s during
  the hold loses 2.5/6.4/12.1/22.9°. A 45° movement that starts at a random point of the pause loses at most
  3.8/6.7/10.3/21.3° if it takes 0.5/1/1.5/2 s, and more than 1° in 34–43% of starts. Both losses point toward
  "looking up" after a look down. Without a drift, a still head loses nothing.
- Out-and-back movements with no re-centre between them (8 in all recordings, `recentre/excursions.py`) return
  within −8.3° to +4.0° of the start (mean −0.9°), so returning to the same pose is accurate to a few degrees when
  nothing is hidden.
- The error has one sign in all data so far (upright drifts towards "looking up"; session 6 likewise). That makes a
  guarded re-baseline effective: if pitch stays below −8° for 3 s, take that pose as upright. Replayed, it keeps
  upright within 0 to −2° and every look down at 28–48° (guided), and upright within about ±7° with the missed
  session-6 slouches at 25–36° (3 re-baselines in 30 min). 5° and 8° thresholds gave the same guided result.
- The user hesitated ~10 s at the "look down at your lap" step (third look down); phase values use the last 60 samples.

## Searching for a drift-free signal (2026-11-11)

- Probed: `43/151:1=` no reply; `43/152:1=` → 127=000186a3 (unsupported, like 43/48, 43/115, 43/124). `43/88:1=02` and
  `43/88:1=03` are acked (`2=00`) but start no stream; `43/88:1=01,2=01` gives the normal 43/89 stream. No absolute or
  gravity-referenced data found.
- Other earbuds: Samsung Galaxy Buds stream a gravity-referenced game rotation vector plus raw gyroscope and
  accelerometer at ~50 Hz (geekyshubham/Murmury, research/FINDINGS_samsung_sensors.md); AkbulutMirac/galaxy-buds-headtracker
  measures pitch as a forward vector's angle to the horizon in that frame, so there is no drift to fix. LibrePods
  reads AirPods orientation only for nod/shake gestures and documents no recentring or drift handling `[from search
  summaries, repos not read]`.
- Without an absolute reference, the remaining options are priors about posture: a one-sided bound on "looking up"
  (proposed guard), the dominant still pose over a window as upright (fails for users who slouch most of the time),
  or judging only the latest movement since the last re-centre (no accumulation, but misses a slouch that creeps
  down in small held steps).

## Sample rate varies between sessions; glides stay ~68 samples (data/live_check_raw.jsonl, 2026-11-12)

- **The 43/89 rate is not fixed.** In the live check after the 2026-11-12 refactors, continuous stretches ran at
  **34.9–35.0 Hz** (seq +1 per sample, so nothing was dropped). Every fixture and every earlier raw log (sessions 2–8)
  ran at **52.2–53.0 Hz**. What sets the rate is unknown `[unverified]`; candidates are the audio codec or load from
  the other connected source (the phone).
- **Glides are a fixed sample count, not a fixed duration.** Both clear glides in the live check were 68 samples
  long, which took **1.95 s** at 35 Hz (1.3 s at 52.5 Hz, as in sessions 6–8).
- **Consequence for the rebuilder:** `DELAY_S` (2.0 s) must be longer than a glide. At 35 Hz the identity sample
  arrived 1.95–1.98 s after its glide began, so nothing got through (checked offline: 0 glide samples released before
  each identity), but the margin was 20–50 ms. Glides measured at 74–101 samples in sessions 7–8 would last 2.1–2.9 s
  at 35 Hz and partly get through.
- Since 2026-11-12 the rebuilder holds samples by count (DECISIONS "Hold samples by count"), so the rate no longer
  matters.
- **Transport stalls.** Gaps of 2.6–3.65 s between consecutive sequence numbers occur: at stream start (identity
  samples only) and, four times, early in a stream mid-movement. One (data/outage2_raw.jsonl, +7.63 s) fell inside a
  small (2°) glide. `GLIDE_STALL_S` (5 s), how long a suspected unfinished glide waits, is longer than all of these.
- **The earbuds also go quiet without a re-centre.** In tests/fixtures/freeclip2/stream_restart.csv the head was held
  down and the stream went silent for 10.7 s with no identity sample, then restarted.
- Analysis scripts were run inline (not committed). The method: rate = samples / time over runs of consecutive seq
  with gaps under 0.2 s; glide = consecutive samples before an identity whose angle to identity strictly decreases.

## 43/88 acknowledgement says whether the switch took effect (data/live2_raw.jsonl, 2026-11-12)

- The 43/88=1 sent at connect, while this computer was idle, was acknowledged with TLV 2 = `02`. The 43/88=1 re-sent
  when this computer started streaming (43/54 state 0x09) was acknowledged with TLV 2 = `00`, and samples followed
  within ~40 ms. Reading: `00` = applied, `02` = not applied (no stream to this device) `[inferred from one session]`.
  This fits "Stream switch sent while idle" and explains why a refused switch is re-sent on streaming.
- The session ran at 35.1 Hz again (see "Sample rate varies between sessions"); glides at 68 samples.

## Glide model checked against every recording (2026-11-12)

Offline analysis of all raw logs and fixtures (scripts in the session scratchpad, not committed), with the exact
identity sample as ground truth for each glide's end.

- **Model:** from the pose `P` held before a glide, the `k`-th sample after it reads `slerp(P, I, (k+1)/69)` for
  k = 1…68, and sample 68 is the exact identity sample. The reading's rotation angle falls linearly towards identity on
  a fixed axis, but the first sample is already about two steps along: in all 60 clean glides from ≥ 10°, the jump
  from `P` to the first glide sample is 1.78–2.07 steps (typically 1.95), and `P` itself was still. (A first version
  of this entry fitted a 67-step ramp from the first glide sample and called that `q_s`; that start was one sample
  late.) From `P`, 145 of 170 glides fit within 0.5° (median 0.23° for glides from ≥ 10°, 0.11° below). (These
  misfits were measured with `acos` and are mostly its artifact; "Re-centre and stillness" has the exact fit.)
- **28 glides (16%) do not fit:** they are cut short (section "Head movement during a glide": the earbuds end a
  glide early when the head moves, and the movement does not show in its readings). The code settles which device
  behaviour it is (2026-11-12, "Re-centre and stillness"): the earbuds move the reference to the input two samples
  back at every step, and the readings are a synthetic glide that ignores the head.
- ~~The head barely moves during clean glides~~ (withdrawn 2026-11-12): inverting clean glides gave at most 0.8° of
  head movement, but the readings during a glide do not depend on the head, so they cannot measure it. Movement
  slower than ~0.8°/sample during a glide is lost without a trace ("Ground truth").
- **Every stream's first re-centre ends at seq 279–280** (all 16 recordings), about 5.3 s after the stream starts
  at 52.5 Hz.
- **A causal test can confirm a glide early:** checking each new sample against the model from recent start poses
  confirms most large glides within 2–4 samples. Real head movement occasionally matches the exact ramp speed for a
  few samples (e.g. session 4, 4 samples at 43°), so a match is only provisional until the identity sample.

## 43/54 forms; live check of the model-based release (data/live3*.jsonl, 2026-11-12)

- 43/54 is the paired-device-list change report ("连接管理—PDL列表变化上报"). Besides the TLV 5 form (device address
  + audio state), it also comes as TLV 1 = `01 00` with nothing else (once, 13.8 s after connecting, around when
  audio started). Meaning unknown `[unverified]`.
- With the model-based release, a slouch held still cued 10.2 s after it started (the earbuds went silent for 9.6 s;
  the held poses came out 0.2 s into the silence). Recovery came the 70-sample hold (1.32 s at 52.6 Hz) after
  sitting up. Sitting up 8.0 s into a slouch gave no reminder.
- Three large re-centres (glides from 61°, 58°, 48°) between calibration and the next still upright put the rebuilt
  upright 22.6° towards "looking up", in line with ~10° per re-centre (section "Ground truth"); re-anchoring
  corrected it.

## Head movement during a glide (2026-11-12, tests/fixtures/freeclip2/glide_move.csv)

- Test: `tools/freeclip2/device/glide_move.py` beeps the moment a glide starts (offline: catches 35 of 38 glides from ≥ 10°
  in the fixtures, no false alarms), and the user looks straight back up. Two of three trials beeped (35 Hz).
- **The earbuds cut the glide short:** the identity sample came ~34 samples into the ramp (~0.4 s after the move
  began) instead of 68. **The movement does not show in the readings before it:** they keep following the still-head
  ramp (0.78°/sample from 52.6°). Those glides fit no full-length model (misfit 28° and 33°); the four still-head
  glides in the same session fit within 0.5°.
- **After the identity sample the reference is the pose two samples before it** (code, "Re-centre and stillness";
  earlier wording "roughly onto the glide's start pose" corrected 2026-11-12). Here the upright head then read
  −63.6° raw (the start pose was 52.6° down), and the rebuilt pose after each move is −11.5° and −19.0° (true: ~0°),
  the usual ~10° per re-centre towards "looking up" (trial 2, with no move, drifted the same way). By the code that
  means the earbud pose two samples before the cut was ~11° below the start pose, i.e. it moved down before or as the
  head came up `[inferred]`. The rebuilder's
  fallback (drop the 70 samples before the identity sample, reference = pose before them) handles it; nothing stays
  at the slouch angle. Test `moving_during_a_glide_is_not_lost`.
- The guided axis runs' glides with a late start and an early end (headpose_axes*.csv, e.g. `turn_left` → `center`
  at 35.8 s) are most likely the same thing: the head started turning at the prompt.

## Instant resets while walking (data/reconnect_raw.jsonl, data/turn_test_raw.jsonl, 2026-11-13)

- Two runs with the user walking to another room and back (fixtures `walk_reconnect.csv`, `turn_walk.csv`). Each
  had two **instant resets**: the output went straight to identity with no glide, the sequence numbers
  consecutive, so nothing was lost in transit. From 68° (yaw −68°) and 17° in the first run; from 108° (yaw
  −107°, pitch 13°) and 6° in the second. Across the 17 earlier recordings, all at the desk, there was one such
  sample (live4 seq 34, after a wear flicker).
- The guided run (prompts in data/turn_test_phases.tsv) turned the head 50–67° left and right at the desk three
  times: no instant reset, and facing the screen read pitch 0.0–1.0°, yaw −5.8° to −8.7° each time. After the walk,
  facing the screen read pitch 8.4°, yaw −121°. After the first run's walk the user sat upright and headup read
  pitch 24–29° [user report].
- The first walk also had long silences (35 s, 25 s) while the user walked [user report], and the return turn
  (~180°) never shows: the largest movement after leaving is 17°. A pause resumes at 4° of movement, so the
  turn should have shown [inferred: samples sent but not received, or a slower stream; see below].
- Near-versus-far run (data/walk2*.jsonl, fixture `walk_near_far.csv`, RSSI from `hcitool rssi` each second in
  data/walk2_rssi.tsv, prompts in data/walk2_phases.tsv): 70 s walking in the same room (RSSI 0 to −9) had 2
  instant resets and only ordinary pauses; 70 s walking to another room and back (RSSI down to −27) had 1 reset
  and no gap at all, every sequence number received, but only 660 samples in 78 s (~8.5 per second instead of
  ~52). So instant resets come from walking, not from a weak link, and on a weak link the stream slows down
  rather than losing samples [one run].
- **Firmware cause [code, 7.0.0.526].** An activity detector on the accelerometer (`0x600fab40`, called from the
  re-centre stage at `0x600fa91a`): a 4th-order IIR filter on |a|, max−min over 30-sample windows ≥ 501 counts a
  window as active; 10 active windows in a row (~310 samples, ~6 s) raise its flag, 4 quiet ones lower it. The
  stage copies it into `S+28` (`0x600fa906–0x600fa984`, read and checked 2026-11-13): on the rising edge
  `ref := q_in` and the output is identity; while it is up, every sample outputs identity with `P.countdown := 1`,
  so the next sample sets `ref := q_in` again. The reference follows the head, the still counter runs, and after
  ~209 samples the usual still trigger pauses the stream. It stays paused until the flag drops (the user quiet for
  ~4 windows) and a later still trigger runs a silent identity glide; the stream then resumes relative to the head
  pose at that moment. The fusion `q`, heading included, is not touched. So **every head movement from the reset
  to the resume is lost**: the output after it is `q(t) ⊗ conj(q(t_end))`. Recorded identity runs before the gap:
  211, 210, 211, 211 samples (one of 626 is unexplained). Units of the accelerometer threshold are unknown
  [inferred: walking bounce]. `S+36` (set by stream-control command 0 with argument 1) disables the detector; the
  stream start sends argument 0.
- Decoder note: opcode 0x0b funct3 3 is "branch if rs1 ≥ imm" (funct3 2 is `<`); `xlinx.py` prints both as `b?i`
  [code: loop at `0x600df116`, warm-up test at `0x600fa34c`].
- Earlier guess, now explained: the yaw error after the first walk (≈ −85° to −99°) is
  close to the sum of its two resets' yaw (−68° − 17°), which would fit a reset that does not move the reference
  to the head [inferred, one run].
- Recalibrating on every instant reset was tried and rejected: walking produced one about every 50 s, so the
  user kept hearing "sit upright", once mid-walk. headup models the hold instead: tracking is unavailable from
  its first identity sample to the first sample that is not identity (DECISIONS "The earbuds' hold makes tracking
  unavailable").
- Live check of that (data/walk3*.jsonl, fixture `walk_slouch.csv`): two holds in a 75 s walk; the earbuds
  released the first one while the user paused in another room (tracking resumed 8.5 s later, then held again).
  After the walk the user's look-downs read +46° raw, as at the desk, so the earbuds' own frame is fine; the
  readings are relative to the head pose where the hold ended.
