//! The Pakon frame (port of `include/pakon_proto.h`, `src/pakon_proto.c`).
//!
//! On the wire a frame is `[type][count][data; count]`, `2 + count` bytes,
//! no checksum and no padding (36 bytes is only the maximum size). `data[0]`
//! is the address byte; in a reply, `data[1]` is the status byte.
//! See `docs/PROTOCOL.md`.

use std::fmt;

/// Largest in-memory frame: type + count + 34 data bytes.
pub const PACKET_SIZE: usize = 36;
/// Maximum `count` (bytes 2..35).
pub const DATA_MAX: usize = 34;

/// Packet type, byte 0. Confirmed from our F-135 and F-135+ captures.
///
/// Type `0` is deliberately absent: the safety rules forbid sending it, so it
/// cannot be expressed when building a frame. A parsed frame keeps its raw
/// type byte (see [`Frame::raw_type`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PacketType {
    /// Host reads N data bytes back.
    Read = 1,
    /// Host writes N data bytes (register/config).
    Write = 2,
    /// One-byte status poll, no command byte.
    ReadStatus = 3,
    /// Command with no data payload.
    Command = 4,
    /// Device-to-host reply.
    Response = 7,
}

impl PacketType {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Read,
            2 => Self::Write,
            3 => Self::ReadStatus,
            4 => Self::Command,
            7 => Self::Response,
            _ => return None,
        })
    }
}

/// Address byte (`data[0]`). Documented values only.
///
/// These name addresses for logs and tests. Controller addresses used to talk
/// to a scanner must come from the probe results, never be picked from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Address {
    Host = 0x10,
    PicL = 0x20,
    BootPicL = 0x22,
    PicM = 0x24,
    BootPicM = 0x26,
    PicLPlus = 0x40,
    BootPicLPlus = 0x42,
    PicMPlus = 0x44,
    BootPicMPlus = 0x46,
}

impl Address {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x10 => Self::Host,
            0x20 => Self::PicL,
            0x22 => Self::BootPicL,
            0x24 => Self::PicM,
            0x26 => Self::BootPicM,
            0x40 => Self::PicLPlus,
            0x42 => Self::BootPicLPlus,
            0x44 => Self::PicMPlus,
            0x46 => Self::BootPicMPlus,
            _ => return None,
        })
    }

    /// A bootloader address. The safety rules forbid addressing these.
    pub fn is_bootloader(self) -> bool {
        matches!(
            self,
            Self::BootPicL | Self::BootPicM | Self::BootPicLPlus | Self::BootPicMPlus
        )
    }

    /// Same strings as `pakon_addr_str`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Host => "AD_HOST",
            Self::PicL => "AD_PICL",
            Self::BootPicL => "AD_BOOT_PICL",
            Self::PicM => "AD_PICM",
            Self::BootPicM => "AD_BOOT_PICM",
            Self::PicLPlus => "AD_PICL_PLUS",
            Self::BootPicLPlus => "AD_BOOT_PICL_PLUS",
            Self::PicMPlus => "AD_PICM_PLUS",
            Self::BootPicMPlus => "AD_BOOT_PICM_PLUS",
        }
    }
}

/// Reply status, `data[1]` of a device-to-host frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Success,
    NotAcked,
    InvalidPacket,
    BadChecksum,
    /// 4..6 are USB-related; exact meanings unknown.
    Usb(u8),
    HostAlgorithm,
    /// Also reported as success in some sequences.
    Success8,
    BusError,
    /// Any other wire value.
    Unknown(u8),
    /// Not a wire value: the reply was too short to carry a status byte.
    /// Never equal to [`Status::Success`], so a truncated reply cannot read
    /// as "device ready".
    None,
}

