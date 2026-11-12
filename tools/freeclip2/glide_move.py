"""Guided test: move the head while the FreeClip 2 is re-centring (a glide), and record it.

usage (inside `nix develop`, audio playing on this computer, headup not running):
    python3 -I tools/freeclip2/glide_move.py MAC OUT.csv [--trials N]

The stream's reference is the upright pose at the start. Each trial asks you to look down and hold
still; as soon as the readings start following the glide model (docs/devices/freeclip2/FINDINGS.md
"Glide model checked against every recording") it beeps, and you look straight back up and hold.
The CSV has one row per sample with its phase (`upright`, `down`, `move`, `after`); the beep starts
`move`. If the move landed before the glide's identity sample, the rebuilt pose during `after`
shows where the earbuds re-centred: ~0° (onto the glide's start pose) or ~the down angle (onto the
end pose). See FINDINGS "Head movement during a glide".
"""
import argparse, math, os, shutil, socket, struct, subprocess, sys, tempfile, threading, time, wave

sys.path.insert(0, os.path.dirname(__file__))
from headpose import build, euler, frames, tlv  # noqa: E402

GLIDE_LEN = 68  # samples from the held pose to the identity sample
FIT_TOL = 0.3  # degrees; the model's clean misfit is ~0.2°
CONFIRM = 3  # glide samples that must fit before beeping
MIN_ANGLE = 10.0  # only glides from at least this far (degrees) are clear enough to detect
STILL = 25  # samples within STILL_RANGE before a glide can start
STILL_RANGE = 1.0  # offline: catches 35 of 38 glides from >= 10°, no false alarms


def angle(q):
    return math.degrees(2 * math.acos(min(1.0, abs(q[0]))))


def between(a, b):
    return math.degrees(2 * math.acos(min(1.0, abs(sum(x * y for x, y in zip(a, b))))))


def toward_identity(q, u):
    """slerp(q, I, u): same axis, angle scaled by (1 - u)."""
    w = q[0]
    s = math.sqrt(max(0.0, 1 - w * w))
    if s < 1e-12:
        return (1.0, 0.0, 0.0, 0.0)
    th = 2 * math.acos(max(-1.0, min(1.0, w))) * (1 - u)
    k = math.sin(th / 2) / s
    return (math.cos(th / 2), q[1] * k, q[2] * k, q[3] * k)


def say(text):
    subprocess.run(["espeak-ng", "-s", "150", text], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def beep_file():
    """A 0.15 s 880 Hz beep as a WAV file."""
    path = os.path.join(tempfile.mkdtemp(), "beep.wav")
    rate = 44100
    n = int(0.15 * rate)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(
            struct.pack("<h", int(0.4 * 32767 * math.sin(2 * math.pi * 880 * i / rate))) for i in range(n)))
    return path


class GlideWatch:
    """Fires once a glide from a still pose of at least MIN_ANGLE has begun (CONFIRM fitting samples)."""

    def __init__(self):
        self.recent = []  # (seq, q) of the last STILL + CONFIRM samples
        self.armed = False

    def arm(self):
        self.recent, self.armed = [], True

    def sample(self, seq, q):
        if not self.armed:
            return False
        self.recent = (self.recent + [(seq, q)])[-(STILL + CONFIRM):]
        if len(self.recent) < STILL + CONFIRM:
            return False
        still, glide = self.recent[:STILL], self.recent[STILL:]
        p_seq, p = still[-1]
        if angle(p) < MIN_ANGLE or any(between(q2, p) > STILL_RANGE for _, q2 in still):
            return False
        fits = all(between(q2, toward_identity(p, (s - p_seq + 1) / (GLIDE_LEN + 1))) <= FIT_TOL
                   for s, q2 in glide)
        moved = between(glide[-1][1], p) > 1.0  # clearly away from holding still
        if fits and moved:
            self.armed = False
            return True
        return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("out")
    ap.add_argument("--trials", type=int, default=3)
    a = ap.parse_args()
    for tool in ("espeak-ng", "pw-play"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not on PATH; run inside `nix develop`")
    beep = beep_file()

    phase = {"name": "upright"}
    watch = GlideWatch()
    glide_seen = threading.Event()
    done = threading.Event()

    def prompter():
        say("Glide test. Sit upright and look straight ahead, and hold.")
        time.sleep(5)
        for trial in range(1, a.trials + 1):
            say(f"Trial {trial}. Look down, and hold very still. "
                "When you hear the beep, look straight ahead at once.")
            phase["name"] = "down"
            glide_seen.clear()
            watch.arm()
            print(f"\n>>> trial {trial}: down, waiting for a glide", flush=True)
            if not glide_seen.wait(timeout=30):
                print(">>> no glide within 30 s; skipping", flush=True)
                watch.armed = False
                say("No re-centre happened. Look straight ahead.")
                phase["name"] = "upright"
                time.sleep(5)
                continue
            time.sleep(3)  # the move happens; samples are labelled `move`
            phase["name"] = "after"
            say("Hold still, looking straight ahead.")
            print(">>> after: hold upright", flush=True)
            time.sleep(10)
            phase["name"] = "upright"
        say("Done. Thank you.")
        done.set()

    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((a.mac, 1))
    sock.settimeout(1)
    sock.send(build(43, 90, [(1, b"")]))  # as headup does at connect
    sock.send(build(43, 88, [(1, b"\x01")]))  # stream on: reference = the pose now (upright)
    t0, n = time.time(), 0
    threading.Thread(target=prompter, daemon=True).start()
    try:
        with open(a.out, "w") as csv:
            csv.write("t,seq,phase,w,x,y,z,roll_x,pitch_y,yaw_z\n")
            last_sample = time.time()
            for f in frames(sock):
                if done.is_set():
                    break
                if n == 0 and time.time() - last_sample > 3:
                    # A stream switch sent while this computer is idle may not take effect
                    # (FINDINGS "43/88 acknowledgement"): repeat it until samples arrive.
                    sock.send(build(43, 88, [(1, b"\x01")]))
                    last_sample = time.time()
                if f is None or (f[4], f[5]) != (43, 89):
                    continue
                p = tlv(f)
                seq = struct.unpack("<I", p[1])[0]
                q = struct.unpack("<4f", p[2])
                if watch.sample(seq, q):
                    subprocess.Popen(["pw-play", beep])
                    phase["name"] = "move"
                    print(f">>> glide detected at seq {seq}: beep", flush=True)
                    glide_seen.set()
                r, pi, ya = euler(*q)
                csv.write(f"{time.time() - t0:.4f},{seq},{phase['name']},{q[0]:.6f},{q[1]:.6f},{q[2]:.6f},"
                          f"{q[3]:.6f},{r:.2f},{pi:.2f},{ya:.2f}\n")
                n += 1
    finally:
        sock.send(build(43, 88, [(1, b"\x00")]))
        time.sleep(0.3)
        sock.close()
        print(f"samples={n} rate={n / max(1e-9, time.time() - t0):.1f}Hz -> {a.out}")


if __name__ == "__main__":
    main()
