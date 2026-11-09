//! The DX sensor block, PICL register 0x90 (port of `include/pakon_dx.h`,
//! `src/pakon_dx.c`). Pure.
//!
//! The event service reads `01 03 LOW 1e 90` after each LOW event; the reply
//! carries 30 bytes after its 4-byte header (`01 20 <LOW> 08`):
//!
//! - bytes 0-1: position counter, big-endian
//! - byte 2: entry count
//! - bytes 3..: entries, 5 bytes each; type in the low nibble of byte 0
//!
//! Entry types seen: 0 (none), 3 (decoded barcode), 5 (code position), 7 and
//! 8 (sensor edge events). Types 5/7/8 carry a flag in byte 1 and a 16-bit
//! big-endian position in bytes 2-3. See the C header for what was verified
//! on which unit.

use std::fmt;

/// Payload after the reply header.
pub const REPLY_LEN: usize = 30;
/// 3 + 5 * 5 = 28 of the 30 bytes.
pub const MAX_ENTRIES: usize = 5;

pub const NONE: u8 = 0;
pub const CODE: u8 = 3;
pub const POSITION: u8 = 5;
pub const EDGE_A: u8 = 7;
pub const EDGE_B: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Entry {
    /// Low nibble of byte 0.
    pub ty: u8,
    /// High nibble of byte 0 (meaning unknown).
    pub hi: u8,
    /// Byte 1.
    pub flag: u8,
    /// Bytes 2-4.
    pub b: [u8; 3],
}

impl Entry {
    /// 16-bit position of a type 5/7/8 entry.
    pub fn position(&self) -> u16 {
        u16::from_be_bytes([self.b[0], self.b[1]])
    }

    /// The decoded barcode if this is a type 3.
    pub fn code(&self) -> Option<Code> {
        (self.ty == CODE).then(|| Code::decode(self))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    pub position: u16,
    pub entries: Vec<Entry>,
}

/// Parse a 30-byte payload. `None` if short or the count does not fit.
pub fn parse(payload: &[u8]) -> Option<Block> {
    if payload.len() < REPLY_LEN {
        return None;
    }
    let count = payload[2] as usize;
    if count > MAX_ENTRIES {
        return None;
    }
    let entries = (0..count)
        .map(|i| {
            let p = &payload[3 + 5 * i..8 + 5 * i];
            Entry { ty: p[0] & 0x0F, hi: p[0] >> 4, flag: p[1], b: [p[2], p[3], p[4]] }
        })
        .collect();
    Some(Block { position: u16::from_be_bytes([payload[0], payload[1]]), entries })
}

/// A decoded barcode (type 3). Bytes 2-4:
///
/// - byte 2: bits 7-2 frame_raw\[5:0\]; bit 1 must be 0; bit 0 parity
/// - byte 3: bits 7-6 product\[1:0\]; bit 5 invalid; bits 4-1 generation;
///   bit 0 frame_raw\[6\]
/// - byte 4: bits 4-0 product\[6:2\]
///
/// Parity is even over the 19 bits of bytes 2, 3 and 4\[4:0\]. frame_raw
/// counts half frames (frame * 2, + 1 for the "A" code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub frame_raw: u8,
    /// DX part 1, 0..127.
    pub product: u8,
    /// DX part 2, 0..15.
    pub generation: u8,
    /// Entry byte 1.
    pub slot: u8,
    pub parity_ok: bool,
    /// Parity good, invalid bit and byte 2 bit 1 clear.
    pub valid: bool,
}

impl Code {
    fn decode(e: &Entry) -> Code {
        let [b2, b3, b4] = e.b;
        let parity_ok = (b2.count_ones() + b3.count_ones() + (b4 & 0x1F).count_ones()) % 2 == 0;
        Code {
            frame_raw: ((b3 & 0x01) << 6) | (b2 >> 2),
            product: ((b4 & 0x1F) << 2) | (b3 >> 6),
            generation: (b3 >> 1) & 0x0F,
            slot: e.flag,
            parity_ok,
            valid: parity_ok && b3 & 0x20 == 0 && b2 & 0x02 == 0,
        }
    }
}

