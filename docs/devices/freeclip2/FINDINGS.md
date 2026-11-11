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

`tools/freeclip2/used_cmds.py` resolves every `{43, X, ...}` byte array in the decompiled source.
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

Scripts: `tools/freeclip2/probe.py` (generic MBB send/log), `tools/freeclip2/headpose.py` (stream + decode).
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

Data: `tests/fixtures/freeclip2/hold_test.csv`, recorded with `tools/freeclip2/guided.py --hold`: look down 30 s, center 15 s, twice.

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

## Wear detection (tools/freeclip2/wear_test.py, 2026-11-11)

- Taking the earbuds off does **not** disconnect Bluetooth (RFCOMM stayed up with both off for 15 s+).
- The earbuds push **43/37** on every wear change: TLV1 = left, TLV2 = right (1 = worn, 0 = off),
  TLV3/4 = 0. Each change was reported about 3–4 s after the prompt.
- Querying 43/37 with empty TLV1/TLV2 returns the current state, e.g. `1=01 2=01 3=00 4=00`.
- OpenFreebuds uses 43/3 TLV 8/9 for in-ear state on other models. FreeClip 2 sent no 43/3 during the test.
