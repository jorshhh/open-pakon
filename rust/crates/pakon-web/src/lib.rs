//! The Pakon web service in Rust (`docs/TECHNICAL.md`): the routes
//! and event streams of `web/app.py` behind the same page,
//! `web/static/index.html`. Scans call `pakon-scan` in this process instead
//! of starting `pakon_replay`; image jobs call `pakon-roll` on the blocking
//! pool.
//!
//! Endpoints
//! ---------
//! GET  /api/status                  scanner state + current job info
//! POST /api/firmware                SSE: load firmware (cold device)
//! POST /api/scan                    SSE: run a scan (a new order in the work folder; film c41|bw|bwc41|slide,
//!                                   base 16|8|4)
//! POST /api/workdir                 set the work folder scans go into
//! POST /api/scan/stop               end the running scan now (the operator's end of roll)
//! POST /api/eject                   SSE: run the film out of the transport
//! POST /api/open                    pick a .raw on the host (keeps its crops)
//! POST /api/process                 SSE: decode the opened .raw or the last scan
//! POST /api/autoframe               place the boxes with the OEM framing (format full|half|pano)
//! GET  /api/frames                  list decoded frames
//! GET  /api/frames/{n}/thumb        JPEG thumbnail
//! POST /api/save                    SSE: save the frames to the work folder (jpeg|tiff|raw|contact)
//! POST /api/reveal                  show a saved export folder (Finder, Explorer, xdg-open)
//! POST /api/app/focus               bring the app's window to the front (second launch)
//! POST /api/pick                    the system's folder / .raw dialog (window app only)
//! POST /api/update/open             open the newer release's page in the browser
//! POST /api/telemetry/consent       the user's crash-report / usage-statistics choice
//! POST /api/telemetry/page|error    a page opened / a page error

pub mod eeprom;
mod files;
mod jobs;
pub mod orders;
pub mod paths;
mod scanner;
mod sse;
pub mod telemetry;
pub mod update;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use crate::paths::Paths;

/// The scanner is one device: the EEPROM read, the model check, a scan, an
/// eject and the firmware load take turns.
static DEVICE: Mutex<()> = Mutex::new(());

pub(crate) fn device_lock() -> MutexGuard<'static, ()> {
    DEVICE.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the native dialog picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// The work folder.
    Folder,
    /// A `.raw` to open.
    Raw,
}

/// Shows the dialog, starting in the folder given; the chosen path, or
/// None when cancelled. Blocks until the dialog closes.
pub type Picker = dyn Fn(Pick, Option<PathBuf>) -> Option<PathBuf> + Send + Sync;

/// The roll being worked on.
#[derive(Default)]
pub struct Roll {
    /// The last scan's raw, or the one opened.
    pub scan_path: Option<PathBuf>,
    /// The exported frames' files.
    pub frames: Vec<pakon_roll::FrameFiles>,
    /// `prescan`'s metadata (ribbon path, film base, pitch, centres...).
    pub prescan: Option<pakon_roll::Meta>,
    pub processing: bool,
}

pub struct App {
    pub paths: Paths,
    pub settings: Arc<orders::Settings>,
    pub eeprom: eeprom::Eeprom,
    pub update: Arc<update::Update>,
    pub telemetry: Arc<telemetry::Telemetry>,
    roll: Mutex<Roll>,
    /// Ends the running scan, eject or advance as at the end of the roll.
    stop: Arc<AtomicBool>,
    /// A scan the Stop button can end is running.
    scan_active: AtomicBool,
    /// Stop was pressed for it.
    scan_stop: AtomicBool,
    /// A scan, eject or firmware load is running.
    device_busy: AtomicBool,
    /// Image jobs run one at a time.
    jobs: Mutex<()>,
    /// Set by the window app: a second launch brings the running window to
    /// the front instead of starting another service.
    focus: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Set by the window app: the system's own folder / file dialog.
    picker: Mutex<Option<Arc<Picker>>>,
    update_seen: Mutex<HashSet<String>>,
}

impl App {
    pub fn new(root: PathBuf) -> Arc<App> {
        let paths = Paths::new(root);
        let settings = Arc::new(orders::Settings::new(paths.settings.clone()));
        Arc::new(App {
            eeprom: eeprom::Eeprom::new(pakon_roll::eeprom_root(&paths.root)),
            telemetry: Arc::new(telemetry::Telemetry::new(settings.clone())),
            update: Arc::new(update::Update::default()),
            settings,
            paths,
            roll: Mutex::new(Roll::default()),
            stop: Arc::new(AtomicBool::new(false)),
            scan_active: AtomicBool::new(false),
            scan_stop: AtomicBool::new(false),
            device_busy: AtomicBool::new(false),
            jobs: Mutex::new(()),
            focus: Mutex::new(None),
            picker: Mutex::new(None),
            update_seen: Mutex::new(HashSet::new()),
        })
    }

