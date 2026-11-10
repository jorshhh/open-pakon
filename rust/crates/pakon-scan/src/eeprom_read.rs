//! Reading the unit's EEPROM from a warm scanner (port of `read_section` /
//! `backup` in `tools/pakon_eeprom.py`): only the OEM read select (vendor
//! request 0xA4) and read (0xA9), over fixed documented extents, never a
//! length taken from the EEPROM. Nothing is written to the scanner.

use std::time::Duration;

use pakon_usb::device::Control;

use crate::link::Link;

/// (archive name, EEPROM offset, length) of the four copies.
pub const SECTIONS: [(&str, u16, usize); 4] =
    [("sectionA_primary", 0x000, 398), ("sectionA_backup", 0x400, 398), ("sectionB_primary", 0x800, 36), ("sectionB_backup", 0xA00, 36)];

/// The archive file of a section.
pub fn filename(name: &str) -> String {
    format!("eeprom_0x52_{name}.bin")
}

/// What a read got: the bytes of each section tried, in order, and the
/// error that ended it early.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sections {
    pub data: Vec<(&'static str, Vec<u8>)>,
    pub error: Option<String>,
}

/// One section: an 8-byte header first, then chunks of at most 32 bytes.
/// No retries; the bytes read so far stay in `received` on an error.
fn read_section(link: &mut dyn Link, offset: u16, length: usize, timeout: Duration, received: &mut Vec<u8>) -> Result<(), String> {
    while received.len() < length {
        let size = (if received.is_empty() { 8 } else { 32 }).min(length - received.len());
        let select = Control { request_type: 0x40, request: 0xA4, value: 0x00A5, index: 0x1234, length: 0, data: vec![] };
        link.control(&select, timeout).map_err(|e| format!("EEPROM select: {e}"))?;
        let read = Control { request_type: 0xC0, request: 0xA9, value: offset + received.len() as u16, index: 0x1234, length: size as u16, data: vec![] };
        let data = link.control(&read, timeout).map_err(|e| format!("EEPROM read: {e}"))?;
        received.extend_from_slice(&data);
        if data.len() != size {
            return Err(format!("short/oversized EEPROM read: expected {size}, got {}", data.len()));
        }
    }
    Ok(())
}

/// Read the four sections; stops at the first failed transfer (no more
/// transfers after an uncertain read), keeping what was read.
pub fn read_sections(link: &mut dyn Link, timeout: Duration) -> Sections {
    let mut out = Sections::default();
    for (name, offset, length) in SECTIONS {
        let mut received = Vec::with_capacity(length);
        let r = read_section(link, offset, length, timeout, &mut received);
        out.data.push((name, received));
        if let Err(e) = r {
            out.error = Some(format!("{name}: {e}"));
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScanError;

    /// An EEPROM behind the two OEM requests; records every transfer.
    struct Rom {
        bytes: Vec<u8>,
        selected: bool,
        seen: Vec<(u8, u8, u16, u16, u16)>,
        fail_at: Option<u16>,
    }

    impl Link for Rom {
        fn send_cmd(&mut self, _: &[u8], _: Duration) -> Result<usize, ScanError> {
            panic!("the EEPROM read sends no frames")
        }
        fn recv_cmd(&mut self, _: &mut [u8], _: Duration) -> Result<usize, ScanError> {
            panic!("the EEPROM read reads no replies")
        }
        fn recv_image(&mut self, _: &mut [u8], _: Duration) -> Result<usize, ScanError> {
            panic!("the EEPROM read reads no image")
        }
        fn control(&mut self, x: &Control, _: Duration) -> Result<Vec<u8>, ScanError> {
            self.seen.push((x.request_type, x.request, x.value, x.index, x.length));
            assert!(x.data.is_empty(), "nothing is ever written");
            match (x.request_type, x.request) {
                (0x40, 0xA4) => {
                    assert_eq!((x.value, x.index, x.length), (0x00A5, 0x1234, 0));
                    self.selected = true;
                    Ok(vec![])
                }
                (0xC0, 0xA9) => {
                    assert!(std::mem::take(&mut self.selected), "a select before every read");
                    assert_eq!(x.index, 0x1234);
                    if self.fail_at == Some(x.value) {
                        return Err(ScanError::Timeout);
                    }
                    let a = x.value as usize;
                    Ok(self.bytes[a..a + x.length as usize].to_vec())
                }
                other => panic!("unexpected request {other:02x?}"),
            }
        }
    }

    fn rom() -> Rom {
        Rom { bytes: (0..0xC00u32).map(|i| (i * 7 + i / 256) as u8).collect(), selected: false, seen: vec![], fail_at: None }
    }

    #[test]
    fn reads_the_four_fixed_extents() {
        let mut r = rom();
        let got = read_sections(&mut r, Duration::from_secs(2));
        assert_eq!(got.error, None);
        for ((name, data), (n, off, len)) in got.data.iter().zip(SECTIONS) {
            assert_eq!(*name, n);
            assert_eq!(data[..], r.bytes[off as usize..off as usize + len]);
        }
        // Section A: an 8-byte header, twelve 32-byte chunks, then 6 bytes.
        let reads: Vec<(u16, u16)> = r.seen.iter().filter(|x| x.1 == 0xA9).map(|x| (x.2, x.4)).collect();
        assert_eq!(reads[0], (0, 8));
        assert_eq!(reads[1], (8, 32));
        assert_eq!(reads[13], (8 + 12 * 32, 6));
        assert_eq!(reads[14], (0x400, 8));
        assert_eq!(*reads.last().unwrap(), (0xA00 + 8, 28));
        assert_eq!(r.seen.len(), 2 * reads.len());
    }

    #[test]
    fn stops_at_the_first_failure_and_keeps_the_bytes() {
        let mut r = rom();
        r.fail_at = Some(0x400 + 8);
        let got = read_sections(&mut r, Duration::from_secs(2));
        assert_eq!(got.data.len(), 2, "no transfers after an uncertain read");
        assert_eq!(got.data[1].1.len(), 8);
        assert!(got.error.unwrap().starts_with("sectionA_backup: EEPROM read"));
    }
}
