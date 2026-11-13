//! What headup needs from the computer's Bluetooth stack: the connected devices, this computer's
//! adapter address, and an RFCOMM connection to a device.
//!
//! Each operating system has its own implementation, chosen when headup is compiled. Each one
//! provides [`Host`] (the operations) and [`RfcommStream`] (a connection, read and written with
//! tokio). Only Linux (BlueZ) has one so far.

use anyhow::{Context, Result, bail};
use std::fmt;
use std::str::FromStr;

#[cfg(target_os = "linux")]
mod bluez;
#[cfg(target_os = "linux")]
pub use bluez::{Host, RfcommStream};

#[cfg(not(target_os = "linux"))]
compile_error!("headup talks to Bluetooth only through BlueZ so far, so it builds only on Linux");

/// A Bluetooth device address, most significant byte first: `AA:BB:CC:DD:EE:FF` is
/// `[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Address(pub [u8; 6]);

/// A device the computer is connected to.
#[derive(Debug, Clone)]
pub struct Device {
    pub address: Address,
    pub name: String,
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Address {
    type Err = anyhow::Error;

    /// Six two-digit hex bytes separated by colons, in either case.
    fn from_str(s: &str) -> Result<Address> {
        let parts: Vec<&str> = s.split(':').collect();
        let byte = |p: &&str| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit());
        if parts.len() != 6 || !parts.iter().all(byte) {
            bail!("{s:?} is not a Bluetooth address like AA:BB:CC:DD:EE:FF");
        }
        let mut bytes = [0; 6];
        for (b, p) in bytes.iter_mut().zip(parts) {
            *b = u8::from_str_radix(p, 16).context("hex digits checked above")?;
        }
        Ok(Address(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_round_trips_through_text() {
        let a: Address = "c0:DA:5e:F1:A0:A6".parse().unwrap();
        assert_eq!(a.0, [0xC0, 0xDA, 0x5E, 0xF1, 0xA0, 0xA6]);
        assert_eq!(a.to_string(), "C0:DA:5E:F1:A0:A6");
    }

    #[test]
    fn malformed_addresses_are_errors() {
        for s in [
            "",
            "C0:DA:5E:F1:A0",
            "C0:DA:5E:F1:A0:A6:00",
            "C0-DA-5E-F1-A0-A6",
            "C0:DA:5E:F1:A0:G6",
            "C0:DA:5E:F1:A0:+6",
        ] {
            assert!(s.parse::<Address>().is_err(), "{s:?}");
        }
    }
}