/// Frame label of a frame_raw value: "12", "12A"; 124/125 print "X"/"XA" and
/// 126/127 "00"/"00A", the leader labels.
pub fn frame_label(frame_raw: u8) -> String {
    let frame = (frame_raw & 0x7F) >> 1;
    let half = if frame_raw & 1 != 0 { "A" } else { "" };
    match frame {
        62 => format!("X{half}"),
        63 => format!("00{half}"),
        f => format!("{f}{half}"),
    }
}

impl fmt::Display for Block {
    /// One line describing a block, for logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.entries.len();
        write!(f, "pos 0x{:04x}, {n} entr{}", self.position, if n == 1 { "y" } else { "ies" })?;
        for e in &self.entries {
            if let Some(c) = e.code() {
                let note = if c.valid { "" } else if c.parity_ok { " INVALID" } else { " BAD-PARITY" };
                write!(f, "; code {} {}/{} slot {}{note}", frame_label(c.frame_raw), c.product, c.generation, c.slot)?;
            } else if e.ty == NONE {
                write!(f, "; none")?;
            } else {
                write!(f, "; type {} flag 0x{:02x} pos 0x{:04x}", e.ty, e.flag, e.position())?;
            }
        }
        Ok(())
    }
}

/// A code collected over a scan, with the position of the type 5 entry
/// before it (the engine takes a code's position from there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollCode {
    pub frame_raw: u8,
    pub slot: u8,
    pub position: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmVotes {
    pub product: u8,
    pub generation: u8,
    pub votes: u32,
}

pub const ROLL_MAX: usize = 160;
pub const ROLL_FILMS: usize = 8;

/// Codes collected over a scan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roll {
    pub blocks: u32,
    pub entries: u32,
    pub codes: u32,
    pub rejected: u32,
    pub type_count: [u32; 16],
    pub films: Vec<FilmVotes>,
    pub code: Vec<RollCode>,
    last_position: Option<u16>,
}

impl Roll {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, b: &Block) {
        self.blocks += 1;
        for e in &b.entries {
            self.entries += 1;
            self.type_count[e.ty as usize] += 1;
            if e.ty == POSITION {
                self.last_position = Some(e.position());
                continue;
            }
            let Some(c) = e.code() else { continue };
            if !c.valid {
                self.rejected += 1;
                continue;
            }
            self.codes += 1;
            match self.films.iter().position(|f| f.product == c.product && f.generation == c.generation) {
                Some(i) => self.films[i].votes += 1,
                None if self.films.len() < ROLL_FILMS => {
                    self.films.push(FilmVotes { product: c.product, generation: c.generation, votes: 1 })
                }
                None => {}
            }
            if self.code.len() < ROLL_MAX {
                self.code.push(RollCode { frame_raw: c.frame_raw, slot: c.slot, position: self.last_position });
            }
            self.last_position = None;
        }
    }

    /// Product and generation read most often (the first on a tie).
    pub fn film(&self) -> Option<(u8, u8)> {
        let mut best: Option<&FilmVotes> = None;
        for f in &self.films {
            if best.is_none_or(|b| f.votes > b.votes) {
                best = Some(f);
            }
        }
        best.map(|f| (f.product, f.generation))
    }
}

impl fmt::Display for Roll {
    /// One line summarising the roll, for logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((prod, gen)) = self.film() else {
            return write!(
                f,
                "no code read ({} sensor reads, {} entries, {} rejected)",
                self.blocks, self.entries, self.rejected
            );
        };
        let lab = |c: Option<&RollCode>| c.map_or("-".to_string(), |c| frame_label(c.frame_raw));
        write!(
            f,
            "film {prod}/{gen}, {} codes ({} rejected), frames {}..{}, {} sensor reads",
            self.codes,
            self.rejected,
            lab(self.code.first()),
            lab(self.code.last()),
            self.blocks
        )
    }
}
