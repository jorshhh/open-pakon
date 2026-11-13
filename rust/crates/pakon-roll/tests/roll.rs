//! Prescan + export end to end against web/decode.py
//! (rust/tools/golden_roll.py, at git tag python-final, writes the reference into $PAKON_GOLDEN).

use std::path::{Path, PathBuf};

use ndarray::{Array2, Array3};
use ndarray_npy::read_npy;
use pakon_image::framing::Format;
use pakon_roll::{adjust_from_json, Env, ExportRequest, FilmKind};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The pixels of one of our own TIFFs (one strip, little endian).
fn tiff_pixels(path: &Path) -> Vec<u16> {
    let b = std::fs::read(path).unwrap();
    let n = u16::from_le_bytes([b[8], b[9]]) as usize;
    let tag = |t: u16| {
        (0..n).map(|i| 10 + 12 * i).find(|&e| u16::from_le_bytes([b[e], b[e + 1]]) == t).map(|e| u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize).unwrap()
    };
    let (off, len) = (tag(273), tag(279));
    b[off..off + len].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

#[test]
fn slice_matches_python() {
    if let Some(gold) = std::env::var_os("PAKON_GOLDEN").map(PathBuf::from) {
        roll_matches_python(&gold);
    }
}

/// The whole roll: PAKON_GOLDEN_ROLL=DIR after `golden_roll.py DIR RAW`.
#[test]
fn full_roll_matches_python() {
    if let Some(gold) = std::env::var_os("PAKON_GOLDEN_ROLL").map(PathBuf::from) {
        roll_matches_python(&gold);
    }
}

fn roll_matches_python(gold: &Path) {
    if !gold.join("roll.json").is_file() {
        eprintln!("skipped: make the reference with rust/tools/golden_roll.py (git tag python-final)");
        return;
    }
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(gold.join("roll.json")).unwrap()).unwrap();
    let want = &j["meta"];
    let mut env = Env::from_env(&repo(), None);
    env.eeprom_dir = Some(repo().join("backups/eeprom/F135-3054-cycle1"));
    env.work_dir = gold.join("roll-rs");
    let py = gold.join("roll-py");

    let t = std::time::Instant::now();
    let meta = pakon_roll::prescan(&env, Path::new(j["raw"].as_str().unwrap()), FilmKind::C41, Format::Full, Some(&|s: &str, p: f64| eprintln!("  {p:.3} {s}"))).unwrap();
    eprintln!("prescan in {:?}", t.elapsed());
    let mut got = serde_json::to_value(&meta).unwrap();
    got["ribbon"] = want["ribbon"].clone();
    assert_eq!(got, *want, "prescan meta");
    let a: Array3<u16> = read_npy(env.ribbon_path()).unwrap();
    let b: Array3<u16> = read_npy(py.join("ribbon.npy")).unwrap();
    assert_eq!(a, b, "ribbon cache");
    let a: Array2<u16> = read_npy(env.ir_ribbon_path()).unwrap();
    let b: Array2<u16> = read_npy(py.join("ir_ribbon.npy")).unwrap();
    assert_eq!(a, b, "IR cache");
    let pv = image::open(env.preview_path()).unwrap();
    assert_eq!((pv.width() as usize, pv.height() as usize), (meta.preview_w, meta.preview_h));

    let req = ExportRequest {
        ribbon: env.ribbon_path(),
        base: meta.base.map(|v| v as f32),
        centres: meta.centres.clone(),
        rotate: 90,
        frame_w: meta.frame_w,
        rpd_balance: meta.rpd_balance.map(|b| [b[0].map(|v| v as f32), b[1].map(|v| v as f32)]),
        roll_centres: Some(meta.centres.clone()),
        film: FilmKind::C41,
        ice: meta.ice,
        sba_dpi: meta.sba_dpi.clone(),
        scene_dmin: meta.scene_dmin,
        adjustments: Some(j["adjust"].as_array().unwrap().iter().map(adjust_from_json).collect()),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let frames = pakon_roll::export_frames(&env, &req, None).unwrap();
    eprintln!("export of {} frames in {:?}", frames.len(), t.elapsed());
    assert_eq!(frames.len() as u64, j["n"].as_u64().unwrap());
    for f in &frames {
        let w: Array3<u16> = read_npy(py.join(format!("rawt_{:02}.npy", f.index))).unwrap();
        assert_eq!(tiff_pixels(&f.raw_tiff), w.iter().copied().collect::<Vec<_>>(), "raw tiff {}", f.index);
        let w: Array3<u16> = read_npy(py.join(format!("pos_{:02}.npy", f.index))).unwrap();
        let g = tiff_pixels(&f.tiff);
        let bad = g.iter().zip(w.iter()).filter(|(a, b)| a != b).count();
        eprintln!("frame {}: {bad} of {} samples differ", f.index, g.len());
        assert_eq!(bad, 0, "positive tiff {}", f.index);
    }

    let req = ExportRequest { film: FilmKind::Bw, centres: meta.centres[..2].to_vec(), roll_centres: None, ice: false, sba_dpi: None, scene_dmin: None, adjustments: None, ..req };
    let frames = pakon_roll::export_frames(&env, &req, None).unwrap();
    for f in &frames {
        let w: Array2<u16> = read_npy(py.join(format!("grey_{:02}.npy", f.index))).unwrap();
        let g = tiff_pixels(&f.tiff);
        let maxd = g.iter().zip(w.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        eprintln!("grey frame {}: max diff {maxd}", f.index);
        assert!(maxd <= 1);
    }
    let sheet = pakon_roll::contact_sheet(&env, &frames).unwrap().unwrap();
    assert!(sheet.is_file());
}
