//! Saturation and B&W tone (port of `adjusted_table`, `_lab_to_srgb` and
//! the slide side of `_adjust_12bit` in `tools/pakon_image.py`).
//!
//! The OEM chains an abstract Lab -> Lab profile (satPlusNN / satMinusNN.pf,
//! or a B&W tone) between the input profile and the output. Applied as a
//! correction to the rendering table: our own evaluation of the input
//! profile with the step minus without, so 0 / none leaves the table exactly
//! as it was.

use std::path::Path;

use ndarray::{Array3, Axis};
use rayon::prelude::*;

use crate::colour::{Adjust, Table, Tone, CONTRAST_PIVOT};
use crate::slide::Link;

/// mft2-encoded Lab (ICC v2 lut16, [0, 1]) -> sRGB [0, 1] (D50, Bradford),
/// in numpy's float32 steps.
pub fn lab_to_srgb(v: [f32; 3]) -> [f32; 3] {
    let l = v[0] * 65535.0f32 / 65280.0f32 * 100.0f32;
    let a = v[1] * 65535.0f32 / 256.0f32 - 128.0f32;
    let b = v[2] * 65535.0f32 / 256.0f32 - 128.0f32;
    let fy = (l + 16.0f32) / 116.0f32;
    let (fx, fz) = (fy + a / 500.0f32, fy - b / 200.0f32);
    let k = (3.0 * (6.0f64 / 29.0).powi(2)) as f32;
    let f = |t: f32| if t > (6.0f64 / 29.0) as f32 { t.powf(3.0) } else { k * (t - (4.0f64 / 29.0) as f32) };
    let xyz = [0.9642f32 * f(fx), f(fy), 0.8249f32 * f(fz)];
    #[allow(clippy::excessive_precision)] // the Python constants, rounded to f32 as numpy does
    const M: [[f32; 3]; 3] =
        [[3.1338561, -1.6168667, -0.4906146], [-0.9787684, 1.9161415, 0.0334540], [0.0719453, -0.2289914, 1.4052427]];
    let gamma = (1.0f64 / 2.4) as f32;
    std::array::from_fn(|i| {
        let lin = (M[i][0] * xyz[0] + M[i][1] * xyz[1] + M[i][2] * xyz[2]).clamp(0.0, 1.0);
        if lin <= 0.0031308f32 {
            12.92f32 * lin
        } else {
            1.055f32 * lin.powf(gamma) - 0.055f32
        }
    })
}

/// An input profile evaluated to mft2-encoded Lab: an A2B0 lut16 (rpd.pf),
/// or a matrix/TRC RGB profile (romm.pf; `tools/pakon_image.py` cannot read
/// those and fails on slide saturation).
pub enum Input {
    Link(Link),
    MatrixTrc { gamma: [f64; 3], m: [[f64; 3]; 3] },
}

impl Input {
    pub fn read(path: &Path) -> Result<Input, String> {
        let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Ok(l) = Link::parse(&d) {
            return Ok(Input::Link(l));
        }
        let be32 = |p: usize| d.get(p..p + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
        let tag = |sig: &[u8]| -> Option<usize> {
            (0..be32(128)? as usize).find_map(|k| {
                let t = 132 + 12 * k;
                (d.get(t..t + 4)? == sig).then(|| be32(t + 4)).flatten().map(|o| o as usize)
            })
        };
        let err = || format!("{}: neither a lut16 nor a matrix/TRC profile", path.display());
        let mut gamma = [0.0; 3];
        let mut m = [[0.0; 3]; 3];
        for (c, (trc, xyz)) in [(b"rTRC", b"rXYZ"), (b"gTRC", b"gXYZ"), (b"bTRC", b"bXYZ")].iter().enumerate() {
            let t = tag(*trc).ok_or_else(err)?;
            if d.get(t..t + 4) != Some(b"curv") || be32(t + 8) != Some(1) {
                return Err(err());
            }
            gamma[c] = u16::from_be_bytes([d[t + 12], d[t + 13]]) as f64 / 256.0;
            let x = tag(*xyz).ok_or_else(err)?;
            for r in 0..3 {
                m[r][c] = be32(x + 8 + 4 * r).ok_or_else(err)? as i32 as f64 / 65536.0;
            }
        }
        Ok(Input::MatrixTrc { gamma, m })
    }

    /// One value in [0, 1]³ -> mft2 Lab.
    pub fn to_lab(&self, x: [f32; 3]) -> [f32; 3] {
        match self {
            Input::Link(l) => l.apply(x),
            Input::MatrixTrc { gamma, m } => {
                let lin: [f64; 3] = std::array::from_fn(|c| (x[c].clamp(0.0, 1.0) as f64).powf(gamma[c]));
                let xyz: [f64; 3] = std::array::from_fn(|r| m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2]);
                let f = |t: f64| if t > (6.0f64 / 29.0).powi(3) { t.cbrt() } else { t / (3.0 * (6.0f64 / 29.0).powi(2)) + 4.0 / 29.0 };
                let (fx, fy, fz) = (f(xyz[0] / 0.9642), f(xyz[1]), f(xyz[2] / 0.8249));
                let (l, a, b) = (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz));
                [(l / 100.0 * 65280.0 / 65535.0) as f32, ((a + 128.0) * 256.0 / 65535.0) as f32, ((b + 128.0) * 256.0 / 65535.0) as f32]
            }
        }
    }
}

