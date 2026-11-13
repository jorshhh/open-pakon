//! The scanner's routes: firmware load, scan, stop, eject. What
//! `web/app.py` did by running `pakon_probe` / `pakon_replay` and reading
//! their output is done here by calling `pakon-scan` and reading the same
//! lines.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use pakon_proto::lightcal::Film;
use pakon_proto::setup::Model;
use pakon_scan::{replay, scan, session, transport};
use pakon_usb::device::{self, DevClass, Device, UsbError};
use regex::Regex;
use serde_json::{json, Value};

use crate::jobs::{film_json, save_film, FILMS};
use crate::paths::env;
use crate::sse::{self, Emitter};
use crate::{error, fields, App};

/// Resolutions: 2000 x 3000, 1500 x 2250 and 1000 x 1500 px per frame. The
/// replayed scans are Base 16 only.
const SCAN_BASES: [u64; 3] = [16, 8, 4];

/// USB per-transfer timeout, `pakon_replay`'s default.
const TIMEOUT: Duration = Duration::from_millis(1000);

/// Windows reaches the scanner only through the WinUSB driver, installed
/// once for each of its two USB IDs (before and after the firmware load).
const DRIVER_HINT: &str = if cfg!(windows) {
    ". On Windows the scanner needs the WinUSB driver: see https://github.com/jorshhh/open-pakon/blob/main/packaging/windows/README.md#the-usb-driver"
} else {
    ""
};

/// Lines of a code-built scan worth showing, and the one that means "the
/// motor runs, feed the film".
const SCAN_LOG_KEYS: &[&str] = &[
    "calibration:",
    "calibrated:",
    "film detected",
    "end-of-roll",
    "stream ended",
    "stop requested",
    "no film fed",
    "safety cap",
    "event service:",
    "scan-code:",
    "teardown",
    "failed",
    "did not converge",
    "insufficient",
    "] dx:",
    "black means",
    "lamp:",
];
const FEED_KEY: &str = "feed the film now";
const EJECT_LOG_KEYS: &[&str] = &["motor", "eject:", "advance", "failed", "FAILED"];

/// The scanner's lamp lines in the words the Scan page shows.
fn plain(line: &str) -> String {
    static PATS: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (warming, stable) =
        PATS.get_or_init(|| (Regex::new(r"^lamp: warming up, (-?[\d.]+) C").expect("regex"), Regex::new(r"^lamp: stable(?:, (-?[\d.]+) C)?").expect("regex")));
    if let Some(m) = warming.captures(line) {
        return format!("Warming up the lamp ({} °C)…", &m[1]);
    }
    if let Some(m) = stable.captures(line) {
        return match m.get(1) {
            Some(t) => format!("Lamp ready ({} °C)", t.as_str()),
            None => "Lamp ready".to_string(),
        };
    }
    line.to_string()
}

