//! The connected scanner's EEPROM archive, read automatically (port of
//! `web/eeprom.py` and the archive writer of `tools/pakon_eeprom.py`).
//!
//! The replay-free F-135 scan, the eject and the OEM colour path all need
//! the unit's EEPROM archive. The service reads the EEPROM itself the first
//! time it sees a warm scanner on a USB connection (app start, after a
//! firmware load, or before a scan / eject), with the read-only OEM requests
//! of `pakon_scan::eeprom_read`. A new archive is kept only when none exists
//! yet for that serial; otherwise the fresh read is discarded and the
//! existing archive is used.
//!
//! Callers run `sync` between device operations, never during a scan.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use pakon_proto::eeprom::crc32;
use pakon_scan::eeprom_read::{filename, read_sections, Sections, SECTIONS};
use pakon_usb::device::{self, Device};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::paths::env;

#[derive(Debug, Clone, Default)]
struct Session {
    /// The USB connection the last read was made on (bus, address); a power
    /// cycle or firmware load re-enumerates.
    key: Option<(String, u8)>,
    serial: Option<u64>,
    model: Option<String>,
    archive: Option<PathBuf>,
    created: bool,
    error: Option<String>,
}

pub struct Eeprom {
    /// `$PAKON_DATA/eeprom`, else `<root>/backups/eeprom`.
    root: PathBuf,
    session: Mutex<Session>,
    /// One read at a time.
    reading: Mutex<()>,
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().expect("4 bytes"))
}

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().expect("2 bytes"))
}

/// Length and CRC of one copy (`inspect_section`).
fn inspect(data: &[u8], expected: usize) -> Map<String, Value> {
    let mut r = Map::new();
    r.insert("bytes_read".into(), json!(data.len()));
    r.insert("expected_length".into(), json!(expected));
    r.insert("declared_length".into(), Value::Null);
    r.insert("stored_crc32".into(), Value::Null);
    r.insert("computed_crc32".into(), Value::Null);
    r.insert("valid".into(), json!(false));
    if data.len() < 8 {
        r.insert("error".into(), json!("missing header"));
        return r;
    }
    let (length, stored) = (u32_at(data, 0), u32_at(data, 4));
    r.insert("declared_length".into(), json!(length));
    r.insert("stored_crc32".into(), json!(format!("{stored:08x}")));
    if data.len() != expected || length as usize != expected {
        r.insert("error".into(), json!("unexpected section length"));
        return r;
    }
    let actual = crc32(&data[8..]);
    r.insert("computed_crc32".into(), json!(format!("{actual:08x}")));
    r.insert("valid".into(), json!(stored == actual));
    if stored != actual {
        r.insert("error".into(), json!("CRC mismatch"));
    }
    r
}

fn model_name(scanner_type: u32) -> Option<&'static str> {
    match scanner_type {
        1350 => Some("F-135"),
        1351 => Some("F-135+"),
        _ => None,
    }
}

