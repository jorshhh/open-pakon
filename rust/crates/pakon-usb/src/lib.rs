//! Pakon transport layer.
//!
//! Enumeration, FX2 firmware download and bulk/control I/O on `nusb`. Knows
//! nothing about Pakon frames (that is `pakon-proto`), as `pakon_usb` in the
//! C driver.

pub mod device;
pub mod fw;
pub mod hex;

pub use device::{DevClass, Device, UsbError};

/// `PAKON_DEBUG` (0..4): 1 info, 2 details, 4 hexdumps of every transfer.
pub fn debug_level() -> u8 {
    static LEVEL: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| std::env::var("PAKON_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
}
