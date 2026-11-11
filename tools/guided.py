"""Guided head-motion recording for FreeClip 2 with spoken prompts (espeak-ng).

usage (inside `nix develop`): python3 -I tools/guided.py MAC OUT.csv [--still SECONDS] [--hold]
--hold: long-hold schedule (look down 30 s, center 15 s, twice) to study device re-centring.
Every sample is labelled with the phase that was active when it arrived.
"""
import argparse, os, shutil, socket, struct, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(__file__))
from headpose import build, euler, frames, tlv  # noqa: E402

HOLD = 6.0  # seconds per phase (prompt takes ~1 s of it)
MOVES = [
    ("down", "Look down. Chin to chest. Hold."),
    ("center", "Back to center."),
    ("up", "Look up at the ceiling. Hold."),
    ("center", "Back to center."),
    ("turn_left", "Turn your head to the left. Hold."),
    ("center", "Back to center."),
    ("turn_right", "Turn your head to the right. Hold."),
    ("center", "Back to center."),
    ("tilt_left", "Tilt your left ear toward your left shoulder. Hold."),
    ("center", "Back to center."),
    ("tilt_right", "Tilt your right ear toward your right shoulder. Hold."),
    ("center", "Back to center."),
]


def say(text):
    subprocess.run(["espeak-ng", "-s", "150", text], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("out")
    ap.add_argument("--still", type=float, default=120)
    ap.add_argument("--hold", action="store_true")
    a = ap.parse_args()
    if shutil.which("espeak-ng") is None:
        sys.exit("espeak-ng not on PATH; run inside `nix develop`")

    schedule = [("upright", "Sit upright and look straight ahead. Recording starts now.", HOLD)]
    if a.hold:
        for _ in range(2):
            schedule += [("down", "Look down at your desk and hold it there.", 30.0),
                         ("center", "Back to straight ahead and hold.", 15.0)]
    else:
        schedule += [(name, text, HOLD) for name, text in MOVES]
        schedule += [("still", f"Now stay still, looking straight ahead, for {int(a.still)} seconds.", a.still)]

    say("Head motion test. Sit upright and look straight ahead.")
    time.sleep(2)

    phase = {"name": "upright"}
    done = threading.Event()

    def prompter():
        for name, text, dur in schedule:
            phase["name"] = name
            print(f"[{name}] {text}", flush=True)
            start = time.time()
            say(text)
            time.sleep(max(0.0, dur - (time.time() - start)))
        say("Done. Thank you.")
        done.set()

    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((a.mac, 1))
    sock.settimeout(1)
    sock.send(build(43, 88, [(1, b"\x01")]))  # reference pose = pose at this moment
    t0, n = time.time(), 0
    threading.Thread(target=prompter, daemon=True).start()
    try:
        with open(a.out, "w") as csv:
            csv.write("t,seq,phase,w,x,y,z,roll_x,pitch_y,yaw_z\n")
            for f in frames(sock):
                if done.is_set():
                    break
                if f is None or (f[4], f[5]) != (43, 89):
                    continue
                p = tlv(f)
                seq = struct.unpack("<I", p[1])[0]
                w, x, y, z = struct.unpack("<4f", p[2])
                r, pi, ya = euler(w, x, y, z)
                csv.write(f"{time.time() - t0:.4f},{seq},{phase['name']},{w:.6f},{x:.6f},{y:.6f},{z:.6f},{r:.2f},{pi:.2f},{ya:.2f}\n")
                n += 1
    finally:
        sock.send(build(43, 88, [(1, b"\x00")]))
        time.sleep(0.3)
        sock.close()
        print(f"samples={n} rate={n / max(1e-9, time.time() - t0):.1f}Hz -> {a.out}")


if __name__ == "__main__":
    main()