/// The archive's report.json (`decode_sections`): what was read, which
/// copies validate, and the values of the selected ones. Only CRC-valid
/// copies are decoded.
pub fn report(raw: &Sections) -> Value {
    let get = |name: &str| raw.data.iter().find(|(n, _)| *n == name).map(|(_, d)| d.as_slice());
    let mut sections = Map::new();
    let mut warnings: Vec<String> = vec![];
    for (name, offset, length) in SECTIONS {
        let mut s = Map::new();
        s.insert("offset".into(), json!(offset));
        match get(name) {
            None => {
                s.insert("file".into(), Value::Null);
                s.insert("sha256".into(), Value::Null);
                s.insert("missing".into(), json!(true));
                s.extend(inspect(&[], length));
                s.insert("error".into(), json!("section not present"));
            }
            Some(data) => {
                s.insert("file".into(), json!(filename(name)));
                s.insert("sha256".into(), json!(sha256_hex(data)));
                s.insert("missing".into(), json!(false));
                s.extend(inspect(data, length));
            }
        }
        sections.insert(name.into(), Value::Object(s));
    }
    let valid = |name: &str| sections[name]["valid"].as_bool().unwrap_or(false);
    let (mut selected, mut copies_equal) = (Map::new(), Map::new());
    let mut chosen: [Option<&[u8]>; 2] = [None, None];
    for (i, section) in ["A", "B"].into_iter().enumerate() {
        let (primary, backup) = (format!("section{section}_primary"), format!("section{section}_backup"));
        let (pvalid, bvalid) = (valid(&primary), valid(&backup));
        let whole = |n: &str| get(n).is_some_and(|d| SECTIONS.iter().any(|s| s.0 == n && s.2 == d.len()));
        let equal = (whole(&primary) && whole(&backup)).then(|| get(&primary) == get(&backup));
        copies_equal.insert(section.into(), json!(equal));
        let pick = if pvalid { Some(primary.clone()) } else { bvalid.then(|| backup.clone()) };
        selected.insert(section.into(), json!(pick));
        chosen[i] = pick.as_deref().and_then(get);
        if !pvalid && !bvalid {
            warnings.push(format!("Section {section}: neither copy validates"));
        } else if !pvalid || !bvalid {
            let invalid = if pvalid { &backup } else { &primary };
            warnings.push(format!("Section {section}: {invalid} invalid; {} selected", pick.as_deref().unwrap_or("")));
        } else if equal != Some(true) {
            warnings.push(format!("Section {section}: valid copies differ; primary selected"));
        }
    }
    let mut decoded = Map::new();
    let mut semantic_ok = true;
    if let Some(data) = chosen[0] {
        let (hw, ty, serial) = (u32_at(data, 8), u32_at(data, 12), u32_at(data, 16));
        decoded.insert("hardware_version".into(), json!(hw));
        decoded.insert("scanner_type".into(), json!(ty));
        decoded.insert("model".into(), json!(model_name(ty).unwrap_or("unknown")));
        decoded.insert("serial".into(), json!(serial));
        if model_name(ty).is_none() {
            semantic_ok = false;
            warnings.push("Unknown scanner type; no model profile selected".into());
        }
        let mut bases = Map::new();
        for (base, at) in [(4, 0x14), (8, 0x1A), (16, 0x20)] {
            bases.insert(base.to_string(), json!({"offset": u16_at(data, at), "motor_speed": u16_at(data, at + 2), "motor_speed_ir": u16_at(data, at + 4)}));
        }
        decoded.insert("resolution_bases".into(), Value::Object(bases));
        for (name, at) in [("negative_matrix", 0x26), ("positive_matrix", 0x9E)] {
            let values: Vec<f32> = (0..30).map(|i| f32::from_bits(u32_at(data, at + 4 * i))).collect();
            if !values.iter().all(|v| v.is_finite()) {
                semantic_ok = false;
                warnings.push(format!("{name}: nonfinite coefficients"));
            }
            // The exact bytes stay in the archive; JSON null stands for NaN / Inf.
            let rows: Vec<Vec<Value>> = values.chunks(10).map(|r| r.iter().map(|&v| json!(v as f64)).collect()).collect();
            decoded.insert(name.into(), json!(rows));
        }
    }
    if let Some(data) = chosen[1] {
        let mut adj = Map::new();
        for (base, at) in [(4, 8), (8, 16), (16, 24)] {
            adj.insert(base.to_string(), json!({"normal": u16_at(data, at), "drag": u16_at(data, at + 2), "ir": u16_at(data, at + 4), "drag_ir": u16_at(data, at + 6)}));
        }
        decoded.insert("motor_adjustments".into(), Value::Object(adj));
        decoded.insert("section_b_unknown_u32".into(), json!(u32_at(data, 32)));
    }
    let all_valid = SECTIONS.iter().all(|s| valid(s.0));
    json!({
        "schema_version": 1,
        "sections": sections,
        "selected": selected,
        "copies_equal": copies_equal,
        "decoded": decoded,
        "warnings": warnings,
        "decodable": chosen.iter().all(Option::is_some) && semantic_ok,
        "all_copies_valid": all_valid,
    })
}

