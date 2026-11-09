//! Per-column flat field (TLB.dll FUN_1001f550 + the per-line pass; port of
//! `find_flat_field`, `flat_row_layout`, `load_flat_field`,
//! `apply_flat_field` in `tools/pakon_image.py`).
//!
//! The OEM corrects every scan line per column: `(raw - dark[c]) * gain[c]`,
//! `gain[c] = 64000 / ((bright[c] - dark[c]) - black drift)`, so every column
//! reads 64000 through the empty gate. Applied in the raw 16-bit domain,
//! keeping the ~300 black.

use std::path::{Path, PathBuf};

use ndarray::{Array2, Array3, Axis};
use rayon::prelude::*;
use serde::Deserialize;

use crate::RAW_BLACK;

/// The fields of a `.flat.json` / `flatfield*.json` the pipeline reads.
#[derive(Debug, Clone, Deserialize)]
pub struct FlatJson {
    pub line_px: usize,
    pub px0: usize,
    pub black_px: usize,
    pub dark: Vec<Vec<f64>>,
    pub bright: Vec<Vec<f64>>,
    #[serde(default)]
    pub film: Option<String>,
    #[serde(default)]
    pub duties: Option<Vec<f64>>,
    #[serde(default)]
    pub scan_duties: Option<Vec<f64>>,
    #[serde(default)]
    pub row_samples: Option<usize>,
    #[serde(default)]
    pub ir_lane: Option<bool>,
}

impl FlatJson {
    pub fn read(path: impl AsRef<Path>) -> Result<FlatJson, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn own_flat(raw: &Path) -> PathBuf {
    let mut p = raw.as_os_str().to_owned();
    p.push(".flat.json");
    PathBuf::from(p)
}

/// The flat-field file for a raw: its own `<raw>.flat.json`, else
/// `flatfield.json` / `flatfield-base8.json` / `flatfield-base4.json` in the
/// EEPROM archive, taken only if it covers `width` columns (when given).
pub fn find_flat_field(raw: &Path, eeprom_dir: Option<&Path>, width: Option<usize>) -> Option<PathBuf> {
    let own = own_flat(raw);
    if own.is_file() {
        return Some(own);
    }
    let dir = eeprom_dir?;
    ["flatfield.json", "flatfield-base8.json", "flatfield-base4.json"].iter().map(|n| dir.join(n)).find(|p| {
        p.is_file()
            && match width {
                None => true,
                Some(w) => FlatJson::read(p).is_ok_and(|f| f.line_px.checked_sub(f.px0) == Some(w)),
            }
    })
}

/// (row_samples, ir_lane) recorded in the raw's own `.flat.json`: the stride
/// alone is ambiguous (F-135 Base 8 + IR and F-135+ Base 16 are both 6000).
pub fn flat_row_layout(raw: &Path) -> Option<(usize, bool)> {
    let f = FlatJson::read(own_flat(raw)).ok()?;
    Some((f.row_samples?, f.ir_lane.unwrap_or(true)))
}

/// Per scan column: dark level and gain, `(width, 3)` f32 each.
#[derive(Debug, Clone, PartialEq)]
pub struct Flat {
    pub dark: Array2<f32>,
    pub gain: Array2<f32>,
}

/// Mean of a short f64 slice (calibration tables).
fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// The flat field for scan columns `0..width`. A slide scan runs its LEDs
/// brighter than the calibration, so its gain is scaled by calibration duty
/// / scan duty per channel; other films keep the OEM behaviour.
pub fn load_flat_field(ff: &FlatJson, width: usize) -> Result<Flat, String> {
    let (lp, px0, nb) = (ff.line_px, ff.px0, ff.black_px);
    if lp.checked_sub(px0) != Some(width) {
        return Err(format!("flat field covers {} columns, the scan has {width}", lp as i64 - px0 as i64));
    }
    if ff.dark.len() < 3 || ff.bright.len() < 3 || (0..3).any(|c| ff.dark[c].len() != lp || ff.bright[c].len() != lp) {
        return Err(format!("flat field tables are not 3 x {lp}"));
    }
    let mut scale = [1.0f64; 3];
    if ff.film.as_deref() == Some("slide") {
        if let (Some(cal), Some(scan)) = (&ff.duties, &ff.scan_duties) {
            if cal.len() >= 3 && scan.len() >= 3 && cal[..3].iter().chain(&scan[..3]).all(|&v| v > 0.0) {
                scale = std::array::from_fn(|c| cal[c] / scan[c]);
            }
        }
    }
    let mut dark = Array2::<f32>::zeros((width, 3));
    let mut gain = Array2::<f32>::zeros((width, 3));
    for c in 0..3 {
        let (d, b) = (&ff.dark[c], &ff.bright[c]);
        let drift = mean(&b[..nb]) - mean(&d[..nb]);
        for x in 0..width {
            let i = px0 + x;
            let den = (b[i] - d[i]) - drift;
            let g = if den > 0.0 { 64000.0 / den.max(1e-9) } else { 0.0 };
            let g = g.min(0x3FFFF as f64 / 65536.0) * scale[c];
            dark[(x, c)] = d[i] as f32;
            gain[(x, c)] = g as f32;
        }
    }
    Ok(Flat { dark, gain })
}

/// In place on (rows, width, 3): `black + (raw - dark) * gain`, clipped to
/// 0..65535, rounded (the same f32 steps as numpy).
pub fn apply_flat_field(rgb: &mut Array3<u16>, flat: &Flat) {
    rgb.axis_iter_mut(Axis(0)).into_par_iter().for_each(|mut row| {
        for ((px, c), v) in row.indexed_iter_mut() {
            let mut x = *v as f32;
            x -= flat.dark[(px, c)];
            x *= flat.gain[(px, c)];
            x += RAW_BLACK;
            x = x.clamp(0.0, 65535.0);
            *v = (x + 0.5) as u16;
        }
    });
}