/// The saturation profile for −5..5 steps (`satplusNN.pf` / `satminusNN.pf`).
pub fn saturation_profile(saturation: i32) -> Option<String> {
    (saturation != 0).then(|| format!("sat{}{:02}.pf", if saturation > 0 { "plus" } else { "minus" }, 3 * saturation.abs()))
}

/// `base` (the input profile's table, n³ nodes on [0, 1]) with the OEM's
/// saturation and tone profiles from `input`'s folder.
pub fn adjusted_table(base: &Table, input: &Path, saturation: i32, tone: Tone) -> Result<Table, String> {
    let steps: Vec<String> = saturation_profile(saturation).into_iter().chain(tone.profile().map(String::from)).collect();
    if steps.is_empty() {
        return Ok(base.clone());
    }
    let folder = input.parent().unwrap_or(Path::new("."));
    let src = Input::read(input)?;
    let links: Vec<Link> = steps.iter().map(|n| Link::read(folder.join(n))).collect::<Result<_, _>>()?;
    let n = base.n;
    let node = |i: usize| if i == n - 1 { 1.0f32 } else { (i as f64 * (1.0 / (n - 1) as f64)) as f32 };
    let data: Vec<f32> = (0..n * n * n)
        .into_par_iter()
        .flat_map_iter(|k| {
            let x = [node(k / (n * n)), node(k / n % n), node(k % n)];
            let lab = src.to_lab(x);
            let out = links.iter().fold(lab, |v, l| l.apply(v));
            let (a, b) = (lab_to_srgb(out), lab_to_srgb(lab));
            let base = &base.data[3 * k..3 * k + 3];
            (0..3).map(move |c| (base[c] + (a[c] - b[c])).clamp(0.0, 1.0))
        })
        .collect();
    Ok(Table { n, data })
}

/// PakonIMAu's unsharp mask on (H, W, 3) 12-bit values, in place:
/// `x + amount (x − blur)`, blur the separable [1/4, 1/2, 1/4] kernel (rows
/// first), edges repeated, then clipped to 0..4095.
pub fn unsharp(v: &mut Array3<f32>, amount: f32) {
    let (h, w, _) = v.dim();
    if h == 0 || w == 0 {
        return;
    }
    let src = v.clone();
    v.axis_iter_mut(Axis(0)).into_par_iter().enumerate().for_each(|(y, mut row)| {
        let (up, mid, dn) = (src.index_axis(Axis(0), y.saturating_sub(1)), src.index_axis(Axis(0), y), src.index_axis(Axis(0), (y + 1).min(h - 1)));
        let rows: Vec<[f32; 3]> =
            (0..w).map(|x| std::array::from_fn(|c| 0.25f32 * up[(x, c)] + 0.5f32 * mid[(x, c)] + 0.25f32 * dn[(x, c)])).collect();
        for x in 0..w {
            let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
            for c in 0..3 {
                let blur = 0.25f32 * rows[xl][c] + 0.5f32 * rows[x][c] + 0.25f32 * rows[xr][c];
                let s = mid[(x, c)];
                row[(x, c)] = (s + amount * (s - blur)).clamp(0.0, 4095.0);
            }
        }
    });
}

/// `_adjust_12bit` without a balance (the slide path, on 12-bit ROMM):
/// contrast about 1550, the colour + brightness offsets, the unsharp mask.
pub fn adjust_12bit(v: &mut Array3<f32>, adj: &Adjust) {
    if adj.contrast != 0 {
        let k = (1000.0 + adj.contrast as f64 / 2.0) as f32;
        v.par_mapv_inplace(|o| ((o - CONTRAST_PIVOT) * k / 1000.0f32 + CONTRAST_PIVOT).clamp(0.0, 4095.0));
    }
    let off = [adj.red, adj.green, adj.blue].map(|c| (c + adj.brightness) as f32 * 4096.0f32 / 4000.0f32);
    v.lanes_mut(Axis(2)).into_iter().for_each(|mut p| {
        for c in 0..3 {
            p[c] = (p[c] + off[c]).clamp(0.0, 4095.0);
        }
    });
    if adj.sharpness != 0 {
        unsharp(v, (adj.sharpness as f64 * 0.01) as f32);
    }
}
