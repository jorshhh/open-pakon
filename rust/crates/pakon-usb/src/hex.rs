//! Intel HEX parser for the FX2 firmware (port of `src/pakon_hex.c`).
//!
//! Pure text to bytes: no USB, no Pakon frames. The records it yields feed the
//! FX2 RAM download.

use std::fmt;
use std::path::Path;

pub const DATA: u8 = 0x00;
pub const EOF: u8 = 0x01;
/// Extended segment address.
pub const ESA: u8 = 0x02;
/// Extended linear address.
pub const ELA: u8 = 0x04;

/// One decoded record. `addr` is the raw 16-bit field, no base applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub addr: u16,
    pub ty: u8,
    pub data: Vec<u8>,
}

/// A data record with its absolute address (ESA/ELA base applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub addr: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HexError {
    NoColon,
    BadHeader,
    Truncated(usize),
    MissingChecksum,
    Checksum(u8),
    TrailingJunk,
    BadAddressRecord,
    UnsupportedType(u8),
    NoEof,
    /// With the 1-based line number.
    Line(usize, Box<HexError>),
    Io(String),
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoColon => write!(f, "line does not start with ':'"),
            Self::BadHeader => write!(f, "truncated/invalid header"),
            Self::Truncated(i) => write!(f, "truncated data (byte {i})"),
            Self::MissingChecksum => write!(f, "missing checksum"),
            Self::Checksum(c) => write!(f, "checksum mismatch (got {c:02x})"),
            Self::TrailingJunk => write!(f, "trailing junk after record"),
            Self::BadAddressRecord => write!(f, "address record length != 2"),
            Self::UnsupportedType(t) => write!(f, "unsupported record type {t:02x}"),
            Self::NoEof => write!(f, "no EOF record"),
            Self::Line(n, e) => write!(f, "hex line {n}: {e}"),
            Self::Io(e) => write!(f, "hex: {e}"),
        }
    }
}

impl std::error::Error for HexError {}

fn nibble(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Parse one `:`-prefixed line, validating length, checksum and bounds.
/// Surrounding whitespace and CR/LF are tolerated.
pub fn parse_line(line: &str) -> Result<Record, HexError> {
    let s = line.trim_start().as_bytes();
    let s = s.strip_prefix(b":").ok_or(HexError::NoColon)?;
    let mut pos = 0;
    let mut next = || -> Option<u8> {
        let b = (nibble(*s.get(pos)?)? << 4) | nibble(*s.get(pos + 1)?)?;
        pos += 2;
        Some(b)
    };

    let (count, ah, al, ty) = match (next(), next(), next(), next()) {
        (Some(c), Some(h), Some(l), Some(t)) => (c, h, l, t),
        _ => return Err(HexError::BadHeader),
    };
    let mut sum = count as u32 + ah as u32 + al as u32 + ty as u32;
    let mut data = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let b = next().ok_or(HexError::Truncated(i))?;
        sum += b as u32;
        data.push(b);
    }
    let ck = next().ok_or(HexError::MissingChecksum)?;
    if (sum + ck as u32) & 0xFF != 0 {
        return Err(HexError::Checksum(ck));
    }
    if !s[pos..].iter().all(|c| c.is_ascii_whitespace()) {
        return Err(HexError::TrailingJunk);
    }
    Ok(Record { addr: u16::from_be_bytes([ah, al]), ty, data })
}

/// All data records of a HEX text, base addressing applied, up to the EOF
/// record. Blank lines are skipped; a missing EOF is an error.
pub fn parse(text: &str) -> Result<Vec<Chunk>, HexError> {
    let mut base: u32 = 0;
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec = parse_line(line).map_err(|e| HexError::Line(i + 1, Box::new(e)))?;
        let addr_word = || -> Result<u32, HexError> {
            match rec.data[..] {
                [hi, lo] => Ok(u16::from_be_bytes([hi, lo]) as u32),
                _ => Err(HexError::Line(i + 1, Box::new(HexError::BadAddressRecord))),
            }
        };
        match rec.ty {
            DATA => out.push(Chunk { addr: base.wrapping_add(rec.addr as u32), data: rec.data }),
            EOF => return Ok(out),
            ESA => base = addr_word()? << 4,
            ELA => base = addr_word()? << 16,
            t => return Err(HexError::Line(i + 1, Box::new(HexError::UnsupportedType(t)))),
        }
    }
    Err(HexError::NoEof)
}

/// Read and parse a HEX file.
pub fn parse_file(path: impl AsRef<Path>) -> Result<Vec<Chunk>, HexError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|e| HexError::Io(format!("cannot open '{}': {e}", path.display())))?;
    parse(&String::from_utf8_lossy(&bytes))
}

#[cfg(test)]
mod tests {
    // Ported from test/test_hex.c.
    use super::*;

    #[test]
    fn data_line() {
        let r = parse_line(":02000000AABB99").unwrap();
        assert_eq!(r, Record { addr: 0, ty: DATA, data: vec![0xAA, 0xBB] });
    }

    #[test]
    fn eof_line() {
        assert_eq!(parse_line(":00000001FF").unwrap().ty, EOF);
    }

    #[test]
    fn rejects() {
        assert_eq!(parse_line(":02000000AABB00"), Err(HexError::Checksum(0)));
        assert_eq!(parse_line("02000000AABB99"), Err(HexError::NoColon));
        assert!(matches!(parse_line(":02000000AA"), Err(HexError::Truncated(1))));
        assert_eq!(parse_line(":00000001FFzz"), Err(HexError::TrailingJunk));
    }

    #[test]
    fn whitespace_tolerated() {
        assert!(parse_line("  :00000001FF\r\n").is_ok());
    }

    #[test]
    fn multi_record() {
        let img = ":02000000AABB99\n:020002001122C9\n:00000001FF\n";
        let c = parse(img).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].addr, c[1].addr), (0, 2));
        assert_eq!((c[0].data[0], c[1].data[0]), (0xAA, 0x11));
    }

    #[test]
    fn ela_base() {
        let img = ":020000040001F9\n:02000000AABB99\n:00000001FF\n";
        let c = parse(img).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].addr, 0x10000);
    }

    #[test]
    fn missing_eof() {
        assert_eq!(parse(":02000000AABB99\n"), Err(HexError::NoEof));
    }

    #[test]
    fn stops_at_eof() {
        // anything after EOF is ignored, as in the C parser
        assert!(parse(":00000001FF\ngarbage\n").unwrap().is_empty());
    }

    /// The committed firmware parses end to end (when present).
    #[test]
    fn real_firmware() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../firmware");
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("hex")) {
                let chunks = parse_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                assert!(!chunks.is_empty(), "{}", p.display());
            }
        }
    }
}
