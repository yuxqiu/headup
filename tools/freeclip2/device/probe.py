"""Huawei MBB SPP probe.

usage: probe.py MAC [--listen SECONDS] CMD...
CMD is svc/cmd[:type=hex,type=hex...], e.g. 43/90:1= or 43/88:1=01
Each CMD is sent and all incoming frames are printed for --wait seconds.
"""
import argparse, os, sys, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mbb import build, connect, frames, parse  # noqa: E402


def reader(sock, t0):
    try:
        for f in frames(sock):
            print(f"  <- {time.time() - t0:7.3f} {parse(f)}", flush=True)
        print("  connection closed by the earbuds", flush=True)
    except OSError:
        return  # main() closed the socket


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
    sock = connect(a.mac, a.channel)
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
