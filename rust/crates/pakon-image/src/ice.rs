//! Infrared dust and scratch removal (port of `tools/pakon_ice.py`, itself a
//! numpy port of nkscan's `dust.rs`, MIT OR Apache-2.0, after a6o's openICE
//! description of Applied Science Fiction's Digital ICE as tuned for the
//! Coolscan LS-5000). The Coolscan constants are kept; what is Pakon-specific
//! is getting the IR plane (`ir_plane`) and the gate margin per column.
//!
//! Every step repeats numpy's arithmetic: f32 where numpy stays f32 (Python
//! floats meeting f32 arrays are cast to f32 first), f64 where numpy
//! promotes, numpy's reduction orders. The comments name the numpy line.

use ndarray::{Array2, Array3, ArrayView2, ArrayView3, Axis, Zip};
use rayon::prelude::*;

use crate::flat::FlatJson;
use crate::stats;
use crate::RAW_BLACK;

/// Full scale of a 16-bit sample.
pub const M: f64 = 65535.0;
const MF: f32 = 65535.0;
/// Raw IR above which film counts as clear.
const TAU: f32 = 8847.23;
const SLOPE_LIMIT: f32 = 0.2;
const W_FLOOR: f32 = 0.02;
const B_ANCHOR: f64 = 0.98;
const PHI_ANCHOR: f64 = 0.065;
const MIN3_DPI: u32 = 550;
const ALPHA: [f64; 3] = [0.015, 0.015, 0.025];
/// Gate shift for Pakon IR (see `PAKON_METERING`).
pub const PAKON_METERING: f64 = 0.985;

/// The LS-5000 profile: gate bias, IR-reference gain, dither band, beta
/// resolution, weight-ramp anchor, soft-threshold coefficients
/// `a[channel][band] = (a_hi, a_lo)`.
const THETA: f64 = 1.0;
const GAMMA: f32 = 1.1;
const DITHER: (f64, f64) = (0.01, 0.99);
const CONTRAST_DPI: u32 = 1600;
const RAMP: f64 = -960.42;
const A: [[(f64, f64); 3]; 3] = [
    [(1.210, 1.090), (1.170, 1.080), (1.040, 0.960)],
    [(1.230, 1.130), (1.140, 1.050), (0.930, 0.840)],
    [(1.130, 1.040), (1.080, 1.020), (0.970, 0.890)],
];
/// Normal quality.
const DETAIL_GAIN: f32 = 1.25;

/// Row spans (dy, dx_lo, dx_hi) of the 9x9 and 5x5 octagons.
const LEVEL0: [(i32, i32, i32); 9] = [(-4, -2, 2), (-3, -3, 3), (-2, -4, 4), (-1, -4, 4), (0, -4, 4), (1, -4, 4), (2, -4, 4), (3, -3, 3), (4, -2, 2)];
const LEVEL1: [(i32, i32, i32); 5] = [(-2, -1, 1), (-1, -2, 2), (0, -2, 2), (1, -2, 2), (2, -1, 1)];
const TENT: [f64; 3] = [1.0, 2.0, 1.0];

/// Scan resolution for a line `width` px across the 24 mm frame height.
pub fn pakon_dpi(width: usize) -> u32 {
    (width as f64 / (24.0 / 25.4)).round_ties_even() as u32
}

/// D(v) = M/16 log2(v + 1).
#[inline]
pub fn density(v: f32) -> f32 {
    (v + 1.0).log2() * (MF / 16.0)
}

/// 2^(16 d / M) - 1, clamped to 0..M.
#[inline]
pub fn from_density(d: f32) -> f32 {
    ((d * (16.0 / M) as f32).exp2() - 1.0).clamp(0.0, MF)
}

fn anchor(fraction: f64) -> f64 {
    (M * fraction) as i64 as f64
}

/// density() of a Python float, back as a Python float.
fn density_f(v: f64) -> f64 {
    density(v as f32) as f64
}

// ----- step 2, calibrate

