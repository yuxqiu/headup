"""Guided test: does FreeClip 2 stream head pose (43/89) with only one earbud worn?

usage (inside `nix develop`): python3 -I tools/freeclip2/single_bud_test.py MAC OUT.log
Enables the pose stream once at the start and never again, so any gap shows the device's own behaviour.
Logs a per-second summary: pose frames received, first/last seq, and any other frames (wear state etc.).
"""
import os, shutil, socket, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from probe import build, parse  # noqa: E402

SCHEDULE = [
    ("both_on", "Single earbud test. Keep both earbuds on and nod slowly.", 10),
    ("left_only", "Take off the right earbud. Keep the left one on, and keep nodding slowly.", 15),
    ("right_only", "Put the right earbud on, take off the left one, and keep nodding slowly.", 15),
    ("both_off", "Take both earbuds off and put them on the desk.", 12),
    ("one_back", "Put on only the left earbud and nod slowly.", 20),
    ("done", "Done. Thank you.", 3),
]


def say(text):
    subprocess.run(["espeak-ng", "-s", "150", text], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    mac, out = sys.argv[1], sys.argv[2]
    if shutil.which("espeak-ng") is None:
        sys.exit("espeak-ng not on PATH; run inside `nix develop`")
    sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
    sock.connect((mac, 1))
    t0 = time.time()
    phase = {"name": "start"}
    stats = {"poses": 0, "seq": []}
    lock = threading.Lock()
    log = open(out, "w")

    def emit(line):
        line = f"{time.time() - t0:7.2f} [{phase['name']}] {line}"
        print(line, flush=True)
        log.write(line + "\n")
        log.flush()

    def reader():
        buf = b""
        while True:
            try:
                data = sock.recv(4096)
            except OSError as e:
                emit(f"CONNECTION ERROR: {e}")
                return
            if not data:
                emit("CONNECTION CLOSED by earbuds")
                return
            buf += data
            while len(buf) >= 6 and buf[0] == 0x5A:
                n = int.from_bytes(buf[1:3], "big") + 5
                if len(buf) < n:
                    break
                frame, buf = buf[:n], buf[n:]
                if (frame[4], frame[5]) == (43, 89):
                    seq = int.from_bytes(frame[8:12], "little")  # TLV1 value: type, len, 4 bytes
                    with lock:
                        stats["poses"] += 1
                        stats["seq"].append(seq)
                else:
                    emit(f"<- {parse(frame)}")
            if buf and buf[0] != 0x5A:
                emit(f"?? desync {buf.hex()}")
                buf = b""

    def summary():
        while True:
            time.sleep(1)
            with lock:
                n, seqs = stats["poses"], stats["seq"]
                stats["poses"], stats["seq"] = 0, []
            emit(f"poses/s={n:3d}" + (f" seq {seqs[0]}..{seqs[-1]}" if seqs else ""))

    threading.Thread(target=reader, daemon=True).start()
    sock.send(build(43, 37, [(1, b""), (2, b"")]))  # current wear state
    sock.send(build(43, 88, [(1, b"\x01")]))  # pose stream on (once)
    threading.Thread(target=summary, daemon=True).start()
    try:
        for name, text, dur in SCHEDULE:
            phase["name"] = name
            emit(f"PROMPT {text}")
            start = time.time()
            try:
                say(text)
            except subprocess.CalledProcessError as e:
                emit(f"speech failed: {e}")
            time.sleep(max(0.0, dur - (time.time() - start)))
    finally:
        sock.send(build(43, 88, [(1, b"\x00")]))
        time.sleep(0.3)
        sock.close()


if __name__ == "__main__":
    main()
