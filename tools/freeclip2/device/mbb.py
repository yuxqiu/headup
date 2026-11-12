"""Huawei MBB over RFCOMM, shared by the device scripts: framing, connection, spoken prompts.

Frame: 'Z', u16 BE length (of what follows up to the CRC), 0x00, service, command, TLVs (type, length, value),
CRC-16/XMODEM (BE) over everything before it. docs/devices/freeclip2/FINDINGS.md "Transport".
"""
import shutil, socket, subprocess, sys


class FrameError(Exception):
    """The byte stream is not a valid MBB frame sequence (desync or bad CRC)."""


def crc16(data: bytes) -> bytes:
    """CRC-16/XMODEM, big-endian."""
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
    return frame + crc16(frame)


STREAM_ON = build(43, 88, [(1, b"\x01")])  # head-pose stream switch on
STREAM_OFF = build(43, 88, [(1, b"\x00")])
WEAR_QUERY = build(43, 37, [(1, b""), (2, b"")])  # current wear state


def tlvs(frame: bytes) -> list[tuple[int, bytes]]:
    length = int.from_bytes(frame[1:3], "big")
    pos, out = 6, []
    while pos < length + 3:
        t, n = frame[pos], frame[pos + 1]
        out.append((t, frame[pos + 2:pos + 2 + n]))
        pos += n + 2
    return out


def tlv(frame: bytes) -> dict[int, bytes]:
    return dict(tlvs(frame))


def parse(frame: bytes) -> str:
    """'svc/cmd type=hex ...', the format of every log in data/."""
    return f"{frame[4]}/{frame[5]} " + " ".join(f"{t}={v.hex()}" for t, v in tlvs(frame))


def frames(sock):
    """Yield CRC-checked frames from `sock` until the peer closes it. Raises FrameError on desync or a bad CRC.
    If the socket has a timeout, yields None when it expires (the earbuds pause the stream while the head is still)."""
    buf = b""
    while True:
        try:
            data = sock.recv(4096)
        except TimeoutError:
            yield None
            continue
        if not data:
            if buf:
                raise FrameError(f"connection closed inside a frame: {buf.hex()}")
            return
        buf += data
        while len(buf) >= 3:
            if buf[0] != 0x5A:
                raise FrameError(f"desync: {buf[:16].hex()}")
            n = int.from_bytes(buf[1:3], "big") + 5  # Z + length(2) + (length) + CRC(2)
            if len(buf) < n:
                break
            f, buf = buf[:n], buf[n:]
            if crc16(f[:-2]) != f[-2:]:
                raise FrameError(f"bad CRC: {f.hex()}")
            yield f


def connect(mac: str, channel: int = 1) -> socket.socket:
    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((mac, channel))
    return sock


def require(*tools):
    """Exit unless every tool is on PATH."""
    for tool in tools:
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not on PATH; run inside `nix develop`")


def say(text, speed=150):
    """Speak `text` with espeak-ng (blocks until done; fails loudly)."""
    subprocess.run(["espeak-ng", "-s", str(speed), text], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