impl Status {
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => Self::Success,
            1 => Self::NotAcked,
            2 => Self::InvalidPacket,
            3 => Self::BadChecksum,
            4..=6 => Self::Usb(b),
            7 => Self::HostAlgorithm,
            8 => Self::Success8,
            9 => Self::BusError,
            _ => Self::Unknown(b),
        }
    }

    /// Same strings as `pakon_status_str`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NotAcked => "not acked",
            Self::InvalidPacket => "invalid packet",
            Self::BadChecksum => "bad checksum",
            Self::Usb(4) => "usb error (4)",
            Self::Usb(5) => "usb error (5)",
            Self::Usb(6) => "usb error (6)",
            Self::Usb(_) | Self::Unknown(_) => "unknown status",
            Self::HostAlgorithm => "host algorithm error",
            Self::Success8 => "success (8)",
            Self::BusError => "bus error",
            Self::None => "no status (reply too short)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// More than [`DATA_MAX`] data bytes.
    TooLong(usize),
    /// Fewer than 2 bytes received.
    Truncated(usize),
    /// Received length is not `2 + count`.
    LengthMismatch { count: u8, len: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong(n) => write!(f, "frame data too long: {n} > {DATA_MAX}"),
            Self::Truncated(n) => write!(f, "frame truncated: {n} bytes"),
            Self::LengthMismatch { count, len } => {
                write!(f, "frame length {len} != 2 + count {count}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// One frame. Holds the raw type byte so that any received frame round-trips.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    ty: u8,
    count: u8,
    data: [u8; DATA_MAX],
}

impl Frame {
    /// Build a frame to send. `data` includes the address byte as `data[0]`.
    /// No checksum is added: captured frames carry none.
    pub fn new(ty: PacketType, data: &[u8]) -> Result<Self, FrameError> {
        Self::with_raw_type(ty as u8, data)
    }

    fn with_raw_type(ty: u8, data: &[u8]) -> Result<Self, FrameError> {
        if data.len() > DATA_MAX {
            return Err(FrameError::TooLong(data.len()));
        }
        let mut buf = [0u8; DATA_MAX];
        buf[..data.len()].copy_from_slice(data);
        Ok(Self { ty, count: data.len() as u8, data: buf })
    }

    /// Parse received bytes, requiring `len == 2 + raw[1]` and
    /// `count <= DATA_MAX`.
    pub fn parse(raw: &[u8]) -> Result<Self, FrameError> {
        if raw.len() < 2 {
            return Err(FrameError::Truncated(raw.len()));
        }
        let count = raw[1];
        if count as usize > DATA_MAX || raw.len() != 2 + count as usize {
            return Err(FrameError::LengthMismatch { count, len: raw.len() });
        }
        Self::with_raw_type(raw[0], &raw[2..])
    }

    /// The type byte as received or built.
    pub fn raw_type(&self) -> u8 {
        self.ty
    }

    pub fn packet_type(&self) -> Option<PacketType> {
        PacketType::from_u8(self.ty)
    }

    pub fn count(&self) -> u8 {
        self.count
    }

    /// The `count` data bytes (address first).
    pub fn data(&self) -> &[u8] {
        &self.data[..self.count as usize]
    }

    /// On-wire length, `2 + count`.
    pub fn wire_len(&self) -> usize {
        2 + self.count as usize
    }

    /// The address byte, or 0 for an empty frame (as `pakon_packet_addr`).
    pub fn addr(&self) -> u8 {
        if self.count >= 1 { self.data[0] } else { 0 }
    }

    /// The reply status; [`Status::None`] if the frame is too short to
    /// carry one.
    pub fn status(&self) -> Status {
        if self.count >= 2 { Status::from_u8(self.data[1]) } else { Status::None }
    }

    /// Serialize into `out`, returning the bytes written (`wire_len`).
    pub fn write_to(&self, out: &mut [u8]) -> Option<usize> {
        let n = self.wire_len();
        let out = out.get_mut(..n)?;
        out[0] = self.ty;
        out[1] = self.count;
        out[2..].copy_from_slice(self.data());
        Some(n)
    }

    /// Serialize to a new vector.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![0u8; self.wire_len()];
        self.write_to(&mut v);
        v
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex: Vec<String> = self.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        write!(f, "Frame[{}]", hex.join(" "))
    }
}

#[cfg(test)]
mod tests {
    // Ported from test/test_proto.c.
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(PACKET_SIZE, 36);
        assert_eq!(DATA_MAX, 34);
        assert_eq!(PACKET_SIZE, 2 + DATA_MAX);
    }

    #[test]
    fn documented_values() {
        assert_eq!(Address::Host as u8, 0x10);
        assert_eq!(Address::BootPicMPlus as u8, 0x46);
        assert_eq!(Status::from_u8(0), Status::Success);
        assert_eq!(Status::from_u8(9), Status::BusError);
        assert_eq!(
            [
                PacketType::Read,
                PacketType::Write,
                PacketType::ReadStatus,
                PacketType::Command,
                PacketType::Response
            ]
            .map(|t| t as u8),
            [1, 2, 3, 4, 7]
        );
        assert_eq!(PacketType::from_u8(0), None);
    }

    #[test]
    fn build_open_frame() {
        // the real open packet: 04 03 10 00 85
        let f = Frame::new(PacketType::Command, &[0x10, 0x00, 0x85]).unwrap();
        assert_eq!(f.raw_type(), 0x04);
        assert_eq!(f.count(), 3);
        assert_eq!(f.wire_len(), 5);
        assert_eq!(f.addr(), Address::Host as u8);
        assert_eq!(f.to_bytes(), [0x04, 0x03, 0x10, 0x00, 0x85]);

        let mut wire = [0u8; PACKET_SIZE];
        assert_eq!(f.write_to(&mut wire), Some(5));
        assert_eq!(&wire[..5], &[0x04, 0x03, 0x10, 0x00, 0x85]);
        assert_eq!(f.write_to(&mut [0u8; 4]), None);
    }

    #[test]
    fn parse_reply() {
        let f = Frame::parse(&[0x07, 0x02, 0x10, 0x00]).unwrap();
        assert_eq!(f.packet_type(), Some(PacketType::Response));
        assert_eq!(f.count(), 2);
        assert_eq!(f.status(), Status::Success);
    }

    #[test]
    fn reject_inconsistent_length() {
        // a 0xA9 read: length != 2 + count
        assert_eq!(
            Frame::parse(&[0x8e, 0x01, 0x00, 0x00, 0x66]),
            Err(FrameError::LengthMismatch { count: 1, len: 5 })
        );
        assert_eq!(Frame::parse(&[0x07]), Err(FrameError::Truncated(1)));
    }

    #[test]
    fn short_reply_is_not_success() {
        let f = Frame::parse(&[0x07, 0x01, 0x10]).unwrap();
        assert_eq!(f.status(), Status::None);
        assert_ne!(f.status(), Status::Success);
    }

    #[test]
    fn too_long() {
        assert_eq!(
            Frame::new(PacketType::Write, &[0u8; 35]),
            Err(FrameError::TooLong(35))
        );
        assert!(Frame::new(PacketType::Write, &[0u8; 34]).is_ok());
    }

    #[test]
    fn parse_round_trip() {
        let raw = [0x02, 0x05, 0x20, 0x02, 0x06, 0x00, 0x00];
        assert_eq!(Frame::parse(&raw).unwrap().to_bytes(), raw);
    }

    #[test]
    fn names() {
        assert_eq!(Address::PicMPlus.name(), "AD_PICM_PLUS");
        assert!(Address::BootPicL.is_bootloader());
        assert!(!Address::PicL.is_bootloader());
        assert_eq!(Status::None.name(), "no status (reply too short)");
        assert_eq!(Status::from_u8(5).name(), "usb error (5)");
    }
}
