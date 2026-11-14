//! Status, the work folder, the host folder browser, reveal-in-Finder, the
//! window and update routes, and the telemetry routes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::paths::{expand_user, home, resolve};
use crate::scanner::scanner_state;
use crate::{error, fields, missing, update, App};

/// On Windows, the level above the drive roots: the drives, to browse to
/// another one (D:, a mapped network drive) from C:\.
const DRIVES: &str = "This PC";

fn file_size(p: &Path) -> u64 {
    p.metadata().map(|m| m.len()).unwrap_or(0)
}

pub async fn api_status(State(app): State<Arc<App>>) -> Response {
    let (state, state_error) = tokio::task::spawn_blocking(scanner_state).await.unwrap_or(("unknown", None));
    let scan_path = app.roll().scan_path.clone();
    let (frame_count, processing) = {
        let roll = app.roll();
        (roll.frames.len(), roll.processing)
    };
    let update = app.update_status();
    if let Some(latest) = update.as_ref().and_then(|u| u["latest"].as_str()) {
        // once per run and version
        if app.update_seen.lock().unwrap_or_else(|e| e.into_inner()).insert(latest.to_string()) {
            app.telemetry.event("update_shown", "", json!({"latest": latest}));
        }
    }
    Json(json!({
        "state": state,
        "state_error": state_error,
        "scan_file": scan_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()),
        "scan_bytes": scan_path.as_deref().map_or(0, file_size),
        "work_dir": app.settings.work_dir().to_string_lossy(),
        "next_order": app.settings.next_order(),
        "frame_count": frame_count,
        "processing": processing,
        "eeprom": app.eeprom_dir().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())),
        "eeprom_unit": app.eeprom.status(),
        "firmware_hex": app.paths.fw_hex_available(),
        "update": update,                       // a newer release, or null
        "telemetry": app.telemetry.consent(),
        "version": update::current(),           // the app's, null from a checkout
        "native_picker": app.picker().is_some(), // /api/pick shows the system's dialog
        "logs": log_dir().is_some() && reveal_label().is_some(), // Settings › Show log
        // a warm scanner Pakon did not start (TLX, PakonScan, another computer)
        "foreign_firmware": if state == "warm" { app.loads.foreign() } else { None },
    }))
    .into_response()
}

// ── Troubleshooting: the log folder and a bug report by email ───────────────

/// Where Settings › Report a problem sends its email.
pub const REPORT_TO: &str = "jorsh@jor.sh";

/// The app's log folder (PAKON_LOGS: set by the Mac and Windows app and the
/// Linux launcher); none when the service runs from a checkout.
fn log_dir() -> Option<PathBuf> {
    crate::paths::env("PAKON_LOGS").map(PathBuf::from).filter(|d| d.is_dir())
}

/// Open the log folder in the file manager.
pub async fn api_logs_open() -> Response {
    let Some(dir) = log_dir().filter(|_| reveal_label().is_some()) else {
        return error(StatusCode::NOT_FOUND, "no log folder");
    };
    let log = dir.join("pakon.log");
    if cfg!(target_os = "macos") && log.is_file() {
        spawn("open", &["-R".as_ref(), log.as_os_str()]);
    } else if cfg!(target_os = "macos") {
        spawn("open", &[dir.as_os_str()]);
    } else if cfg!(windows) && log.is_file() {
        let select = format!("/select,{}", log.display());
        spawn("explorer", &[select.as_ref()]);
    } else if cfg!(windows) {
        spawn("explorer", &[dir.as_os_str()]);
    } else {
        spawn("xdg-open", &[dir.as_os_str()]);
    }
    Json(json!({"ok": true})).into_response()
}

/// RFC 3986 percent-encoding for a mailto: query value.
fn mail_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The report email: a short template plus the facts we always ask for.
fn report_mailto(version: &str, os: &str, scanner: &str) -> String {
    let subject = format!("Pakon {version}: problem report");
    let body = format!(
        "What happened:\n\n\nWhat you were doing (scan, process, export), film type and resolution:\n\n\n\
         Please attach pakon.log (Settings > Troubleshooting > Show log).\n\n---\n\
         Pakon {version}\n{os}\nScanner: {scanner}\n"
    );
    format!("mailto:{REPORT_TO}?subject={}&body={}", mail_encode(&subject), mail_encode(&body))
}

/// Open a pre-filled report email in the user's mail app. Returns the
/// mailto: URL too, for the page to open itself where the service cannot.
pub async fn api_report(State(app): State<Arc<App>>) -> Response {
    let (state, _) = tokio::task::spawn_blocking(scanner_state).await.unwrap_or(("unknown", None));
    let unit = app.eeprom.status();
    let scanner = match (unit["model"].as_str(), unit["serial"].as_u64()) {
        (Some(m), Some(s)) => format!("{m} #{s} ({state})"),
        _ => format!("not identified ({state})"),
    };
    let version = update::current().unwrap_or_else(|| "from source".into());
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let url = report_mailto(&version, &os, &scanner);
    let opened = if cfg!(target_os = "macos") {
        spawn("open", &[url.as_ref()]);
        true
    } else if cfg!(windows) {
        spawn("explorer", &[url.as_ref()]);
        true
    } else if reveal_label().is_some() {
        spawn("xdg-open", &[url.as_ref()]);
        true
    } else {
        false
    };
    Json(json!({"opened": opened, "url": url})).into_response()
}

