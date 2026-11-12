"""Guided test: does FreeClip 2 stream head pose (43/89) with only one earbud worn?

usage (inside `nix develop`): python3 -I tools/freeclip2/device/single_bud_test.py MAC OUT.log
Enables the pose stream once at the start and never again, so any gap shows the device's own behaviour.
Logs a per-second summary: pose frames received, first/last seq, and any other frames (wear state etc.).
"""
import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mbb  # noqa: E402
from runner import Phase, run  # noqa: E402

SCHEDULE = [
    Phase("both_on", "Single earbud test. Keep both earbuds on and nod slowly.", 10),
    Phase("left_only", "Take off the right earbud. Keep the left one on, and keep nodding slowly.", 15),
    Phase("right_only", "Put the right earbud on, take off the left one, and keep nodding slowly.", 15),
    Phase("both_off", "Take both earbuds off and put them on the desk.", 12),
    Phase("one_back", "Put on only the left earbud and nod slowly.", 20),
    Phase("done", "Done. Thank you.", 3),
]


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    mbb.require("espeak-ng")
    run(sys.argv[1], sys.argv[2], SCHEDULE, speak=lambda text, step: mbb.say(text),
        setup=[(0, "wear"), (0, "on")])


if __name__ == "__main__":
    main()
