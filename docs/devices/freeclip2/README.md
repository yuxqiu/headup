# HUAWEI FreeClip 2 with headup

What to know when you use headup with FreeClip 2 (tested on firmware HarmonyOS 7.0.0.526). The evidence behind
each point is in [FINDINGS.md](FINDINGS.md).

## When the earbuds track your head

- **Audio must play on this computer.** FreeClip 2 sends head tracking only to the device it plays audio from. If
  you pause audio on the computer, or play something on your phone, tracking stops, and headup pauses monitoring.
  When you play something on this computer again, headup asks you to sit upright once more. Skipping a track does
  not interrupt it. If nothing happens at start, play something on the computer: headup says so in its output.
- **Both earbuds must be worn.** Taking either one off pauses monitoring; putting it back on calibrates again.

## Walking around

FreeClip 2 stops tracking your head while you walk around. headup pauses monitoring then, so no reminder comes from
where your head was before the walk.

When you are back, **move your head a little**. The earbuds resume only once your head moves; headup then waits until
you hold still and asks you to sit upright. If you sit down and keep perfectly still, the earbuds stay silent, and
headup cannot tell that from walking, so it waits for that movement.

## Drift

The earbuds re-centre themselves while your head is still, and each re-centre lands a little off, towards "looking
up". headup corrects this after each time you sit back up from a slouch.