/// Write a new archive into `dir` (which must not exist): the section
/// files, report.json and SHA256SUMS, as `pakon_eeprom.py backup` does.
/// Returns the report.
pub fn write_archive(dir: &Path, raw: &Sections, usb: Value) -> Result<Value, String> {
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir(dir).map_err(io)?;
    for (name, data) in &raw.data {
        std::fs::write(dir.join(filename(name)), data).map_err(io)?;
    }
    let mut rep = report(raw);
    let extra = json!({
        "created_utc": chrono::Utc::now().to_rfc3339(),
        "usb": usb,
        "acquisition_complete": raw.error.is_none(),
        "acquisition_error": raw.error,
        "scope": "Four documented sections of EEPROM 0x52, not the entire chip",
        "tool": "pakon-web (pakon_eeprom.py schema 1)",
    });
    rep.as_object_mut().expect("an object").extend(extra.as_object().expect("an object").clone());
    let text = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())? + "\n";
    std::fs::write(dir.join("report.json"), text).map_err(io)?;
    let mut names: Vec<String> = raw.data.iter().map(|(n, _)| filename(n)).collect();
    names.push("report.json".into());
    names.sort();
    let mut sums = String::new();
    for name in names {
        sums += &format!("{}  {name}\n", sha256_hex(&std::fs::read(dir.join(&name)).map_err(io)?));
    }
    std::fs::write(dir.join("SHA256SUMS"), sums).map_err(io)?;
    Ok(rep)
}

