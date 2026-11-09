//! The OEM F-135 colour path (TLB.dll FN_bLoadImageFromBuffer,
//! docs/IMAGING.md): raw → 14-bit → the ColNeg density LUT → the unit's 3x10
//! NegMatrix (density → 12-bit RPD) → per-picture adjustments → rpd.pf as a
//! 3-D table → 16-bit sRGB. Ports `src/pakon_render.c` (itself bit-identical
//! to numpy's `render_oem`) and the shared helpers of `tools/pakon_image.py`.
//!
//! Every step repeats numpy's arithmetic: f32, f64 where numpy promotes, the
//! same operation order. Rust never contracts to FMA, so no pragma is needed.

use ndarray::{Array2, Array3, ArrayView3, Axis, Zip};
use rayon::prelude::*;

use crate::RAW_BLACK;

/// RPD the contrast slope turns about (0x60E).
pub const CONTRAST_PIVOT: f32 = 1550.0;

/// OEM ColNeg shared log-density LUT: `3500 * log10(16383 / i)`, 16383 at 0
/// (`_c41_lut`, computed in f64, stored f32).
pub fn c41_lut() -> Vec<f32> {
    (0..16384)
        .map(|i| {
            let v = if i < 1 { 16383.0 } else { 3500.0 * (16383.0 / (i as f64).max(1.0)).log10() };
            v.clamp(0.0, 16383.0) as f32
        })
        .collect()
}

/// A 3x10 colour polynomial (NegMatrix / PosMatrix), rows R, G, B.
pub type Matrix = [[f32; 10]; 3];

/// The PosMatrix every unit seen so far carries: 0.25 on the diagonal.
pub fn pos_matrix_default() -> Matrix {
    let mut m = [[0.0; 10]; 3];
    for (c, row) in m.iter_mut().enumerate() {
        row[c] = 0.25;
    }
    m
}

/// `m0 R + m1 G + m2 B + m3 R² + m4 G² + m5 B² + m6 RG + m7 BR + m8 GB + m9`
/// in numpy's order.
#[inline]
fn poly(k: &[f32; 10], r: f32, g: f32, b: f32) -> f32 {
    k[0] * r + k[1] * g + k[2] * b + k[3] * r * r + k[4] * g * g + k[5] * b * b + k[6] * r * g + k[7] * b * r + k[8] * g * b + k[9]
}

/// Density of one raw sample through the 14-bit LUT.
#[inline]
fn density_lut(v: f32, lut: &[f32]) -> f32 {
    let x = ((v - RAW_BLACK) / 4.0).clamp(1.0, 16383.0);
    lut[x as u16 as usize]
}

/// Density computed in floating point (slides: `oem_rpd(exact=True)`).
#[inline]
fn density_exact(v: f32) -> f32 {
    let x = ((v - RAW_BLACK) / 4.0).clamp(0.25, 16383.0);
    3500.0f32 * (16383.0f32 / x).log10()
}

/// `oem_rpd`: raw RGB (as f32) → 12-bit RPD (0..4095), per pixel.
#[inline]
pub fn rpd(px: [f32; 3], m: &Matrix, lut: &[f32], exact: bool) -> [f32; 3] {
    let d = px.map(|v| if exact { density_exact(v) } else { density_lut(v, lut) });
    std::array::from_fn(|c| poly(&m[c], d[0], d[1], d[2]).clamp(0.0, 4095.0))
}

/// `oem_rpd` over an image: (H, W, 3) f32 RPD.
pub fn rpd_image(rgb: ArrayView3<u16>, m: &Matrix, lut: &[f32]) -> Array3<f32> {
    let mut out = Array3::<f32>::zeros(rgb.dim());
    Zip::from(out.lanes_mut(Axis(2))).and(rgb.lanes(Axis(2))).par_for_each(|mut o, p| {
        let v = rpd([p[0] as f32, p[1] as f32, p[2] as f32], m, lut, false);
        o.assign(&ndarray::arr1(&v));
    });
    out
}

/// A 3-D colour table on even nodes over [0, 1]: n³ entries of 3 values,
/// C order (the RpdLut / romm.pf table, a ColRevLink grid).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub n: usize,
    pub data: Vec<f32>,
}

