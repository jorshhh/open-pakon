//! The OEM slide (positive) path and the ICC device links (port of
//! `ColRevLink`, `find_slide_gaps`, `measure_slide_flare`, `render_slide` in
//! `tools/pakon_image.py`).
//!
//! TLB.dll film type 2: the same density LUT, the unit's PosMatrix (no
//! inversion), ColRevLut1.pf (a lut16 device link to 12-bit ROMM), then
//! romm.pf to sRGB. Two departures from the OEM, both for the shadows: the
//! density is computed in floating point, and the measured stray light
//! (`flare`) is subtracted first.

use std::path::Path;

use ndarray::{s, Array3, ArrayView3, Axis, Zip};

use crate::colour::{self, Matrix, Table};
use crate::stats;
use crate::RAW_BLACK;

/// numpy's `np.interp(x, linspace(0, 1, n), fp)` for one value, f64.
fn interp(x: f64, fp: &[f64]) -> f64 {
    let n = fp.len();
    let xp = |i: usize| if i == n - 1 { 1.0 } else { i as f64 * (1.0 / (n - 1) as f64) };
    if x.is_nan() {
        return x;
    }
    if x < 0.0 {
        return fp[0];
    }
    if x > 1.0 {
        return fp[n - 1];
    }
    // largest j with xp[j] <= x
    let (mut lo, mut hi) = (0usize, n - 1);
    if x >= xp(n - 1) {
        lo = n - 1;
    } else {
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if xp(mid) <= x { lo = mid } else { hi = mid }
        }
    }
    let j = lo;
    if j == n - 1 || xp(j) == x {
        return fp[j];
    }
    let slope = (fp[j + 1] - fp[j]) / (xp(j + 1) - xp(j));
    let r = slope * (x - xp(j)) + fp[j];
    if r.is_nan() {
        let r2 = slope * (x - xp(j + 1)) + fp[j + 1];
        if r2.is_nan() && fp[j] == fp[j + 1] {
            return fp[j];
        }
        return r2;
    }
    r
}

/// An ICC lut16 (`mft2`) A2B0 device link, 3 in / 3 out: input curves, a
/// g³ grid, output curves, all on [0, 1] (`ColRevLink`).
#[derive(Debug, Clone)]
pub struct Link {
    inp: [Vec<f64>; 3],
    clut: Table,
    outp: [Vec<f64>; 3],
}

impl Link {
    pub fn read(path: impl AsRef<Path>) -> Result<Link, String> {
        let path = path.as_ref();
        let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Link::parse(&d).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(d: &[u8]) -> Result<Link, String> {
        let be32 = |p: usize| -> Result<usize, String> {
            d.get(p..p + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).ok_or("truncated".to_string())
        };
        let be16 = |p: usize| -> Result<u16, String> {
            d.get(p..p + 2).map(|b| u16::from_be_bytes(b.try_into().unwrap())).ok_or("truncated".to_string())
        };
        let ntags = be32(128)?;
        let mut off = None;
        for k in 0..ntags {
            let t = 132 + 12 * k;
            if d.get(t..t + 4) == Some(b"A2B0") {
                off = Some(be32(t + 4)?);
            }
        }
        let off = off.filter(|&o| d.get(o..o + 4) == Some(b"mft2")).ok_or("no lut16 A2B0 tag")?;
        let (ni, no, g) = (d[off + 8], d[off + 9], d[off + 10] as usize);
        if ni != 3 || no != 3 {
            return Err("expected a 3-in, 3-out link".into());
        }
        let (nin, nout) = (be16(off + 48)? as usize, be16(off + 50)? as usize);
        let mut p = off + 52;
        let curves = |n: usize, p: &mut usize| -> Result<[Vec<f64>; 3], String> {
            let mut out: [Vec<f64>; 3] = Default::default();
            for c in out.iter_mut() {
                for _ in 0..n {
                    c.push(be16(*p)? as f64 / 65535.0);
                    *p += 2;
                }
            }
            Ok(out)
        };
        let inp = curves(nin, &mut p)?;
        let mut grid = Vec::with_capacity(g * g * g * 3);
        for _ in 0..g * g * g * 3 {
            grid.push((be16(p)? as f64 / 65535.0) as f32);
            p += 2;
        }
        let outp = curves(nout, &mut p)?;
        Ok(Link { inp, clut: Table { n: g, data: grid }, outp })
    }

    fn curves(tabs: &[Vec<f64>; 3], x: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| interp(x[c] as f64, &tabs[c]) as f32)
    }

    /// One value in [0, 1]³ through the link.
    pub fn apply(&self, x: [f32; 3]) -> [f32; 3] {
        Self::curves(&self.outp, self.clut.lookup(Self::curves(&self.inp, x)))
    }
}

