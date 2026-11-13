"""Quaternion helpers and the firmware's re-centre glide, shared by recordings/ and device/ scripts.

Quaternions are (w, x, y, z) tuples. The glide is the firmware's nlerp (docs/devices/freeclip2/FINDINGS.md "Where
the head-tracking computation is"): the k-th sample after the held pose is nlerp(I, held, (GLIDE_LEN - k) /
(GLIDE_LEN + 1)), and sample GLIDE_LEN is the exact identity. src/devices/freeclip2/rebuild.rs holds the same constant.
"""
import math

GLIDE_LEN = 68  # samples from the held pose to the identity sample (firmware 7.0.0.526: N = 69)
IDENTITY = (1.0, 0.0, 0.0, 0.0)


def mul(a, b):
    w1, x1, y1, z1 = a
    w2, x2, y2, z2 = b
    return (w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2, w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
            w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2, w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2)


def conj(q):
    return (q[0], -q[1], -q[2], -q[3])


def angle(q):
    """Rotation angle of a quaternion, in degrees. Uses atan2: acos of `w` loses precision near zero,
    where single-precision rounding alone would read as ~0.05°."""
    w, x, y, z = q
    return math.degrees(2 * math.atan2(math.sqrt(x * x + y * y + z * z), abs(w)))


def between(a, b):
    """Angle between two orientations, in degrees (normalised first)."""
    def unit(q):
        n = math.sqrt(sum(c * c for c in q))
        return tuple(c / n for c in q)
    return angle(mul(unit(a), conj(unit(b))))


def euler(q):
    """(roll about x, pitch about y, yaw about z) in degrees."""
    w, x, y, z = q
    return (math.degrees(math.atan2(2 * (w * x + y * z), 1 - 2 * (x * x + y * y))),
            math.degrees(math.asin(max(-1.0, min(1.0, 2 * (w * y - z * x))))),
            math.degrees(math.atan2(2 * (w * z + x * y), 1 - 2 * (y * y + z * z))))


def nlerp(q, t):
    """normalize(t * q + (1 - t) * I), with q negated first if w < 0 (as the firmware does)."""
    s = t if q[0] >= 0 else -t
    v = (s * q[0] + (1 - t), s * q[1], s * q[2], s * q[3])
    n = math.sqrt(sum(c * c for c in v))
    return tuple(c / n for c in v)


def glide_point(held, k):
    """The k-th glide sample after the held pose (k = GLIDE_LEN is the identity)."""
    return nlerp(held, (GLIDE_LEN - k) / (GLIDE_LEN + 1))