impl Table {
    /// From the 8-bit sRGB values the LittleCMS transform produced, /255 in
    /// f32 as `RpdLut` does (see `rust/tools/tables.py`).
    pub fn from_u8(n: usize, bytes: &[u8]) -> Result<Table, String> {
        if bytes.len() != n * n * n * 3 {
            return Err(format!("table: {} bytes, want {}", bytes.len(), n * n * n * 3));
        }
        Ok(Table { n, data: bytes.iter().map(|&b| b as f32 / 255.0f32).collect() })
    }

    /// Trilinear lookup of one value in [0, 1] (`_interp3`): weights in f64,
    /// as numpy promotes `f32 - int32`; the result rounds to f32.
    #[inline]
    pub fn lookup(&self, x01: [f32; 3]) -> [f32; 3] {
        let n = self.n;
        let mut i = [0usize; 3];
        let mut w = [[0f64; 3]; 2];
        for c in 0..3 {
            let f = x01[c].clamp(0.0, 1.0) * (n - 1) as f32;
            let k = (f as i32).min(n as i32 - 2);
            i[c] = k as usize;
            w[1][c] = f as f64 - k as f64;
            w[0][c] = 1.0 - w[1][c];
        }
        let base = 3 * ((i[0] * n + i[1]) * n + i[2]);
        let mut acc = [0f64; 3];
        for dr in 0..2 {
            for dg in 0..2 {
                let wrg = w[dr][0] * w[dg][1];
                for db in 0..2 {
                    let ww = wrg * w[db][2];
                    let e = base + 3 * ((dr * n + dg) * n + db);
                    for c in 0..3 {
                        acc[c] += ww * self.data[e + c] as f64;
                    }
                }
            }
        }
        acc.map(|a| a as f32)
    }
}

/// Per-picture adjustments in TLX units (`clean_adjust`): one slider click
/// is 25 for the colour offsets, brightness and contrast, ~67 for sharpness;
/// saturation −5..5; tone a B&W effect profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Adjust {
    pub red: i32,
    pub green: i32,
    pub blue: i32,
    pub brightness: i32,
    pub contrast: i32,
    pub sharpness: i32,
    pub saturation: i32,
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    None,
    Warm,
    Cold,
    Sepia,
}

impl Tone {
    /// The B&W effect profile (`ADJUST_TONES`).
    pub fn profile(self) -> Option<&'static str> {
        match self {
            Tone::None => None,
            Tone::Warm => Some("warm_bw_ld0_1_4-5.pf"),
            Tone::Cold => Some("cold_bw.pf"),
            Tone::Sepia => Some("sepia_ld0_9_22.pf"),
        }
    }
}

impl Adjust {
    /// Clamp to the slider ranges; `None` when nothing changes
    /// (`clean_adjust`).
    pub fn clean(self) -> Option<Adjust> {
        let c = |v: i32| v.clamp(-1000, 1000);
        let a = Adjust {
            red: c(self.red),
            green: c(self.green),
            blue: c(self.blue),
            brightness: c(self.brightness),
            contrast: c(self.contrast),
            sharpness: c(self.sharpness),
            saturation: self.saturation.clamp(-5, 5),
            tone: self.tone,
        };
        (a != Adjust::default()).then_some(a)
    }

    /// Contrast slope, the C's `1000 + contrast / 2`, 0 for none.
    fn contrast_slope(&self) -> f32 {
        if self.contrast != 0 { (1000.0 + self.contrast as f64 / 2.0) as f32 } else { 0.0 }
    }

    /// The red / green / blue + brightness offsets: `x 4096 / 4000` in f32.
    fn offsets(&self) -> [f32; 3] {
        [self.red, self.green, self.blue].map(|v| (v + self.brightness) as f32 * 4096.0f32 / 4000.0f32)
    }

    fn sharp(&self) -> f32 {
        if self.sharpness != 0 { (self.sharpness as f64 * 0.01) as f32 } else { 0.0 }
    }
}

/// A scene balance applied as `rpd * gain + offset` (roll balance, or OEM
/// SBA shifts with gain 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Balance {
    pub gain: [f32; 3],
    pub offset: [f32; 3],
}

