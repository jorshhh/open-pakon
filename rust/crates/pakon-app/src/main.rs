//! Pakon in its own window (port of `packaging/macos/pakon_app.py`): the web
//! service on a free localhost port, shown in the system webview (WKWebView
//! on macOS, WebView2 on Windows, WebKitGTK on Linux). Closing the window
//! ends any running scan as at the end of the roll, waits for the scanner's
//! teardown, and quits.
//!
//! In the Mac app the EEPROM archives and settings go to
//! `~/Library/Application Support/Pakon` (PAKON_DATA; the signed bundle is
//! read-only) and the log to `~/Library/Logs/Pakon/pakon.log`. On Windows
//! both are under `%LOCALAPPDATA%\Pakon`. From a checkout it runs as is:
//!
//!     cargo run --release -p pakon-app

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use pakon_web::paths::{self, home};
use pakon_web::{App, Pick};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, WebViewBuilder};

#[derive(Debug)]
enum UserEvent {
    /// A second launch: bring the window to the front.
    Focus,
    /// The page wants the system's folder / .raw dialog (POST /api/pick):
    /// shown here, on the main thread, as macOS and GTK require.
    Pick(Pick, Option<PathBuf>, mpsc::Sender<Option<PathBuf>>),
    /// The service has torn the scanner down and stopped: quit.
    Stopped,
    /// Quit from the menu (Cmd-Q): as closing the window.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Quit,
}

/// Running from inside a Mac app bundle (`Pakon.app/Contents/MacOS/`).
fn bundle_contents() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    (macos.file_name()? == "MacOS").then(|| macos.parent().map(Path::to_path_buf)).flatten()
}

/// The version the new-build check compares against, and where crash
/// reports / usage statistics go: from the bundle's Info.plist (Mac app) or
/// `release.env` beside the files (Linux and Windows installs). Only release
/// builds carry the report server.
fn release_settings(root: &Path) {
    let set = |key: &str, value: &str| {
        if std::env::var_os(key).is_none() && !value.trim().is_empty() {
            // SAFETY: called at startup, before any other thread exists.
            unsafe { std::env::set_var(key, value.trim()) };
        }
    };
    #[cfg(target_os = "macos")]
    if let Some(contents) = bundle_contents() {
        if let Ok(plist::Value::Dictionary(info)) = plist::Value::from_file(contents.join("Info.plist")) {
            let s = |k: &str| info.get(k).and_then(|v| v.as_string()).unwrap_or("").to_string();
            set("PAKON_VERSION", &s("CFBundleShortVersionString"));
            set("PAKON_RYBBIT_URL", &s("PakonRybbitURL"));
            set("PAKON_RYBBIT_SITE_ID", &s("PakonRybbitSiteID"));
            set("PAKON_RYBBIT_DOMAIN", &s("PakonRybbitDomain"));
        }
        return;
    }
    if let Ok(text) = std::fs::read_to_string(root.join("release.env")) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if !key.trim_start().starts_with('#') {
                    set(key.trim(), value);
                }
            }
        }
    }
}

/// (data folder, log folder) for an installed app; a checkout keeps the
/// repo's folders and the terminal.
fn app_folders() -> (Option<PathBuf>, Option<PathBuf>) {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Local")).join("Pakon");
        return (Some(local.clone()), Some(local.join("Logs")));
    }
    if cfg!(target_os = "macos") && bundle_contents().is_some() {
        return (Some(home().join("Library/Application Support/Pakon")), Some(home().join("Library/Logs/Pakon")));
    }
    (None, paths::env("PAKON_LOGS").map(PathBuf::from))
}

/// A windowed app has no terminal: the service's and the scanner's lines
/// go to `pakon.log`.
fn log_to(dir: &Path) {
    let Ok(f) = File::options().create(true).append(true).open(dir.join("pakon.log")) else { return };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: duplicating an open descriptor onto stdout / stderr.
        unsafe {
            libc::dup2(f.as_raw_fd(), 1);
            libc::dup2(f.as_raw_fd(), 2);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
        // Rust's stdout / stderr look the handle up on every write. The
        // handle is kept open for the life of the process.
        let h = f.into_raw_handle();
        // SAFETY: an open file handle, never closed.
        unsafe {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

/// One app per user: a second one would start a second service on the same
/// scanner and temp folder. The lock is the OS's on an open file, so a crash
/// leaves none behind; the port file says where the running one listens.
fn run_dir() -> PathBuf {
    match paths::env("PAKON_DATA") {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir().join(format!("pakon-{}", std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default())),
    }
}

/// `Err` when another Pakon holds the lock. `Ok(None)` when the lock can't
/// be taken for another reason: run anyway, as a missing lock is better than
/// an app that never opens.
fn hold_lock(dir: &Path) -> Result<Option<File>, ()> {
    let _ = std::fs::create_dir_all(dir);
    // read + write: Windows locks (LockFileEx) need one of them, and an
    // append-only handle is refused with "access denied"
    let f = match File::options().read(true).write(true).create(true).truncate(false).open(dir.join("app.lock")) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("app.lock: {e}; running without the one-app check");
            return Ok(None);
        }
    };
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Err(()),
        Err(std::fs::TryLockError::Error(e)) => {
            eprintln!("app.lock: {e}; running without the one-app check");
            Ok(None)
        }
    }
}