/// Area-average by 4 in both axes: each 4x4 block summed row by row (numpy's
/// order for `mean(axis=(1, 3))`), / 16.
fn reduce(a: ArrayView2<f32>) -> Array2<f32> {
    let (h, w) = (a.nrows() / 4, a.ncols() / 4);
    Array2::from_shape_fn((h, w), |(i, j)| {
        let mut out = 0.0f32;
        for r in 0..4 {
            let mut s = 0.0f32;
            for c in 0..4 {
                s += a[(4 * i + r, 4 * j + c)];
            }
            out += s;
        }
        out / 16.0
    })
}

/// Mean (or sum) of a rows x cols block at (y0, x0), numpy's tile order.
fn block(a: &Array2<f32>, y0: usize, x0: usize, rows: usize, cols: usize) -> f32 {
    let mut out = 0.0f32;
    let mut row = [0f32; 8];
    for r in 0..rows {
        for c in 0..cols {
            row[c] = a[(y0 + r, x0 + c)];
        }
        out += stats::pairwise_sum(&row[..cols]);
    }
    out
}

/// (c, ir_ref) from a reduced frame: c the red → IR dye leakage slope, ir_ref
/// the clear-film IR density without it. `None` without clear film.
pub fn calibrate(red: &Array2<f32>, ir: &Array2<f32>) -> Option<(f64, f64)> {
    let d_r = red.mapv(density);
    let d_ir = ir.mapv(density);
    let (mut w, mut wr, mut wi) = (Vec::new(), Vec::new(), Vec::new());
    for ((&v, &dr), &di) in ir.iter().zip(d_r.iter()).zip(d_ir.iter()) {
        if v > TAU {
            let ww = (v as f64).powi(2);
            w.push(ww);
            wr.push(ww * dr as f64);
            wi.push(ww * di as f64);
        }
    }
    if w.is_empty() {
        return None;
    }
    let ws = stats::pairwise_sum_f64(&w);
    let r_ref = stats::pairwise_sum_f64(&wr) / ws;
    let ir_raw = stats::pairwise_sum_f64(&wi) / ws;

    // Leakage: weighted least squares over the 4x4 quadrants of every 8x8
    // tile that is clear film all the way through.
    let (th, tw) = (red.nrows() / 8, red.ncols() / 8);
    let mut c = 0.0;
    if th > 0 && tw > 0 {
        let mut ok = Vec::with_capacity(th * tw);
        let mut tr = Vec::with_capacity(th * tw);
        let mut ti = Vec::with_capacity(th * tw);
        let mut traw = Vec::with_capacity(th * tw);
        for i in 0..th {
            for j in 0..tw {
                ok.push((0..8).all(|r| (0..8).all(|q| ir[(8 * i + r, 8 * j + q)] > TAU)));
                tr.push(block(&d_r, 8 * i, 8 * j, 8, 8) / 64.0);
                ti.push(block(&d_ir, 8 * i, 8 * j, 8, 8) / 64.0);
                traw.push(block(ir, 8 * i, 8 * j, 8, 8));
            }
        }
        let (mut num, mut den) = (0.0, 0.0);
        for dy0 in [0, 4] {
            for dx0 in [0, 4] {
                let (mut sw, mut ww) = (Vec::new(), Vec::new());
                for i in 0..th {
                    for j in 0..tw {
                        let k = i * tw + j;
                        let qr = block(&d_r, 8 * i + dy0, 8 * j + dx0, 4, 4) / 16.0 - tr[k];
                        let qi = block(&d_ir, 8 * i + dy0, 8 * j + dx0, 4, 4) / 16.0 - ti[k];
                        let slope = qi / qr;
                        if ok[k] && slope.is_finite() && slope.abs() <= SLOPE_LIMIT {
                            let wt = (qr * qr * traw[k] * traw[k]) as f64;
                            sw.push(slope as f64 * wt);
                            ww.push(wt);
                        }
                    }
                }
                num += stats::pairwise_sum_f64(&sw);
                den += stats::pairwise_sum_f64(&ww);
            }
        }
        c = if den > 0.0 { num / den } else { 0.0 };
    }
    Some((c, (ir_raw - c * r_ref) / (1.0 - c)))
}