impl Eeprom {
    pub fn new(root: PathBuf) -> Eeprom {
        Eeprom { root, session: Mutex::new(Session::default()), reading: Mutex::new(()) }
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Serial of the scanner the last `sync` read.
    pub fn connected_serial(&self) -> Option<u64> {
        self.session().serial
    }

    pub fn status(&self) -> Value {
        let s = self.session();
        json!({
            "serial": s.serial,
            "model": s.model,
            "archive": s.archive.as_ref().and_then(|a| a.file_name()).map(|n| n.to_string_lossy().into_owned()),
            "created": s.created,
            "error": s.error,
        })
    }

    /// Read the EEPROM of the one warm scanner if it has not been read on
    /// this USB connection yet, and make sure an archive exists for it.
    /// Does nothing without a warm scanner or with PAKON_EEPROM_DIR set.
    /// Takes the device for the read.
    pub fn sync(&self) -> Value {
        if env("PAKON_EEPROM_DIR").is_some() {
            return self.status();
        }
        let _reading = self.reading.lock().unwrap_or_else(|e| e.into_inner());
        let warm: Vec<device::DevInfo> = match device::list() {
            Ok(all) => all.into_iter().filter(|d| device::is_warm_id(d.vid, d.pid)).collect(),
            Err(e) => {
                self.session().error = Some(format!("USB detection failed: {e}"));
                return self.status();
            }
        };
        let [dev] = warm.as_slice() else { return self.status() };
        let key = (dev.bus.clone(), dev.address);
        if self.session().key.as_ref() == Some(&key) {
            return self.status();
        }

        let now = chrono::Local::now();
        let tmp = self.root.join(format!(".reading-{}-{}", std::process::id(), now.format("%Y%m%d%H%M%S%6f")));
        let read = || -> Result<Value, String> {
            std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
            let raw = {
                let _device = crate::device_lock();
                let mut d = Device::open().map_err(|e| e.to_string())?;
                read_sections(&mut d, Duration::from_millis(2000))
            };
            let usb = json!({"vid": "0f05", "pid": "f135", "bus": dev.bus, "address": dev.address, "bcd_device": dev.version});
            write_archive(&tmp, &raw, usb)
        };
        let rep = match read() {
            Ok(rep) => rep,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                self.session().error = Some(format!("EEPROM read failed: {e}"));
                return self.status();
            }
        };
        let serial = rep["decoded"]["serial"].as_u64();
        let model = rep["decoded"]["model"].as_str().map(str::to_string);
        let complete = rep["acquisition_complete"].as_bool().unwrap_or(false) && rep["decodable"].as_bool().unwrap_or(false);
        let Some(serial) = serial.filter(|_| complete) else {
            let _ = std::fs::remove_dir_all(&tmp);
            let why = match rep["acquisition_error"].as_str() {
                Some(e) => e.to_string(),
                None => rep["warnings"].as_array().map(|w| w.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default(),
            };
            let mut s = self.session();
            s.key = Some(key);
            s.error = Some(format!("EEPROM read incomplete or not decodable: {why}"));
            drop(s);
            return self.status();
        };

        let existing = pakon_roll::archives(&self.root).into_iter().find(|d| pakon_roll::archive_serial(d) == Some(serial));
        let (archive, created) = match existing {
            Some(a) => {
                let _ = std::fs::remove_dir_all(&tmp);
                (a, false)
            }
            None => {
                let tag = model.as_deref().unwrap_or("unknown").replace('-', "").replace('+', "PLUS");
                let archive = self.root.join(format!("{tag}-{serial}-{}", now.format("%Y%m%d-%H%M%S")));
                if let Err(e) = std::fs::rename(&tmp, &archive) {
                    let _ = std::fs::remove_dir_all(&tmp);
                    self.session().error = Some(format!("EEPROM read failed: {e}"));
                    return self.status();
                }
                (archive, true)
            }
        };
        *self.session() = Session { key: Some(key), serial: Some(serial), model, archive: Some(archive), created, error: None };
        self.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid copy: length, CRC, payload.
    fn copy(len: usize, fill: impl Fn(&mut [u8])) -> Vec<u8> {
        let mut d = vec![0u8; len];
        fill(&mut d);
        d[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        let crc = crc32(&d[8..]);
        d[4..8].copy_from_slice(&crc.to_le_bytes());
        d
    }

    fn unit() -> Sections {
        let a = copy(398, |d| {
            d[8..12].copy_from_slice(&3u32.to_le_bytes());
            d[12..16].copy_from_slice(&1350u32.to_le_bytes());
            d[16..20].copy_from_slice(&3054u32.to_le_bytes());
            d[0x20..0x22].copy_from_slice(&77u16.to_le_bytes());
            d[0x26..0x2A].copy_from_slice(&0.5f32.to_le_bytes());
        });
        let b = copy(36, |d| d[24..26].copy_from_slice(&9u16.to_le_bytes()));
        Sections { data: vec![("sectionA_primary", a.clone()), ("sectionA_backup", a), ("sectionB_primary", b.clone()), ("sectionB_backup", b)], error: None }
    }

    #[test]
    fn report_of_a_good_read() {
        let r = report(&unit());
        assert_eq!(r["decodable"], true);
        assert_eq!(r["all_copies_valid"], true);
        assert_eq!(r["decoded"]["serial"], 3054);
        assert_eq!(r["decoded"]["model"], "F-135");
        assert_eq!(r["decoded"]["resolution_bases"]["16"]["offset"], 77);
        assert_eq!(r["decoded"]["negative_matrix"][0][0], 0.5);
        assert_eq!(r["decoded"]["motor_adjustments"]["16"]["normal"], 9);
        assert_eq!(r["selected"]["A"], "sectionA_primary");
        assert_eq!(r["copies_equal"]["B"], true);
        assert_eq!(r["warnings"].as_array().unwrap().len(), 0);
        assert_eq!(r["sections"]["sectionA_primary"]["offset"], 0);
        assert_eq!(r["sections"]["sectionB_backup"]["offset"], 0xA00);
    }

    #[test]
    fn a_bad_primary_selects_the_backup() {
        let mut u = unit();
        u.data[0].1[100] ^= 0xFF;
        let r = report(&u);
        assert_eq!(r["decodable"], true);
        assert_eq!(r["all_copies_valid"], false);
        assert_eq!(r["selected"]["A"], "sectionA_backup");
        assert_eq!(r["sections"]["sectionA_primary"]["error"], "CRC mismatch");
        assert_eq!(r["warnings"][0], "Section A: sectionA_primary invalid; sectionA_backup selected");
    }

    #[test]
    fn a_partial_read_is_not_decodable() {
        let mut u = unit();
        u.data.truncate(2);
        u.data[1].1.truncate(8);
        u.error = Some("sectionA_backup: EEPROM read: timeout".into());
        let r = report(&u);
        assert_eq!(r["decodable"], false);
        assert_eq!(r["sections"]["sectionB_primary"]["missing"], true);
        assert_eq!(r["sections"]["sectionA_backup"]["error"], "unexpected section length");
        assert_eq!(r["copies_equal"]["A"], Value::Null);
    }

    #[test]
    fn the_archive_reads_back_with_the_roll_code() {
        let dir = std::env::temp_dir().join(format!("pakon-web-eeprom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("F135-3054-test");
        let rep = write_archive(&a, &unit(), json!({})).unwrap();
        assert_eq!(rep["acquisition_complete"], true);
        assert_eq!(pakon_roll::archives(&dir), vec![a.clone()]);
        assert_eq!(pakon_roll::archive_serial(&a), Some(3054));
        assert!(pakon_scan::session::load_eeprom_archive(&a).is_some_and(|e| e.serial == 3054));
        let sums = std::fs::read_to_string(a.join("SHA256SUMS")).unwrap();
        assert_eq!(sums.lines().count(), 5);
        assert!(write_archive(&a, &unit(), json!({})).is_err(), "an archive is never overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
