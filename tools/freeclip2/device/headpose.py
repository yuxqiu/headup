"""Stream FreeClip 2 head-orientation quaternions (MBB 43/88 enable -> 43/89 reports).

usage: headpose.py MAC SECONDS [CSV]
Prints euler angles ~5x/s, optionally writes every sample to CSV, disables stream on exit.
"""
import math, os, struct, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from mbb import STREAM_OFF, STREAM_ON, connect, frames, tlv  # noqa: E402
from quat import euler  # noqa: E402


def main():
    mac, secs = sys.argv[1], float(sys.argv[2])
    csv = open(sys.argv[3], "w") if len(sys.argv) > 3 else None
    if csv:
        csv.write("t,seq,w,x,y,z,norm,roll_x,pitch_y,yaw_z\n")
    sock = connect(mac)
    sock.settimeout(1)
    sock.send(STREAM_ON)
    t0, last_print, n = time.time(), 0.0, 0
    try:
        for f in frames(sock):
            if time.time() - t0 >= secs:
                break
            if f is None or (f[4], f[5]) != (43, 89):
                continue
            p = tlv(f)
            seq = struct.unpack("<I", p[1])[0]
            w, x, y, z = struct.unpack("<4f", p[2])
            t = time.time() - t0
            r, pi, ya = euler((w, x, y, z))
            norm = math.sqrt(w * w + x * x + y * y + z * z)
            n += 1
            if csv:
                csv.write(f"{t:.4f},{seq},{w:.6f},{x:.6f},{y:.6f},{z:.6f},{norm:.5f},{r:.2f},{pi:.2f},{ya:.2f}\n")
            if t - last_print >= 0.2:
                last_print = t
                print(f"{t:6.2f}s seq={seq:6d} |q|={norm:.4f} roll(x)={r:7.1f} pitch(y)={pi:7.1f} yaw(z)={ya:7.1f}", flush=True)
            if t >= secs:
                break
    finally:
        sock.send(STREAM_OFF)
        time.sleep(0.3)
        sock.close()
        print(f"samples={n} rate={n / max(1e-9, time.time() - t0):.1f}Hz")


if __name__ == "__main__":
    main()