fn writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
        // SAFETY: a valid NUL-terminated path.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(dir).is_ok_and(|m| !m.permissions().readonly())
    }
}

pub async fn api_workdir(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(path) = f.get("path").and_then(Value::as_str) else { return missing("path") };
    let target = expand_user(path);
    if !target.is_dir() {
        return error(StatusCode::BAD_REQUEST, format!("not a directory: {}", target.display()));
    }
    if !writable(&target) {
        return error(StatusCode::BAD_REQUEST, format!("not writable: {}", target.display()));
    }
    if let Err(e) = app.settings.set_work_dir(&resolve(&target)) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({"work_dir": app.settings.work_dir().to_string_lossy(), "next_order": app.settings.next_order()})).into_response()
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    path: Option<String>,
    #[serde(default)]
    raws: Option<String>,
}

fn sorted_names(base: &Path, keep: impl Fn(&Path, &str) -> bool) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(base)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| keep(&base.join(n), n)).collect())
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// List the subfolders of a host folder, for picking the work folder (and
/// its .raw files with raws=true, for opening one). With full paths
/// (dir_paths, file_paths) to browse to; "virtual" for a list that is not a
/// folder (Windows' drives).
pub async fn api_browse(Query(q): Query<BrowseQuery>) -> Response {
    let raws = matches!(q.raws.as_deref(), Some("true" | "1" | "True" | "yes" | "on"));
    if cfg!(windows) && q.path.as_deref() == Some(DRIVES) {
        let drives: Vec<String> = ('A'..='Z').map(|d| format!("{d}:\\")).filter(|d| Path::new(d).is_dir()).collect();
        return Json(json!({"path": DRIVES, "parent": null, "virtual": true, "dirs": drives, "dir_paths": drives, "files": [], "file_paths": []}))
            .into_response();
    }
    let start = q.path.as_deref().filter(|p| !p.is_empty()).map(expand_user).unwrap_or_else(home);
    let base = Some(resolve(&start)).filter(|b| b.is_dir()).unwrap_or_else(|| resolve(&home()));
    let dirs = sorted_names(&base, |p, n| p.is_dir() && !n.starts_with('.'));
    let files = if raws {
        sorted_names(&base, |p, n| p.is_file() && Path::new(n).extension().is_some_and(|e| e.eq_ignore_ascii_case("raw")))
    } else {
        vec![]
    };
    let parent: Value = match base.parent() {
        Some(p) if p != base => json!(p.to_string_lossy()),
        _ if cfg!(windows) => json!(DRIVES),
        _ => Value::Null,
    };
    let full = |names: &[String]| names.iter().map(|n| base.join(n).to_string_lossy().into_owned()).collect::<Vec<_>>();
    Json(json!({
        "path": base.to_string_lossy(), "parent": parent, "virtual": false,
        "dirs": dirs, "dir_paths": full(&dirs), "files": files, "file_paths": full(&files),
    }))
    .into_response()
}

/// Create a new subfolder under `path` on the host.
pub async fn api_mkdir(body: Bytes) -> Response {
    let f = fields(&body);
    let (Some(path), Some(name)) = (f.get("path").and_then(Value::as_str), f.get("name").and_then(Value::as_str)) else {
        return missing("path, name");
    };
    let base = expand_user(path);
    if !base.is_dir() {
        return error(StatusCode::BAD_REQUEST, format!("not a directory: {}", base.display()));
    }
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return error(StatusCode::BAD_REQUEST, "invalid folder name");
    }
    let new = base.join(name);
    match std::fs::create_dir(&new) {
        Ok(()) => Json(json!({"path": new.to_string_lossy()})).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => error(StatusCode::BAD_REQUEST, "folder already exists"),
        Err(e) => error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// The Save page's button for /api/reveal, or None where it can't work:
/// Finder on macOS, Explorer on Windows; on Linux xdg-open, only with a
/// desktop session here (a headless service would open the folder on the
/// wrong machine).
pub(crate) fn reveal_label() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        return Some("Show in Finder");
    }
    if cfg!(windows) {
        return Some("Show in Explorer");
    }
    let desktop = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let xdg = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("xdg-open").is_file()));
    (cfg!(target_os = "linux") && desktop && xdg).then_some("Open folder")
}

