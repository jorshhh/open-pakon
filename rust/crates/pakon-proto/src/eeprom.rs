//! Per-unit EEPROM decoder (port of `include/pakon_eeprom.h`,
//! `src/pakon_eeprom.c`; `tools/pakon_eeprom.py` is the reference and all
//! three must agree).
//!
//! The EEPROM (I2C 0x52) holds two sections, each stored twice and framed as
//! `{u32 length; u32 crc32 (zlib, over the payload)}`:
//!
//! - A primary 0x000, A backup 0x400 — 398 bytes: hardware version, scanner
//!   type, serial, per-base Offset / MotorSpeed / MotorSpeed_Ir, and the
//!   NegMatrix / PosMatrix (3x10 floats each).
//! - B primary 0x800, B backup 0xA00 — 36 bytes: per-base motor-adjust words.
//!
//! Only parses bytes already read. Nothing here writes to a scanner.

/// The four section copies, in read order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopyId {
    APrimary = 0,
    ABackup = 1,
    BPrimary = 2,
    BBackup = 3,
}

impl CopyId {
    pub const ALL: [CopyId; 4] = [CopyId::APrimary, CopyId::ABackup, CopyId::BPrimary, CopyId::BBackup];

    /// Chip offset.
    pub fn offset(self) -> u16 {
        [0x000, 0x400, 0x800, 0xA00][self as usize]
    }

    /// Fixed length, header included.
    pub fn size(self) -> usize {
        match self {
            CopyId::APrimary | CopyId::ABackup => A_LEN,
            CopyId::BPrimary | CopyId::BBackup => B_LEN,
        }
    }

    /// Archive file stem (`eeprom_0x52_<name>.bin`).
    pub fn name(self) -> &'static str {
        ["sectionA_primary", "sectionA_backup", "sectionB_primary", "sectionB_backup"][self as usize]
    }
}

pub const A_LEN: usize = 398;
pub const B_LEN: usize = 36;

/// Scanner type at section A 0x0C.
pub const TYPE_F135: u32 = 1350;
pub const TYPE_F135_PLUS: u32 = 1351;

/// Resolution bases, in EEPROM order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    B4 = 0,
    B8 = 1,
    B16 = 2,
}

impl Base {
    pub fn from_base(b: u32) -> Option<Self> {
        match b {
            4 => Some(Self::B4),
            8 => Some(Self::B8),
            16 => Some(Self::B16),
            _ => None,
        }
    }
}

const A_BASE_AT: [usize; 3] = [0x14, 0x1A, 0x20];
const A_NEG_MATRIX_AT: usize = 0x26;
const A_POS_MATRIX_AT: usize = 0x9E;
const B_ADJUST_AT: [usize; 3] = [8, 16, 24];
const B_UNKNOWN_AT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BaseValues {
    /// The OEM's "Offset" (CCD window start).
    pub offset: u16,
    pub motor_speed: u16,
    pub motor_speed_ir: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MotorAdjust {
    pub normal: u16,
    pub drag: u16,
    pub ir: u16,
    pub drag_ir: u16,
}

/// Decoded EEPROM plus per-copy validity and the copy each section came from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Eeprom {
    /// Length + CRC check per copy, indexed by [`CopyId`].
    pub valid: [bool; 4],
    /// A, B: whether primary and backup are identical; `None` when either is
    /// missing or the wrong length.
    pub copies_equal: [Option<bool>; 2],
    /// A, B: the copy decoded, if any validated.
    pub selected: [Option<CopyId>; 2],
    /// Recovery notes (a copy invalid, copies differing, unknown type...).
    pub warnings: Vec<String>,
    /// Both sections decoded, the type is known and the matrices are finite.
    pub decodable: bool,

    // Section A (meaningful only when selected[0] is set).
    pub hardware_version: u32,
    pub scanner_type: u32,
    pub serial: u32,
    /// Indexed by [`Base`].
    pub base: [BaseValues; 3],
    pub neg_matrix: [[f32; 10]; 3],
    pub pos_matrix: [[f32; 10]; 3],

    // Section B (meaningful only when selected[1] is set).
    /// Indexed by [`Base`].
    pub adjust: [MotorAdjust; 3],
    pub section_b_unknown: u32,
}

impl Eeprom {
    pub fn base(&self, b: Base) -> BaseValues {
        self.base[b as usize]
    }

    pub fn adjust(&self, b: Base) -> MotorAdjust {
        self.adjust[b as usize]
    }

    /// "F-135", "F-135+" or "unknown".
    pub fn model_name(&self) -> &'static str {
        model_name(self.scanner_type)
    }
}

