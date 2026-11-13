//! Crash reports and usage statistics, sent to a Rybbit instance
//! (https://rybbit.com, open-source analytics). Port of `web/telemetry.py`.
//!
//! Where they go is set by the build, not the code: PAKON_RYBBIT_URL (the
//! instance), PAKON_RYBBIT_SITE_ID and PAKON_RYBBIT_DOMAIN (the Rybbit site
//! and its domain). Release builds carry them (release.env, or the Mac
//! app's Info.plist) and the window app puts them in the environment; a
//! build or checkout without them never sends anything and does not ask.
//!
//! Opt-in: nothing is sent until the user says yes in the first-launch
//! popup, and each kind has its own switch (settings.json "telemetry", the
//! Settings page). Only the app sends (PAKON_VERSION is set); a checkout
//! does not, unless PAKON_TELEMETRY=1. PAKON_TELEMETRY=0 turns it off
//! everywhere.
//!
//! Events go straight to Rybbit's /api/track over HTTPS, one background
//! thread, dropped on any failure. Never sent: file paths (the home folder
//! becomes ~), images, roll names, scanner serial numbers. The user is a
//! random install ID that turning both switches off forgets.

use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::orders::Settings;
use crate::paths::{env, home};

const TIMEOUT: Duration = Duration::from_secs(8);
const PAGES: [&str; 6] = ["home", "scan", "process", "frames", "export", "settings"];

pub struct Telemetry {
    settings: Arc<Settings>,
    endpoint: String,
    site_id: String,
    hostname: String,
    sender: Mutex<Option<SyncSender<Value>>>,
}

fn version() -> String {
    env("PAKON_VERSION").unwrap_or_else(|| "dev".into())
}

/// No home folder, no scanner serials; the tail when it is too long.
pub fn scrub(text: &str, limit: usize) -> String {
    static SERIAL: OnceLock<[Regex; 2]> = OnceLock::new();
    let pats = SERIAL.get_or_init(|| [Regex::new(r"(?i)(serial\s*)\d+").expect("regex"), Regex::new(r"(?i)(F135(?:PLUS)?-)\d+").expect("regex")]);
    let mut text = text.replace(&*home().to_string_lossy(), "~");
    for p in pats {
        text = p.replace_all(&text, "${1}#").into_owned();
    }
    let n = text.chars().count();
    if n <= limit {
        return text;
    }
    format!("…{}", text.chars().skip(n - (limit - 1)).collect::<String>())
}

/// `json.dumps(props)[:limit]`, cut on a character.
fn cut(s: String, limit: usize) -> String {
    s.chars().take(limit).collect()
}

fn command_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn mac_version() -> Option<String> {
    cfg!(target_os = "macos").then(|| command_line("sw_vers", &["-productVersion"])).flatten()
}

/// "Linux Mint 22" from /etc/os-release, on Linux.
fn linux_release() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let text = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    Some(line.trim().trim_matches('"').to_string())
}

fn windows_version() -> Option<String> {
    // "Microsoft Windows [Version 10.0.22631.4317]" -> "10.0.22631.4317"
    let ver = cfg!(windows).then(|| command_line("cmd", &["/C", "ver"])).flatten()?;
    let inner = ver.rsplit("Version ").next()?.trim_end_matches(']').trim();
    Some(inner.to_string())
}

/// Rybbit reads the OS from the user agent: a Mac's, Linux's (X11, as
/// WebKitGTK sends it, with the machine's architecture) or Windows' (10 and
/// 11 both say NT 10.0).
fn user_agent() -> String {
    let arch = std::env::consts::ARCH;
    let os = if cfg!(target_os = "linux") {
        format!("X11; Linux {arch}")
    } else if cfg!(windows) {
        format!("Windows NT 10.0; {}", if arch == "aarch64" { "ARM64" } else { "Win64; x64" })
    } else {
        format!("Macintosh; Intel Mac OS X {}", mac_version().unwrap_or_else(|| "10.15.7".into()).replace('.', "_"))
    };
    format!("Mozilla/5.0 ({os}) AppleWebKit/605.1.15 (KHTML, like Gecko) Pakon/{}", version())
}

impl Telemetry {
    pub fn new(settings: Arc<Settings>) -> Telemetry {
        let var = |k: &str| env(k).map(|v| v.trim().to_string()).unwrap_or_default();
        Telemetry {
            settings,
            endpoint: format!("{}/api/track", var("PAKON_RYBBIT_URL").trim_end_matches('/')),
            site_id: var("PAKON_RYBBIT_SITE_ID"),
            hostname: var("PAKON_RYBBIT_DOMAIN"),
            sender: Mutex::new(None),
        }
    }