fn spawn(cmd: &str, args: &[&std::ffi::OsStr]) {
    let _ = std::process::Command::new(cmd).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// Show a saved export in the file manager. Only inside the work folder.
pub async fn api_reveal(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(path) = f.get("path").and_then(Value::as_str) else { return missing("path") };
    let target = resolve(Path::new(path));
    let base = resolve(&app.settings.work_dir());
    if reveal_label().is_none() || !target.starts_with(&base) {
        return error(StatusCode::BAD_REQUEST, "not available");
    }
    let file = target.is_file();
    if cfg!(target_os = "macos") {
        // a file is selected in its folder; a folder is opened
        if file {
            spawn("open", &["-R".as_ref(), target.as_os_str()]);
        } else {
            spawn("open", &[target.as_os_str()]);
        }
    } else if cfg!(windows) {
        if file {
            let select = format!("/select,{}", target.display());
            spawn("explorer", &[select.as_ref()]);
        } else {
            spawn("explorer", &[target.as_os_str()]);
        }
    } else {
        // xdg-open can't select a file: open the folder it is in
        let dir: PathBuf = if file { target.parent().map(Path::to_path_buf).unwrap_or(target) } else { target };
        spawn("xdg-open", &[dir.as_os_str()]);
    }
    Json(json!({"ok": true})).into_response()
}

pub async fn api_app_focus(State(app): State<Arc<App>>) -> Response {
    match &*app.focus.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(focus) => {
            focus();
            Json(json!({"ok": true})).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "no app window"),
    }
}

/// The system's own dialog for the work folder ("kind": "folder") or a
/// .raw to open ("kind": "raw"), starting in "start": {"path"}, null when
/// cancelled. Only the window app has one; a browser uses /api/browse.
pub async fn api_pick(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let Some(picker) = app.picker() else { return error(StatusCode::NOT_FOUND, "no app window") };
    let f = fields(&body);
    let kind = match f.get("kind").and_then(Value::as_str) {
        Some("folder") => crate::Pick::Folder,
        Some("raw") => crate::Pick::Raw,
        _ => return missing("kind"),
    };
    let start = f.get("start").and_then(Value::as_str).map(expand_user).filter(|p| p.is_dir());
    let path = tokio::task::spawn_blocking(move || picker(kind, start)).await.ok().flatten();
    Json(json!({"path": path.map(|p| p.to_string_lossy().into_owned())})).into_response()
}

/// Open the newer release's page in the default browser (the app window
/// cannot); elsewhere the page opens the returned URL itself.
pub async fn api_update_open(State(app): State<Arc<App>>) -> Response {
    let st = app.update_status();
    if let Some(install) = app.updater().filter(|_| st.as_ref().is_some_and(|s| s["install"] == true)) {
        // installing quits the app: not in the middle of a scan
        if app.device_busy.load(std::sync::atomic::Ordering::SeqCst) {
            return Json(json!({"busy": true})).into_response();
        }
        app.telemetry.event("update_install", "", json!({"latest": st.as_ref().map(|s| s["latest"].clone())}));
        install();
        return Json(json!({"installing": true})).into_response();
    }
    let url = st.as_ref().and_then(|s| s["url"].as_str()).map(str::to_string).unwrap_or_else(|| format!("{}/latest", update::RELEASES));
    app.telemetry.event("update_download", "", json!({"latest": st.as_ref().map(|s| s["latest"].clone())}));
    if cfg!(target_os = "macos") {
        spawn("open", &[url.as_ref()]);
        return Json(json!({"opened": true, "url": url})).into_response();
    }
    if cfg!(windows) {
        spawn("explorer", &[url.as_ref()]);
        return Json(json!({"opened": true, "url": url})).into_response();
    }
    Json(json!({"opened": false, "url": url})).into_response()
}

pub async fn api_telemetry_consent(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let (Some(usage), Some(crashes)) = (f.get("usage").and_then(Value::as_bool), f.get("crashes").and_then(Value::as_bool)) else {
        return missing("usage, crashes");
    };
    let state = app.telemetry.set_consent(usage, crashes);
    app.telemetry.event("consent", "", json!({"usage": usage, "crashes": crashes}));
    Json(state).into_response()
}

pub async fn api_telemetry_page(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(page) = f.get("page").and_then(Value::as_str) else { return missing("page") };
    app.telemetry.page(page);
    Json(json!({"ok": true})).into_response()
}

/// A JavaScript error in the page.
pub async fn api_telemetry_error(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let s = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    app.telemetry.error("PageError", &s("message"), &s("stack"), &s("page"));
    Json(json!({"ok": true})).into_response()
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[test]
    fn report_mail() {
        assert_eq!(mail_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        let url = report_mailto("1.0.0", "windows x86_64", "F-135 #2233 (warm)");
        assert!(url.starts_with("mailto:jorsh@jor.sh?subject=Pakon%201.0.0%3A%20problem%20report&body="));
        assert!(url.contains("F-135%20%232233%20%28warm%29"));
        assert!(!url.contains(' ') && !url.contains('\n'));
    }
}