pub fn model_name(scanner_type: u32) -> &'static str {
    match scanner_type {
        TYPE_F135 => "F-135",
        TYPE_F135_PLUS => "F-135+",
        _ => "unknown",
    }
}

fn le16(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([p[at], p[at + 1]])
}

fn le32(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(p[at..at + 4].try_into().unwrap())
}

/// CRC-32 as zlib's `crc32(0, data, len)`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// A complete copy of `which`: exact fixed length, a matching length field,
/// and a matching CRC over bytes 8..end.
pub fn copy_valid(which: CopyId, data: &[u8]) -> bool {
    let n = which.size();
    data.len() == n && le32(data, 0) as usize == n && le32(data, 4) == crc32(&data[8..])
}

fn read_matrix(p: &[u8], at: usize) -> [[f32; 10]; 3] {
    let mut m = [[0f32; 10]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = f32::from_bits(le32(p, at + 4 * (10 * r + c)));
        }
    }
    m
}

/// Decode up to four copies, indexed by [`CopyId`] (`None` = not read). For
/// each section the primary is used if valid, else the backup; a section with
/// no valid copy is left zeroed. Whatever decoded is filled in; check
/// [`Eeprom::decodable`] before using it.
pub fn decode(data: [Option<&[u8]>; 4]) -> Eeprom {
    let mut e = Eeprom::default();
    for c in CopyId::ALL {
        e.valid[c as usize] = data[c as usize].is_some_and(|d| copy_valid(c, d));
    }

    for s in 0..2 {
        let (pri, bak) = (CopyId::ALL[2 * s], CopyId::ALL[2 * s + 1]);
        let n = pri.size();
        let name = if s == 0 { 'A' } else { 'B' };
        e.copies_equal[s] = match (data[pri as usize], data[bak as usize]) {
            (Some(p), Some(b)) if p.len() == n && b.len() == n => Some(p == b),
            _ => None,
        };
        let (vp, vb) = (e.valid[pri as usize], e.valid[bak as usize]);
        e.selected[s] = if vp { Some(pri) } else if vb { Some(bak) } else { None };
        if !vp && !vb {
            e.warnings.push(format!("section {name}: neither copy validates"));
        } else if !vp || !vb {
            let (bad, used) = if vp { ("backup", "primary") } else { ("primary", "backup") };
            e.warnings.push(format!("section {name}: {bad} invalid, {used} selected"));
        } else if e.copies_equal[s] == Some(false) {
            e.warnings.push(format!("section {name}: valid copies differ, primary selected"));
        }
    }

    let mut ok = e.selected.iter().all(Option::is_some);

    if let Some(c) = e.selected[0] {
        let a = data[c as usize].unwrap();
        e.hardware_version = le32(a, 0x08);
        e.scanner_type = le32(a, 0x0C);
        e.serial = le32(a, 0x10);
        if !matches!(e.scanner_type, TYPE_F135 | TYPE_F135_PLUS) {
            e.warnings.push(format!("unknown scanner type {}", e.scanner_type));
            ok = false;
        }
        for (b, at) in A_BASE_AT.iter().enumerate() {
            e.base[b] = BaseValues {
                offset: le16(a, *at),
                motor_speed: le16(a, at + 2),
                motor_speed_ir: le16(a, at + 4),
            };
        }
        e.neg_matrix = read_matrix(a, A_NEG_MATRIX_AT);
        e.pos_matrix = read_matrix(a, A_POS_MATRIX_AT);
        if !e.neg_matrix.iter().chain(&e.pos_matrix).flatten().all(|v| v.is_finite()) {
            e.warnings.push("nonfinite matrix coefficients".into());
            ok = false;
        }
    }

    if let Some(c) = e.selected[1] {
        let b = data[c as usize].unwrap();
        for (i, at) in B_ADJUST_AT.iter().enumerate() {
            e.adjust[i] = MotorAdjust {
                normal: le16(b, *at),
                drag: le16(b, at + 2),
                ir: le16(b, at + 4),
                drag_ir: le16(b, at + 6),
            };
        }
        e.section_b_unknown = le32(b, B_UNKNOWN_AT);
    }

    e.decodable = ok;
    e
}

/// Load an archive directory (`eeprom_0x52_<copy>.bin` files); missing files
/// are `None`.
pub fn load_dir(dir: impl AsRef<std::path::Path>) -> [Option<Vec<u8>>; 4] {
    CopyId::ALL.map(|c| std::fs::read(dir.as_ref().join(format!("eeprom_0x52_{}.bin", c.name()))).ok())
}

/// Borrow loaded copies for [`decode`].
pub fn as_refs(copies: &[Option<Vec<u8>>; 4]) -> [Option<&[u8]>; 4] {
    [0, 1, 2, 3].map(|i| copies[i].as_deref())
}