    fn stored(&self) -> Map<String, Value> {
        match self.settings.setting("telemetry") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    /// The build says where reports go (all three PAKON_RYBBIT_* set).
    pub fn configured(&self) -> bool {
        self.endpoint != "/api/track" && !self.site_id.is_empty() && !self.hostname.is_empty()
    }

    pub fn available(&self) -> bool {
        if !self.configured() {
            return false;
        }
        match std::env::var("PAKON_TELEMETRY") {
            Ok(flag) => flag == "1",
            Err(_) => env("PAKON_VERSION").is_some(),
        }
    }

    /// `{"available", "asked", "usage", "crashes"}` for the page.
    pub fn consent(&self) -> Value {
        let s = self.stored();
        let on = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        json!({"available": self.available(), "asked": s.contains_key("usage"), "usage": on("usage"), "crashes": on("crashes")})
    }

    pub fn set_consent(&self, usage: bool, crashes: bool) -> Value {
        let mut s = self.stored();
        s.insert("usage".into(), json!(usage));
        s.insert("crashes".into(), json!(crashes));
        if !(usage || crashes) {
            s.remove("install_id"); // forget this install
        } else if s.get("install_id").and_then(Value::as_str).is_none_or(str::is_empty) {
            s.insert("install_id".into(), json!(uuid::Uuid::new_v4().simple().to_string()));
        }
        if let Err(e) = self.settings.set_setting("telemetry", Value::Object(s)) {
            println!("telemetry choice not saved: {e}");
        }
        self.consent()
    }

    fn allowed(&self, kind: &str) -> bool {
        self.available() && self.stored().get(kind).and_then(Value::as_bool).unwrap_or(false)
    }

    fn send(&self, kind: &str, payload: Value) {
        if !self.allowed(kind) {
            return;
        }
        let mut sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        let tx = sender.get_or_insert_with(|| {
            let (tx, rx) = sync_channel::<Value>(200);
            let endpoint = self.endpoint.clone();
            std::thread::spawn(move || {
                let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
                for body in rx {
                    let ua = body["user_agent"].as_str().unwrap_or("").to_string();
                    let r = agent.post(&endpoint).set("Content-Type", "application/json").set("User-Agent", &ua).send_string(&body.to_string());
                    if let Err(e) = r {
                        println!("telemetry: not sent ({e})"); // offline etc.: drop it
                    }
                }
            });
            tx
        });
        let mut body = json!({
            "site_id": self.site_id,
            "hostname": self.hostname,
            "pathname": "/",
            "user_id": self.stored().get("install_id"),
            "user_agent": user_agent(),
            "language": "en",
        });
        body.as_object_mut().expect("an object").extend(payload.as_object().cloned().unwrap_or_default());
        let _ = tx.try_send(body); // full: dropped
    }

    /// A page of the app was opened.
    pub fn page(&self, name: &str) {
        if PAGES.contains(&name) {
            let title = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
            self.send("usage", json!({"type": "pageview", "pathname": format!("/{name}"), "page_title": title}));
        }
    }

    /// A usage event; `props` must be small and anonymous (no paths).
    pub fn event(&self, name: &str, page: &str, props: Value) {
        let mut props = props.as_object().cloned().unwrap_or_default();
        props.insert("version".into(), json!(version()));
        self.send(
            "usage",
            json!({
                "type": "custom_event",
                "event_name": name,
                "pathname": if page.is_empty() { "/".to_string() } else { format!("/{page}") },
                "properties": cut(Value::Object(props).to_string(), 2048),
            }),
        );
    }

    /// A crash or failure report: Rybbit keeps 500 chars of message and
    /// 2000 of stack (here a backtrace, or the tail of a scanner log).
    pub fn error(&self, kind: &str, message: &str, stack: &str, page: &str) {
        let props = json!({"message": scrub(message, 500), "stack": scrub(stack, 2000), "version": version()});
        self.send(
            "crashes",
            json!({
                "type": "error",
                "event_name": cut(kind.to_string(), 256),
                "pathname": if page.is_empty() { "/".to_string() } else { format!("/{page}") },
                "properties": cut(props.to_string(), 4096),
            }),
        );
    }

    /// Report panics and send app_open. Call once at startup.
    pub fn start(self: &Arc<Self>) {
        if !self.available() {
            return;
        }
        let me = self.clone();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let trace = std::backtrace::Backtrace::force_capture().to_string();
            me.error("Panic", &info.to_string(), &trace, "");
            prev(info);
        }));
        self.event("app_open", "home", json!({"macos": mac_version(), "linux": linux_release(), "windows": windows_version()}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_hides_the_home_folder_and_serials() {
        let h = home().to_string_lossy().into_owned();
        assert_eq!(scrub(&format!("{h}/pakon/000001/scan.raw"), 500), "~/pakon/000001/scan.raw");
        assert_eq!(scrub("EEPROM read from F-135 serial 3054", 500), "EEPROM read from F-135 serial #");
        assert_eq!(scrub("backups/eeprom/F135-3054-20261109 and f135plus-16402-x", 500), "backups/eeprom/F135-#-20261109 and f135plus-#-x");
        assert_eq!(scrub("abcdefghij", 5), "…ghij");
    }

    #[test]
    fn nothing_is_sent_without_a_configured_build() {
        let d = std::env::temp_dir().join(format!("pakon-web-telemetry-{}", std::process::id()));
        let t = Telemetry { settings: Arc::new(Settings::new(d.join("settings.json"))), endpoint: "/api/track".into(), site_id: String::new(), hostname: String::new(), sender: Mutex::new(None) };
        assert!(!t.configured() && !t.available());
        t.event("x", "home", json!({}));
        assert!(t.sender.lock().unwrap().is_none());
        // the choice is still kept, with an install id only while a switch is on
        assert_eq!(t.set_consent(true, false), json!({"available": false, "asked": true, "usage": true, "crashes": false}));
        assert!(t.stored()["install_id"].as_str().is_some_and(|s| s.len() == 32));
        t.set_consent(false, false);
        assert!(!t.stored().contains_key("install_id"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
