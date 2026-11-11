//! Unit quaternion for head orientation.

use std::ops::Mul;

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

    /// Rotation angle between two orientations, degrees.
    pub fn angle_to(self, b: Quat) -> f64 {
        let d = (self.w * b.w + self.x * b.x + self.y * b.y + self.z * b.z)
            .abs()
            .min(1.0);
        (2.0 * d.acos()).to_degrees()
    }

    /// (roll about x, pitch about y, yaw about z) in degrees, in the framework convention:
    /// pitch > 0 = looking down, yaw > 0 = turned left, roll > 0 = tilted right.
    pub fn euler(self) -> (f64, f64, f64) {
        let Quat { w, x, y, z } = self;
        let roll = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
        let pitch = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
        let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
        (roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees())
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
