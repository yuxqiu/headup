#!/usr/bin/env python3
"""Turn a `headup --raw-log` file into a test fixture: `t,seq,w,x,y,z`, one row per 43/89 sample.

usage: python3 -I tools/freeclip2/recordings/to_fixture.py RAW.jsonl OUT.csv

Keeps only what the rebuilder reads, so the fixture can be published:
- `t` is seconds since the first sample; the wall-clock time is dropped.
- Every frame other than a sample (wear state, 43/54 device addresses, replies) is dropped.
- The components are written as logged (f32 values in recent logs), so glides still match the firmware formula.
Checks: the log has samples, every sample has four finite components, and time never goes backwards.
"""
import json, math, sys


def main(src, dst):
    rows = []
    with open(src) as f:
        for n, line in enumerate(f, 1):
            r = json.loads(line)
            if 'seq' not in r:
                continue
            raw = r['raw']
            if len(raw) != 4 or not all(math.isfinite(c) for c in raw):
                sys.exit(f"{src}:{n}: malformed sample {r}")
            rows.append((r['t'], r['seq'], raw))
    if not rows:
        sys.exit(f"{src}: no samples")
    t0 = rows[0][0]
    for (ta, _, _), (tb, seq, _) in zip(rows, rows[1:]):
        if tb < ta:
            sys.exit(f"{src}: time goes backwards at seq {seq}")
    with open(dst, 'w') as out:
        out.write('t,seq,w,x,y,z\n')
        for t, seq, raw in rows:
            out.write(f"{t - t0:.4f},{seq}," + ','.join(repr(float(c)) for c in raw) + '\n')
    print(f"{dst}: {len(rows)} samples, {rows[-1][0] - t0:.1f} s")


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(*sys.argv[1:])
