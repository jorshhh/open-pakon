//! In-place updates on macOS with Sparkle.framework, when the bundle carries
//! it (`Contents/Frameworks`: packaging/macos/build.sh adds it, with the feed
//! and the public key in Info.plist). It is loaded at run time, so a build
//! without it (a checkout, a build without the key) links nothing and the
//! banner keeps opening the release page.
//!
//! Sparkle's own scheduled checks are off (SUEnableAutomaticChecks): the
//! page's banner (pakon-web update.rs) is the one prompt; its Install and
//! Pakon > Check for Updates… call `check`. Installing quits the app the
//! system's way, which tears the scanner down as any quit does
//! (LoopDestroyed).

use std::ffi::CString;
use std::path::Path;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, MainThreadMarker};

pub struct Sparkle(Retained<AnyObject>);

impl Sparkle {
    /// Load Sparkle from `Contents/Frameworks` and start its updater; None
    /// without it.
    pub fn start(contents: &Path, _main: MainThreadMarker) -> Option<Sparkle> {
        let framework = contents.join("Frameworks").join("Sparkle.framework");
        if !framework.is_dir() {
            return None;
        }
        let path = CString::new(framework.to_str()?).ok()?;
        // SAFETY: Foundation and Sparkle's documented API, on the main thread.
        unsafe {
            let ns: Option<Retained<AnyObject>> = msg_send![AnyClass::get(c"NSString")?, stringWithUTF8String: path.as_ptr()];
            let bundle: Option<Retained<AnyObject>> = msg_send![AnyClass::get(c"NSBundle")?, bundleWithPath: &*ns?];
            let bundle = bundle?;
            let loaded: bool = msg_send![&*bundle, load];
            let Some(class) = AnyClass::get(c"SPUStandardUpdaterController").filter(|_| loaded) else {
                eprintln!("Sparkle did not load: updates open the release page");
                return None;
            };
            let none: Option<&AnyObject> = None;
            let alloc: Allocated<AnyObject> = msg_send![class, alloc];
            let controller: Option<Retained<AnyObject>> =
                msg_send![alloc, initWithStartingUpdater: true, updaterDelegate: none, userDriverDelegate: none];
            controller.map(Sparkle)
        }
    }

    /// Sparkle's own window: the new version, then Install and Relaunch.
    pub fn check(&self) {
        let none: Option<&AnyObject> = None;
        // SAFETY: as above.
        unsafe {
            let () = msg_send![&*self.0, checkForUpdates: none];
        }
    }
}