// ----- parameters

struct Params {
    c: f64,
    ir_ref: f64,
    theta: f64,
    ramp_bias: f64,
    ramp_s: f64,
    phi: f64,
    eta: (f64, f64),
    min3: bool,
    cross_beta: bool,
    /// Per column, f32 (`col_extra`).
    col_extra: Vec<f32>,
}

impl Params {
    fn new(cal: (f64, f64), dpi: u32, metering: f64, col_noise: &[f32]) -> Params {
        let (c, ir_ref) = cal;
        let met = density_f(anchor(metering.clamp(0.0, 1.0)));
        let b = density_f(anchor(B_ANCHOR));
        let margin = ((M - b) + (M - met)) as f32;
        Params {
            c,
            ir_ref,
            theta: THETA + met - M,
            ramp_bias: ir_ref + b - M,
            ramp_s: 1.0 / RAMP,
            phi: density_f(anchor(PHI_ANCHOR)),
            eta: (density_f(anchor(DITHER.0)), density_f(anchor(DITHER.1))),
            min3: dpi > MIN3_DPI,
            cross_beta: dpi > CONTRAST_DPI,
            col_extra: col_noise.iter().map(|&k| (k.clamp(1.0, 4.0) - 1.0) * margin).collect(),
        }
    }

    #[inline]
    fn weight(&self, g: f32) -> f32 {
        (1.0f32 + (self.ramp_bias as f32 - g) * self.ramp_s as f32).clamp(W_FLOOR, 1.0)
    }
}

// ----- steps 3-5

fn shift_cols<T: Copy>(a: &Array2<T>, k: i32) -> Array2<T> {
    let w = a.ncols() as i32;
    Array2::from_shape_fn(a.dim(), |(y, x)| a[(y, (x as i32 + k).clamp(0, w - 1) as usize)])
}

fn shift_rows<T: Copy>(a: &Array2<T>, k: i32) -> Array2<T> {
    let h = a.nrows() as i32;
    Array2::from_shape_fn(a.dim(), |(y, x)| a[((y as i32 + k).clamp(0, h - 1) as usize, x)])
}

fn min3_cols(a: &Array2<f32>) -> Array2<f32> {
    let (l, r) = (shift_cols(a, -1), shift_cols(a, 1));
    Zip::from(&l).and(a).and(&r).map_collect(|&l, &m, &r| l.min(m).min(r))
}

fn min3_rows(a: &Array2<f32>) -> Array2<f32> {
    let (u, d) = (shift_rows(a, -1), shift_rows(a, 1));
    Zip::from(&u).and(a).and(&d).map_collect(|&u, &m, &d| u.min(m).min(d))
}

fn gate(red: ArrayView2<f32>, ir: ArrayView2<f32>, p: &Params) -> Array2<f32> {
    let (c, den, theta) = (p.c as f32, (1.0 - p.c) as f32, p.theta as f32);
    Zip::from(red).and(ir).par_map_collect(|&r, &i| (density(i) - c * density(r)) / den - theta)
}

fn confidence(g: &Array2<f32>, p: &Params) -> Array2<f32> {
    let m = if p.min3 { min3_rows(&min3_cols(g)) } else { g.clone() };
    Array2::from_shape_fn(m.dim(), |(y, x)| p.weight(m[(y, x)] + p.col_extra[x]))
}

fn and3_cols(a: &Array2<bool>, k: i32) -> Array2<bool> {
    let (l, r) = (shift_cols(a, -k), shift_cols(a, k));
    Zip::from(&l).and(a).and(&r).map_collect(|&l, &m, &r| l & m & r)
}

fn and3_rows(a: &Array2<bool>, k: i32) -> Array2<bool> {
    let (u, d) = (shift_rows(a, -k), shift_rows(a, k));
    Zip::from(&u).and(a).and(&d).map_collect(|&u, &m, &d| u & m & d)
}

