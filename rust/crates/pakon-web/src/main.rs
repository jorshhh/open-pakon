//! `pakon_web`: the web service without a window, as
//! `uvicorn web.app:app --host 0.0.0.0 --port 8000` was.
//!
//!     pakon_web [--host 0.0.0.0] [--port 8000]
//!
//! Run from the repo (or set PAKON_ROOT to the folder that holds
//! `web/static`). Ctrl-C ends a running scan as at the end of the roll and
//! waits for the teardown before exiting.

use pakon_web::{paths, App};

fn usage() -> ! {
    eprintln!("usage: pakon_web [--host ADDR] [--port N]   (PAKON_ROOT: the folder with web/static)");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut host, mut port) = ("0.0.0.0".to_string(), 8000u16);
    let mut i = 0;
    while i < args.len() {
        match (args[i].as_str(), args.get(i + 1)) {
            ("--host", Some(v)) => host = v.clone(),
            ("--port", Some(v)) => port = v.parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
        i += 2;
    }
    let Some(root) = paths::find_root() else {
        eprintln!("pakon_web: web/static/index.html not found above this program or the current folder; set PAKON_ROOT");
        std::process::exit(1);
    };
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pakon_web: cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    println!("Pakon on http://{host}:{port}/ (files from {})", root.display());
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
        println!("stopping: ending any scan and tearing the scanner down...");
    };
    if let Err(e) = pakon_web::serve(listener, App::new(root), stop).await {
        eprintln!("pakon_web: {e}");
        std::process::exit(1);
    }
}
