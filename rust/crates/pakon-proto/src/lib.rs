//! Pakon F-135 / F-135+ protocol layer.
//!
//! Pure logic: no USB. (File helpers read captures, EEPROM archives and the
//! calibration cache.) The transport lives in `pakon-usb`
//! and never sees packet structure; this crate never sees the transport
//! (the same layering as `pakon_proto` / `pakon_usb` in the C driver).

pub mod calcache;
pub mod dx;
pub mod eeprom;
pub mod frame;
pub mod lightcal;
pub mod script;
pub mod setup;

/// Open-gate rows a raw keeps in front of the film (of the roll, or of each
/// cut strip): the scan writes no more of the gate than this, and older
/// raws are trimmed to it when processed.
pub const LEAD_ROWS: usize = 200;

pub use frame::{Address, Frame, FrameError, PacketType, Status};
pub use setup::{Model, ScanMode, Seq, SetupError, SetupState};
