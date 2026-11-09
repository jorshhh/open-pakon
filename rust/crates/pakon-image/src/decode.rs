//! Raw stream to the registered RGB ribbon (port of `tools/pakon_image.py`:
//! `detect_row_layout`, `marker_align`, `measure_leads`, `validated_leads`,
//! `register_zones` for the single-zone case `web/decode.py` uses).
//!
//! The 0x86 stream is 16-bit little-endian samples, one scan line per
//! `row_samples`: `width` R,G,B triplets, then `width` IR samples when the
//! row has an IR lane. The scanner sets bit 0 of one fixed word on every
//! line; rotating that word to column 0 fixes the triplet phase wherever the
//! capture started. The sensor is trilinear, so green and blue lead red by a
//! few lines; the ribbon is co-registered on red.

use ndarray::{s, Array2, Array3, ArrayView2, Axis};
use rayon::prelude::*;

/// Every row layout seen on real hardware: stride in samples → whether the
/// row ends in an IR block.
pub const KNOWN_ROW_LAYOUTS: [(usize, bool); 5] = [
    (3000, false), // F-135+ Base 4, no IR (1000 px)
    (4000, true),  // F-135+ Base 4 with IR
    (4500, false), // F-135+ Base 8, no IR (1500 px)
    (6000, false), // F-135+ Base 16, no IR (2000 px)
    (8000, true),  // F-135 (2000 px RGB + IR), F-135+ Base 16 with IR
];

/// A row layout: samples per row and whether it carries an IR lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub row_samples: usize,
    pub ir_lane: bool,
}

impl Layout {
    /// Visible pixels per line.
    pub fn width(&self) -> usize {
        if self.ir_lane { self.row_samples / 4 } else { self.row_samples / 3 }
    }
}

/// Measure a raw stream's row stride by autocorrelation against the known
/// layouts. Among candidates within 0.02 of the best score the smallest wins
/// (multiples of the true stride score as high). `None` when the stream is
/// too short or nothing scores 0.6.
pub fn detect_row_layout(raw: &[u16]) -> Option<Layout> {
    let total = raw.len();
    let window_len = (total / 2).min(2_000_000);
    if window_len < 200_000 {
        return None;
    }
    let start = (total - window_len) / 2;
    let w = &raw[start..start + window_len];
    let mean = w.iter().map(|&v| v as f64).sum::<f64>() / window_len as f64;
    let win: Vec<f64> = w.iter().map(|&v| v as f64 - mean).collect();

    let scores: Vec<(usize, f64)> = KNOWN_ROW_LAYOUTS
        .par_iter()
        .filter(|(lag, _)| lag * 4 <= window_len)
        .filter_map(|&(lag, _)| {
            let (head, tail) = (&win[..window_len - lag], &win[lag..]);
            let (mut hh, mut tt, mut ht) = (0.0, 0.0, 0.0);
            for (h, t) in head.iter().zip(tail) {
                hh += h * h;
                tt += t * t;
                ht += h * t;
            }
            let den = hh.sqrt() * tt.sqrt();
            (den != 0.0).then(|| (lag, ht / den))
        })
        .collect();
    let best = scores.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
    if scores.is_empty() || best < 0.6 {
        return None;
    }
    // KNOWN_ROW_LAYOUTS is sorted by stride: the first close to best wins.
    scores.iter().find(|s| s.1 >= best - 0.02).map(|&(lag, _)| Layout {
        row_samples: lag,
        ir_lane: KNOWN_ROW_LAYOUTS.iter().find(|l| l.0 == lag).unwrap().1,
    })
}

/// The column of the per-line marker bit: the word whose bit 0 is set on at
/// least `min_frac` of the sampled rows (three blocks of `sample_rows / 3`:
/// start, middle, end). `None` when no column is reliably set.
pub fn marker_column(img: ArrayView2<u16>, min_frac: f64, sample_rows: usize) -> Option<usize> {
    let (n, lw) = img.dim();
    let k = (sample_rows / 3).max(1);
    let blocks: Vec<(usize, usize)> =
        if n <= 3 * k { vec![(0, n)] } else { vec![(0, k), ((n - k) / 2, k), (n - k, k)] };
    let mut hits = vec![0u32; lw];
    let mut rows = 0usize;
    for (s0, len) in blocks {
        for row in img.slice(s![s0..s0 + len, ..]).outer_iter() {
            for (h, v) in hits.iter_mut().zip(row) {
                *h += (v & 1) as u32;
            }
            rows += 1;
        }
    }
    let mut cm = 0;
    for (i, &h) in hits.iter().enumerate() {
        if h > hits[cm] {
            cm = i;
        }
    }
    (rows > 0 && hits[cm] as f64 / rows as f64 >= min_frac).then_some(cm)
}

/// The three visible channels of marker-aligned rows: `chan[c][(row, px)] =
/// row[(3 px + c + marker) % row_samples]` (numpy's roll then the stride-3
/// slices).
pub fn deinterleave(img: ArrayView2<u16>, width: usize, marker: usize) -> [Array2<u16>; 3] {
    let (n, lw) = img.dim();
    std::array::from_fn(|c| {
        let mut out = Array2::<u16>::zeros((n, width));
        out.axis_iter_mut(Axis(0)).into_par_iter().zip(img.axis_iter(Axis(0))).for_each(|(mut o, r)| {
            for (px, v) in o.iter_mut().enumerate() {
                *v = r[(3 * px + c + marker) % lw];
            }
        });
        out
    })
}