fn decide(g: &Array2<f32>, w: &Array2<f32>, p: &Params) -> Array2<bool> {
    let phi = p.phi as f32;
    let dark = g.mapv(|v| v < phi);
    let row_dark = and3_cols(&and3_cols(&dark, 1), 3);
    let col_dark = and3_rows(&and3_rows(&dark, 1), 3);
    let (a, b, c, d) = (shift_rows(&row_dark, -4), shift_rows(&row_dark, 4), shift_cols(&col_dark, -4), shift_cols(&col_dark, 4));
    Array2::from_shape_fn(g.dim(), |i| !(a[i] | b[i] | c[i] | d[i]) && w[i] < 1.0)
}

// ----- steps 6-8, rebuild

/// Sum over an octagon of row spans; rows past the edge add nothing,
/// columns past the edge repeat the edge column. Cumulative sums per row in
/// f64 (np.cumsum into an f64 out accumulates in f64), f32 result.
fn octagon_sum(a: &Array2<f32>, spans: &[(i32, i32, i32)]) -> Array2<f32> {
    let (h, w) = a.dim();
    let pad = spans.iter().map(|&(_, lo, hi)| (-lo).max(hi)).max().unwrap() as usize;
    let cs: Vec<Vec<f64>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let row = a.row(y);
            let mut c = Vec::with_capacity(w + 2 * pad + 1);
            c.push(0.0f64);
            let mut acc = 0.0f64;
            for k in 0..w + 2 * pad {
                let x = (k as i64 - pad as i64).clamp(0, w as i64 - 1) as usize;
                acc += row[x] as f64;
                c.push(acc);
            }
            c
        })
        .collect();
    let mut out = Array2::<f64>::zeros((h, w));
    for &(dy, lo, hi) in spans {
        let span = |y: usize, x: usize| {
            let xp = x + pad;
            cs[y][(xp as i64 + hi as i64 + 1) as usize] - cs[y][(xp as i64 + lo as i64) as usize]
        };
        out.axis_iter_mut(Axis(0)).into_par_iter().enumerate().for_each(|(y, mut o)| {
            let src = y as i64 + dy as i64;
            if src < 0 || src >= h as i64 {
                return;
            }
            for (x, v) in o.iter_mut().enumerate() {
                *v += span(src as usize, x);
            }
        });
    }
    out.mapv(|v| v as f32)
}

/// 3x3 binomial tent / 16, cells past any edge add nothing.
fn tent_sum(a: &Array2<f32>) -> Array2<f32> {
    let (h, w) = a.dim();
    let mut out = Array2::<f32>::zeros((h, w));
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let k = (TENT[(dy + 1) as usize] * TENT[(dx + 1) as usize] / 16.0) as f32;
            Zip::indexed(&mut out).par_for_each(|(y, x), o| {
                let (sy, sx) = (y as i64 + dy, x as i64 + dx);
                let v = if sy < 0 || sx < 0 || sy >= h as i64 || sx >= w as i64 { 0.0 } else { a[(sy as usize, sx as usize)] };
                *o += k * v;
            });
        }
    }
    out
}

struct Levels {
    c: Vec<Array2<f32>>,
    p: Vec<Array2<f32>>,
    l: Vec<[Array2<f32>; 3]>,
}

fn levels(g: &Array2<f32>, w: &Array2<f32>, dens: &[Array2<f32>; 3]) -> Levels {
    let wg = w * g;
    let wd: [Array2<f32>; 3] = std::array::from_fn(|c| w * &dens[c]);
    let mut out = Levels { c: vec![], p: vec![], l: vec![] };
    for level in 0..3 {
        let s = |a: &Array2<f32>| -> Array2<f32> {
            match level {
                0 => octagon_sum(a, &LEVEL0) * (1.0 / 69.0) as f32,
                1 => octagon_sum(a, &LEVEL1) * (1.0 / 21.0) as f32,
                _ => tent_sum(a),
            }
        };
        let cw = s(w);
        let inv = cw.mapv(|v| if v > 0.0 { 1.0f32 / v.max(1e-30) } else { 0.0 });
        out.p.push(s(&wg) * &inv);
        out.l.push(std::array::from_fn(|c| s(&wd[c]) * &inv));
        out.c.push(cw);
    }
    out.c.push(w.clone());
    out.p.push(g.clone());
    out.l.push(dens.clone());
    out
}

