//! Driving the scanner (ports `pakon_cmd.c` and, step by step,
//! `tools/pakon_replay.c`). Everything goes through [`link::Link`], so the
//! rules are tested against a simulated scanner; [`pakon_usb::Device`] is the
//! real one.
//!
//! Safety (pakon-reference `per-unit-data-and-safety.md`): no frame of type
//! 0, nothing to the bootloader addresses except the presence probe, no
//! vendor request 0xA2 (refused by the transport), no invented commands.

pub mod calib;
pub mod cmd;
pub mod link;
pub mod replay;
pub mod scan;
pub mod session;
pub mod transport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    Timeout,
    /// A malformed or unexpected reply.
    Proto(String),
    /// A reply status or flags the rules treat as an error.
    Status(u8),
    /// Refused before sending (safety rules, bad arguments).
    Param(String),
    NoDevice,
    Usb(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Timeout => write!(f, "timeout"),
            ScanError::Proto(s) => write!(f, "protocol: {s}"),
            ScanError::Status(s) => write!(f, "device status {s:#04x}"),
            ScanError::Param(s) => write!(f, "refused: {s}"),
            ScanError::NoDevice => write!(f, "no scanner"),
            ScanError::Usb(s) => write!(f, "USB: {s}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<pakon_usb::UsbError> for ScanError {
    fn from(e: pakon_usb::UsbError) -> Self {
        match e {
            pakon_usb::UsbError::Timeout => ScanError::Timeout,
            pakon_usb::UsbError::NoDevice => ScanError::NoDevice,
            pakon_usb::UsbError::Refused(s) => ScanError::Param(s),
            e => ScanError::Usb(e.to_string()),
        }
    }
}

/// Log at `PAKON_DEBUG` >= `level` (0 errors, 1 warnings/info, 2 details).
pub(crate) fn log(level: u8, msg: &str) {
    if level == 0 || pakon_usb::debug_level() >= level {
        eprintln!("{msg}");
    }
}
