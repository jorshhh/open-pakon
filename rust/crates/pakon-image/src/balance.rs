//! Film base (Dmin) and the roll balance (port of `measure_dmin` and
//! `oem_roll_balance` in `tools/pakon_image.py`).

use ndarray::{s, ArrayView3, Axis};
use rayon::prelude::*;

use crate::colour::{self, Balance, Matrix};
use crate::stats;

/// Per-channel film base: the median over `nwin` local row bands of each
/// channel's `pct` percentile below `sat` (a whole-roll percentile is
/// dominated by blown highlights). Raw 16-bit, at least 1.
pub fn measure_dmin(rgb: ArrayView3<u16>, pct: f64, sat: f32, nwin: usize, winrows: usize) -> [f32; 3] {
    let n = rgb.len_of(Axis(0));
    let bands: Vec<[f32; 3]> = (0..nwin)
        .into_par_iter()
        .map(|w| {
            let r0 = ((w as f64 + 0.5) / nwin as f64 * n as f64) as usize;
            let seg = rgb.slice(s![r0..(r0 + winrows).min(n), .., ..]);
            std::array::from_fn(|c| {
                let all: Vec<f32> = seg.index_axis(Axis(2), c).iter().map(|&v| v as f32).collect();
                let mut below: Vec<f32> = all.iter().copied().filter(|&v| v < sat).collect();
                if below.is_empty() {
                    let mut all = all;
                    stats::percentile(&mut all, pct)
                } else {
                    stats::percentile(&mut below, pct)
                }
            })
        })
        .collect();
    std::array::from_fn(|c| {
        let mut col: Vec<f32> = bands.iter().map(|b| b[c]).collect();
        stats::median(&mut col).max(1.0)
    })
}

/// `measure_dmin` with its defaults (99.5th percentile, saturation 63000,
/// 12 bands of 1500 rows).
pub fn dmin(rgb: ArrayView3<u16>) -> [f32; 3] {
    measure_dmin(rgb, 99.5, 63000.0, 12, 1500)
}

/// Default roll-balance strength.
pub const OEM_BALANCE: f32 = 0.5;

/// Roll-level two-point balance in RPD, a stand-in for the OEM's Ansel roll
/// balance: per channel the film base stays put and the roll's picture
/// median moves `strength` of the way to the neutral mean of the three
/// medians. Picture pixels leave out the darkest 20% and clipped ones.
pub fn roll_balance(rgb: ArrayView3<u16>, m: &Matrix, lut: &[f32], strength: f32) -> Balance {
    let none = Balance { gain: [1.0; 3], offset: [0.0; 3] };
    let p = colour::rpd_image(rgb, m, lut);
    let px: Vec<[f32; 3]> = p.lanes(Axis(2)).into_iter().map(|l| [l[0], l[1], l[2]]).collect();
    let lum: Vec<f32> = px.iter().map(|&v| stats::mean3(v)).collect();
    let cut = stats::percentile(&mut lum.clone(), 20.0);
    let keep: Vec<&[f32; 3]> = px
        .iter()
        .zip(&lum)
        .filter(|(v, &l)| l > cut && v[0].max(v[1]).max(v[2]) < 4000.0)
        .map(|(v, _)| v)
        .collect();
    if keep.len() < 100 {
        return none;
    }
    let med: [f32; 3] = std::array::from_fn(|c| stats::median(&mut keep.iter().map(|v| v[c]).collect::<Vec<_>>()));
    let d = dmin(rgb).map(|v| v as u16 as f32);
    let base = colour::rpd(d, m, lut, false);
    let mm = stats::mean3(med);
    let mut gain = [0.0; 3];
    let mut offset = [0.0; 3];
    for c in 0..3 {
        let target = med[c] + strength * (mm - med[c]);
        gain[c] = (target - base[c]) / (med[c] - base[c]).max(100.0);
        offset[c] = base[c] * (1.0 - gain[c]);
    }
    Balance { gain, offset }
}
