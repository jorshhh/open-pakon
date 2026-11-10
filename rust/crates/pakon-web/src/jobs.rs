//! The image routes: open a raw, prescan (Process), auto-frame, export the
//! frames, save them to the work folder. The pipeline is `pakon-roll`; this
//! is the state around it and the files beside a raw.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use pakon_image::film::Film;
use pakon_image::framing::Format;
use pakon_roll::{ExportRequest, FilmKind, FrameFiles, Progress};
use serde_json::{json, Value};

use crate::files::reveal_label;
use crate::paths::{beside, expand_user, resolve};
use crate::sse::{self, refuse};
use crate::{error, fields, missing, App};

pub(crate) const FILMS: [&str; 4] = ["c41", "bw", "bwc41", "slide"];
const FRAME_FORMATS: [&str; 3] = ["full", "half", "pano"];

fn film_kind(name: &str) -> FilmKind {
    serde_json::from_value(json!(name)).unwrap_or_default()
}

fn frame_format(name: &str) -> Format {
    serde_json::from_value(json!(name)).unwrap_or_default()
}

/// The film a DX code names, as the page gets it.
pub(crate) fn film_json(film: Option<Film>) -> Value {
    match film {
        Some(f) => json!({"name": f.name, "iso": f.iso, "product": f.product, "generation": f.generation}),
        None => Value::Null,
    }
}

// ── Film type of a raw: <raw>.film beside it ─────────────────────────────────
// Written by the scan and whenever the operator picks another type in
// Process; a code-built scan also records it in <raw>.flat.json. Colour
// negative when neither says.

fn read_choice(path: &Path, allowed: &[&str]) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let choice = text.trim();
    allowed.contains(&choice).then(|| choice.to_string())
}

