"""Stream FreeClip 2 head-orientation quaternions (MBB 43/88 enable -> 43/89 reports).

usage: headpose.py MAC SECONDS [CSV]
Prints euler angles ~5x/s, optionally writes every sample to CSV, disables stream on exit.
"""
import math, socket, struct, sys, time

sys.path.insert(0, __import__("os").path.dirname(__file__))
from probe import build, crc16_xmodem  # noqa: E402


def frames(sock):
    buf = b""
    while True:
        try:
            data = sock.recv(4096)
        except TimeoutError:  # device pauses the stream while the head is still
            yield None
            continue
        if not data:
            return
        buf += data
        while len(buf) >= 6:
            if buf[0] != 0x5A:
                raise RuntimeError(f"desync: {buf[:16].hex()}")
            n = int.from_bytes(buf[1:3], "big") + 5
            if len(buf) < n:
                break
            f, buf = buf[:n], buf[n:]
            if crc16_xmodem(f[:-2]) != f[-2:]:
                raise RuntimeError(f"bad crc: {f.hex()}")
            yield f


def tlv(f):
    length = int.from_bytes(f[1:3], "big")
    pos, out = 6, {}
    while pos < length + 3:
        t, l = f[pos], f[pos + 1]
        out[t] = f[pos + 2:pos + 2 + l]
        pos += l + 2
    return out


def euler(w, x, y, z):
    roll = math.degrees(math.atan2(2 * (w * x + y * z), 1 - 2 * (x * x + y * y)))
    pitch = math.degrees(math.asin(max(-1.0, min(1.0, 2 * (w * y - z * x)))))
    yaw = math.degrees(math.atan2(2 * (w * z + x * y), 1 - 2 * (y * y + z * z)))
    return roll, pitch, yaw


def main():
    mac, secs = sys.argv[1], float(sys.argv[2])
    csv = open(sys.argv[3], "w") if len(sys.argv) > 3 else None
    if csv:
        csv.write("t,seq,w,x,y,z,norm,roll_x,pitch_y,yaw_z\n")
    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((mac, 1))
    sock.settimeout(1)
    sock.send(build(43, 88, [(1, b"\x01")]))
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
            r, pi, ya = euler(w, x, y, z)
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
        sock.send(build(43, 88, [(1, b"\x00")]))
        time.sleep(0.3)
        sock.close()
        print(f"samples={n} rate={n / max(1e-9, time.time() - t0):.1f}Hz")


if __name__ == "__main__":
    main()