/// The central band `_vlead` measures on: rows [0.35 n, 0.65 n).
pub fn lead_band(n: usize) -> (usize, usize) {
    ((n as f64 * 0.35) as usize, (n as f64 * 0.65) as usize)
}

/// Vertical lead of channel `ch` over `refc` in scan lines: the dy that
/// maximises the correlation of their vertical log gradients (every 3rd
/// column), measured on all the rows given (callers pass [`lead_band`]). A
/// feature at ref row y appears in `ch` at row y - dy.
pub fn vlead(ch: ArrayView2<u16>, refc: ArrayView2<u16>, rng: usize) -> i32 {
    let (a, b) = (0, ch.nrows());
    let cols: Vec<usize> = (0..ch.ncols()).step_by(3).collect();
    let grad = |m: ArrayView2<u16>| -> Array2<f32> {
        let lg = |v: u16| (v as f32 + 50.0).ln();
        let rows = b.saturating_sub(a).saturating_sub(1);
        let mut g = Array2::<f32>::zeros((rows, cols.len()));
        for i in 0..rows {
            for (k, &c) in cols.iter().enumerate() {
                g[(i, k)] = lg(m[(a + i + 1, c)]) - lg(m[(a + i, c)]);
            }
        }
        g
    };
    let (ga, gr) = (grad(ch), grad(refc));
    let rows = ga.nrows();
    let rng = rng.min(rows / 4);
    if rows <= 2 * rng {
        return 0;
    }
    let len = rows - 2 * rng;
    let base = ga.slice(s![rng..rng + len, ..]);
    let bmean = base.iter().map(|&v| v as f64).sum::<f64>() / base.len() as f64;
    let bnorm = base.iter().map(|&v| (v as f64 - bmean).powi(2)).sum::<f64>().sqrt();

    let score = |dy: i32| -> Option<f64> {
        let r0 = (rng as i32 + dy) as usize;
        let cmp = gr.slice(s![r0..r0 + len, ..]);
        let cmean = cmp.iter().map(|&v| v as f64).sum::<f64>() / cmp.len() as f64;
        let (mut dot, mut cc) = (0.0, 0.0);
        for (x, y) in base.iter().zip(cmp.iter()) {
            let (x, y) = (*x as f64 - bmean, *y as f64 - cmean);
            dot += x * y;
            cc += y * y;
        }
        let d = bnorm * cc.sqrt();
        (d != 0.0).then(|| dot / d)
    };
    let r = rng as i32;
    let scores: Vec<(i32, Option<f64>)> = (-r..=r).into_par_iter().map(|dy| (dy, score(dy))).collect();
    let mut best = (0, -2.0);
    for (dy, c) in scores {
        if let Some(c) = c {
            if c > best.1 {
                best = (dy, c);
            }
        }
    }
    best.0
}

/// Trilinear leads of G and B relative to R, in scan lines (R is 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leads {
    pub g: i32,
    pub b: i32,
}

impl Leads {
    fn of(&self, c: usize) -> i32 {
        [0, self.g, self.b][c]
    }
}

/// Measure the G/B leads of marker-aligned rows (full width, 40-line
/// search), deinterleaving only the central band.
pub fn measure_leads(img: ArrayView2<u16>, width: usize, marker: usize) -> Leads {
    let (a, b) = lead_band(img.nrows());
    let ch = deinterleave(img.slice(s![a..b, ..]), width, marker);
    let (g, b) = rayon::join(|| vlead(ch[1].view(), ch[0].view(), 40), || vlead(ch[2].view(), ch[0].view(), 40));
    Leads { g, b }
}

/// The fixed trilinear spacing for a line `width`: -8 / -16 at Base 16
/// (2000 px), scaled with the width.
pub fn trilinear_leads(width: usize) -> Leads {
    let s = width as f64 / 2000.0;
    // Python's round(): half to even.
    let r = |v: f64| (v * s).round_ties_even() as i32;
    Leads { g: r(-8.0), b: r(-16.0) }
}

/// Accept measured leads only if physically plausible for the width, else
/// the fixed spacing. Returns (leads, measured_ok).
pub fn validated_leads(m: Leads, width: usize) -> (Leads, bool) {
    let s = width as f64 / 2000.0;
    let (g, b) = (m.g as f64, m.b as f64);
    if -13.0 * s <= g && g <= -3.0 * s && -22.0 * s <= b && b <= -9.0 * s {
        (m, true)
    } else {
        (trilinear_leads(width), false)
    }
}

/// Deinterleave and co-register marker-aligned rows on red in one pass:
/// (rows, width, 3), cropped to the rows every channel covers
/// (`register_zones` with one zone).
pub fn register(img: ArrayView2<u16>, width: usize, marker: usize, leads: Leads) -> Array3<u16> {
    let all = [0, leads.g, leads.b];
    let (gm, glo) = (*all.iter().max().unwrap(), *all.iter().min().unwrap());
    let (n, lw) = img.dim();
    let l = (n as i32 - (gm - glo)).max(0) as usize;
    let mut out = Array3::<u16>::zeros((l, width, 3));
    out.axis_iter_mut(Axis(0)).into_par_iter().enumerate().for_each(|(i, mut row)| {
        for c in 0..3 {
            let src = img.row((gm - leads.of(c)) as usize + i);
            for px in 0..width {
                row[(px, c)] = src[(3 * px + c + marker) % lw];
            }
        }
    });
    out
}
