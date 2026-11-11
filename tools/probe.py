"""Huawei MBB SPP probe.

usage: probe.py MAC [--listen SECONDS] CMD...
CMD is svc/cmd[:type=hex,type=hex...], e.g. 43/90:1= or 43/88:1=01
Each CMD is sent and all incoming frames are printed for --wait seconds.
"""
import argparse, socket, struct, sys, threading, time


def crc16_xmodem(data: bytes) -> bytes:
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc.to_bytes(2, "big")


def build(svc: int, cmd: int, params: list[tuple[int, bytes]]) -> bytes:
    body = bytes([svc, cmd]) + b"".join(bytes([t, len(v)]) + v for t, v in params)
    frame = b"Z" + (len(body) + 1).to_bytes(2, "big") + b"\x00" + body
    return frame + crc16_xmodem(frame)


def parse(frame: bytes) -> str:
    length = int.from_bytes(frame[1:3], "big")
    svc, cmd = frame[4], frame[5]
    pos, out = 6, []
    while pos < length + 3:
        t, l = frame[pos], frame[pos + 1]
        out.append(f"{t}={frame[pos + 2:pos + 2 + l].hex()}")
        pos += l + 2
    ok = crc16_xmodem(frame[:-2]) == frame[-2:]
    return f"{svc}/{cmd} {' '.join(out)}{'' if ok else ' [BAD CRC]'}"


def reader(sock, t0):
    buf = b""
    while True:
        try:
            data = sock.recv(4096)
        except OSError:
            return
        if not data:
            return
        buf += data
        while len(buf) >= 4:
            if buf[0] != 0x5A:
                print(f"  ?? junk {buf.hex()}", flush=True)
                buf = b""
                break
            n = int.from_bytes(buf[1:3], "big") + 5  # Z + len(2) + (len) + crc(2)
            if len(buf) < n:
                break
            frame, buf = buf[:n], buf[n:]
            print(f"  <- {time.time() - t0:7.3f} {parse(frame)}", flush=True)


def parse_cmd(s: str):
    head, _, rest = s.partition(":")
    svc, cmd = (int(x) for x in head.split("/"))
    params = []
    if rest:
        for kv in rest.split(","):
            t, _, v = kv.partition("=")
            params.append((int(t), bytes.fromhex(v)))
    return svc, cmd, params


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("cmds", nargs="*")
    ap.add_argument("--wait", type=float, default=1.5)
    ap.add_argument("--listen", type=float, default=0)
    ap.add_argument("--channel", type=int, default=1)
    a = ap.parse_args()
    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((a.mac, a.channel))
    t0 = time.time()
    threading.Thread(target=reader, args=(sock, t0), daemon=True).start()
    for c in a.cmds:
        svc, cmd, params = parse_cmd(c)
        frame = build(svc, cmd, params)
        print(f"-> {time.time() - t0:7.3f} {c}  [{frame.hex()}]", flush=True)
        sock.send(frame)
        time.sleep(a.wait)
    if a.listen:
        print(f"listening {a.listen}s ...", flush=True)
        time.sleep(a.listen)
    sock.close()


if __name__ == "__main__":
    main()
