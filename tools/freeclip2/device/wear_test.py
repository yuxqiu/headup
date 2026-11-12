"""Guided wear test: does taking FreeClip 2 off disconnect Bluetooth, and what does MBB report?

usage (inside `nix develop`): python3 -I tools/freeclip2/device/wear_test.py MAC OUT.log
Speaks prompts, logs every MBB frame with the active phase, and records connection loss.
"""
import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mbb  # noqa: E402
from runner import Phase, run  # noqa: E402

SCHEDULE = [
    Phase("both_on", "Wear test. Keep both earbuds on.", 6),
    Phase("left_off", "Take off the left earbud and hold it in your hand.", 12),
    Phase("left_back", "Put the left earbud back on.", 10),
    Phase("right_off", "Take off the right earbud and hold it in your hand.", 12),
    Phase("right_back", "Put the right earbud back on.", 10),
    Phase("both_off", "Take both earbuds off, put them on the desk for fifteen seconds, then put them back on.", 35),
    Phase("done", "Done. Thank you.", 3),
]


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    mbb.require("espeak-ng")
    run(sys.argv[1], sys.argv[2], SCHEDULE, speak=lambda text, step: mbb.say(text), count_poses=False, stamp="8.3f")


if __name__ == "__main__":
    main()