/// Uniform [0, 1) fixed per (pixel, channel): Stafford's Mix13.
fn uniform(idx: u64, ch: usize) -> f32 {
    let mut x = (idx << 2) | ch as u64;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    ((x ^ (x >> 31)) & 0xFFFFFF) as f32 / (1u32 << 24) as f32
}

/// f64 `a`, f32 dither (numpy: `np.where(ok, d, 0.0).astype(float32)`).
fn dither(a: f64, idx: u64, ch: usize, p: &Params) -> f32 {
    let (lo, hi) = p.eta;
    let inside = a > lo && a < hi;
    let env = 4.0 / (hi - lo).powi(2) * (a - lo) * (hi - a);
    let d = env * (uniform(idx, ch) - 0.5f32) as f64 * ALPHA[ch] * a;
    if inside && a + d > lo && a + d < hi { d as f32 } else { 0.0 }
}

/// Min and max of a band value and its 4 neighbours (NaN-free `nanmin` /
/// `nanmax` over the stack of shifted copies).
fn cross_minmax(d: &Array2<f32>) -> (Array2<f32>, Array2<f32>) {
    let (h, w) = d.dim();
    let mut lo = d.clone();
    let mut hi = d.clone();
    Zip::indexed(&mut lo).and(&mut hi).par_for_each(|(y, x), lo, hi| {
        for (dy, dx) in [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)] {
            let (ny, nx) = (y as i64 + dy, x as i64 + dx);
            if ny >= 0 && nx >= 0 && ny < h as i64 && nx < w as i64 {
                let v = d[(ny as usize, nx as usize)];
                *lo = lo.min(v);
                *hi = hi.max(v);
            }
        }
    });
    (lo, hi)
}

/// The masked pixels that survive the Normal clamp, with their rebuilt
/// densities.
fn rebuild(g: &Array2<f32>, w: &Array2<f32>, dens: &[Array2<f32>; 3], mask: &Array2<bool>, p: &Params) -> Vec<(usize, usize, [f32; 3])> {
    let lv = levels(g, w, dens);
    let bands: Vec<Array2<f32>> = (1..4).map(|l| &lv.p[l] - &lv.p[l - 1]).collect();
    let (los, his): (Vec<_>, Vec<_>) = if p.cross_beta {
        bands.iter().map(cross_minmax).unzip()
    } else {
        (bands.clone(), bands.clone())
    };
    let width = g.ncols();
    let pts: Vec<(usize, usize)> = mask.indexed_iter().filter(|(_, &m)| m).map(|(i, _)| i).collect();
    pts.par_iter()
        .filter_map(|&(y, x)| {
            let i = (y, x);
            let idx = (y * width + x) as u64;
            let conf = [(2.0f32 * lv.c[1][i]).min(1.0), lv.c[2][i], lv.c[3][i] * lv.c[3][i]];
            let mut out = [0f32; 3];
            for ch in 0..3 {
                let a0 = lv.l[0][ch][i] + GAMMA * (p.ir_ref as f32 - lv.p[0][i]);
                let mut a = a0 as f64;
                for l in 1..4 {
                    let detail = (lv.l[l][ch][i] - lv.l[l - 1][ch][i]) * DETAIL_GAIN;
                    let (a_hi, a_lo) = A[ch][l - 1];
                    let h = his[l - 1][i];
                    let lo_l = los[l - 1][i];
                    let (alo, ahi) = if h < 0.0 { (a_hi, a_lo) } else { (a_lo, a_hi) };
                    let (lo_t, hi_t) = (alo * lo_l as f64, ahi * h as f64);
                    let dd = detail as f64;
                    let r = if dd < lo_t { dd - lo_t } else if dd > hi_t { dd - hi_t } else { 0.0 };
                    a += r * conf[l - 1] as f64;
                }
                out[ch] = (a + dither(a, idx, ch, p) as f64) as f32;
            }
            // Normal: keep only pixels rebuilt above zero, never darker.
            if !out.iter().all(|&v| v > 0.0) {
                return None;
            }
            Some((y, x, std::array::from_fn(|c| out[c].max(dens[c][i]))))
        })
        .collect()
}

