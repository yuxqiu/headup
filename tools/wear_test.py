"""Guided wear test: does taking FreeClip 2 off disconnect Bluetooth, and what does MBB report?

usage (inside `nix develop`): python3 -I tools/wear_test.py MAC OUT.log
Speaks prompts, logs every MBB frame with the active phase, and records connection loss.
"""
import os, shutil, socket, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from probe import parse  # noqa: E402

SCHEDULE = [
    ("both_on", "Wear test. Keep both earbuds on.", 6),
    ("left_off", "Take off the left earbud and hold it in your hand.", 12),
    ("left_back", "Put the left earbud back on.", 10),
    ("right_off", "Take off the right earbud and hold it in your hand.", 12),
    ("right_back", "Put the right earbud back on.", 10),
    ("both_off", "Take both earbuds off, put them on the desk for fifteen seconds, then put them back on.", 35),
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
    log = open(out, "w")

    def emit(line):
        line = f"{time.time() - t0:8.3f} [{phase['name']}] {line}"
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
                emit(f"<- {parse(frame)}")
            if buf and buf[0] != 0x5A:
                emit(f"?? desync {buf.hex()}")
                buf = b""

    threading.Thread(target=reader, daemon=True).start()
    for name, text, dur in SCHEDULE:
        phase["name"] = name
        emit(f"PROMPT {text}")
        start = time.time()
        try:
            say(text)
        except subprocess.CalledProcessError as e:
            emit(f"speech failed: {e}")
        time.sleep(max(0.0, dur - (time.time() - start)))
    sock.close()


if __name__ == "__main__":
    main()
