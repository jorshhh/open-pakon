//! OEM scene balance against tools/oem_sba.py (rust/tools/golden_sba.py at git tag python-final
//! writes the reference into $PAKON_GOLDEN from $PAKON_GOLDEN_FULL's roll).

use std::path::PathBuf;
use std::time::Instant;

use ndarray::{s, Array1, Array2, Array3, Array4, Axis};
use ndarray_npy::read_npy;
use pakon_oem::sba;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn dpi_for_follows_sba_map() {
    let dir = sba::sba_dir(&repo());
    let f = |p, g| sba::dpi_for(&dir, p, g, sba::SOURCE_NEGATIVE_35).unwrap();
    assert_eq!(f(None, None), "sba-CN-default.dpi");
    assert_eq!(f(Some(78), Some(13)), "sba-CN-default-78-13.dpi");
    assert_eq!(f(Some(78), Some(12)), "sba-CN-default.dpi");
    assert_eq!(f(Some(96), Some(14)), "sba-CN-default-96-1.dpi");
    assert_eq!(f(Some(43), Some(2)), "sba-CN-default-43-1.dpi");
}

/// Runs the emulator once: on an x86_64 build this catches a Unicorn with
/// the wrong host code generator (it crashed every Intel Mac, 1.0.2).
#[test]
fn sba_loads() {
    let dll = sba::default_dll(&repo());
    if !dll.is_file() {
        eprintln!("skipped: no {}", dll.display());
        return;
    }
    sba::Sba::new(&dll, &sba::sba_dir(&repo()), "sba-CN-default.dpi").unwrap();
}

#[test]
fn sba_matches_python() {
    let (Some(gold), Some(full)) = (std::env::var_os("PAKON_GOLDEN"), std::env::var_os("PAKON_GOLDEN_FULL")) else { return };
    let gold = PathBuf::from(gold);
    if !gold.join("sba.json").is_file() {
        eprintln!("skipped: make the reference with rust/tools/golden_sba.py (git tag python-final)");
        return;
    }
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(gold.join("sba.json")).unwrap()).unwrap();
    let rgb: Array3<u16> = read_npy(PathBuf::from(full)).unwrap();
    let negmat: Array2<f32> = read_npy(gold.join("negmat.npy")).unwrap();
    let lut: Array1<f32> = read_npy(gold.join("lut.npy")).unwrap();
    let m: [[f32; 10]; 3] = std::array::from_fn(|c| std::array::from_fn(|k| negmat[(c, k)]));
    let lut = lut.to_vec();
    let length = j["length"].as_u64().unwrap() as usize;
    let starts: Vec<usize> = j["starts"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();

    let t = Instant::now();
    let imgs: Vec<Array3<i16>> =
        starts.iter().map(|&s0| sba::analysis_image(rgb.slice(s![s0..s0 + length, .., ..]), &m, &lut)).collect();
    eprintln!("analysis: {} frames in {:?}", imgs.len(), t.elapsed());
    let want: Array4<i16> = read_npy(gold.join("sba_analysis.npy")).unwrap();
    for (i, img) in imgs.iter().enumerate() {
        assert_eq!(img, &want.index_axis(Axis(0), i), "analysis image {i}");
    }

    let base = sba::film_base(rgb.view(), starts[0], j["rows_per_mm"].as_f64().unwrap(), 1.0, 2.0).unwrap();
    assert_eq!(serde_json::json!(base), j["base"]);
    let dmin = sba::film_base_rpd(base, &m, &lut);
    assert_eq!(serde_json::json!(dmin), j["dmin"]);

    let dir = sba::sba_dir(&repo());
    let dll = sba::default_dll(&repo());
    let dpi = j["dpi"].as_str().unwrap();
    let t = Instant::now();
    let shifts = sba::roll_shifts(&dll, &dir, &imgs[..5], dpi, Some(&imgs), Some(dmin)).unwrap();
    eprintln!("roll balance in {:?}: {shifts:?}", t.elapsed());
    assert_eq!(serde_json::json!(shifts), j["shifts"]);
    let solo = sba::roll_shifts(&dll, &dir, &imgs[..3], dpi, None, None).unwrap();
    assert_eq!(serde_json::json!(solo), j["solo"]);
}