/// Remove dust from one frame: rgb (H, W, 3) linear 0..M above black, ir
/// (H, W) linear 0..M, `col_noise` per column. Returns the cleaned frame and
/// the mask of rebuilt pixels (the input when there is no clear film).
pub fn clean(rgb: &Array3<f32>, ir: &Array2<f32>, dpi: u32, metering: f64, col_noise: &[f32]) -> (Array3<f32>, Array2<bool>) {
    let mut out = rgb.clone();
    let mut done = Array2::<bool>::from_elem(ir.dim(), false);
    let red = rgb.index_axis(Axis(2), 0);
    let Some(cal) = calibrate(&reduce(red), &reduce(ir.view())) else {
        return (out, done);
    };
    let p = Params::new(cal, dpi, metering, col_noise);
    let g = gate(red, ir.view(), &p);
    let w = confidence(&g, &p);
    let mask = decide(&g, &w, &p);
    let dens: [Array2<f32>; 3] = std::array::from_fn(|c| rgb.index_axis(Axis(2), c).mapv(density));
    for (y, x, d) in rebuild(&g, &w, &dens, &mask, &p) {
        for c in 0..3 {
            let v = from_density(d[c]);
            out[(y, x, c)] = v.max(out[(y, x, c)]);
        }
        done[(y, x)] = true;
    }
    (out, done)
}

/// One frame of a Pakon scan, cleaned: raw (H, W, 3) registered,
/// flat-fielded samples (black at 300), its IR plane, the calibration's
/// noise factor per column. Returns the 16-bit frame and the rebuilt mask.
pub fn clean_frame(raw: ArrayView3<u16>, ir: ArrayView2<u16>, ir_noise: &[f32], dpi: u32) -> (Array3<u16>, Array2<bool>) {
    let ir = ir.mapv(|v| v as f32);
    let f = raw.mapv(|v| (v as f32 - RAW_BLACK).clamp(0.0, MF));
    let tail = ir_tail_factor(&ir);
    let k: Vec<f32> = ir_noise.iter().zip(&tail).map(|(a, b)| a.max(*b)).collect();
    let (out, mask) = clean(&f, &ir, dpi, PAKON_METERING, &k);
    (out.mapv(|v| (v + RAW_BLACK + 0.5).clamp(0.0, 65535.0) as u16), mask)
}

// ----- the Pakon side: the IR plane

