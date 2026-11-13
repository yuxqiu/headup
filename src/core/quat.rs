//! Unit quaternion for head orientation.

use std::ops::Mul;

/// Head orientation as angles in degrees. Pitch > 0 is looking down, yaw > 0 is turned left, and
/// roll > 0 is tilted right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Euler {
    pub roll: f64,
    pub pitch: f64,
    pub yaw: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn conj(self) -> Quat {
        Quat {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn dot(self, b: Quat) -> f64 {
        self.w * b.w + self.x * b.x + self.y * b.y + self.z * b.z
    }

    /// Rotation angle between two orientations, in degrees. Uses `atan2` on the normalised
    /// difference rotation, which stays exact near zero. `acos` of the dot product would not: for
    /// single-precision input, rounding alone would read as ~0.05°.
    pub fn angle_to(self, b: Quat) -> f64 {
        let d = self.normalized() * b.normalized().conj();
        let v = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        (2.0 * v.atan2(d.w.abs())).to_degrees()
    }

    /// Mean of nearby orientations: the normalised sum, with each sign aligned to the first (q and
    /// -q are the same rotation). Panics on an empty slice.
    pub fn mean(samples: &[Quat]) -> Quat {
        let first = *samples.first().expect("mean of zero orientations");
        let (mut w, mut x, mut y, mut z) = (0.0, 0.0, 0.0, 0.0);
        for q in samples {
            let s = if first.dot(*q) < 0.0 { -1.0 } else { 1.0 };
            (w, x, y, z) = (w + s * q.w, x + s * q.x, y + s * q.y, z + s * q.z);
        }
        Quat { w, x, y, z }.normalized()
    }

    /// The unit quaternion in the direction of `self`.
    pub fn normalized(self) -> Quat {
        let n = self.dot(self).sqrt();
        Quat {
            w: self.w / n,
            x: self.x / n,
            y: self.y / n,
            z: self.z / n,
        }
    }

    /// Euler angles (x-y-z: roll, pitch, yaw) in degrees.
    pub fn euler(self) -> Euler {
        let Quat { w, x, y, z } = self;
        let roll = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
        let pitch = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
        let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
        Euler {
            roll: roll.to_degrees(),
            pitch: pitch.to_degrees(),
            yaw: yaw.to_degrees(),
        }
    }
}

impl Mul for Quat {
    type Output = Quat;

    fn mul(self, b: Quat) -> Quat {
        let a = self;
        Quat {
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        }
    }
}