pub(crate) fn raw_film(raw: &Path) -> String {
    if let Some(f) = read_choice(&beside(raw, ".film"), &FILMS) {
        return f;
    }
    let flat: Option<Value> = std::fs::read_to_string(beside(raw, ".flat.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    match flat.as_ref().and_then(|f| f["film"].as_str()) {
        Some(f) if FILMS.contains(&f) => f.to_string(),
        _ => "c41".to_string(),
    }
}

pub(crate) fn save_film(raw: &Path, film: &str) {
    if let Err(e) = std::fs::write(beside(raw, ".film"), format!("{film}\n")) {
        println!("film type not saved: {e}");
    }
}

// ── Picture format of a raw: <raw>.format beside it ──────────────────────────
// full | half | pano. Written when the operator auto-frames the raw in
// Frames. Full frame when absent.

fn raw_format(raw: &Path) -> String {
    read_choice(&beside(raw, ".format"), &FRAME_FORMATS).unwrap_or_else(|| "full".to_string())
}

fn save_format(raw: &Path, fmt: &str) {
    if let Err(e) = std::fs::write(beside(raw, ".format"), format!("{fmt}\n")) {
        println!("format not saved: {e}");
    }
}

// ── Saved crop positions: <raw>.crops.json beside the raw ────────────────────
// Written at export, read back when the same raw is processed again, so the
// operator's boxes come back.

fn load_crops(raw: &Path, rows: usize) -> Value {
    let saved: Option<Value> = std::fs::read_to_string(beside(raw, ".crops.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    // another decode of the raw (layout override, new ribbon code) can
    // change the ribbon length; the positions would no longer line up
    match saved {
        Some(s) if s["rows"].as_u64() == Some(rows as u64) && s["centres"].as_array().is_some_and(|c| !c.is_empty()) => s,
        _ => Value::Null,
    }
}

fn save_crops(raw: &Path, rows: usize, centres: &[i64], widths: &[usize]) {
    let text = serde_json::to_string_pretty(&json!({"rows": rows, "centres": centres, "widths": widths})).unwrap_or_default();
    if let Err(e) = std::fs::write(beside(raw, ".crops.json"), text) {
        println!("crop positions not saved: {e}");
    }
}

// ── Jobs ─────────────────────────────────────────────────────────────────────

/// Run `work(progress)` on the blocking pool, one job at a time, streaming
/// its progress; `done(result)` builds the final 'done' event and the
/// usage statistics. Reports `<name>_done` or the failure.
fn run_job<T, W, D>(app: Arc<App>, name: &'static str, page: &'static str, work: W, done: D) -> Response
where
    T: Send + 'static,
    W: FnOnce(Progress) -> Result<T, String> + Send + 'static,
    D: FnOnce(&App, T) -> (Value, Value) + Send + 'static,
{
    let (tx, resp) = sse::channel();
    tokio::task::spawn_blocking(move || {
        let _one = app.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let t0 = Instant::now();
        let ptx = tx.clone();
        let progress = move |step: &str, pct: f64| {
            let _ = ptx.send(json!({"type": "progress", "step": step, "pct": pct}));
        };
        let result = catch_unwind(AssertUnwindSafe(|| work(&progress))).unwrap_or_else(|_| Err("internal error, see the app's log".to_string()));
        match result {
            Ok(value) => {
                let (event, stats) = done(&app, value);
                app.roll().processing = false;
                let _ = tx.send(event);
                let mut props = stats.as_object().cloned().unwrap_or_default();
                props.insert("seconds".into(), json!(t0.elapsed().as_secs_f64().round() as u64));
                app.telemetry.event(&format!("{name}_done"), page, Value::Object(props));
            }
            Err(e) => {
                app.roll().processing = false;
                app.telemetry.error("Error", &format!("{name}: {e}"), "", page);
                let _ = tx.send(json!({"type": "error", "message": e}));
            }
        }
    });
    resp
}

/// Make a .raw on the host the file Process decodes (in place, no copy).
pub async fn api_open(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(path) = f.get("path").and_then(Value::as_str) else { return missing("path") };
    let raw = expand_user(path);
    let is_raw = raw.extension().is_some_and(|e| e.eq_ignore_ascii_case("raw"));
    if !is_raw || !raw.is_file() {
        return error(StatusCode::BAD_REQUEST, format!("not a .raw file: {}", raw.display()));
    }
    let raw = resolve(&raw);
    let mut roll = app.roll();
    roll.scan_path = Some(raw.clone());
    roll.frames.clear();
    roll.prescan = None;
    drop(roll);
    Json(json!({
        "path": raw.to_string_lossy(),
        "film": raw_film(&raw),
        "format": raw_format(&raw),
        "dx": film_json(pakon_roll::dx_for_raw(&app.paths.root, &raw)),
    }))
    .into_response()
}

/// Prescan: cache the ribbon and render the preview strip. `film`
/// (c41 | bw | bwc41 | slide) defaults to the one recorded for the raw. No
/// boxes are placed: Frames restores the saved ones, and /api/autoframe
/// places new ones when the operator asks.
pub async fn api_process(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let raw = {
        let mut roll = app.roll();
        if roll.processing {
            return refuse("Already processing");
        }
        let Some(raw) = roll.scan_path.clone().filter(|p| p.exists()) else {
            return refuse("No raw file — run a scan or open a .raw file");
        };
        roll.frames.clear();
        roll.prescan = None;
        roll.processing = true;
        raw
    };
    let film = match f.get("film").and_then(Value::as_str) {
        None => raw_film(&raw),
        Some(f) if !FILMS.contains(&f) => "c41".to_string(),
        Some(f) => {
            save_film(&raw, f);
            f.to_string()
        }
    };
    let fmt = raw_format(&raw);
    let env = app.env();
    let (job_raw, job_film) = (raw.clone(), film.clone());
    run_job(
        app,
        "process",
        "process",
        move |progress| pakon_roll::prescan(&env, &job_raw, film_kind(&job_film), frame_format(&fmt), Some(progress)),
        move |app, meta| {
            // hand the browser everything it needs to place crops (no ribbon path)
            let all = serde_json::to_value(&meta).unwrap_or_default();
            let mut event = json!({"type": "done", "saved": load_crops(&raw, meta.rows)});
            for key in ["rows", "cols", "pitch", "frame_w", "box_w", "preview_w", "preview_h", "scale", "film", "format", "ice"] {
                event[key] = all[key].clone();
            }
            app.roll().prescan = Some(meta);
            (event, json!({"film": film}))
        },
    )
}

/// Place a box on every picture with the OEM framing in `format`
/// (full | half | pano), and remember the format for the raw. The boxes
/// replace nothing on disk: <raw>.crops.json is written only at export.
pub async fn api_autoframe(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let format = f.get("format").and_then(Value::as_str).unwrap_or("full").to_string();
    let (ribbon, film, raw) = {
        let roll = app.roll();
        let Some(meta) = &roll.prescan else {
            return error(StatusCode::BAD_REQUEST, "Process a roll first");
        };
        if !FRAME_FORMATS.contains(&format.as_str()) {
            return error(StatusCode::BAD_REQUEST, format!("unknown format: {format}"));
        }
        if roll.processing {
            return error(StatusCode::CONFLICT, "Already processing");
        }
        (PathBuf::from(&meta.ribbon), meta.film, roll.scan_path.clone())
    };
    let (me, fmt) = (app.clone(), frame_format(&format));
    let found = tokio::task::spawn_blocking(move || {
        let _one = me.jobs.lock().unwrap_or_else(|e| e.into_inner());
        pakon_roll::autoframe(&ribbon, film, fmt)
    })
    .await;
    let found = match found {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal error, see the app's log"),
    };
    // the scene balance weighs exported frames against these (export_frames)
    if let Some(meta) = &mut app.roll().prescan {
        meta.centres = found.centres.clone();
        meta.frame_w = found.frame_w;
        meta.pitch = found.pitch;
        meta.format = found.format;
    }
    if let Some(raw) = raw {
        save_format(&raw, &format);
    }
    app.telemetry.event("autoframe", "frames", json!({"format": format, "frames": found.centres.len()}));
    Json(found).into_response()
}

fn jpeg(path: &Path) -> Option<Response> {
    let bytes = std::fs::read(path).ok()?;
    Some(([(CONTENT_TYPE, "image/jpeg")], bytes).into_response())
}

pub async fn api_preview(State(app): State<Arc<App>>) -> Response {
    jpeg(&app.env().preview_path()).unwrap_or_else(|| error(StatusCode::NOT_FOUND, "no preview"))
}

/// Stage 2: export the operator-confirmed crops at full resolution.
pub async fn api_export_frames(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(centres) = f.get("centres").and_then(Value::as_array) else { return missing("centres") };
    let centres: Vec<i64> = centres.iter().filter_map(|c| c.as_i64().or_else(|| c.as_f64().map(|v| v as i64))).collect();
    let widths: Option<Vec<usize>> = f.get("widths").and_then(Value::as_array).map(|w| w.iter().filter_map(|v| v.as_u64().map(|v| v as usize)).collect());
    let ice = f.get("ice").and_then(Value::as_bool).unwrap_or(false);
    // Sharpening at export, the OEM's unsharp mask at 1 / 2 / 3 TLX clicks.
    let sharpen = f.get("sharpen").and_then(Value::as_str).unwrap_or("off");
    let amount = pakon_roll::SHARPEN_LEVELS.iter().find(|l| l.0 == sharpen).map_or(0, |l| l.1);

    let (meta, raw) = {
        let mut roll = app.roll();
        let Some(meta) = roll.prescan.clone() else {
            return refuse("Run a scan/process first");
        };
        if roll.processing {
            return refuse("Already processing");
        }
        roll.frames.clear();
        roll.processing = true;
        (meta, roll.scan_path.clone())
    };
    let ice = ice && meta.ice;
    let f32s = |v: [f64; 3]| v.map(|x| x as f32);
    let adjust = pakon_roll::adjust_from_json(&json!({"sharpness": amount}));
    let req = ExportRequest {
        ribbon: PathBuf::from(&meta.ribbon),
        base: f32s(meta.base),
        centres: centres.clone(),
        widths: widths.clone(),
        rotate: 90,
        frame_w: meta.frame_w,
        rpd_balance: meta.rpd_balance.map(|b| b.map(f32s)),
        roll_centres: Some(meta.centres.clone()),
        film: meta.film,
        flare: meta.flare.map(f32s),
        dx_film: raw.as_ref().and_then(|r| pakon_roll::dx_for_raw(&app.paths.root, r)),
        ice,
        sba_dpi: meta.sba_dpi.clone(),
        scene_dmin: meta.scene_dmin,
        adjustments: (amount != 0).then(|| vec![adjust; centres.len()]),
    };
    let env = app.env();
    run_job(
        app,
        "export",
        "frames",
        move |progress| pakon_roll::export_frames(&env, &req, Some(progress)),
        move |app, frames: Vec<FrameFiles>| {
            let n = frames.len();
            app.roll().frames = frames;
            if let Some(raw) = &raw {
                let widths = widths.unwrap_or_else(|| vec![meta.frame_w; centres.len()]);
                save_crops(raw, meta.rows, &centres, &widths);
            }
            (json!({"type": "done", "count": n}), json!({"frames": n, "film": meta.film, "ice": ice}))
        },
    )
}

pub async fn api_frames(State(app): State<Arc<App>>) -> Response {
    let roll = app.roll();
    Json(json!({
        "folder": roll.scan_path.as_ref().map(|raw| app.settings.export_dir(raw).to_string_lossy().into_owned()),
        "reveal": reveal_label(), // /api/reveal works
        "frames": roll.frames.iter().map(|f| json!({"index": f.index})).collect::<Vec<_>>(),
    }))
    .into_response()
}

pub async fn api_thumb(State(app): State<Arc<App>>, UrlPath(n): UrlPath<usize>) -> Response {
    let thumb = app.roll().frames.iter().find(|f| f.index == n).map(|f| f.thumb.clone());
    thumb.and_then(|p| jpeg(&p)).unwrap_or_else(|| error(StatusCode::NOT_FOUND, "not found"))
}

// ── Save ─────────────────────────────────────────────────────────────────────
// Saved exports: <export dir>/<subfolder>/frame_NN.<ext> (orders.rs
// export_dir), from the rendered files in the work folder.

/// fmt -> (subfolder, file extension, what the page calls them).
fn save_format_of(fmt: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match fmt {
        "jpeg" => Some(("jpeg", "jpg", "JPEGs")),        // rendered positive
        "tiff" => Some(("tiff", "tif", "TIFFs")),        // 16-bit positive
        "raw" => Some(("raw", "tif", "raw negatives")),  // 16-bit, uninverted
        _ => None,
    }
}

fn save_frames(env: &pakon_roll::Env, out: &Path, frames: &[FrameFiles], fmt: &str, progress: Progress) -> Result<Value, String> {
    let io = |e: std::io::Error| e.to_string();
    if fmt == "contact" {
        progress("Making the contact sheet", 0.1);
        let sheet = pakon_roll::contact_sheet(env, frames)?.ok_or("no rendered frames")?;
        std::fs::create_dir_all(out).map_err(io)?;
        let path = out.join("contact_sheet.jpg");
        std::fs::copy(&sheet, &path).map_err(io)?;
        progress("Saved", 1.0);
        return Ok(json!({"folder": out.to_string_lossy(), "file": path.to_string_lossy(), "count": 1, "what": "contact sheet"}));
    }
    let (sub, ext, what) = save_format_of(fmt).ok_or_else(|| format!("unknown format: {fmt}"))?;
    let folder = out.join(sub);
    std::fs::create_dir_all(&folder).map_err(io)?;
    // an earlier save of this roll may have had more frames
    for old in std::fs::read_dir(&folder).map_err(io)?.flatten() {
        let name = old.file_name().to_string_lossy().into_owned();
        if name.starts_with("frame_") && name.ends_with(&format!(".{ext}")) {
            let _ = std::fs::remove_file(old.path());
        }
    }
    let n = frames.len();
    for (i, f) in frames.iter().enumerate() {
        progress(&format!("Saving frame {}/{n}", i + 1), i as f64 / n as f64);
        let from = match fmt {
            "jpeg" => &f.jpg,
            "tiff" => &f.tiff,
            _ => &f.raw_tiff,
        };
        std::fs::copy(from, folder.join(format!("frame_{:02}.{ext}", f.index))).map_err(io)?;
    }
    progress("Saved", 1.0);
    Ok(json!({"folder": folder.to_string_lossy(), "count": n, "what": what}))
}

/// Save the exported frames of the current roll into the work folder.
pub async fn api_save(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let f = fields(&body);
    let Some(fmt) = f.get("fmt").and_then(Value::as_str).map(str::to_string) else { return missing("fmt") };
    let (frames, raw) = {
        let mut roll = app.roll();
        if fmt != "contact" && save_format_of(&fmt).is_none() {
            return refuse(&format!("unknown format: {fmt}"));
        }
        let Some(raw) = roll.scan_path.clone().filter(|_| !roll.frames.is_empty()) else {
            return refuse("Export the frames first");
        };
        if roll.processing {
            return refuse("Already processing");
        }
        roll.processing = true;
        (roll.frames.clone(), raw)
    };
    let (env, out) = (app.env(), app.settings.export_dir(&raw));
    let job_fmt = fmt.clone();
    run_job(
        app,
        "save",
        "export",
        move |progress| save_frames(&env, &out, &frames, &job_fmt, progress),
        move |_, result: Value| {
            let stats = json!({"format": fmt, "frames": result["count"]});
            let mut event = result;
            event["type"] = json!("done");
            (event, stats)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pakon-web-jobs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_film_of_a_raw() {
        let d = temp("film");
        let raw = d.join("scan.raw");
        assert_eq!(raw_film(&raw), "c41");
        std::fs::write(beside(&raw, ".flat.json"), r#"{"film": "slide"}"#).unwrap();
        assert_eq!(raw_film(&raw), "slide");
        save_film(&raw, "bw");
        assert_eq!(std::fs::read_to_string(d.join("scan.raw.film")).unwrap(), "bw\n");
        assert_eq!(raw_film(&raw), "bw");
        std::fs::write(beside(&raw, ".film"), "kodachrome\n").unwrap();
        assert_eq!(raw_film(&raw), "slide", "an unknown type falls back to the flat file");
        assert_eq!(film_kind("bwc41"), FilmKind::Bwc41);
        assert_eq!(film_kind("nope"), FilmKind::C41);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_format_and_crops_of_a_raw() {
        let d = temp("crops");
        let raw = d.join("scan.raw");
        assert_eq!(raw_format(&raw), "full");
        save_format(&raw, "pano");
        assert_eq!(raw_format(&raw), "pano");
        assert_eq!(frame_format("half"), Format::Half);
        assert_eq!(load_crops(&raw, 100), Value::Null);
        save_crops(&raw, 100, &[10, 60], &[40, 40]);
        assert_eq!(load_crops(&raw, 100), json!({"rows": 100, "centres": [10, 60], "widths": [40, 40]}));
        assert_eq!(load_crops(&raw, 101), Value::Null, "another ribbon length");
        save_crops(&raw, 100, &[], &[]);
        assert_eq!(load_crops(&raw, 100), Value::Null, "no boxes");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saving_copies_the_frames_and_clears_older_ones() {
        let d = temp("save");
        let work = d.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let frames: Vec<FrameFiles> = [1usize, 2]
            .iter()
            .map(|&i| {
                let file = |ext: &str| {
                    let p = work.join(format!("frame_{i:02}{ext}"));
                    std::fs::write(&p, format!("{i}{ext}")).unwrap();
                    p
                };
                FrameFiles { index: i, raw_tiff: file("_raw.tif"), tiff: file(".tif"), jpg: file(".jpg"), thumb: file("_t.jpg") }
            })
            .collect();
        let out = d.join("order");
        std::fs::create_dir_all(out.join("tiff")).unwrap();
        std::fs::write(out.join("tiff").join("frame_09.tif"), "old").unwrap();
        std::fs::write(out.join("tiff").join("notes.txt"), "keep").unwrap();
        let env = pakon_roll::Env::from_env(&d, None);
        let steps = std::sync::Mutex::new(vec![]);
        let progress = |s: &str, p: f64| steps.lock().unwrap().push((s.to_string(), p));
        let r = save_frames(&env, &out, &frames, "tiff", &progress).unwrap();
        assert_eq!(r, json!({"folder": out.join("tiff").to_string_lossy(), "count": 2, "what": "TIFFs"}));
        assert_eq!(std::fs::read_to_string(out.join("tiff").join("frame_02.tif")).unwrap(), "2.tif");
        assert!(!out.join("tiff").join("frame_09.tif").exists());
        assert!(out.join("tiff").join("notes.txt").exists());
        let r = save_frames(&env, &out, &frames, "raw", &progress).unwrap();
        assert_eq!(r["what"], "raw negatives");
        assert_eq!(std::fs::read_to_string(out.join("raw").join("frame_01.tif")).unwrap(), "1_raw.tif");
        assert_eq!(steps.lock().unwrap().last().unwrap(), &("Saved".to_string(), 1.0));
        assert!(save_frames(&env, &out, &frames, "bmp", &progress).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
