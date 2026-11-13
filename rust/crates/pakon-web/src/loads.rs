//! Which warm scanners Pakon started itself. The firmware is RAM-only and
//! the same Kodak image whoever loads it, so a warm scanner looks the same
//! whether Pakon, TLX or PakonScan loaded it. What differs is the state the
//! other software leaves in the controllers until the scanner is switched
//! off, which can make our light calibration fail (#82). A firmware load
//! re-enumerates the scanner at a new USB address, so a warm scanner at an
//! address Pakon did not load to was started by something else (or on
//! another computer).

use std::path::PathBuf;
use std::sync::Mutex;

use pakon_usb::device::{self, DevInfo};
use serde_json::{json, Value};

/// Loads kept: enough for a day of power cycles.
const KEEP: usize = 16;

pub struct Loads {
    path: PathBuf,
    lock: Mutex<()>,
}

/// The one warm scanner, if there is exactly one.
pub fn warm_device() -> Option<DevInfo> {
    let warm: Vec<DevInfo> = device::list().ok()?.into_iter().filter(|d| device::is_warm_id(d.vid, d.pid)).collect();
    match <[DevInfo; 1]>::try_from(warm) {
        Ok([d]) => Some(d),
        Err(_) => None,
    }
}

impl Loads {
    pub fn new(path: PathBuf) -> Loads {
        Loads { path, lock: Mutex::new(()) }
    }

    fn read(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["loads"].as_array().cloned())
            .unwrap_or_default()
    }

    /// Note a scanner Pakon just loaded (best effort).
    pub fn record(&self, dev: &DevInfo) {
        let _l = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut loads = self.read();
        loads.retain(|l| !(l["bus"] == json!(dev.bus) && l["address"] == json!(dev.address)));
        loads.push(json!({"bus": dev.bus, "address": dev.address, "at": chrono::Local::now().to_rfc3339()}));
        let skip = loads.len().saturating_sub(KEEP);
        let loads: Vec<Value> = loads.into_iter().skip(skip).collect();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(&json!({"loads": loads})).unwrap_or_default());
    }

    /// Whether Pakon loaded this warm scanner's firmware.
    pub fn ours(&self, dev: &DevInfo) -> bool {
        let _l = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read().iter().any(|l| l["bus"] == json!(dev.bus) && l["address"] == json!(dev.address))
    }

    /// Some(true) when the one warm scanner was started by other software;
    /// None without exactly one warm scanner.
    pub fn foreign(&self) -> Option<bool> {
        warm_device().map(|d| !self.ours(&d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(bus: &str, address: u8) -> DevInfo {
        DevInfo { vid: 0x0f05, pid: 0xf135, bus: bus.into(), address, class: 0, version: 0 }
    }

    #[test]
    fn records_and_matches() {
        let dir = std::env::temp_dir().join(format!("pakon-loads-{}", std::process::id()));
        let l = Loads::new(dir.join("firmware-loads.json"));
        assert!(!l.ours(&dev("8", 31)));
        l.record(&dev("8", 31));
        assert!(l.ours(&dev("8", 31)));
        assert!(!l.ours(&dev("8", 32)), "TLX re-enumerates it at another address");
        assert!(!l.ours(&dev("1", 31)));
        for a in 40..40 + KEEP as u8 + 3 {
            l.record(&dev("8", a));
        }
        assert!(!l.ours(&dev("8", 31)), "old loads age out");
        assert!(l.ours(&dev("8", 40 + KEEP as u8 + 2)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
