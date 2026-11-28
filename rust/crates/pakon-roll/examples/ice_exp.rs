//! Prescan a raw and export every frame, with or without dust removal, into
//! OUT/work (the ribbon, IR plane, TIFFs and JPEGs), outside the app's own
//! cache. For dust-removal experiments (docs/SLIDE_ICE.md): stderr gives the
//! share of each frame that was rebuilt. Dust removal runs only where the
//! prescan keeps the IR plane (every film but silver B&W).
//!
//!   cargo run --release -p pakon-roll --example ice_exp -- RAW OUT FILM [1|0]
//!
//! FILM: c41, bw, bwc41 or slide; the last argument turns dust removal on
//! (default) or off. PAKON_OEM_DICE=0 uses the native port instead of DICE.
use pakon_image::framing::Format;
use pakon_roll::*;
use std::path::{Path, PathBuf};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = PathBuf::from(&a[1]);
    let out = PathBuf::from(&a[2]);
    let film: FilmKind = serde_json::from_value(serde_json::json!(a[3])).unwrap();
    let fmt: Format = serde_json::from_value(serde_json::json!("full")).unwrap();
    let ice = a.get(4).map_or(true, |v| v == "1");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut env = Env::from_env(&repo, None);
    env.work_dir = out.join("work");
    let meta = prescan(&env, &raw, film, fmt, None).unwrap();
    let f = |v: [f64; 3]| v.map(|x| x as f32);
    let req = ExportRequest {
        ribbon: PathBuf::from(&meta.ribbon), base: f(meta.base), centres: meta.centres.clone(), widths: None,
        rotate: 90, frame_w: meta.frame_w, rpd_balance: meta.rpd_balance.map(|b| b.map(f)),
        roll_centres: Some(meta.centres.clone()), film, flare: meta.flare.map(f), dx_film: None, ice,
        sba_dpi: meta.sba_dpi.clone(), scene_dmin: meta.scene_dmin, adjustments: None, orient: None,
        ..Default::default()
    };
    for fr in export_frames(&env, &req, None).unwrap() { println!("{}", fr.jpg.display()); }
}