/// The IR block of marker-aligned rows (the last quarter of each row) for
/// ribbon rows `0..rows`, flat-fielded per column against the calibration's
/// open-gate IR and scaled so the open gate reads `metering` x M; the IR duty
/// ratio comes out when the scan ran another duty than the calibration.
pub fn ir_plane(img: ArrayView2<u16>, marker: usize, flat: &FlatJson, rows: usize, metering: f64) -> Result<Array2<f32>, String> {
    let lw = img.ncols();
    let w = lw / 4;
    if flat.row_samples.unwrap_or(4 * w) != 4 * w || !flat.ir_lane.unwrap_or(true) {
        return Err("ir_plane: the raw has no IR lane (scanned with IR off?)".into());
    }
    let bright = flat.bright.get(3).ok_or("ir_plane: no IR table")?;
    let ratio = match (&flat.duties, &flat.scan_duties) {
        (Some(d), Some(s)) if d.len() > 3 && s.len() > 3 && s[3] > 0.0 => Some((d[3] / s[3]) as f32),
        _ => None,
    };
    let num = (metering * M) as f32;
    let gain: Vec<f32> = (0..w)
        .map(|x| {
            let b = bright[flat.px0 + x] as f32 - RAW_BLACK;
            let g = if b > 0.0 { num / b.max(1.0) } else { 0.0 };
            ratio.map_or(g, |r| g * r)
        })
        .collect();
    let n = img.nrows();
    let mut out = Array2::<f32>::zeros((rows, w));
    out.axis_iter_mut(Axis(0)).into_par_iter().enumerate().for_each(|(y, mut o)| {
        let src = img.row(y.min(n - 1));
        for (x, v) in o.iter_mut().enumerate() {
            let ir = src[(3 * w + x + marker) % lw] as f32 - RAW_BLACK;
            *v = (ir * gain[x]).clamp(0.0, MF);
        }
    });
    Ok(out)
}

/// Per-column noise factor from the calibration's open-gate IR: 0.9 /
/// (open gate / its 90th percentile), at least 1.
pub fn ir_noise_factor(flat: &FlatJson, width: usize) -> Vec<f32> {
    let b: Vec<f32> = flat.bright[3][flat.px0..flat.px0 + width].iter().map(|&v| v as f32 - RAW_BLACK).collect();
    let top = stats::percentile(&mut b.clone(), 90.0);
    b.iter()
        .map(|&v| {
            let rel = if top > 0.0 { (v / top).max(1e-3) } else { 1.0 };
            (0.9f32 / rel).max(1.0)
        })
        .collect()
}

/// Per-column factor from the frame's own IR: how deep its rare dark pixels
/// go relative to the middle of the line, median-filtered over 15 columns,
/// 1..4.
pub fn ir_tail_factor(ir: &Array2<f32>) -> Vec<f32> {
    const WINDOW: usize = 15;
    let (h, w) = ir.dim();
    if h < 50 || w < 4 * WINDOW {
        return vec![1.0; w];
    }
    let med_all = stats::median(&mut ir.iter().copied().collect::<Vec<_>>());
    let scale = (med_all as f64).max(1.0) as f32;
    // dev rows 3..h-3 (the percentile skips the edges)
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            (3..h - 3)
                .map(|y| {
                    let mut v: [f32; 5] = std::array::from_fn(|k| ir[((y as i64 + k as i64 - 2).clamp(0, h as i64 - 1) as usize, x)]);
                    let med = stats::median(&mut v);
                    (ir[(y, x)] - med) / scale
                })
                .collect()
        })
        .collect();
    let tail: Vec<f32> = cols.into_par_iter().map(|mut c| (-stats::percentile(&mut c, 0.5)).max(1e-6)).collect();
    let refv = stats::percentile(&mut tail[w / 4..3 * w / 4].to_vec(), 75.0);
    let r = WINDOW / 2;
    (0..w)
        .map(|x| {
            let mut win: Vec<f32> = (0..WINDOW).map(|k| tail[(x as i64 + k as i64 - r as i64).clamp(0, w as i64 - 1) as usize]).collect();
            (stats::median(&mut win) / refv).clamp(1.0, 4.0)
        })
        .collect()
}

/// IR as stored with the ribbon: rounded to u16.
pub fn ir_to_u16(ir: &Array2<f32>) -> Array2<u16> {
    ir.mapv(|v| (v + 0.5) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi() {
        // 1500 / (24 / 25.4) is 1587.4999…: Python rounds to 1587 (its docstring says 1588)
        assert_eq!([2000, 1500, 1000].map(pakon_dpi), [2117, 1587, 1058]);
    }

    #[test]
    fn density_round_trip() {
        for v in [0.0f32, 1.0, 100.0, 30000.0, 65535.0] {
            assert!((from_density(density(v)) - v).abs() <= v * 1e-5 + 1e-3, "{v}");
        }
    }
}
