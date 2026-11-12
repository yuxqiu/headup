# Posture-app UX research (AirPods-based apps)

Sources:
- **Posture Pal**: the best-known iOS app. It is closed source; details come from press coverage.
- **Five open-source macOS apps**, all built on `CMHeadphoneMotionManager`. Shallow clones are in
  `research/ios-refs/` (gitignored). Code facts carry `file:line` refs, taken from a source read on
  2026-11-11 and spot-checked by hand.

No open-source *iOS* posture app of note turned up besides wizenheimer/workwell (not reviewed). The
open-source ecosystem lives on macOS as menu-bar apps. That model is closer to our Linux desktop use case anyway.

## Posture Pal (iOS, Jordi Bruin)

Press coverage: [9to5Mac](https://9to5mac.com/2022/03/17/posture-pal-iphone-app-airpods/),
[Cult of Mac](https://www.cultofmac.com/reviews/posture-pal-use-airpods-to-improve-posture-awesome-apps),
[BGR](https://www.bgr.com/2209617/how-to-use-airpods-to-fix-posture-app/).

- The user starts a "Posture Session". The app tracks neck tilt in the background, including while music plays.
- Sensitivity presets are 10°, 20° and 35° of neck tilt. Pro adds a custom value.
- Alerts:
  - visual, sound, vibration and Apple Watch notification
  - it can *lower music volume while slouching* and restore it on correction
  - a mascot pops up on slouch
- Also: session timers of 5–60 min, daily goals, and session stats (average pitch/roll/yaw).
- Reported complaints: it fired on every head movement until tuned, and "calibration should be easier".

## Open-source apps: how each one works

| App | Reference (baseline) | Detection | Alert / UX |
|---|---|---|---|
| slouchless | Manual 3 s mean, persisted | EMA α 0.15; \|Δpitch\| 8° warn / 15° bad; alert after 5 s | One notification per episode |
| HeadOrbit | Manual "sit straight & recenter", plus guarded auto-recovery. A pitch/yaw/roll jump >35° at <8°/s is treated as an unannounced reference reset (`QuietRecovery.swift:189-194`) | Pitch **up** +15° held 5 s; off at −5° for 0.8 s (`PostureReminderAction.swift:38-39`) | Full-screen blur "Sit up straight", Esc snoozes 60 s |
| dorso | Look at the 4 screen corners, averaged; per-desk profiles | ≈9.5° default (presets 8.6–20°); 8 consecutive bad readings | Progressive blur/glow/border; pauses of 5–60 min; daily score |
| posture-fix | One tap, not persisted, dropped on data loss | EMA α 0.2; 12° down; hold 8 s; recover after 1.5 s | Edge glow → after 30 s optional sound/voice "Gently lift your head"/notification; 180 s cooldown (`AlertManager.swift:19-22`); away detection |
| posture (cristdulcey) | Guided 3 s capture, rejected if p90 wobble ≥ 8°; learns axes from look down/up/tilt/turn | Gravity vector, yaw-free; EMA 0.15 (`Tracker.swift:46`); forward ≥15° or roll ≥20°; 30 s grace; re-nudge ≥ max(60, 3×grace) s (`Tracker.swift:68`) | Notification + "Tink" on recovery; bending menu-bar figure; daily score, streaks; CLI with JSONL |

## Common defaults across these apps

- **Threshold:** 8–15° from the baseline (typically 12–15°). Almost all count forward/down only.
- **Smoothing:** EMA with α 0.15–0.2, or none.
- **Hold time:** about 5–8 s before a soft/visual cue; about 30 s before anything interruptive.
- **Recovery:** hysteresis is rare. Only HeadOrbit uses −5° / 0.8 s.
- **Re-nudge cooldown:** 60–180 s.
- **Baseline capture:** 3 s averaged, ideally with a wobble check.
- **Known limitation, stated by several apps:** head angle ≠ spine posture. A slumped back with the head level
  goes undetected. Position this as a *head-tilt* reminder.

## What this means for FreeClip 2 on Linux (approved by the user 2026-11-11; implemented parameters in DECISIONS.md)

1. **We own the baseline.**
   - Guided calibration with spoken prompts (we already have this): "sit upright", then 3 s capture with a
     wobble check (p90 < 8°, retry otherwise). Persist it to `~/.config/...`.
   - The device's identity resets are frame changes. They are handled by `src/devices/freeclip2/rebuild.rs` logic and never treated
     as "upright". This matches HeadOrbit's stance.
2. **Silence means "unchanged", not "stale".** The FreeClip 2 stops sending while the head is still. Hold the last
   state and keep the dwell timer running. Only an RFCOMM disconnect or earbud removal (in-ear state is
   available over MBB) pauses tracking.
3. **Yaw-free forward pitch.**
   - Measure lean relative to the baseline and ignore heading, so turning to a second monitor is benign.
   - Count forward/down only.
   - Optionally measure roll at ≥20°.
4. **Account for our ±5–10° error.**
   - Default threshold 20° (about 15° plus error margin), turn off 5° below it.
   - EMA α≈0.15 at 50 Hz; implemented as a 0.123 s time constant so it does not depend on the sample rate.
   - A dwell of about 10 s for the first cue.
   - Don't reset the dwell timer on a single noisy sample.
5. **Earbud-first cues.** The earbuds are the one output the user always has:
   1. Short soft tone in the earbuds after the dwell time.
   2. Spoken "lift your head" if the user is still slouching after about 30 s.
   3. Desktop notification (libnotify) if still slouching. Re-nudge cooldown about 120 s.
   4. Positive "tink" on recovery.
   5. Optional: duck the PipeWire volume while slouching (Posture Pal's best-liked feature).
6. **Controls and visibility:**
   - Snooze/pause (CLI command or tray).
   - A live angle readout, so users can trust the threshold.
   - JSONL output for stats.
   - Daily score = upright time ÷ monitored time.
7. **Drift.** We have no gravity reference proof yet. Offer re-calibration on demand. Measure long-session drift
   before adding any automatic re-baselining. If we add it, copy HeadOrbit's guarded approach: input-activity
   triggered, multi-window agreement, and conservative.