/// The black film between slide frames, as `(first_row, last_row)`: rows
/// whose 95th percentile (central columns, every 5th) stays within `k` times
/// the darkest rows' level, in runs of at least `min_rows`.
pub fn find_slide_gaps(rgb: ArrayView3<u16>, min_rows: usize, k: f64) -> Vec<(usize, usize)> {
    let cols = rgb.len_of(Axis(1));
    let band = rgb.slice(s![.., cols / 8..cols - cols / 8;5, ..]);
    let mut p95: Vec<f32> = band
        .outer_iter()
        .map(|row| {
            let mut lum: Vec<f32> = row.outer_iter().map(|p| stats::mean3([p[0] as f32, p[1] as f32, p[2] as f32]) - RAW_BLACK).collect();
            stats::percentile(&mut lum, 95.0)
        })
        .collect();
    let p1 = stats::percentile(&mut p95.clone(), 1.0) as f64;
    let thr = (k * p1.max(1.0)) as f32;
    let dark: Vec<bool> = p95.drain(..).map(|v| v < thr).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < dark.len() {
        if dark[i] {
            let a = i;
            while i < dark.len() && dark[i] {
                i += 1;
            }
            if i - a >= min_rows {
                out.push((a, i - 1));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Default share of the measured veil to remove.
pub const SLIDE_FLARE: f32 = 0.5;

/// Stray light on a slide strip, per channel, in raw counts above black:
/// `strength` x (the gap rows' median − the D 3.5 level), the level capped at
/// D 2. Zeros when no gap is found.
pub fn measure_slide_flare(rgb: ArrayView3<u16>, strength: f32, gaps: Option<&[(usize, usize)]>) -> [f32; 3] {
    let found;
    let gaps = match gaps {
        Some(g) => g,
        None => {
            found = find_slide_gaps(rgb, 80, 3.0);
            &found
        }
    };
    if strength <= 0.0 || gaps.is_empty() {
        return [0.0; 3];
    }
    let cols = rgb.len_of(Axis(1));
    let (c0, c1) = (cols / 8, cols - cols / 8);
    let mut ch: [Vec<f32>; 3] = Default::default();
    for &(a, b) in gaps {
        for r in a..=b {
            for x in (c0..c1).step_by(4) {
                for c in 0..3 {
                    ch[c].push(rgb[(r, x, c)] as f32);
                }
            }
        }
    }
    let floor = (64000.0 * 10f64.powf(-3.5)) as f32;
    let cap = (64000.0 * 10f64.powf(-2.0)) as f32;
    std::array::from_fn(|c| {
        let level = (stats::median(&mut ch[c]) - RAW_BLACK).min(cap);
        strength * (level - floor).max(0.0)
    })
}

/// Everything `render_slide` needs besides the frame.
pub struct SlidePath {
    pub posmat: Matrix,
    pub lut: Vec<f32>,
    pub link: Link,
    pub romm: Table,
}

impl SlidePath {
    /// ColRevLut1.pf from `profiles`, the embedded romm.pf table.
    pub fn load(posmat: Matrix, profiles: &Path) -> Result<SlidePath, String> {
        Ok(SlidePath {
            posmat,
            lut: colour::c41_lut(),
            link: Link::read(profiles.join("ColRevLut1.pf"))?,
            romm: Table::from_u8(52, crate::ROMM_TABLE_U8)?,
        })
    }
}

/// Raw slide frame → 16-bit sRGB (`render_slide`, no adjustments).
pub fn render_slide(rgb: ArrayView3<u16>, sp: &SlidePath, flare: Option<[f32; 3]>) -> Array3<u16> {
    let mut out = Array3::<u16>::zeros(rgb.dim());
    let fl = flare.unwrap_or([0.0; 3]);
    Zip::from(out.lanes_mut(Axis(2))).and(rgb.lanes(Axis(2))).par_for_each(|mut o, p| {
        let x: [f32; 3] = std::array::from_fn(|c| if flare.is_some() { p[c] as f32 - fl[c] } else { p[c] as f32 });
        let cmy = colour::rpd(x, &sp.posmat, &sp.lut, true).map(|v| v / 4095.0f32);
        let srgb = sp.romm.lookup(sp.link.apply(cmy));
        for c in 0..3 {
            o[c] = (srgb[c] * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16;
        }
    });
    out
}

/// `render_slide` with per-picture adjustments: the 12-bit arithmetic on the
/// ROMM values (where the OEM runs PIColorAdjustPlanar for slides), then
/// `table` (romm.pf, or `adjust::adjusted_table` of it for saturation/tone).
pub fn render_slide_adjusted(rgb: ArrayView3<u16>, sp: &SlidePath, flare: Option<[f32; 3]>, adj: &colour::Adjust, table: &Table) -> Array3<u16> {
    let fl = flare.unwrap_or([0.0; 3]);
    let mut romm = Array3::<f32>::zeros(rgb.dim());
    Zip::from(romm.lanes_mut(Axis(2))).and(rgb.lanes(Axis(2))).par_for_each(|mut o, p| {
        let x: [f32; 3] = std::array::from_fn(|c| if flare.is_some() { p[c] as f32 - fl[c] } else { p[c] as f32 });
        let cmy = colour::rpd(x, &sp.posmat, &sp.lut, true).map(|v| v / 4095.0f32);
        let r = sp.link.apply(cmy);
        for c in 0..3 {
            o[c] = r[c] * 4095.0f32;
        }
    });
    crate::adjust::adjust_12bit(&mut romm, adj);
    let mut out = Array3::<u16>::zeros(rgb.dim());
    Zip::from(out.lanes_mut(Axis(2))).and(romm.lanes(Axis(2))).par_for_each(|mut o, r| {
        let srgb = table.lookup([r[0] / 4095.0f32, r[1] / 4095.0f32, r[2] / 4095.0f32]);
        for c in 0..3 {
            o[c] = (srgb[c] * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16;
        }
    });
    out
}
