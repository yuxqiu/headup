# headup

Gentle reminders to lift your head, using the motion sensors already in your earbuds.

headup reads head tracking from supported earbuds on Linux. When you look down for too long, for
example at a phone or a low laptop screen, it reminds you through your earbuds. It needs no camera
and no cloud; everything runs locally.

## Supported earbuds

| Earbuds | Status | Head tracking | Wear detection (pauses when taken off) | Tested firmware |
|---|---|---|---|---|
| HUAWEI FreeClip 2 | Supported | Yes | Yes | HarmonyOS 7.0.0.516 |

## Requirements

- Linux with BlueZ (the standard Bluetooth stack) and PipeWire audio.
- Earbuds that are paired and connected to the computer.
- `pw-play` (PipeWire) and `notify-send` (libnotify). The Nix package includes both.

## Install and run

With Nix (flakes enabled), from a checkout of this repository:

```sh
nix run .
```

headup finds connected supported earbuds by itself. It keeps running: if no supported earbuds are
connected yet, or the connection drops (for example when you put them back in the case), it waits and
reconnects. Stop it with Ctrl-C.

To keep a record of your head angle and reminders, add `--log FILE` (JSON lines).

If several supported earbuds are connected, pass the address of the ones to use:

```sh
bluetoothctl devices Connected
# Device C0:DA:5E:F1:A0:A6 HUAWEI FreeClip 2
nix run . -- C0:DA:5E:F1:A0:A6
```

### Run in the background (Home Manager)

The flake provides a Home Manager module that runs headup as a systemd user service in your
graphical session:

```nix
# flake inputs: headup.url = "path:/path/to/headup";  (or wherever you keep it)
{
  imports = [ inputs.headup.homeManagerModules.default ];
  services.headup = {
    enable = true;
    # device = "C0:DA:5E:F1:A0:A6";   # optional; default: first connected supported earbuds
    # logFile = "${config.xdg.stateHome}/headup/log.jsonl";   # optional
  };
}
```

Check on it with `systemctl --user status headup` and `journalctl --user -u headup`.

## What to expect

1. **Calibration.** A voice asks you to sit upright and look straight ahead. Hold still for about
   3 seconds. If you move, it asks again.
2. **Monitoring.** Looking down counts. Looking up, or turning your head to another screen, does not.
   Short glances down are fine.
3. **Reminders.** If you keep looking down (more than about 20°):

   | After | Reminder |
   |---|---|
   | 10 s | a soft tone in your earbuds |
   | 30 s | a voice says "Gently lift your head", repeated every 2 minutes |
   | 60 s | a desktop notification |
   | when you look up again | a short "tink" |

4. **Taking the earbuds off** pauses monitoring. When you put them back on, headup calibrates again,
   because the earbuds sit slightly differently each time.

## Credits

Spoken prompts use the Piper "alba" voice (CC BY 4.0). See [`assets/voice/CREDITS.md`](assets/voice/CREDITS.md).

## For developers

Protocol research, design decisions and how-tos are in [`docs/`](docs/). Start with
`docs/RUNBOOK.md` (`nix develop` gives a shell with everything needed).
