"""Rebuild orientation from the re-centring 43/89 stream: on identity sample, reference = last stationary absolute pose.
usage: rebuild.py CSV [STILL_DEG WIN_S]"""
import csv, math, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from headpose import euler
from itertools import groupby


def qmul(a, b):
    aw, ax, ay, az = a; bw, bx, by, bz = b
    return (aw*bw - ax*bx - ay*by - az*bz, aw*bx + ax*bw + ay*bz - az*by,
            aw*by - ax*bz + ay*bw + az*bx, aw*bz + ax*by - ay*bx + az*bw)

def ang(a, b):
    return math.degrees(2 * math.acos(min(1.0, abs(sum(x * y for x, y in zip(a, b))))))

def rebuild(ts, qs, still_deg=3.0, win=0.5):
    ref = last = still = (1.0, 0.0, 0.0, 0.0); prev = None; hist = []; out = []
    for t, q in zip(ts, qs):
        if q == (1.0, 0.0, 0.0, 0.0) and prev not in (None, q):
            ref = still
        last = qmul(ref, q); prev = q; out.append(last)
        hist.append((t, last)); hist = [h for h in hist if t - h[0] <= win]
        if t - hist[0][0] >= 0.8 * win and max(ang(h[1], last) for h in hist) < still_deg:
            still = last
    return out

if __name__ == "__main__":
    rows = list(csv.DictReader(open(sys.argv[1])))
    args = [float(x) for x in sys.argv[2:4]]
    out = rebuild([float(r["t"]) for r in rows], [tuple(float(r[k]) for k in "wxyz") for r in rows], *args)
    for ph, g in groupby(range(len(rows)), key=lambda j: rows[j]["phase"]):
        e = [euler(*out[j]) for j in list(g)[-60:]]
        m = lambda k: sorted(v[k] for v in e)[len(e) // 2]
        print(f"{ph:10s} pitch={m(1):6.1f} yaw={m(2):6.1f} roll={m(0):6.1f}")
