//! The routes without a scanner: request and response shapes as
//! `web/app.py` gives them (the full comparison against the Python server is
//! a script that runs both). No device commands are sent: the scan routes
//! are only called with arguments they refuse.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use pakon_web::App;
use serde_json::{json, Value};
use tower::ServiceExt;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pakon-web-api-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// (status, body: JSON, or the SSE events as a JSON list).
async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    let req = req.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let sse = resp.headers().get("content-type").is_some_and(|c| c.to_str().unwrap().starts_with("text/event-stream"));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    if sse {
        let events: Vec<Value> = text.split("\n\n").filter_map(|c| c.strip_prefix("data: ")).map(|j| serde_json::from_str(j).unwrap()).collect();
        return (status, Value::Array(events));
    }
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned())))
}

#[tokio::test]
async fn routes_without_a_scanner() {
    let data = temp("data");
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("PAKON_DATA", &data);
        std::env::set_var("PAKON_UPDATE_CHECK", "0");
        std::env::set_var("PAKON_TELEMETRY", "0");
    }
    let r = pakon_web::router(App::new(repo()));

    let (s, st) = call(&r, "GET", "/api/status", None).await;
    assert_eq!(s, StatusCode::OK);
    for key in ["state", "state_error", "scan_file", "scan_bytes", "work_dir", "next_order", "frame_count", "processing", "eeprom", "eeprom_unit", "firmware_hex", "update", "telemetry", "version"] {
        assert!(st.get(key).is_some(), "status has {key}");
    }
    assert_eq!(st["telemetry"], json!({"available": false, "asked": false, "usage": false, "crashes": false}));

    // the work folder and the order counter
    let work = temp("work");
    let (s, v) = call(&r, "POST", "/api/workdir", Some(json!({"path": work}))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"work_dir": work, "next_order": "000001"})));
    let (s, v) = call(&r, "POST", "/api/workdir", Some(json!({"path": "/no/such"}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "not a directory: /no/such"})));
    let (s, _) = call(&r, "POST", "/api/workdir", Some(json!({}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // the folder browser
    std::fs::create_dir_all(work.join("b")).unwrap();
    std::fs::create_dir_all(work.join("A")).unwrap();
    std::fs::create_dir_all(work.join(".hidden")).unwrap();
    std::fs::write(work.join("x.RAW"), b"").unwrap();
    std::fs::write(work.join("y.txt"), b"").unwrap();
    let (_, v) = call(&r, "GET", &format!("/api/browse?raws=true&path={}", work.display()), None).await;
    assert_eq!(v["dirs"], json!(["A", "b"]));
    assert_eq!(v["files"], json!(["x.RAW"]));
    assert_eq!(v["parent"], json!(work.parent().unwrap()));
    assert_eq!(v["virtual"], false);
    let (_, v) = call(&r, "GET", &format!("/api/browse?path={}", work.display()), None).await;
    assert_eq!(v["files"], json!([]));

    let (s, v) = call(&r, "POST", "/api/mkdir", Some(json!({"path": work, "name": " new "}))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"path": work.join("new")})));
    let (s, v) = call(&r, "POST", "/api/mkdir", Some(json!({"path": work, "name": "new"}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "folder already exists"})));
    let (s, v) = call(&r, "POST", "/api/mkdir", Some(json!({"path": work, "name": ".."}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "invalid folder name"})));

    // nothing to work on yet
    let (_, v) = call(&r, "POST", "/api/process", Some(json!({}))).await;
    assert_eq!(v, json!([{"type": "error", "message": "No raw file — run a scan or open a .raw file"}]));
    let (s, v) = call(&r, "POST", "/api/autoframe", Some(json!({"format": "full"}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "Process a roll first"})));
    let (_, v) = call(&r, "POST", "/api/export_frames", Some(json!({"centres": [1]}))).await;
    assert_eq!(v, json!([{"type": "error", "message": "Run a scan/process first"}]));
    let (_, v) = call(&r, "POST", "/api/save", Some(json!({"fmt": "bmp"}))).await;
    assert_eq!(v, json!([{"type": "error", "message": "unknown format: bmp"}]));
    let (_, v) = call(&r, "POST", "/api/save", Some(json!({"fmt": "jpeg"}))).await;
    assert_eq!(v, json!([{"type": "error", "message": "Export the frames first"}]));
    let (_, v) = call(&r, "GET", "/api/frames", None).await;
    assert_eq!(v["frames"], json!([]));
    assert_eq!(v["folder"], Value::Null);
    let (s, _) = call(&r, "GET", "/api/frames/1/thumb", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // opening a raw (in place) reads the film recorded beside it
    let raw = work.join("roll.raw");
    std::fs::write(&raw, b"").unwrap();
    std::fs::write(work.join("roll.raw.film"), "slide\n").unwrap();
    let (s, v) = call(&r, "POST", "/api/open", Some(json!({"path": raw}))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"path": raw, "film": "slide", "format": "full", "dx": null})));
    let (s, v) = call(&r, "POST", "/api/open", Some(json!({"path": work.join("y.txt")}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().starts_with("not a .raw file: "));
    let (_, v) = call(&r, "GET", "/api/frames", None).await;
    assert_eq!(v["folder"], json!(work.join("roll")));

    // scans refused before the scanner is touched
    let (s, v) = call(&r, "POST", "/api/scan", Some(json!({"film": "kodachrome"}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "unknown film type: kodachrome"})));
    let (s, v) = call(&r, "POST", "/api/scan", Some(json!({"base": 5}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "unknown resolution: Base 5"})));
    let (s, v) = call(&r, "POST", "/api/scan/stop", None).await;
    assert_eq!((s, v), (StatusCode::CONFLICT, json!({"error": "no scan is running"})));

    // reveal only inside the work folder; no window to focus
    let (s, v) = call(&r, "POST", "/api/reveal", Some(json!({"path": "/etc"}))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, json!({"error": "not available"})));
    let (s, v) = call(&r, "POST", "/api/app/focus", None).await;
    assert_eq!((s, v), (StatusCode::NOT_FOUND, json!({"error": "no app window"})));
    // no native dialog either: the page uses /api/browse
    let (s, v) = call(&r, "POST", "/api/pick", Some(json!({"kind": "raw"}))).await;
    assert_eq!((s, v), (StatusCode::NOT_FOUND, json!({"error": "no app window"})));

    // the telemetry choice is kept even where nothing is sent
    let (_, v) = call(&r, "POST", "/api/telemetry/consent", Some(json!({"usage": true, "crashes": false}))).await;
    assert_eq!(v, json!({"available": false, "asked": true, "usage": true, "crashes": false}));
    let (_, v) = call(&r, "POST", "/api/telemetry/page", Some(json!({"page": "scan"}))).await;
    assert_eq!(v, json!({"ok": true}));

    // the page
    let (s, v) = call(&r, "GET", "/", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.as_str().unwrap().contains("js/main.js"));
    // and its modules, beside it
    let (s, v) = call(&r, "GET", "/js/status.js", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.as_str().unwrap().contains("/api/status"));

    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&data);
}