    pub(crate) fn roll(&self) -> MutexGuard<'_, Roll> {
        self.roll.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The image pipeline's configuration, with the connected scanner's
    /// EEPROM archive.
    pub(crate) fn env(&self) -> pakon_roll::Env {
        pakon_roll::Env::from_env(&self.paths.root, self.eeprom.connected_serial())
    }

    pub(crate) fn eeprom_dir(&self) -> Option<PathBuf> {
        pakon_roll::find_eeprom_dir(&self.paths.root, self.eeprom.connected_serial())
    }

    /// What a second launch of the window app calls (POST /api/app/focus).
    pub fn set_focus(&self, focus: impl Fn() + Send + Sync + 'static) {
        *self.focus.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(focus));
    }

    /// The window app's native dialog for POST /api/pick.
    pub fn set_picker(&self, picker: impl Fn(Pick, Option<PathBuf>) -> Option<PathBuf> + Send + Sync + 'static) {
        *self.picker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(picker));
    }

    pub(crate) fn picker(&self) -> Option<Arc<Picker>> {
        self.picker.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Once, before serving: clear the temp files of an earlier run (the
    /// ribbon cache is ~1 GB a roll), start the reports, and read a warm
    /// scanner's EEPROM in the background.
    pub fn startup(self: &Arc<Self>) {
        self.env().clear_work(true);
        self.telemetry.start();
        let me = self.clone();
        std::thread::spawn(move || me.eeprom.sync());
    }

    /// Before the process exits: end a running scan or eject as at the end
    /// of the roll and wait until the scanner is torn down (the motor
    /// stopped, the lamp off). Leaving in the middle would leave the
    /// transport running.
    pub fn shutdown(&self) {
        self.shutdown_begin();
        drop(device_lock());
    }

    /// Ask a running scan, eject or advance to end now, as at the end of
    /// the roll; its stream then closes, so the server can stop.
    pub fn shutdown_begin(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.scan_stop.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn error(status: StatusCode, message: impl AsRef<str>) -> Response {
    (status, Json(json!({"error": message.as_ref()}))).into_response()
}

/// A request's JSON object; empty when there is no body.
pub(crate) fn fields(body: &Bytes) -> Map<String, Value> {
    serde_json::from_slice(body).unwrap_or_default()
}

/// A required field is missing (FastAPI answers 422).
pub(crate) fn missing(field: &str) -> Response {
    (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"detail": format!("field required: {field}")}))).into_response()
}

pub fn router(app: Arc<App>) -> Router {
    let static_dir = app.paths.static_dir();
    Router::new()
        .route("/api/status", get(files::api_status))
        .route("/api/firmware", post(scanner::api_firmware))
        .route("/api/scan", post(scanner::api_scan))
        .route("/api/scan/stop", post(scanner::api_scan_stop))
        .route("/api/eject", post(scanner::api_eject))
        .route("/api/workdir", post(files::api_workdir))
        .route("/api/browse", get(files::api_browse))
        .route("/api/mkdir", post(files::api_mkdir))
        .route("/api/open", post(jobs::api_open))
        .route("/api/process", post(jobs::api_process))
        .route("/api/autoframe", post(jobs::api_autoframe))
        .route("/api/preview", get(jobs::api_preview))
        .route("/api/export_frames", post(jobs::api_export_frames))
        .route("/api/frames", get(jobs::api_frames))
        .route("/api/frames/{n}/thumb", get(jobs::api_thumb))
        .route("/api/save", post(jobs::api_save))
        .route("/api/reveal", post(files::api_reveal))
        .route("/api/app/focus", post(files::api_app_focus))
        .route("/api/pick", post(files::api_pick))
        .route("/api/update/open", post(files::api_update_open))
        .route("/api/telemetry/consent", post(files::api_telemetry_consent))
        .route("/api/telemetry/page", post(files::api_telemetry_page))
        .route("/api/telemetry/error", post(files::api_telemetry_error))
        .fallback_service(tower_http::services::ServeDir::new(static_dir))
        .with_state(app)
}

/// Serve on `listener` until `stop` resolves, then tear the scanner down.
pub async fn serve(listener: tokio::net::TcpListener, app: Arc<App>, stop: impl std::future::Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    app.startup();
    let me = app.clone();
    let ending = app.clone();
    // a scan's stream stays open until its teardown: end the scan first,
    // or the graceful shutdown would wait for the whole roll
    let stop = async move {
        stop.await;
        ending.shutdown_begin();
    };
    let r = axum::serve(listener, router(app)).with_graceful_shutdown(stop).await;
    let _ = tokio::task::spawn_blocking(move || me.shutdown()).await;
    r
}