/// RPD plus the 12-bit adjustments, all but the unsharp mask (C `rpd_pixel`).
#[inline]
fn rpd_adjusted(px: &[u16], m: &Matrix, lut: &[f32], contrast: f32, bal: Option<&Balance>, off: Option<&[f32; 3]>) -> [f32; 3] {
    let d: [f32; 3] = std::array::from_fn(|c| density_lut(px[c] as f32, lut));
    std::array::from_fn(|c| {
        let mut o = poly(&m[c], d[0], d[1], d[2]).clamp(0.0, 4095.0);
        if contrast != 0.0 {
            o = ((o - CONTRAST_PIVOT) * contrast / 1000.0 + CONTRAST_PIVOT).clamp(0.0, 4095.0);
        }
        if let Some(b) = bal {
            o = (o * b.gain[c] + b.offset[c]).clamp(0.0, 4095.0);
        }
        if let Some(a) = off {
            o = (o + a[c]).clamp(0.0, 4095.0);
        }
        o
    })
}

#[inline]
fn to_u16(v: f32) -> u16 {
    (v * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16
}

/// The OEM colour-negative render (`render_oem` / `pakon_render_oem`): raw
/// (H, W, 3) → 16-bit sRGB (H, W, 3). `grey` averages the three RPD channels
/// before the table; the caller converts to one channel (`to_grey`) unless a
/// B&W tone keeps it in colour.
pub fn render_oem(
    rgb: ArrayView3<u16>,
    m: &Matrix,
    lut: &[f32],
    table: &Table,
    bal: Option<&Balance>,
    adj: Option<&Adjust>,
    grey: bool,
) -> Array3<u16> {
    let (h, w, _) = rgb.dim();
    let contrast = adj.map_or(0.0, |a| a.contrast_slope());
    let off = adj.map(|a| a.offsets());
    let sharp = adj.map_or(0.0, |a| a.sharp());
    let mut out = Array3::<u16>::zeros((h, w, 3));
    if h == 0 || w == 0 {
        return out;
    }
    let rgb = rgb.as_standard_layout();

    // RPD of every pixel (the unsharp mask needs the neighbours).
    let rpd_of = |y: usize| -> Vec<f32> {
        let row = rgb.index_axis(Axis(0), y);
        let mut v = Vec::with_capacity(3 * w);
        for x in 0..w {
            let p = row.index_axis(Axis(0), x);
            v.extend(rpd_adjusted(p.as_slice().unwrap(), m, lut, contrast, bal, off.as_ref()));
        }
        v
    };
    let rpd: Vec<Vec<f32>> = (0..h).into_par_iter().map(rpd_of).collect();

    out.axis_iter_mut(Axis(0)).into_par_iter().enumerate().for_each(|(y, mut orow)| {
        let v = &rpd[y];
        let sharpened;
        let v: &[f32] = if sharp != 0.0 {
            // _unsharp: x + amount * (x - blur), blur the separable
            // [1/4, 1/2, 1/4] kernel (rows first), edges repeated.
            let up = &rpd[y.saturating_sub(1)];
            let dn = &rpd[(y + 1).min(h - 1)];
            let rows: Vec<f32> = (0..3 * w).map(|k| 0.25f32 * up[k] + 0.5f32 * v[k] + 0.25f32 * dn[k]).collect();
            sharpened = (0..w)
                .flat_map(|x| {
                    let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
                    let rows = &rows;
                    (0..3).map(move |c| {
                        let blur = 0.25f32 * rows[3 * xl + c] + 0.5f32 * rows[3 * x + c] + 0.25f32 * rows[3 * xr + c];
                        let s = v[3 * x + c];
                        (s + sharp * (s - blur)).clamp(0.0, 4095.0)
                    })
                })
                .collect::<Vec<f32>>();
            &sharpened
        } else {
            v
        };
        for x in 0..w {
            let mut p = [v[3 * x], v[3 * x + 1], v[3 * x + 2]];
            if grey {
                let mean = (p[0] + p[1] + p[2]) / 3.0f32;
                p = [mean; 3];
            }
            let s = table.lookup(p.map(|c| c / 4095.0f32));
            for c in 0..3 {
                orow[(x, c)] = to_u16(s[c]);
            }
        }
    });
    out
}

/// (H, W, 3) 16-bit sRGB → (H, W) 16-bit grey, Rec. 709 luma.
pub fn to_grey(rgb: ArrayView3<u16>) -> Array2<u16> {
    let (h, w, _) = rgb.dim();
    let mut out = Array2::<u16>::zeros((h, w));
    Zip::from(&mut out).and(rgb.lanes(Axis(2))).par_for_each(|o, p| {
        let y = p[0] as f32 * 0.2126f32 + p[1] as f32 * 0.7152f32 + p[2] as f32 * 0.0722f32;
        *o = (y + 0.5).clamp(0.0, 65535.0) as u16;
    });
    out
}
