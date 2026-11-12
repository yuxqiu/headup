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
- **Glides can start after only ~2 s of stillness** (live session 2026-11-11, `tests/fixtures/freeclip2/short_hold_glide.csv`):
  the head settled at ~51° by ~17 s, and the glide ran 19.27–20.49 s at a slower 0.67°/sample, ending at identity.
  With the 2.5 s threshold the glide passed as movement back to upright, so no reminders fired while slouching.
  The threshold is now 1.0 s; real movement still breaks the constant-step pattern and is reported.

## Wear detection (tools/freeclip2/wear_test.py, 2026-11-11)

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

## Single earbud (tools/freeclip2/single_bud_test.py, 2026-11-11)

- With both earbuds worn the pose stream runs at ~52 samples/s, pausing only while the head is still.
- **As soon as either earbud is removed** (43/37 `1=01 2=00` or `1=00 2=01`), the stream stops within about 1 s
  and stays stopped while the user keeps nodding. This held for left only, right only, and putting one earbud
  back on after both were off.
- That test never re-sent 43/88 while one earbud was worn. But headup re-sent it after a one-earbud wear event
  and still got no samples. So a re-enable most likely does not revive the stream with one earbud
  `[unverified beyond that one observation]`.
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
    partly confounded `[re-test needed]`.

## Hold test under continuous music (data/hold_music.csv, 2026-11-11)

- With music playing on the laptop throughout, holds behaved exactly as in tests/fixtures/freeclip2/hold_test.csv:
  ~5–6 s of samples after each move, then the glide to identity (resets at 14.7, 44.7, 58.8, 88.6 s), then
  silence until the head moved again (silences of 24 s and 23 s). The sequence never went backwards.
- So **both** effects are real and independent:
  1. Tracking only flows while this computer streams audio (43/54 state bit 0x08).
  2. During playback, a still head produces the glide and then silence.
  The glide suppression and the "silence = still" rule stay.
- Consequence: silence cannot signal "audio stopped". The backend reports 43/54 for this computer's address as
  `Event::Available`, and headup ends monitoring only after 30 s of *unavailability*.

## Tracking outage (2026-11-11, cause unknown)

- For ~2 h, 43/88 was acked (`2=00`) but no 43/89 arrived, even with the head moving and music playing. At the same
  time: both earbuds worn (43/37), spatial mode follow (43/180 `2=01`), 43/49 listed pc with state 09 (playing).
- No effect: restarting the stream during playback, pausing music, forcing an A2DP suspend/resume (43/54 03→09), SBC
  instead of AAC, a case reset, a reboot, and an older NixOS generation (PipeWire 1.6.9). Firmware unchanged
  (7.0.0.516). tools/freeclip2/headpose.py failed too, so headup's code was not the cause.
- Tracking returned right after a probe that sent only queries: `43/90:1=`, `43/44:1=`, `43/180:1=18,2=,3=`. It
  then survived a case reset with no 43/90 sent.
- Hypothesis `[unverified]`: the 43/90 capability query arms the stream, and the arming survives a case reset but not
  a longer power-off. On the first night, 43/90 (probe_02) was also sent right before the first working 43/88
  (probe_03). The vendor app never sends 43/88, so no app code confirms this.

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
  version `f1.0.0.526`, build 2026-10-15, chip string **`FHISI_BS25`** (HiSilicon BS25 `[inferred]`), and a table of
  5 entries. Our first guess at the table layout gave overlapping ranges and no matching sha256, so the layout is
  still unknown.
- Only the start is readable: a second-stage bootloader (`SSB|...` log strings, EPMU power management). Almost
  everything else has 8.00 bits/byte entropy, so it is encrypted or compressed. No readable strings about the IMU,
  spatial audio, head tracking or MBB 43/88–90 (only `MBBSUBCOMPONENT`).
- Conclusion: the firmware does not explain the 43/88 behaviour without decrypting or decompressing the main
  sections. That is a separate, open-ended project.

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
  from real posture changes `[open]`.

## Ground truth: the earbuds' re-centre is ~10° off (data/slouch_guided.csv, `guided.py --slouch`, 2026-11-11)

- Five cycles of upright (same screen point) 20 s → look down 30 s (two of them turned left/right).
- Raw pitch holds steady within each pose; nothing creeps during a hold. But the re-centre itself is inconsistent:
  looking down read 39.9°, 45.1° and 30.6° at glide start, and the return to the same screen point then read −50.1°,
  −56.1° and −46.7° (10–16° larger). Every reconstruction therefore ends each cycle 10–16° "above" upright.
- Replays: the current rebuilder gives upright 0 → −10 → −11 → −22 → −24 → −41° and the last look down only +5.9°.
  Neither the composition order (`reference * raw` vs `raw * reference`) nor blending over the 68 glide samples changes
  this by more than a few degrees. The error is in what the earbuds report, so no rebuild order can remove it.
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
  This fits "Stream switch sent while idle" and explains why the re-send on streaming is needed.
- The session ran at 35.1 Hz again (see "Sample rate varies between sessions"); glides at 68 samples.

## Glide model checked against every recording (2026-11-12)

Offline analysis of all raw logs and fixtures (scripts in the session scratchpad, not committed), with the exact
identity sample as ground truth for each glide's end.

- **Model:** from the pose `P` held before a glide, the `k`-th sample after it reads `slerp(P, I, (k+1)/69)` for
  k = 1…68, and sample 68 is the exact identity sample. The reading's rotation angle falls linearly towards identity on
  a fixed axis, but the first sample is already about two steps along: in all 60 clean glides from ≥ 10°, the jump
  from `P` to the first glide sample is 1.78–2.07 steps (typically 1.95), and `P` itself was still. (A first version
  of this entry fitted a 67-step ramp from the first glide sample and called that `q_s`; that start was one sample
  late.) From `P`, 145 of 170 glides fit within 0.5° (median 0.23° for glides from ≥ 10°, 0.11° below).
- **28 glides (16%) do not fit:** the head moved during the blend. With the head still, the two possible device
  behaviours (moving the reference vs fading the reported rotation) give identical readings, so this data cannot
  tell them apart.
- **The head barely moves during clean glides:** inverting 37 of them (fading model) gives at most 0.8° of head
  movement. So head movement during glides does not explain the ~10° per re-centre error (section "Ground truth").
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
