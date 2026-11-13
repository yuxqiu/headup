"""Guided live test of the device behaviour headup relies on (skill freeclip2-firmware, FINDINGS "Grounding").

usage (inside `nix develop`, headup service stopped): python3 -I tools/freeclip2/device/grounding_test.py MAC SINK OUT.log
SINK is the earbuds' PipeWire sink, e.g. bluez_output.AA_BB_CC_DD_EE_01.1. Spoken prompts go to the default sink.
This computer streams silence to SINK throughout, except in the "audio gap" phase. 43/88 is sent only where a
phase says so, so each gap shows the earbuds' own behaviour. Logs one line per second (poses, seq range) and every
other frame (runner.py, the same format as single_bud_test.py).
"""
import os, subprocess, sys, tempfile, wave

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mbb  # noqa: E402
from runner import Phase, run  # noqa: E402

SETTLE = 3  # seconds of silence before the first phase, so this computer becomes the streaming device

# Actions: "on" sends 43/88 on (runner); "audio_off"/"audio_on" stop/start the silence stream.
SCHEDULE = [
    Phase("baseline", "Grounding test. Wear both earbuds and nod slowly until told otherwise.", 10, ((0, "on"),)),
    Phase("one_earbud", "Take off the right earbud, hold it in your hand, and keep nodding.", 16, ((10, "on"),)),
    Phase("in_case", "Put the right earbud in its case and close the lid. Keep nodding.", 18, ((12, "on"),)),
    Phase("rewear", "Take the right earbud out and put both earbuds on. Keep nodding.", 22, ((16, "on"),)),
    Phase("audio_gap", "Audio will now stop for twenty seconds. Keep nodding slowly.", 40,
          ((5, "audio_off"), (25, "audio_on"), (35, "on"))),
    Phase("phone_takeover", "Play something on your phone now for about ten seconds, then pause it. Keep nodding.",
          45, ((2, "audio_off"), (25, "audio_on"), (35, "on"))),
    Phase("still", "Hold your head completely still until the next prompt.", 18),
    Phase("move", "Now nod slowly again.", 8),
    Phase("done", "Done. Thank you.", 3),
]


def speak(text, step):
    """Show the prompt (terminal and desktop notification), then speak it."""
    print(f"\n>>> STEP {step}: {text}\n", file=sys.stderr, flush=True)
    subprocess.run(["notify-send", "-u", "critical", "-t", "20000", f"Grounding test, step {step}", text], check=True)
    mbb.say(text, speed=120)


def write_silence(path, seconds):
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(48000)
        w.writeframes(b"\0" * 4 * 48000 * seconds)


class Silence:
    """pw-play of a silent WAV to SINK. The file outlasts the whole test, so a running player never reaches its end."""

    def __init__(self, sink, wav):
        self.sink, self.wav, self.player = sink, wav, None

    def on(self):
        if self.player is None:
            self.player = subprocess.Popen(["pw-play", "--target", self.sink, self.wav])

    def off(self):
        if self.player is not None:
            self.player.terminate()
            self.player.wait()
            self.player = None

    def check(self):
        if self.player is not None and self.player.poll() is not None:
            raise SystemExit(f"pw-play exited ({self.player.returncode}): this computer no longer streams audio")


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    mac, sink, out = sys.argv[1:]
    mbb.require("espeak-ng", "notify-send", "pw-play")
    seconds = SETTLE + sum(p.seconds for p in SCHEDULE) + 60  # + 60 s for speech overrunning a phase
    with tempfile.TemporaryDirectory() as tmp:
        wav = os.path.join(tmp, "silence.wav")
        write_silence(wav, seconds)
        silence = Silence(sink, wav)
        try:
            run(mac, out, SCHEDULE, speak=speak, setup=[(0, "audio_on"), (SETTLE, "wear")],
                actions={"audio_on": silence.on, "audio_off": silence.off}, check=silence.check)
        finally:
            silence.off()


if __name__ == "__main__":
    main()
