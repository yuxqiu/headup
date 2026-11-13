#!/usr/bin/env python3
"""Check recorded re-centres against the firmware's glide and stillness rules (FINDINGS "Where the head-tracking
computation is"; skill freeclip2-firmware).

usage: python3 -I tools/freeclip2/recordings/glide_check.py RAW.jsonl...   (headup --raw-log files)

For each re-centre (an exact identity sample after others) it reports:
- glide length: samples from the held pose to the identity sample (7.0.0.526: 68);
- fit of the firmware formula, nlerp(I, held, (68 - k) / 69) for the k-th sample (526: max 0.0002 deg, single-precision rounding);
- still run: samples before the held pose within 10 deg on every axis of the run's first sample (526: > 208).
"""
import json, os, statistics, sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from quat import GLIDE_LEN, IDENTITY, angle, conj, euler, glide_point, mul  # noqa: E402

RAMP_STEP = 0.05  # deg; walking back, a sample is on the ramp if it is this much farther from I (arbitrary)
MIN_HELD = 8  # deg; nearer held poses give glides too small to fit meaningfully (arbitrary)
CUT_SHORT = 3  # deg; a glide off the formula by more than this was cut short by movement (526 clean fits: < 0.001)
STILL_BOX = 10  # deg per Euler axis around the run's first sample; what counts as still (arbitrary)


def samples(path):
    out = []
    for line in open(path):
        j = json.loads(line)
        if "seq" in j:
            out.append((j["seq"], tuple(j["raw"])))
    return out


def main(paths):
    lengths, misfits, still = [], [], []
    for path in paths:
        s = samples(path)
        for i, (seq, q) in enumerate(s):
            if q != IDENTITY or i < GLIDE_LEN or s[i - 1][1] == q:
                continue
            k = i - 1  # walk back over the ramp: angles rise toward the held pose
            while k > 0 and s[k][0] == s[k + 1][0] - 1 and angle(s[k - 1][1]) > angle(s[k][1]) + RAMP_STEP:
                k -= 1
            lengths.append(i - k)
            held = i - GLIDE_LEN
            if s[i][0] - s[held][0] != GLIDE_LEN or angle(s[held][1]) < MIN_HELD:
                continue
            fit = [angle(mul(conj(glide_point(s[held][1], j)), s[held + j][1])) for j in range(1, GLIDE_LEN)]
            if max(fit) > CUT_SHORT:
                continue
            misfits.append(max(fit))
            j = held
            while j > 0 and s[j - 1][0] == s[j][0] - 1:
                anchor = s[j - 1][1]
                if not all(all(abs(a) <= STILL_BOX for a in euler(mul(conj(anchor), s[m][1]))) for m in range(j - 1, held + 1)):
                    break
                j -= 1
            if j > 0:
                still.append(held - j)
    if not lengths:
        sys.exit("no re-centres found")
    common = statistics.mode(lengths)
    print(f"re-centres: {len(lengths)}; glide length mode {common} ({lengths.count(common)} of {len(lengths)})")
    if not misfits:
        sys.exit("clean glides: 0; no re-centre fits the firmware formula (did the glide change?)")
    print(f"clean glides: {len(misfits)}; firmware formula misfit median {statistics.median(misfits):.3f} deg, "
          f"max {max(misfits):.5f} deg")
    if not still:
        sys.exit("still run before the glide: none measured")
    print(f"still run before the glide: median {statistics.median(still)} samples "
          f"(firmware 7.0.0.526 threshold: 208)")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
