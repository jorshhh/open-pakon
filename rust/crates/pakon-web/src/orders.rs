//! The work folder and the lab-style order counter (port of `web/orders.py`).
//!
//! Every scan is an order: it gets the next order ID and its own folder
//! under the work folder, and the raw is named for the time the scan
//! started: `~/pakon/000042/2026-10-11-14-30-scan.raw`.
//!
//! The work folder (default `~/pakon`) and the next order ID are kept in
//! `settings.json`. An ID is used up when a scan starts, whether or not the
//! scan succeeds. The counter also skips past any order folder already in
//! the work folder, so two installs sharing one work folder never write
//! into the same order.
//!
//! The operator can instead give the scan an order of their own, e.g. the
//! number on a lab's order sticker (#95). It names the folder as typed and
//! leaves the counter alone; a name already taken gets `-2`, `-3`, ...

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::paths::{expand_user, home, resolve};

const ID_DIGITS: usize = 6;
/// Longest custom order name.
const ORDER_MAX: usize = 64;

/// A custom order name as typed, trimmed; `None` when blank. Letters,
/// digits, space, `-`, `_` and `.`, not starting with `.` or ending in `.`
/// or space, so it is one plain folder name on every platform.
pub fn custom_order(s: &str) -> Result<Option<String>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > ORDER_MAX {
        return Err(format!("order number is longer than {ORDER_MAX} characters"));
    }
    if let Some(c) = s.chars().find(|c| !(c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))) {
        return Err(format!("order number can't contain \"{c}\" (use letters, digits, spaces, - _ .)"));
    }
    if s.starts_with('.') || s.ends_with('.') {
        return Err("order number can't start or end with \".\"".into());
    }
    // Windows refuses these as folder names, with or without an extension
    let stem = s.split('.').next().unwrap_or(s).trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("\"{s}\" can't be a folder name on Windows"));
    }
    Ok(Some(s.to_string()))
}

pub struct Settings {
    path: PathBuf,
    lock: Mutex<()>,
}

