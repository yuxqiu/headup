"""Run a guided phase test: prompts on a schedule, scripted actions, and a log of every MBB frame.

Shared by grounding_test.py, single_bud_test.py and wear_test.py. Log line: "<seconds since connect> [<phase>] <text>",
where text is "PROMPT ...", "ACTION ...", "<- svc/cmd t=hex ..." (mbb.parse) or, when poses are counted, a
per-second "poses/s=N seq A..B" summary of the 43/89 samples instead of one line each. The test stops with a non-zero
exit when the link to the earbuds drops or the frame stream is invalid.
"""
import threading, time
from dataclasses import dataclass

import mbb


@dataclass(frozen=True)
class Phase:
    name: str
    prompt: str
    seconds: float
    actions: tuple[tuple[float, str], ...] = ()  # (seconds from the phase start, action name)


class Log:
    def __init__(self, path, stamp):
        self.file, self.stamp, self.t0, self.phase = open(path, "w"), stamp, time.time(), "start"

    def emit(self, text):
        line = f"{time.time() - self.t0:{self.stamp}} [{self.phase}] {text}"
        print(line, flush=True)
        self.file.write(line + "\n")
        self.file.flush()


def run(mac, out, phases, *, speak, setup=(), actions=None, count_poses=True, stamp="7.2f", check=lambda: None):
    """Connect to `mac`, run `setup` [(seconds, action)] silently, then each Phase in turn: log and `speak(prompt,
    step)`, run its actions at their offsets (logged as ACTION), and wait out its duration. Built-in actions: "on"
    (43/88 stream on) and "wear" (43/37 wear-state query); `actions` adds more by name. `check()` runs at every phase
    start and after every action, and raises if a precondition broke."""
    actions = dict(actions or {})
    names = {"on", "wear"} | actions.keys()
    unknown = {a for _, a in setup} | {a for p in phases for _, a in p.actions}
    if unknown - names:
        raise SystemExit(f"unknown actions {sorted(unknown - names)}; known: {sorted(names)}")

    sock = mbb.connect(mac)
    log = Log(out, stamp)
    lost, stopping = threading.Event(), threading.Event()
    lock, seqs = threading.Lock(), []
    streaming = False

    def send(frame):
        nonlocal streaming
        sock.send(frame)
        streaming |= frame == mbb.STREAM_ON

    actions |= {"on": lambda: send(mbb.STREAM_ON), "wear": lambda: send(mbb.WEAR_QUERY)}

    def reader():
        try:
            for f in mbb.frames(sock):
                if count_poses and (f[4], f[5]) == (43, 89):
                    with lock:
                        seqs.append(int.from_bytes(mbb.tlv(f)[1], "little"))
                else:
                    log.emit(f"<- {mbb.parse(f)}")
            log.emit("CONNECTION CLOSED by earbuds")
        except OSError as e:
            if stopping.is_set():
                return
            log.emit(f"CONNECTION ERROR: {e}")
        except mbb.FrameError as e:
            log.emit(f"FRAME ERROR: {e}")
        lost.set()

    def summary():
        while not stopping.wait(1):
            with lock:
                got = seqs[:]
                seqs.clear()
            log.emit(f"poses/s={len(got):3d}" + (f" seq {got[0]}..{got[-1]}" if got else ""))

    def wait_until(t):
        if lost.wait(max(0.0, t - time.time())):
            raise SystemExit("link to the earbuds lost; test aborted (see the log)")

    threading.Thread(target=reader, daemon=True).start()
    try:
        start = time.time()
        for off, a in setup:
            wait_until(start + off)
            actions[a]()
        if count_poses:
            threading.Thread(target=summary, daemon=True).start()
        for step, phase in enumerate(phases, 1):
            log.phase = phase.name
            check()
            log.emit(f"PROMPT {phase.prompt}")
            start = time.time()
            speak(phase.prompt, step)
            for off, a in phase.actions:
                wait_until(start + off)
                log.emit(f"ACTION {a}")
                actions[a]()
                check()
            wait_until(start + phase.seconds)
    finally:
        stopping.set()
        if streaming and not lost.is_set():
            sock.send(mbb.STREAM_OFF)
            time.sleep(0.3)
        sock.close()
