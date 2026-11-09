//! When to recalibrate, and the saved calibration (port of
//! `include/pakon_calcache.h`, `src/pakon_calcache.c`).
//!
//! TLB.dll keeps one calibration record per scan setting and film type with
//! two timestamps, the last calibration and the last full one. Before a scan:
//! no record or the last full one older than 20 h → full calibration; else
//! the last one older than 60 min → calibration from the saved currents;
//! else reuse. We keep the record in a file next to the EEPROM archive, one
//! per film type, resolution base and IR on / off.
//!
//! The file format is the C driver's, byte for byte, so calibrations carry
//! over between the two: magic `PKCAL001`, a byte-order word, then
//! fixed-width little-endian fields and the two tables as f64. (The C code
//! writes host order; every platform we ship is little-endian, and another
//! order fails the byte-order check and is just a miss.)

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::lightcal::{Film, NCH};

/// DarkPointCorrectIntervalMinutes = 60.
pub const INTERVAL_S: i64 = 60 * 60;
/// Full calibration at least every 20 h.
pub const FULL_S: i64 = 72000;

const MAGIC: &[u8; 8] = b"PKCAL001";
const BOM: u32 = 0x0102_0304;
const MAX_LINE_PX: usize = 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Apply the saved values, no calibration.
    Reuse,
    /// Calibrate from the saved currents.
    Refresh,
    /// Calibrate from scratch.
    Full,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Reuse => "reuse",
            Action::Refresh => "refresh",
            Action::Full => "full",
        }
    }
}

/// What to do before a scan at `now`, given the record's `when` (last
/// calibration) and `when_full` (last full calibration), Unix seconds. A
/// record from the future (the clock went back) counts as stale: full.
pub fn needed(now: i64, when: i64, when_full: i64) -> Action {
    if when <= 0 || when_full <= 0 || when > now || when_full > now || now - when_full > FULL_S {
        Action::Full
    } else if now - when > INTERVAL_S {
        Action::Refresh
    } else {
        Action::Reuse
    }
}

/// One calibration: the light-calibration result and what it is valid for.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// EEPROM serial.
    pub serial: u32,
    /// EEPROM Offset for the base.
    pub offset: u32,
    pub film: Film,
    /// Unix time of the last / last full calibration.
    pub when: i64,
    pub when_full: i64,
    /// A/D gain codes and offsets, R, G, B.
    pub gain: [i32; 3],
    pub off: [i32; 3],
    /// LED currents, R, G, B, IR.
    pub cur: [u32; 4],
    /// Open-gate duties, R, G, B, IR.
    pub duty: [u16; 4],
    pub line_px: usize,
    pub lines_dark: usize,
    pub lines_bright: usize,
    /// Fixed-pattern tables, `[channel][pixel]` (`NCH * line_px`).
    pub dark: Vec<f64>,
    pub bright: Vec<f64>,
}

/// `dir/calibration-<film>.bin` at Base 16 + IR (the name it has always had),
/// `dir/calibration-<film>-base<N>.bin` at Base 8 / 4, with `-noir` before
/// `.bin` when IR is off. `None` for another base.
pub fn path(dir: impl AsRef<Path>, film: Film, base: u32, ir: bool) -> Option<PathBuf> {
    let base_part = match base {
        16 => String::new(),
        4 | 8 => format!("-base{base}"),
        _ => return None,
    };
    let noir = if ir { "" } else { "-noir" };
    Some(dir.as_ref().join(format!("calibration-{}{base_part}{noir}.bin", film.name())))
}

impl Record {
    fn encode(&self) -> io::Result<Vec<u8>> {
        let nt = NCH * self.line_px;
        if self.line_px == 0 || self.dark.len() != nt || self.bright.len() != nt {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "calibration tables missing or wrong size"));
        }
        let mut b = Vec::with_capacity(96 + 16 * nt);
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&BOM.to_le_bytes());
        for v in [self.serial as i32, self.offset as i32, self.film as i32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&self.when.to_le_bytes());
        b.extend_from_slice(&self.when_full.to_le_bytes());
        for c in 0..3 {
            b.extend_from_slice(&self.gain[c].to_le_bytes());
            b.extend_from_slice(&self.off[c].to_le_bytes());
        }
        for c in 0..4 {
            b.extend_from_slice(&(self.cur[c] as i32).to_le_bytes());
            b.extend_from_slice(&(self.duty[c] as i32).to_le_bytes());
        }
        for v in [self.line_px, self.lines_dark, self.lines_bright] {
            b.extend_from_slice(&(v as i64).to_le_bytes());
        }
        for v in self.dark.iter().chain(&self.bright) {
            b.extend_from_slice(&v.to_le_bytes());
        }
        Ok(b)
    }

    /// Write the record through a temporary file and a rename, so a crash
    /// never leaves half a record under the real name.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let bytes = self.encode()?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let res = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    }

    /// Read a record and check it is for this scanner, film and line. `None`
    /// when the file is missing, unreadable, from another version, or does
    /// not match. `line_px` 0 accepts the saved width (Base 8, where the line
    /// is measured, not known before calibrating).
    pub fn load(path: impl AsRef<Path>, serial: u32, offset: u32, film: Film, line_px: usize) -> Option<Record> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut r = Reader(&mut f);
        if &r.bytes::<8>()? != MAGIC || r.u32()? != BOM {
            return None;
        }
        let (rs, ro, rf) = (r.i32()? as u32, r.i32()? as u32, r.i32()?);
        let (when, when_full) = (r.i64()?, r.i64()?);
        let (mut gain, mut off) = ([0; 3], [0; 3]);
        for c in 0..3 {
            gain[c] = r.i32()?;
            off[c] = r.i32()?;
        }
        let (mut cur, mut duty) = ([0; 4], [0; 4]);
        for c in 0..4 {
            cur[c] = r.i32()? as u32;
            duty[c] = r.i32()? as u16;
        }
        let (lp, ld, lb) = (r.i64()? as usize, r.i64()? as usize, r.i64()? as usize);
        let rfilm = u32::try_from(rf).ok().and_then(Film::from_u32)?;
        if rs != serial || ro != offset || rfilm != film || (line_px != 0 && lp != line_px) || lp == 0 || lp > MAX_LINE_PX {
            return None;
        }
        let nt = NCH * lp;
        let dark = r.f64s(nt)?;
        let bright = r.f64s(nt)?;
        Some(Record {
            serial: rs,
            offset: ro,
            film: rfilm,
            when,
            when_full,
            gain,
            off,
            cur,
            duty,
            line_px: lp,
            lines_dark: ld,
            lines_bright: lb,
            dark,
            bright,
        })
    }
}

struct Reader<'a, R: Read>(&'a mut R);

impl<R: Read> Reader<'_, R> {
    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut b = [0u8; N];
        self.0.read_exact(&mut b).ok()?;
        Some(b)
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.bytes().map(i32::from_le_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.bytes().map(i64::from_le_bytes)
    }
    fn f64s(&mut self, n: usize) -> Option<Vec<f64>> {
        let mut raw = vec![0u8; 8 * n];
        self.0.read_exact(&mut raw).ok()?;
        Some(raw.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c)).collect())
    }
}