/// ("warm" | "cold" | "disconnected" | "unknown", why it is unknown).
pub(crate) fn scanner_state() -> (&'static str, Option<String>) {
    match device::find() {
        Ok(DevClass::Warm) => ("warm", None),
        Ok(DevClass::Cold) => ("cold", None),
        Err(UsbError::NoDevice) => ("disconnected", None),
        Err(e) => ("unknown", Some(format!("USB detection failed: {e}"))),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

fn size_of(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

fn mb(size: u64) -> f64 {
    (size as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

/// One scan, eject or firmware load at a time.
struct Busy(Arc<App>);

impl Busy {
    fn take(app: &Arc<App>, em: &mut Emitter) -> Option<Busy> {
        if app.device_busy.swap(true, Ordering::SeqCst) {
            em.error("The scanner is busy");
            return None;
        }
        Some(Busy(app.clone()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.device_busy.store(false, Ordering::SeqCst);
    }
}

/// Read the connected scanner's EEPROM if not done on this connection,
/// with a log event when a new archive was made or the read failed. Run
/// only between device operations.
async fn eeprom_sync_events(app: &Arc<App>, em: &mut Emitter) {
    let before = app.eeprom.status();
    let me = app.clone();
    let Some(st) = blocking(move || me.eeprom.sync()).await else { return };
    if st["created"] == true && st["archive"] != before["archive"] {
        em.log(format!(
            "EEPROM read from {} serial {} and archived as backups/eeprom/{}",
            st["model"].as_str().unwrap_or("?"),
            st["serial"],
            st["archive"].as_str().unwrap_or("?")
        ));
    } else if let Some(e) = st["error"].as_str().filter(|_| st["error"] != before["error"]) {
        em.log(format!("{e}{DRIVER_HINT}"));
    }
}

/// The open handshake and the presence probes: the model, or None when the
/// scanner is cold, absent or does not answer.
async fn detect_model() -> Option<Model> {
    blocking(|| {
        let _device = crate::device_lock();
        let mut dev = Device::open().ok()?;
        session::open_check(&mut dev, TIMEOUT, &mut |_| {}).model
    })
    .await
    .flatten()
}

/// Run a device job that prints `pakon_replay`'s lines, sending the
/// selected ones as 'log', the feed prompt as 'feed', and the image size as
/// 'progress' every 0.5 s when `out_path` is given. A `scan` can be ended
/// with the Stop button. Returns whether the job succeeded.
async fn run_streaming<F>(app: &Arc<App>, em: &mut Emitter, out_path: Option<&Path>, keys: Option<&[&str]>, scan: bool, job: F) -> bool
where
    F: FnOnce(&mut dyn FnMut(&str)) -> bool + Send + 'static,
{
    let (ltx, lrx) = std::sync::mpsc::channel::<String>();
    app.stop.store(false, Ordering::SeqCst);
    if scan {
        app.scan_stop.store(false, Ordering::SeqCst);
        app.scan_active.store(true, Ordering::SeqCst);
    }
    // the command layer's own errors (stderr in the tools)
    let sink = ltx.clone();
    pakon_scan::set_log_sink(Some(Arc::new(move |l: &str| {
        let _ = sink.send(l.to_string());
    })));
    let mut handle = tokio::task::spawn_blocking(move || {
        let _device = crate::device_lock();
        job(&mut |l: &str| {
            let _ = ltx.send(l.to_string());
        })
    });
    let ok = loop {
        let done = match tokio::time::timeout(Duration::from_millis(500), &mut handle).await {
            Ok(Ok(ok)) => Some(ok),
            Ok(Err(_)) => {
                // a panic: the drop guards have torn the scanner down
                em.log("failed: internal error, see the app's log");
                Some(false)
            }
            Err(_) => None,
        };
        while let Ok(text) = lrx.try_recv() {
            for line in text.lines().map(str::trim) {
                if line.contains(FEED_KEY) {
                    em.emit(json!({"type": "feed", "message": "Motor running — feed the film now"}));
                } else if keys.is_none_or(|ks| ks.iter().any(|k| line.contains(k))) && !line.is_empty() && !line.contains("0x81: timeout") {
                    em.log(plain(line));
                }
            }
        }
        if let Some(p) = out_path {
            let size = size_of(p);
            em.emit(json!({"type": "progress", "bytes": size, "mb": mb(size)}));
        }
        if let Some(ok) = done {
            break ok;
        }
    };
    pakon_scan::set_log_sink(None);
    if scan {
        app.scan_active.store(false, Ordering::SeqCst);
    }
    ok
}

// ── Firmware load ────────────────────────────────────────────────────────────

async fn firmware_stream(app: Arc<App>, mut em: Emitter) {
    let Some(_busy) = Busy::take(&app, &mut em) else { return };
    let p = &app.paths;
    let hex = p.fw_hex_available();
    let (first, source) = if hex {
        (p.fw_stage1(), format!("Intel HEX ({} + {})", name(&p.fw_stage1()), name(&p.fw_main())))
    } else {
        (p.pakfw(), format!("captured load {} (no HEX files in {})", name(&p.pakfw()), p.firmware.display()))
    };
    if !first.exists() {
        return em.error(format!("firmware file not found: {}", first.display()));
    }
    em.log(format!("Loading firmware from {source}"));

    let (stage1, main, pakfw) = (p.fw_stage1(), p.fw_main(), p.pakfw());
    let ok = run_streaming(&app, &mut em, None, None, false, move |out| {
        let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| UsbError::Firmware(format!("cannot read '{}': {e}", p.display())));
        let (label, r) = if hex {
            ("load-firmware-hex", read(&stage1).and_then(|a| read(&main).map(|b| (a, b))).and_then(|(a, b)| device::load_firmware_hex(&a, &b)))
        } else {
            ("load-firmware", read(&pakfw).and_then(|t| device::load_firmware_script(&t)))
        };
        match &r {
            Ok(()) => out(if hex { "load-firmware-hex: firmware loaded from HEX, device is warm" } else { "load-firmware: firmware sent" }),
            Err(e) => out(&format!("{label}: {e}")),
        }
        r.is_ok()
    })
    .await;
    if !ok {
        return em.error(format!("firmware load failed{DRIVER_HINT}"));
    }
    // The scanner re-enumerates as 0f05:f135; read its EEPROM once it does.
    for _ in 0..20 {
        if blocking(scanner_state).await.is_some_and(|s| s.0 == "warm") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eeprom_sync_events(&app, &mut em).await;
    em.emit(json!({"type": "done"}));
}

fn name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

pub async fn api_firmware(State(app): State<Arc<App>>) -> Response {
    let (tx, resp) = sse::channel();
    let em = Emitter::reported(tx, app.telemetry.clone(), "firmware", "scan", json!({}));
    tokio::spawn(firmware_stream(app, em));
    resp
}

// ── Scan ─────────────────────────────────────────────────────────────────────

/// Open the next order and make its raw the current roll.
fn prepare_out(app: &App) -> Result<PathBuf, String> {
    let (_, out) = app.settings.new_order().map_err(|e| format!("cannot create the order folder in {}: {e}", app.settings.work_dir().display()))?;
    let mut roll = app.roll();
    roll.scan_path = Some(out.clone());
    roll.frames.clear();
    Ok(out)
}

fn order_event(out: &Path) -> Value {
    json!({"type": "order", "path": out.to_string_lossy(), "order": out.parent().map(name).unwrap_or_default()})
}

fn done_event(app: &App, out: &Path, film: &str) -> Value {
    let size = size_of(out);
    save_film(out, film);
    json!({
        "type": "done", "bytes": size, "mb": mb(size), "path": out.to_string_lossy(), "film": film,
        "dx": film_json(pakon_roll::dx_for_raw(&app.paths.root, out)),
    })
}

async fn scan_stream(app: Arc<App>, mut em: Emitter, film: String, base: u32) {
    let Some(_busy) = Busy::take(&app, &mut em) else { return };
    eeprom_sync_events(&app, &mut em).await;
    let eeprom_dir = app.eeprom_dir();
    let over = env("PAKON_PAKSCAN");
    let mut model = None;
    if over.is_none() {
        em.log("Detecting scanner model...");
        model = detect_model().await;
    }
    if let (Some(m), Some(dir)) = (model, &eeprom_dir) {
        return scan_code_stream(&app, &mut em, m, dir.clone(), film, base).await;
    }
    if base != 16 {
        return em.error(format!("Base {base} needs the scan built in code (an EEPROM archive for this scanner); the replayed scan is Base 16 only"));
    }
    if film != "c41" {
        em.log("The replayed scan is set up for colour negative; the film type applies to the decode only");
    }
    if model.is_some() {
        em.log("No EEPROM archive in backups/eeprom/ (or PAKON_EEPROM_DIR): falling back to the replayed scan");
    }

    let pakscan = match (&over, model) {
        (Some(o), _) => {
            em.log(format!("Using PAKON_PAKSCAN override: {o}"));
            PathBuf::from(o)
        }
        (None, None) => {
            return em.error("could not detect the scanner model (is it connected and warm?); set PAKON_PAKSCAN to force a scan script");
        }
        (None, Some(m)) => {
            let (label, script) = if m == Model::F135Plus { ("F-135+", app.paths.pakscan_f135plus()) } else { ("F-135", app.paths.pakscan_f135()) };
            em.log(format!("Model: {label} — scan script {}", name(&script)));
            script
        }
    };
    if !pakscan.exists() {
        return em.error(format!("scan script not found: {}", pakscan.display()));
    }

    let out = match prepare_out(&app) {
        Ok(o) => o,
        Err(e) => return em.error(e),
    };
    em.emit(order_event(&out));

    let (image, stop) = (out.clone(), app.stop.clone());
    // the replayed scan's own lines are not shown, only the image size
    let ok = run_streaming(&app, &mut em, Some(&out), Some(&[]), true, move |print| {
        let lines = match pakon_proto::script::parse_file(&pakscan) {
            Ok(l) => l,
            Err(_) => return false,
        };
        let Ok(mut dev) = Device::open() else { return false };
        let Ok(file) = std::fs::File::create(&image) else { return false };
        let mut img = std::io::BufWriter::with_capacity(1 << 20, file);
        replay::replay_scan(&mut dev, &lines, &mut img, TIMEOUT, true, false, &stop, print).is_ok()
    })
    .await;

    if ok {
        em.emit(done_event(&app, &out, &film));
    } else if app.scan_stop.load(Ordering::SeqCst) {
        // The replayed scan reports a stop as a failure on purpose (a
        // partial raw); the motor is stopped and the teardown sent.
        em.error(format!("scan stopped before the end of the replayed script ({} MB kept, not processed)", mb(size_of(&out))));
    } else {
        em.error("scan failed");
    }
}

/// F-135 / F-135+ scan with nothing replayed: setup, light calibration (open gate,
/// keep the film out; reused for an hour like the OEM software), motor
/// start, then the operator feeds the film.
async fn scan_code_stream(app: &Arc<App>, em: &mut Emitter, model: Model, eeprom_dir: PathBuf, film: String, base: u32) {
    let out = match prepare_out(app) {
        Ok(o) => o,
        Err(e) => return em.error(e),
    };
    em.emit(order_event(&out));
    let label = if model == Model::F135Plus { "F-135+" } else { "F-135" };
    em.log(format!("Model: {label} — scan built in code, Base {base} (EEPROM {}). Keep the film out until the motor starts.", name(&eeprom_dir)));

    let (image, stop) = (out.clone(), app.stop.clone());
    let kind: Film = film.parse().unwrap_or_default();
    let ok = run_streaming(app, em, Some(&out), Some(SCAN_LOG_KEYS), true, move |print| {
        let Some(eeprom) = session::load_eeprom_archive(&eeprom_dir) else {
            print(&format!("scan-code: the EEPROM archive {} does not decode", eeprom_dir.display()));
            return false;
        };
        let mut dev = match Device::open() {
            Ok(d) => d,
            Err(e) => {
                print(&format!("open device failed: {e} (need operational 0F05:F135 on the host)"));
                return false;
            }
        };
        // B&W silver film scans without the IR channel, as pakon_replay does
        let opt = scan::ScanOptions { base, ir: kind != Film::Bw, film: kind, recalibrate: false, max_bytes: 4096 << 20, timeout: TIMEOUT };
        match scan::scan_code(&mut dev, eeprom, &eeprom_dir, &image, &opt, &stop, print) {
            Ok(_) => true,
            Err(e) => {
                print(&format!("scan-code: {e}"));
                false
            }
        }
    })
    .await;

    if ok {
        em.emit(done_event(app, &out, &film));
    } else if app.scan_stop.load(Ordering::SeqCst) {
        em.error("scan stopped before any film went through");
    } else {
        em.error("scan failed — see the log");
    }
}

pub async fn api_scan(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let film = f.get("film").and_then(Value::as_str).unwrap_or("c41").to_string();
    let base = f.get("base").map_or(Some(16), Value::as_u64);
    if !FILMS.contains(&film.as_str()) {
        return error(StatusCode::BAD_REQUEST, format!("unknown film type: {film}"));
    }
    let Some(base) = base.filter(|b| SCAN_BASES.contains(b)) else {
        return error(StatusCode::BAD_REQUEST, format!("unknown resolution: Base {}", f.get("base").cloned().unwrap_or_default()));
    };
    let (tx, resp) = sse::channel();
    let em = Emitter::reported(tx, app.telemetry.clone(), "scan", "scan", json!({"film": film, "base": base}));
    tokio::spawn(scan_stream(app, em, film, base as u32));
    resp
}

/// End the running scan as if the end of the roll were seen: the image
/// loop is left and the scanner torn down (the motor stops) as at the end
/// of a roll. For a strip the white detection misses.
pub async fn api_scan_stop(State(app): State<Arc<App>>) -> Response {
    if !app.scan_active.load(Ordering::SeqCst) {
        return error(StatusCode::CONFLICT, "no scan is running");
    }
    app.scan_stop.store(true, Ordering::SeqCst);
    app.stop.store(true, Ordering::SeqCst);
    Json(json!({"ok": true})).into_response()
}

// ── Eject ────────────────────────────────────────────────────────────────────

async fn eject_stream(app: Arc<App>, mut em: Emitter) {
    let Some(_busy) = Busy::take(&app, &mut em) else { return };
    let Some(model) = detect_model().await else {
        return em.error("could not detect the scanner model (is it warm?)");
    };
    let stop = app.stop.clone();
    let ok = if model == Model::F135 {
        eeprom_sync_events(&app, &mut em).await;
        let Some(dir) = app.eeprom_dir() else {
            return em.error("eject needs an EEPROM archive in backups/eeprom/ (or PAKON_EEPROM_DIR)");
        };
        em.log("Running the transport until the film sensors say the strip is out (at most 60 s)");
        run_streaming(&app, &mut em, None, Some(EJECT_LOG_KEYS), false, move |print| {
            let Some(eeprom) = session::load_eeprom_archive(&dir) else {
                print(&format!("eject: failed, the EEPROM archive {} does not decode", dir.display()));
                return false;
            };
            let mut dev = match Device::open() {
                Ok(d) => d,
                Err(e) => {
                    print(&format!("open device failed: {e} (need operational 0F05:F135 on the host)"));
                    return false;
                }
            };
            match transport::transport(&mut dev, eeprom, true, Duration::from_secs(60), TIMEOUT, &stop, print) {
                Ok(_) => true,
                Err(e) => {
                    print(&format!("eject: failed, {e}"));
                    false
                }
            }
        })
        .await
    } else {
        em.log("F-135+: running the transport for 15 s");
        run_streaming(&app, &mut em, None, Some(EJECT_LOG_KEYS), false, move |print| {
            let mut dev = match Device::open() {
                Ok(d) => d,
                Err(e) => {
                    print(&format!("open device failed: {e} (need operational 0F05:F135 on the host)"));
                    return false;
                }
            };
            transport::timed_advance_plus(&mut dev, 15, TIMEOUT, &stop, print).is_ok()
        })
        .await
    };
    if ok {
        em.emit(json!({"type": "done"}));
    } else {
        em.error("eject failed");
    }
}

pub async fn api_eject(State(app): State<Arc<App>>) -> Response {
    let (tx, resp) = sse::channel();
    let em = Emitter::reported(tx, app.telemetry.clone(), "eject", "scan", json!({}));
    tokio::spawn(eject_stream(app, em));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamp_lines_in_plain_words() {
        assert_eq!(plain("lamp: warming up, 31.5 C (target 35.0)"), "Warming up the lamp (31.5 °C)…");
        assert_eq!(plain("lamp: stable, 35.02 C"), "Lamp ready (35.02 °C)");
        assert_eq!(plain("lamp: stable"), "Lamp ready");
        assert_eq!(plain("[sm] lamp: stable"), "[sm] lamp: stable");
        assert_eq!(plain("calibrated: offsets 1/2/3"), "calibrated: offsets 1/2/3");
    }

    #[test]
    fn megabytes_to_one_decimal() {
        assert_eq!(mb(0), 0.0);
        assert_eq!(mb(1_048_576), 1.0);
        assert_eq!(mb(1_600_000_000), 1525.9);
    }
}