/// Bring the running app's window to the front (POST /api/app/focus).
fn focus_running(dir: &Path) {
    let Ok(port) = std::fs::read_to_string(dir.join("app.port")) else { return };
    let url = format!("http://127.0.0.1:{}/api/app/focus", port.trim());
    // still starting: it shows itself
    let _ = ureq::post(&url).timeout(Duration::from_secs(5)).call();
}

#[cfg(target_os = "macos")]
const QUIT_ID: &str = "quit";

/// The macOS menu bar: without one, a Mac app has no Quit (Cmd-Q) and the
/// page no Copy / Paste. Quit is our own item, not the system's, so it ends a
/// scan and waits for the teardown as closing the window does. Windows and
/// Linux get no menu: the page's shortcuts work without one, and closing the
/// window quits.
#[cfg(target_os = "macos")]
fn menu() -> muda::Menu {
    use muda::{Menu, PredefinedMenuItem as P, Submenu};
    let menu = Menu::new();
    {
        use muda::accelerator::{Accelerator, Code, Modifiers};
        let quit = muda::MenuItem::with_id(QUIT_ID, "Quit Pakon", true, Some(Accelerator::new(Modifiers::META, Code::KeyQ)));
        let app = Submenu::with_items(
            "Pakon",
            true,
            &[&P::about(None, None), &P::separator(), &P::services(None), &P::separator(), &P::hide(None), &P::hide_others(None), &P::show_all(None), &P::separator(), &quit],
        )
        .expect("menu");
        let _ = menu.append(&app);
    }
    let edit = Submenu::with_items("Edit", true, &[&P::undo(None), &P::redo(None), &P::separator(), &P::cut(None), &P::copy(None), &P::paste(None), &P::select_all(None)])
        .expect("menu");
    let window = Submenu::with_items("Window", true, &[&P::minimize(None), &P::fullscreen(None)]).expect("menu");
    let _ = menu.append_items(&[&edit, &window]);
    menu
}

/// Links to elsewhere (the update page, Rybbit) open in the browser.
fn open_in_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}

/// The system's dialog, attached to the window: the chosen path, or None.
fn pick(w: &Window, kind: Pick, start: Option<PathBuf>) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_parent(w);
    if let Some(dir) = start {
        d = d.set_directory(dir);
    }
    match kind {
        Pick::Folder => d.set_title("Choose the work folder").pick_folder(),
        Pick::Raw => d.set_title("Open a raw").add_filter("Pakon raw", &["raw"]).pick_file(),
    }
}

/// Bring a window to the front.
fn raise(w: &Window) {
    w.set_minimized(false);
    w.set_visible(true);
    w.set_always_on_top(true); // to the front, then normal
    w.set_always_on_top(false);
    w.set_focus();
}

fn error_page(log: &Option<PathBuf>, why: &str) -> String {
    let where_ = log.as_ref().map(|l| l.join("pakon.log").display().to_string()).unwrap_or_else(|| "the terminal".into());
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    format!(
        "<body style='font:15px -apple-system,system-ui,sans-serif;padding:2em'><h2>Pakon could not start its scanner service.</h2><p>{}</p><p>Details are in {}.</p></body>",
        esc(why),
        esc(&where_)
    )
}

