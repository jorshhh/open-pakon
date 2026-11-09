//! The FX2 firmware-load sequence built from Intel HEX (port of
//! `pakon_fw.c`; pure, the transport sends it).
//!
//! The cold scanner (0f05:f235) is an EZ-USB FX2 that needs two downloads:
//!   1. hold the 8051 in reset: CPUCS = 1 (written at 0x7F92 and 0xE600)
//!   2. the stage-1 loader to internal RAM, vendor 0xA0 (firmware/PknLdr.hex)
//!   3. run it: CPUCS = 0
//!   4. personality read: OUT 0xA4 wValue 0x00A1, then IN 0xA9 (8 bytes)
//!   5. the main image (firmware/Pakon7.hex): records at >= 0x2000 to
//!      external RAM with 0xA3 (served by the stage-1 loader), then CPUCS = 1
//!      and the records below 0x2000 to internal RAM with 0xA0
//!   6. CPUCS = 1, CPUCS = 0: run it; it re-enumerates as 0f05:f135
//!
//! One transfer per HEX record, in file order.

use crate::hex::{self, HexError};

const CPUCS_EZUSB: u16 = 0x7F92;
const CPUCS_FX2: u16 = 0xE600;
/// First external-RAM address.
const EXT_RAM: u32 = 0x2000;

/// One control transfer of the load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xfer {
    /// 0x40 vendor OUT, 0xC0 vendor IN.
    pub request_type: u8,
    /// 0xA0, 0xA3, 0xA4, 0xA9.
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    /// OUT payload (empty for IN).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    pub xfers: Vec<Xfer>,
    /// Index of the 0xA9 personality read.
    pub personality_at: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FwError {
    Hex(String),
    /// A record over 16 bytes, past 0xFFFF, or a stage-1 record in external RAM.
    BadRecord(u32),
}

impl std::fmt::Display for FwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FwError::Hex(e) => write!(f, "firmware HEX: {e}"),
            FwError::BadRecord(a) => write!(f, "firmware record at {a:#x} does not fit the FX2 load"),
        }
    }
}

impl From<HexError> for FwError {
    fn from(e: HexError) -> Self {
        FwError::Hex(format!("{e:?}"))
    }
}

fn out(req: u8, value: u16, data: &[u8]) -> Xfer {
    Xfer { request_type: 0x40, request: req, value, index: 0, length: data.len() as u16, data: data.to_vec() }
}

fn cpucs(v: &mut Vec<Xfer>, b: u8) {
    v.push(out(0xA0, CPUCS_EZUSB, &[b]));
    v.push(out(0xA0, CPUCS_FX2, &[b]));
}

/// Build the load from the two HEX texts.
pub fn build(stage1: &str, main_image: &str) -> Result<Sequence, FwError> {
    let s1 = hex::parse(stage1)?;
    let main = hex::parse(main_image)?;
    for c in s1.iter().chain(&main) {
        if c.data.len() > 16 || c.addr > 0xFFFF {
            return Err(FwError::BadRecord(c.addr));
        }
    }
    if let Some(c) = s1.iter().find(|c| c.addr >= EXT_RAM) {
        return Err(FwError::BadRecord(c.addr));
    }
    let mut v = Vec::new();
    cpucs(&mut v, 1);
    cpucs(&mut v, 1);
    v.extend(s1.iter().map(|c| out(0xA0, c.addr as u16, &c.data)));
    cpucs(&mut v, 0);
    v.push(out(0xA4, 0x00A1, &[]));
    let personality_at = v.len();
    v.push(Xfer { request_type: 0xC0, request: 0xA9, value: 0, index: 0, length: 8, data: vec![] });
    v.extend(main.iter().filter(|c| c.addr >= EXT_RAM).map(|c| out(0xA3, c.addr as u16, &c.data)));
    cpucs(&mut v, 1);
    v.extend(main.iter().filter(|c| c.addr < EXT_RAM).map(|c| out(0xA0, c.addr as u16, &c.data)));
    cpucs(&mut v, 1);
    cpucs(&mut v, 0);
    Ok(Sequence { xfers: v, personality_at })
}

/// The F-135/F-135+ personality: the C0 boot record for 0f05:f235 revision
/// 0xAA07 (`c0 05 0f 35 f2 07 aa xx`), the one Pakon7.hex is for.
pub fn personality_is_f135(rec: &[u8]) -> bool {
    rec.len() >= 7 && rec[..7] == [0xC0, 0x05, 0x0F, 0x35, 0xF2, 0x07, 0xAA]
}

/// A captured `.pakfw` script: `bmRequestType bRequest wValue wIndex
/// wLength [dataHex]` per line, `#` comments.
pub fn parse_pakfw(text: &str) -> Vec<Xfer> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() < 5 {
                return None;
            }
            let h = |s: &str| u16::from_str_radix(s, 16).ok();
            let (bm, rq, wv, wi, wl) = (h(w[0])? as u8, h(w[1])? as u8, h(w[2])?, h(w[3])?, h(w[4])?);
            let data = if bm & 0x80 == 0 {
                let s = w.get(5).copied().unwrap_or("");
                (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16)).collect::<Result<Vec<u8>, _>>().ok()?
            } else {
                vec![]
            };
            Some(Xfer { request_type: bm, request: rq, value: wv, index: wi, length: wl, data })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personality() {
        assert!(personality_is_f135(&[0xC0, 0x05, 0x0F, 0x35, 0xF2, 0x07, 0xAA, 0x01]));
        assert!(!personality_is_f135(&[0xC0, 0x05, 0x0F, 0x35, 0xF2, 0x08, 0xAA, 0x01]));
    }

    #[test]
    fn matches_the_capture() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let (Ok(s1), Ok(m), Ok(cap)) = (
            std::fs::read_to_string(repo.join("firmware/PknLdr.hex")),
            std::fs::read_to_string(repo.join("firmware/Pakon7.hex")),
            std::fs::read_to_string(repo.join("resources/f135.pakfw")),
        ) else {
            eprintln!("skip - OEM firmware not in firmware/");
            return;
        };
        let seq = build(&s1, &m).unwrap();
        assert_eq!(seq.xfers[seq.personality_at].request, 0xA9);
        let want = parse_pakfw(&cap);
        assert_eq!(want.len(), 1104);
        assert_eq!(seq.xfers, want);
    }
}
