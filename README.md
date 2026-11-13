# headup

Gentle reminders to lift your head, using the motion sensors already in your earbuds.

headup reads head tracking from supported earbuds on Linux. When you look down for too long, for
example at a phone or a low laptop screen, it reminds you through your earbuds. It needs no camera
and no cloud; everything runs locally.

The earbuds only share head tracking while your computer is playing audio to them, so headup watches
your posture while you listen to something on the computer.

## Supported earbuds

| Earbuds | Status | Head tracking | Pauses when taken off | Tested firmware |
|---|---|---|---|---|
| HUAWEI FreeClip 2 | Supported | While the computer plays audio, both earbuds worn | Yes | HarmonyOS 7.0.0.526 |

## Requirements

- Linux with BlueZ (the standard Bluetooth stack) and PipeWire (with its PulseAudio server) or PulseAudio.
- Earbuds that are paired and connected to the computer.

## Install (Nix + Home Manager)

```nix
# flake.nix
inputs.headup.url = "github:yuxqiu/headup";
inputs.headup.inputs.nixpkgs.follows = "nixpkgs";

# home-manager configuration
imports = [ inputs.headup.homeManagerModules.default ];
services.headup = {
  enable = true;
  # device = "AA:BB:CC:DD:EE:01";   # optional; default: the first connected supported earbuds
  # logFile = "${config.xdg.stateHome}/headup/log.jsonl";   # optional record of angles and reminders
};
```

This runs headup as a systemd user service (`headup.service`) with your graphical session. Check on
it with `systemctl --user status headup` and `journalctl --user -u headup`.

Without Home Manager: `nix profile install github:yuxqiu/headup`, then run `headup`. To try it
once: `nix run github:yuxqiu/headup`.

### Without Nix

Download `headup-v<version>-<arch>-linux.tar.gz` from
[Releases](https://github.com/yuxqiu/headup/releases) (x86_64 and aarch64; glibc 2.35 or newer),
check it against its `.sha256`, and put `headup` on your `PATH`. It needs the D-Bus and ALSA libraries
(`libdbus-1`, `libasound2`), which most desktop distributions already include.

## Run

```sh
headup
```

headup finds connected supported earbuds by itself and keeps running. If no supported earbuds are
connected yet, or the connection drops (for example when you put them back in the case), it waits and
reconnects. Stop it with Ctrl-C.

If several supported earbuds are connected, pass the address of the ones to use:

```sh
bluetoothctl devices Connected
# Device AA:BB:CC:DD:EE:01 HUAWEI FreeClip 2
headup AA:BB:CC:DD:EE:01
```

To keep a record of your head angle and reminders, add `--log FILE` (JSON lines).

## What to expect

1. **Calibration.** Once audio is playing, a voice asks you to sit upright and look straight ahead. Hold still for about
   3 seconds. If you move, it asks again. If it hears nothing from the earbuds, it asks you to move your head a
   little and then hold still.
2. **Monitoring.** Looking down counts. Looking up, or turning your head to another screen, does not.
   Short glances down are fine.
3. **Reminders.** If you keep looking down (more than about 20°):

   | After | Reminder |
   |---|---|
   | 10 s | a soft tone in your earbuds |
   | 30 s | a voice says "Gently lift your head", repeated every 2 minutes |
   | 60 s | a desktop notification |
   | when you look up again | a short "tink" |

4. **Taking an earbud off** pauses monitoring, because head tracking needs both earbuds. When both are
   back on, headup calibrates again, because the earbuds sit slightly differently each time.
5. **Pausing your audio**, or playing something on another device, pauses monitoring too: your head
   may move while headup cannot see it. When you play something on this computer again, headup asks
   you to sit upright once more. Skipping a track doesn't interrupt.

## License

MIT, see [LICENSE](LICENSE).

## Credits

Spoken prompts use the Piper "alba" voice (CC BY 4.0). See [`assets/voice/CREDITS.md`](assets/voice/CREDITS.md).

## For developers

Protocol research, design decisions and how-tos are in [`docs/`](docs/). Start with
`docs/RUNBOOK.md` (`nix develop` gives a shell with everything needed).
