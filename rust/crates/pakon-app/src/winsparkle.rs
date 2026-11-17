//! In-place updates on Windows with WinSparkle (https://winsparkle.org), the
//! Windows port of Sparkle, when `WinSparkle.dll` sits next to Pakon.exe
//! (packaging/windows/build.sh adds it). It is loaded at run time, so a build
//! without it links nothing and the banner keeps opening the release page.
//!
//! The feed is `appcast-windows.xml` on the latest release (made when the
//! release is published: .github/workflows/appcast.yml), its installer
//! EdDSA-signed with the Mac feed's key (packaging/macos/sparkle-public-key).
//! WinSparkle's own scheduled checks are off: the page's banner (pakon-web
//! update.rs) is the one prompt, and its Install calls `check`. WinSparkle
//! downloads and verifies the installer, asks whether the app can quit (not
//! during a scan), runs `Pakon-Setup.exe /S /UPDATE` and asks the app to
//! quit, which tears the scanner down as any quit does; the installer waits
//! for Pakon.exe to exit and starts the new one.

use std::ffi::{c_char, c_int, CString};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// The release's Windows feed; PAKON_UPDATE_FEED replaces it (a test build:
/// packaging/windows/build.sh puts it in release.env).
const FEED: &str = "https://github.com/jorshhh/open-pakon/releases/latest/download/appcast-windows.xml";
/// The Mac feed's public key: one key signs both; PAKON_UPDATE_KEY replaces
/// it (a test build, as the feed).
const PUBLIC_KEY: &str = include_str!("../../../../packaging/macos/sparkle-public-key");

type Callback = extern "C" fn();
type CanQuit = extern "C" fn() -> c_int;

/// Asked from WinSparkle's threads: may the app quit now, and quit it.
struct Hooks {
    can_quit: Box<dyn Fn() -> bool + Send + Sync>,
    quit: Mutex<Box<dyn Fn() + Send>>,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

extern "C" fn can_shutdown() -> c_int {
    HOOKS.get().is_some_and(|h| (h.can_quit)()) as c_int
}

extern "C" fn shutdown_request() {
    if let Some(h) = HOOKS.get() {
        (h.quit.lock().unwrap_or_else(|e| e.into_inner()))();
    }
}

pub struct WinSparkle {
    check: Callback,
    cleanup: Callback,
}

/// `name` from the loaded DLL, as a function of type `F`.
///
/// # Safety
/// `F` must be the export's real signature.
unsafe fn proc<F: Copy>(dll: windows_sys::Win32::Foundation::HMODULE, name: &std::ffi::CStr) -> Option<F> {
    // SAFETY: GetProcAddress on a loaded module; the caller vouches for F.
    unsafe {
        let p = GetProcAddress(dll, name.as_ptr() as *const u8)?;
        Some(std::mem::transmute_copy::<_, F>(&p))
    }
}

impl WinSparkle {
    /// Load WinSparkle.dll from Pakon.exe's folder, configure it for
    /// `version` and start it; None without the DLL. On the main thread,
    /// once. `can_quit` and `quit` are called from WinSparkle's threads.
    pub fn start(version: &str, can_quit: impl Fn() -> bool + Send + Sync + 'static, quit: impl Fn() + Send + 'static) -> Option<WinSparkle> {
        let dll_path = std::env::current_exe().ok()?.with_file_name("WinSparkle.dll");
        if !dll_path.is_file() {
            return None;
        }
        let wide: Vec<u16> = dll_path.as_os_str().encode_wide_nul();
        // SAFETY: WinSparkle's documented C API (include/winsparkle.h, 0.9),
        // every configuration call before win_sparkle_init, on the main thread.
        unsafe {
            let dll = LoadLibraryW(wide.as_ptr());
            if dll.is_null() {
                eprintln!("WinSparkle did not load: updates open the release page");
                return None;
            }
            let set_url: extern "C" fn(*const c_char) = proc(dll, c"win_sparkle_set_appcast_url")?;
            let set_key: extern "C" fn(*const c_char) -> c_int = proc(dll, c"win_sparkle_set_eddsa_public_key")?;
            let set_details: extern "C" fn(*const u16, *const u16, *const u16) = proc(dll, c"win_sparkle_set_app_details")?;
            let set_auto: extern "C" fn(c_int) = proc(dll, c"win_sparkle_set_automatic_check_for_updates")?;
            let set_can: extern "C" fn(CanQuit) = proc(dll, c"win_sparkle_set_can_shutdown_callback")?;
            let set_quit: extern "C" fn(Callback) = proc(dll, c"win_sparkle_set_shutdown_request_callback")?;
            let init: Callback = proc(dll, c"win_sparkle_init")?;
            let check: Callback = proc(dll, c"win_sparkle_check_update_with_ui_and_install")?;
            let cleanup: Callback = proc(dll, c"win_sparkle_cleanup")?;

            let feed = std::env::var("PAKON_UPDATE_FEED").unwrap_or_else(|_| FEED.into());
            let key = std::env::var("PAKON_UPDATE_KEY").unwrap_or_else(|_| PUBLIC_KEY.into());
            let url = CString::new(feed).ok()?;
            let key = CString::new(key.trim()).ok()?;
            set_url(url.as_ptr());
            if set_key(key.as_ptr()) != 1 {
                eprintln!("WinSparkle refused the public key: updates open the release page");
                return None;
            }
            let (company, name, ver) = (wstr("Pakon"), wstr("Pakon"), wstr(version));
            set_details(company.as_ptr(), name.as_ptr(), ver.as_ptr());
            set_auto(0);
            let _ = HOOKS.set(Hooks { can_quit: Box::new(can_quit), quit: Mutex::new(Box::new(quit)) });
            set_can(can_shutdown);
            set_quit(shutdown_request);
            init();
            Some(WinSparkle { check, cleanup })
        }
    }

    /// WinSparkle's window: checks, downloads and installs, then asks the
    /// app to quit.
    pub fn check(&self) {
        (self.check)();
    }

    /// Stop WinSparkle (the event loop never returns, so not on drop): when
    /// the app quits.
    pub fn cleanup(&self) {
        (self.cleanup)();
    }
}


/// A NUL-terminated UTF-16 string.
fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}