/// The service on its own runtime thread: (port, a sender that stops it).
/// The thread sends `Stopped` once the scanner is torn down.
fn start_service(root: PathBuf, proxy: EventLoopProxy<UserEvent>) -> Result<(u16, tokio::sync::oneshot::Sender<()>, Arc<App>), String> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    let listener = rt.block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0))).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let app = App::new(root);
    let focus = proxy.clone();
    app.set_focus(move || {
        let _ = focus.send_event(UserEvent::Focus);
    });
    let picker = std::sync::Mutex::new(proxy.clone());
    app.set_picker(move |kind, start| {
        let (tx, rx) = mpsc::channel();
        let sent = picker.lock().unwrap_or_else(|e| e.into_inner()).send_event(UserEvent::Pick(kind, start, tx));
        // the window is gone (quitting): as cancelled
        sent.ok().and_then(|()| rx.recv().ok()).flatten()
    });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let served = app.clone();
    std::thread::spawn(move || {
        let r = rt.block_on(pakon_web::serve(listener, served, async {
            let _ = stop_rx.await;
        }));
        if let Err(e) = r {
            eprintln!("service: {e}");
        }
        let _ = proxy.send_event(UserEvent::Stopped);
    });
    Ok((port, stop_tx, app))
}

fn main() {
    let root = paths::find_root();
    if let Some(r) = &root {
        release_settings(r);
    }
    let (data, logs) = app_folders();
    if let Some(d) = &data {
        if paths::env("PAKON_DATA").is_none() {
            // SAFETY: startup, one thread.
            unsafe { std::env::set_var("PAKON_DATA", d) };
        }
        let _ = std::fs::create_dir_all(d);
    }
    if let Some(l) = &logs {
        let _ = std::fs::create_dir_all(l);
        // SAFETY: startup, one thread.
        unsafe { std::env::set_var("PAKON_LOGS", l) };
        if data.is_some() {
            log_to(l);
        }
    }

    let run = run_dir();
    let Ok(_lock) = hold_lock(&run) else {
        focus_running(&run);
        println!("Pakon is already running");
        return;
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        let menu = menu();
        let menu_proxy = event_loop.create_proxy();
        muda::MenuEvent::set_event_handler(Some(move |e: muda::MenuEvent| {
            if e.id() == QUIT_ID {
                let _ = menu_proxy.send_event(UserEvent::Quit);
            }
        }));
        menu.init_for_nsapp();
        // the menu lives as long as the app
        std::mem::forget(menu);
    }

    let window = WindowBuilder::new()
        .with_title("Pakon")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 640.0))
        .build(&event_loop)
        .expect("a window");

    let service = match &root {
        Some(r) => start_service(r.clone(), event_loop.create_proxy()),
        None => Err("its files (web/static) were not found".to_string()),
    };
    let downloads = home().join("Downloads");
    let builder = WebViewBuilder::new()
        // the frame and zip links save to Downloads
        .with_download_started_handler(move |url, dest| {
            let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("pakon-download");
            *dest = downloads.join(name);
            true
        })
        .with_new_window_req_handler(|url, _| {
            open_in_browser(&url);
            NewWindowResponse::Deny
        });
    let (builder, mut stop, app) = match service {
        Ok((port, stop, app)) => {
            let _ = std::fs::write(run.join("app.port"), port.to_string());
            (builder.with_url(format!("http://127.0.0.1:{port}/")), Some(stop), Some(app))
        }
        Err(e) => {
            eprintln!("Pakon could not start its scanner service: {e}");
            (builder.with_html(error_page(&logs, &e)), None, None)
        }
    };
    // On Linux the page goes into tao's GTK box (under the menu bar); built
    // on the window itself it is never shown there, and the window stays blank.
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("tao's GTK box")).expect("a webview")
    };
    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window).expect("a webview");

    let mut closing: Option<Instant> = None;
    event_loop.run(move |event, _, flow| {
        *flow = match closing {
            // waiting for the teardown: check the time limit every second
            Some(_) => ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1)),
            None => ControlFlow::Wait,
        };
        match event {
            Event::UserEvent(UserEvent::Focus) => raise(&window),
            Event::UserEvent(UserEvent::Pick(kind, start, reply)) => {
                let _ = reply.send(pick(&window, kind, start));
            }
            Event::UserEvent(UserEvent::Stopped) => *flow = ControlFlow::Exit,
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Quit) => match stop.take() {
                // end a scan as at the end of the roll, wait for the
                // teardown (Stopped), then quit
                Some(s) => {
                    window.set_visible(false);
                    closing = Some(Instant::now());
                    *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
                    let _ = s.send(());
                }
                None if closing.is_none() => *flow = ControlFlow::Exit,
                None => {}
            },
            // however the loop ends (the system quitting the app too), the
            // scanner is torn down before the process exits
            Event::LoopDestroyed => {
                if let Some(app) = &app {
                    app.shutdown();
                }
            }
            _ => {}
        }
        if closing.is_some_and(|t| t.elapsed() > Duration::from_secs(120)) {
            eprintln!("the scanner did not stop within 2 minutes; quitting anyway");
            *flow = ControlFlow::Exit;
        }
    });
}
