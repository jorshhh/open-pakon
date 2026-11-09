//! What both binaries share: the stop flag (Ctrl-C, SIGTERM, or a "stop"
//! line on stdin with PAKON_STOP_STDIN, as the web app's Stop button sends
//! on Windows) and line output.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Set by the first Ctrl-C / SIGTERM / "stop"; a second signal exits.
pub fn stop_flag() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let _ = ctrlc::set_handler(move || {
        if s.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
    });
    if std::env::var_os("PAKON_STOP_STDIN").is_some() {
        let s = stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
                if line.starts_with("stop") {
                    s.store(true, Ordering::SeqCst);
                }
                line.clear();
            }
        });
    }
    stop
}

/// Print a line and flush (the web app reads the output line by line).
pub fn say(line: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}
