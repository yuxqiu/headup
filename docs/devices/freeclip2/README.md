# HUAWEI FreeClip 2 with headup

What to know when you use headup with FreeClip 2 (tested on firmware HarmonyOS 7.0.0.526). The evidence behind
each point is in [FINDINGS.md](FINDINGS.md).

## When the earbuds track your head

- **Audio must play on this computer.** FreeClip 2 sends head tracking only to the device it plays audio from. If
  you pause audio on the computer, or play something on your phone, tracking stops, and headup pauses monitoring.
  When you play something on this computer again, headup asks you to sit upright once more. Skipping a track does
  not interrupt it. If nothing plays when headup starts, it says so at once.
- **Both earbuds must be worn.** Taking either one off pauses monitoring; putting it back on calibrates again.

## Moving around

FreeClip 2 stops tracking your head while it senses movement: when you walk around, and sometimes for a moment just
after you put the earbuds back in. headup pauses monitoring then, so no reminder comes from where your head was
before.

When you are back, **move your head a little**. The earbuds resume only once your head moves; headup then waits until
you hold still and asks you to sit upright. If you sit down and keep perfectly still, the earbuds stay silent, and
headup cannot tell that from still moving around, so it waits for that movement.

## Drift

The earbuds re-centre themselves while your head is still and hide slow head movement, so over time headup's idea of
upright drifts. headup corrects it whenever you look down and then back up: where your head comes to rest after
looking back up becomes upright. If reminders seem off, do it on purpose: look down at your desk, then look back up
at your screen and hold still for a second.