fn io<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl Settings {
    pub fn new(path: PathBuf) -> Settings {
        Settings { path, lock: Mutex::new(()) }
    }

    fn load(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, data: &Map<String, Value>) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(data).map_err(io)?).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn work_dir_of(data: &Map<String, Value>) -> PathBuf {
        match data.get("work_dir").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(d) => expand_user(d),
            None => home().join("pakon"),
        }
    }

    pub fn work_dir(&self) -> PathBuf {
        Self::work_dir_of(&self.load())
    }

    pub fn set_work_dir(&self, path: &Path) -> Result<(), String> {
        let _g = self.guard();
        let mut data = self.load();
        data.insert("work_dir".into(), Value::String(path.to_string_lossy().into_owned()));
        self.save(&data)
    }

    /// Where a roll's saved exports go: the raw's own folder when that is a
    /// folder inside the work folder (a scan's order folder), else a folder
    /// named after the raw in the work folder. Each format gets a subfolder
    /// there (jpeg/, tiff/, raw/).
    pub fn export_dir(&self, raw: &Path) -> PathBuf {
        let base = resolve(&self.work_dir());
        let raw = resolve(raw);
        let folder = raw.parent().unwrap_or(Path::new("")).to_path_buf();
        if folder != base && folder.starts_with(&base) {
            return folder;
        }
        base.join(raw.file_stem().unwrap_or_default())
    }

    /// The stored counter, moved past the highest order folder in `base`.
    fn next_id(data: &Map<String, Value>, base: &Path) -> u64 {
        let n = data.get("next_order").and_then(Value::as_u64).unwrap_or(1);
        let taken = std::fs::read_dir(base)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        // only our own IDs: a long custom order number
                        // (#95) must not throw the counter far ahead
                        (name.len() == ID_DIGITS && name.bytes().all(|b| b.is_ascii_digit())).then(|| name.parse::<u64>().ok()).flatten()
                    })
                    .max()
            })
            .ok()
            .flatten();
        n.max(taken.map_or(0, |t| t + 1))
    }

    /// The ID the next scan will get (for display; does not use it up).
    pub fn next_order(&self) -> String {
        let data = self.load();
        format!("{:0ID_DIGITS$}", Self::next_id(&data, &Self::work_dir_of(&data)))
    }

    /// Create the order folder in the work folder and return (id, raw
    /// path): `custom` (from [`custom_order`]) as given, with `-2`, `-3`, ...
    /// when taken; else, or when `custom` is the next ID itself (the page
    /// prefills it), the next ID, used up.
    pub fn new_order(&self, custom: Option<&str>) -> Result<(String, PathBuf), String> {
        let stamp = chrono::Local::now().format("%Y-%m-%d-%H-%M");
        let _g = self.guard();
        let mut data = self.load();
        let base = Self::work_dir_of(&data);
        std::fs::create_dir_all(&base).map_err(io)?;
        let next = Self::next_id(&data, &base);
        if let Some(name) = custom.filter(|&c| c != format!("{next:0ID_DIGITS$}")) {
            for k in 1u32.. {
                let order = if k == 1 { name.to_string() } else { format!("{name}-{k}") };
                let folder = base.join(&order);
                match std::fs::create_dir(&folder) {
                    Ok(()) => return Ok((order, folder.join(format!("{stamp}-scan.raw")))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(io(e)),
                }
            }
        }
        let n = next;
        let order = format!("{n:0ID_DIGITS$}");
        let folder = base.join(&order);
        std::fs::create_dir(&folder).map_err(io)?;
        data.insert("next_order".into(), Value::from(n + 1));
        self.save(&data)?;
        Ok((order, folder.join(format!("{stamp}-scan.raw"))))
    }

    /// Another module's value in settings.json (the telemetry choice).
    pub fn setting(&self, key: &str) -> Option<Value> {
        self.load().remove(key)
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        let _g = self.guard();
        let mut data = self.load();
        data.insert(key.into(), value);
        self.save(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pakon-web-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        resolve(&d)
    }

    #[test]
    fn orders_count_up_and_skip_taken_folders() {
        let d = temp("orders");
        let s = Settings::new(d.join("data").join("settings.json"));
        let work = d.join("work");
        std::fs::create_dir_all(work.join("000007")).unwrap();
        std::fs::create_dir_all(work.join("notes")).unwrap();
        s.set_work_dir(&work).unwrap();
        assert_eq!(s.work_dir(), work);
        assert_eq!(s.next_order(), "000008");
        let (id, raw) = s.new_order(None).unwrap();
        assert_eq!(id, "000008");
        assert_eq!(raw.parent().unwrap(), work.join("000008"));
        assert!(raw.file_name().unwrap().to_string_lossy().ends_with("-scan.raw"));
        assert!(work.join("000008").is_dir());
        assert_eq!(s.next_order(), "000009");
        // the stored counter wins when it is ahead of the folders
        s.set_setting("next_order", Value::from(40)).unwrap();
        assert_eq!(s.next_order(), "000040");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn custom_orders_name_the_folder_and_leave_the_counter() {
        let d = temp("custom");
        let s = Settings::new(d.join("settings.json"));
        let work = d.join("work");
        s.set_work_dir(&work).unwrap();
        assert_eq!(s.next_order(), "000001");
        // the prefilled next number, sent back unedited, uses the counter
        assert_eq!(s.new_order(Some("000001")).unwrap().0, "000001");
        assert_eq!(s.next_order(), "000002");
        let (id, raw) = s.new_order(Some("LAB 4471-A")).unwrap();
        assert_eq!(id, "LAB 4471-A");
        assert_eq!(raw.parent().unwrap(), work.join("LAB 4471-A"));
        // the same sticker again (a second strip): its own folder
        assert_eq!(s.new_order(Some("LAB 4471-A")).unwrap().0, "LAB 4471-A-2");
        assert_eq!(s.new_order(Some("LAB 4471-A")).unwrap().0, "LAB 4471-A-3");
        // a long numeric sticker does not move the counter; a 6-digit one does
        s.new_order(Some("123456789")).unwrap();
        assert_eq!(s.next_order(), "000002");
        assert_eq!(s.new_order(None).unwrap().0, "000002");
        s.new_order(Some("000050")).unwrap();
        assert_eq!(s.next_order(), "000051");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn custom_order_names() {
        assert_eq!(custom_order("  "), Ok(None));
        assert_eq!(custom_order(" A-12_3.4 "), Ok(Some("A-12_3.4".into())));
        assert_eq!(custom_order("Pedido 7"), Ok(Some("Pedido 7".into())));
        assert_eq!(custom_order("CONTACT"), Ok(Some("CONTACT".into())));
        assert_eq!(custom_order("COM"), Ok(Some("COM".into())));
        for bad in ["../x", "a/b", "a\\b", "c:d", ".hidden", "trail.", "a*b", "a?b", "<a>", "a|b", "a\"b", "con", "NUL.txt", "com1", "Lpt9"] {
            assert!(custom_order(bad).is_err(), "{bad}");
        }
        assert!(custom_order(&"9".repeat(65)).is_err());
        assert!(custom_order(&"9".repeat(64)).is_ok());
    }

    #[test]
    fn export_dir_like_python() {
        let d = temp("export");
        let s = Settings::new(d.join("settings.json"));
        let work = d.join("work");
        std::fs::create_dir_all(work.join("000001")).unwrap();
        std::fs::create_dir_all(d.join("elsewhere")).unwrap();
        s.set_work_dir(&work).unwrap();
        // a scan's order folder
        assert_eq!(s.export_dir(&work.join("000001").join("a-scan.raw")), work.join("000001"));
        // a raw from somewhere else, or loose in the work folder
        assert_eq!(s.export_dir(&d.join("elsewhere").join("roll.raw")), work.join("roll"));
        assert_eq!(s.export_dir(&work.join("loose.raw")), work.join("loose"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn settings_keep_other_keys() {
        let d = temp("settings");
        let s = Settings::new(d.join("settings.json"));
        s.set_setting("telemetry", serde_json::json!({"usage": true})).unwrap();
        s.set_work_dir(&d).unwrap();
        assert_eq!(s.setting("telemetry"), Some(serde_json::json!({"usage": true})));
        assert_eq!(s.setting("missing"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
