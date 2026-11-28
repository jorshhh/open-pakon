//! PSI 2.4 EnhancedBlackPrinting (EBP): the four stages the real C-41 path runs on 8-bit sRGB after the
//! Kodak CMM, decoded from `PakonIMAu.dll` 2.4: Gain and Offset (`ImaEBPGainAndOffsetOp`, per-channel
//! percentile auto-levels plus a luma knee), Gamma (`ImaEBPGammaOp`, image-adaptive luma gamma), Noise
//! Filter (`ImaEBPNoiseFilterOp`) and Black Print (`ImaEBPBlackPrintOp`). The constants are the plain
//! attribute defaults of the DLL's 8-bit (`uchar`) instances (256-bin histograms, 256-entry LUTs).
//!
//! The 8-bit stages work on [`Planes`], which stores `level << 4` (a 12-bit-looking plane whose low
//! four bits are zero between stages). [`hq_params`] / [`hq_apply`] run the same chain on float planes.

use rayon::prelude::*;

const MAXV: usize = 4095;

// Plain attribute defaults of the 8-bit instances (probe dump, `PROBE_FINDINGS.md`).
const BLACK_PCT: f64 = 0.10;
const WHITE_PCT: f64 = 0.15;
const MEAN_LO_PCT: f64 = 39.06;
const MEAN_HI_PCT: f64 = 48.82;
const CITRAS: f64 = 0.5;

/// An 8-bit three-plane image stored as `level << 4`.
#[derive(Debug, Clone)]
pub struct Planes {
    pub w: usize,
    pub h: usize,
    pub p: [Vec<u16>; 3],
}

impl Planes {
    pub fn new(w: usize, h: usize) -> Planes {
        Planes { w, h, p: std::array::from_fn(|_| vec![0u16; w * h]) }
    }
}

/// The 8-bit instance's blurs: horizontal pass then vertical pass, each divided
/// by its weight sum with truncation (`>> 4` for [1 4 6 4 1], `>> 2` for
/// [1 2 1]); reproduces the DLL's dumped histograms bit for bit. Returns 8-bit values.
fn blur_floor(src: &[i32], w: usize, h: usize, k: &[i32], shift: u32) -> Vec<i32> {
    let r = k.len() / 2;
    let mut tmp = vec![0i32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0;
            for (i, wt) in k.iter().enumerate() {
                s += wt * row[(x + i).saturating_sub(r).min(w - 1)];
            }
            out[x] = s >> shift;
        }
    });
    let mut out = vec![0i32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        for x in 0..w {
            let mut s = 0;
            for (i, wt) in k.iter().enumerate() {
                s += wt * tmp[((y + i).saturating_sub(r)).min(h - 1) * w + x];
            }
            o[x] = s >> shift;
        }
    });
    out
}

/// Result of [`gain_and_offset`].
#[derive(Debug, Clone, Copy)]
pub struct GainInfo {
    pub lo: [i32; 3],
    pub hi: [i32; 3],
    pub lo2: i32,
    pub hi2: i32,
    pub k1: f64,
    /// `k1 * 3*255/sum(ranges)`: what the Gamma stage reads from this one.
    pub p: f64,
    /// Stage-1 offsets (8-bit units) and gains, as applied (for the HQ path).
    pub off: [i32; 3],
    pub s: [f64; 3],
    /// Knee `(l2, h2, tl, th, k1, k2)` in 12-bit units, `None` when only stage 1 ran.
    pub knee: Option<[f64; 6]>,
}

// Further defaults of ImaEBPGainAndOffsetOp.
const SH_PCT: f64 = 0.06;
const SHADOW_THR: i32 = 5;
const HIGHLIGHT_THR: i32 = 250;

/// Gain and Offset: per-channel LUT from 0.08 % / 0.10 % percentiles of the
/// blurred 8-bit histogram (stretch capped near 2x for the narrowest
/// channel), plus the luma shadow/highlight knee when the histogram needs it.
pub fn gain_and_offset(img: &mut Planes) -> GainInfo {
    let (w, h) = (img.w, img.h);
    let n = w * h;
    let mut hist = [[0u64; 256]; 3];
    for c in 0..3 {
        let x8: Vec<i32> = img.p[c].iter().map(|&v| (v >> 4) as i32).collect();
        for v in blur_floor(&x8, w, h, &[1, 4, 6, 4, 1], 4) {
            hist[c][v.clamp(0, 255) as usize] += 1;
        }
    }
    let nb = (n as f64 * BLACK_PCT * 0.01 + 0.5) as u64;
    let nw = (n as f64 * WHITE_PCT * 0.01 + 0.5) as u64;
    let mut lo = [0i32; 3];
    let mut hi = [0i32; 3];
    let mut valid = [false; 3];
    for c in 0..3 {
        lo[c] = if nb > 0 {
            let mut acc = 0;
            (0..256).find(|&i| { acc += hist[c][i]; acc >= nb }).unwrap_or(256) as i32
        } else {
            0
        };
        hi[c] = if nw > 0 {
            let mut acc = 0;
            255 - (0..256).find(|&k| { acc += hist[c][255 - k]; acc >= nw }).unwrap_or(256) as i32
        } else {
            255
        };
        valid[c] = hi[c] > lo[c];
    }
    let avg_hi = hi.iter().sum::<i32>() / 3;
    let mut bvar2 = false;
    if hi[0] <= avg_hi {
        for c in 0..3 {
            if valid[c] {
                hi[c] = (hi[c] + avg_hi) / 2;
            }
        }
    } else {
        bvar2 = true;
        let h0 = hi[0];
        for c in 0..3 {
            if valid[c] {
                hi[c] = ((hi[c] + h0) / 2 + 255) / 2;
            }
        }
    }
    let t = ((lo[0] + lo[2]) >> 1) & 0xff;
    if t < lo[1] && valid[1] {
        lo[1] = t;
    }
    // A channel with no range (hi <= lo) counts as empty: range 0 makes the whole stage the identity.
    let r: [i32; 3] = std::array::from_fn(|c| if hi[c] <= lo[c] { 0 } else { hi[c] - lo[c] });
    let tot: i32 = r.iter().sum();
    let gain_avg = if tot > 0 { 3.0 * 255.0 / tot as f64 } else { 1.0 };
    let minr = *r.iter().min().unwrap();
    let r2: [i32; 3] = if minr < 128 {
        if minr <= 0 { [0; 3] } else { std::array::from_fn(|c| ((r[c] * 128 + (minr >> 1)) / minr).min(255)) }
    } else {
        r
    };
    let s: [f64; 3] = std::array::from_fn(|c| if r2[c] > 0 { 255.0 / r2[c] as f64 } else { 1.0 });
    let off: [i32; 3] = std::array::from_fn(|c| if r2[c] > 0 { lo[c] } else { 0 });
    let lut8: [Vec<i32>; 3] = std::array::from_fn(|c| (0..256).map(|v| ((((v - off[c]) as f64 * s[c]) + 0.5).floor() as i32).clamp(0, 255)).collect());
    // Luma histogram of the stage-1 image (unblurred).
    let mut h2 = [0u64; 256];
    for i in 0..n {
        let m: i32 = (0..3).map(|c| lut8[c][(img.p[c][i] >> 4) as usize]).sum();
        h2[((3 + 2 * m) / 6) as usize] += 1;
    }
    let cnt2 = (n as f64 * SH_PCT * 0.03 + 0.5) as u64;
    let mut b = 3usize;
    let mut acc = h2[2] + h2[3] + h2[4];
    while acc < cnt2 && b < 255 {
        b += 1;
        if b < 255 {
            acc = h2[b - 1] + h2[b] + h2[b + 1];
        }
    }
    let lo2 = b as i32;
    let cnt2h = if bvar2 { cnt2 / 2 } else { cnt2 };
    let mut u = 0xfcusize;
    acc = h2[0xfb] + h2[0xfc] + h2[0xfd];
    while acc < cnt2h && u > 3 {
        u -= 1;
        acc = h2[u - 1] + h2[u] + h2[u + 1];
    }
    let hi2 = u as i32;
    let mut k1 = 1.0;
    let mut knee = None;
    let mut luts: [Vec<u16>; 3] = Default::default();
    let stage1_only = (lo2 <= SHADOW_THR && HIGHLIGHT_THR <= hi2) || (hi2 as f64) < lo2 as f64 + 63.75;
    if stage1_only {
        for c in 0..3 {
            luts[c] = (0..=MAXV).map(|x| ((x as f64 - (off[c] << 4) as f64) * s[c]).floor().clamp(0.0, MAXV as f64) as u16).collect();
        }
    } else {
        let tgt_lo = if lo2 > SHADOW_THR { lo2 * 2 / 3 } else { lo2 };
        let tgt_hi = if bvar2 { (hi2 * 3 + 255) >> 2 } else { (hi2 + 255) / 2 };
        let (l2, h2f, tl, th) = ((lo2 << 4) as f64, (hi2 << 4) as f64, (tgt_lo << 4) as f64, (tgt_hi << 4) as f64);
        k1 = if h2f > l2 { (th - tl) / (h2f - l2) } else { 1.0 };
        // the 8-bit instance tops out at 255 (= 4080 in these units), not at 4095
        let top = 4080.0;
        let k2 = if top > h2f { (top - th) / (top - h2f) } else { 1.0 };
        knee = Some([l2, h2f, tl, th, k1, k2]);
        for c in 0..3 {
            luts[c] = (0..=MAXV)
                .map(|x| {
                    // the knee is computed on the 0..255 scale and rounded once
                    let (l2, h2, tl, th) = (lo2 as f64, hi2 as f64, tgt_lo as f64, tgt_hi as f64);
                    let k2 = if 255.0 > h2 { (255.0 - th) / (255.0 - h2) } else { 1.0 };
                    let y1 = (x as f64 / 16.0 - off[c] as f64) * s[c];
                    let y = if y1 < l2 {
                        y1 * if l2 != 0.0 { tl / l2 } else { 1.0 }
                    } else if y1 <= h2 {
                        tl + (y1 - l2) * k1
                    } else {
                        th + (y1 - h2) * k2
                    };
                    ((y + 0.5).floor().clamp(0.0, 255.0) as u16) << 4
                })
                .collect();
        }
    }
    for c in 0..3 {
        let l = &luts[c];
        img.p[c].par_iter_mut().for_each(|v| *v = l[(*v as usize).min(MAXV)]);
    }
    // the DLL stores a byte: keep no fractional bits of the 12-bit stage-1 LUT
    for c in 0..3 {
        img.p[c].par_iter_mut().for_each(|v| *v = (((*v as u32 + 8) >> 4).min(255) << 4) as u16);
    }
    GainInfo { lo, hi, lo2, hi2, k1, p: k1 * gain_avg, off, s, knee }
}

// Further defaults of ImaEBPGammaOp.
const G_LO: f64 = 0.7;
const G_HI: f64 = 2.0;
/// Edge threshold factor (struct +0x38, in Q7 units of the blur difference).
const EDGE_THR: f64 = 0.75;

/// How Gamma picks its exponent.
#[derive(Debug, Clone, Copy)]
pub enum Gamma {
    /// The decoded estimator, clamped to `[0.7, cap]`.
    Auto { cap: f64 },
    Fixed(f64),
}

/// Trimmed-histogram statistics of the luma, the inputs of the estimator.
#[derive(Debug, Clone, Copy)]
pub struct GammaEstimate {
    pub gamma: f64,
    pub mean: f64,
    pub median: i32,
}

/// The estimator: 5x5-blurred 8-bit luma histogram with the two outermost
/// non-empty bins dropped at each end gives `mean` and `median`; a dark image
/// (mean below 30 %) gets `g2` (pulling the median up), a bright one `g1`
/// (pulling the mean of the edge pixels to the target), in between a blend.
/// Narrow histograms (span <= 63.75 bins) give 1. The edge-pixel threshold
/// scale (`EDGE_THR * sigma`, Q7) is read from the disassembly and reproduces the DLL's g1 exactly.
pub fn estimate_gamma(img: &Planes) -> GammaEstimate {
    let (w, h) = (img.w, img.h);
    let n = w * h;
    let t_lo = (MEAN_LO_PCT * 2.56).ceil();
    let t_hi = (MEAN_HI_PCT * 2.56).ceil();
    let l8: Vec<i32> = (0..n).map(|i| ((img.p[0][i] >> 4) as i32 + (img.p[1][i] >> 4) as i32 + (img.p[2][i] >> 4) as i32 + 1) / 3).collect();
    // The DLL runs the luma through 5-tap blurs in Q7 (l8 << 7, `>> 4` per pass) = b5 and rounds once when
    // binning: (b5 + 64) >> 7 (reproduces the DLL's gamma, struct +0x58, on all frames).
    let q7: Vec<i32> = l8.iter().map(|v| v << 7).collect();
    drop(l8);
    let b5 = blur_floor(&q7, w, h, &[1, 4, 6, 4, 1], 4);
    drop(q7);
    let mut hist = [0i64; 256];
    for &s in &b5 {
        hist[((s + 64) >> 7).clamp(0, 255) as usize] += 1;
    }
    let (mut i, mut seen, mut cnt_lo, mut sum_lo) = (0usize, 0, 0i64, 0i64);
    while i < 255 && seen < 2 {
        if hist[i] > 0 {
            cnt_lo += hist[i];
            seen += 1;
            sum_lo += hist[i] * i as i64;
        }
        i += 1;
    }
    let lo_end = i as i64;
    let (mut j, mut seen, mut cnt_hi, mut sum_hi) = (255i64, 0, 0i64, 0i64);
    while j >= 0 && seen < 2 {
        if hist[j as usize] > 0 {
            cnt_hi += hist[j as usize];
            seen += 1;
            sum_hi += hist[j as usize] * j;
        }
        j -= 1;
    }
    let inner = n as i64 - cnt_hi - cnt_lo;
    let half = (inner as f64 * 0.5).ceil() as i64;
    let mut est = GammaEstimate { gamma: 1.0, mean: 0.0, median: 0 };
    if half >= 1 && ((j - (lo_end - 1) - 1) as f64) > 63.75 {
        let (mut cum, mut k) = (0i64, 0usize);
        while cum - cnt_lo < half && k < 256 {
            cum += hist[k];
            k += 1;
        }
        let med = k as i32 - 1;
        let tot: i64 = hist.iter().enumerate().map(|(i, &c)| c * i as i64).sum();
        let mean = (tot - sum_lo - sum_hi) as f64 / inner.max(1) as f64;
        let mut g1 = 1.0;
        if mean >= t_lo {
            // DLL (0x10181245..0x10181405): b35 = [1 2 1] blur of b5 (`>> 2` per pass); diff = b35 - b5;
            // threshold round(0.75 * sigma) in Q7, two-sided; the edge mean averages round(b5 / 128).
            let b35 = blur_floor(&b5, w, h, &[1, 2, 1], 2);
            let ss: u64 = (0..n).into_par_iter().map(|i| { let d = (b35[i] - b5[i]) as i64; (d * d) as u64 }).sum();
            let sig = (ss as f64 / (n.max(2) - 1) as f64).sqrt();
            let thr = (sig * EDGE_THR + 0.5) as i32;
            let (mut sum, mut cnt) = (0u64, 0u64);
            for i in 0..n {
                if (b35[i] - b5[i]).abs() > thr {
                    sum += ((b5[i] + 64) >> 7) as u64;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                let e = sum as f64 / cnt as f64 / 255.0;
                if e.ln() != 0.0 {
                    g1 = ((MEAN_HI_PCT * 0.01 + e) * 0.5).ln() / e.ln();
                }
            }
        }
        let g2 = (mean < t_hi).then(|| (((med as f64 * 3.0 + t_hi) * mean / (med as f64 * 3.0 + mean)) / 255.0).ln() / (mean / 255.0).ln());
        let g = match g2 {
            None => g1,
            Some(g2) if mean <= t_lo => g2,
            Some(g2) => ((mean - t_lo) * g1 + (t_hi - mean) * g2) / (t_hi - t_lo),
        };
        est = GammaEstimate { gamma: g.clamp(G_LO, G_HI), mean, median: med };
    }
    est
}

/// Gamma: luma through `x^g`, chroma scaled by `1 + citras * (slope - 1)`
/// (`p` is [`GainInfo::p`]). Returns the exponent used.
pub fn gamma_correction(img: &mut Planes, p: f64, mode: Gamma) -> f64 {
    let g = match mode {
        Gamma::Fixed(g) => g,
        Gamma::Auto { cap } => estimate_gamma(img).gamma.min(cap),
    };
    {
        // 256-entry tables, chroma term truncated toward zero (checked against the real stage dumps).
        const M8: i32 = 255;
        let lut8: Vec<i32> = (0..=M8).map(|i| ((M8 as f64 * (i as f64 / M8 as f64).powf(g) + 0.5).floor() as i32).clamp(0, M8)).collect();
        let gain8: Vec<i32> = (0..=M8)
            .map(|i| {
                let t = i as f64 / (M8 as f64 * p);
                let mut slope = t.powf(g - 1.0) * p.powf(g) * g;
                if !slope.is_finite() {
                    slope = 1.0;
                }
                (((slope - 1.0) * CITRAS + 1.0) * (1.0 / p) * 4096.0 + 0.5).floor() as i32
            })
            .collect();
        let [pr, pg, pb] = &mut img.p;
        (pr, pg, pb).into_par_iter_zip(|r, g_, b| {
            let (r8, g8, b8) = ((*r >> 4) as i32, (*g_ >> 4) as i32, (*b >> 4) as i32);
            let l = ((r8 + g8 + b8) + 1) / 3;
            let (ln, gk) = (lut8[l as usize], gain8[l as usize]);
            for (v, c) in [(r, r8), (g_, g8), (b, b8)] {
                *v = ((ln + ((c - l) * gk) / 4096).clamp(0, M8) as u16) << 4;
            }
        });
    }
    g
}

trait ZipThree {
    fn into_par_iter_zip(self, f: impl Fn(&mut u16, &mut u16, &mut u16) + Sync + Send);
}
impl ZipThree for (&mut Vec<u16>, &mut Vec<u16>, &mut Vec<u16>) {
    fn into_par_iter_zip(self, f: impl Fn(&mut u16, &mut u16, &mut u16) + Sync + Send) {
        self.0.par_iter_mut().zip(self.1.par_iter_mut()).zip(self.2.par_iter_mut()).for_each(|((a, b), c)| f(a, b, c));
    }
}

/// The whole 8-bit chain: Gain and Offset, Gamma, Noise Filter, Black Print.
pub fn enhanced_black_printing(img: &mut Planes, gamma: Gamma) -> (GainInfo, f64) {
    let gi = gain_and_offset(img);
    let g = gamma_correction(img, gi.p, gamma);
    noise_filter(img);
    black_print(img);
    (gi, g)
}


// ImaEBPNoiseFilterOp<uchar>, exec at 0x1016c830, decoded from the DLL (attributes at
// +0xc4 = 20.0, +0xc8 = 2.0, +0xcc = 10.0) and checked against the stage dumps of four real
// frames (99.9-100 % of samples exact). It is image-adaptive:
//   1. noise estimate nv = sum((d*d + 32) >> 6) / (n*64 - 64) over the interior, d = the 3x3
//      Laplacian (8*c - sum of the 8 neighbours) of R - B;
//   2. luma Y = (R+G+B)/3; if nv > 0.25 Y is first blurred with [1 1 1; 1 8 1; 1 1 1] >> 4;
//   3. detail hp3 = Y - lp3(Y), hp5 = Y - lp5(Y) (+128, byte): lp3 = [1 3 1]/5 horizontally and
//      [1 2 2]/5 over rows (y-1, y, y+1) vertically (the DLL's vertical pass overwrites its row
//      buffer before reloading it), lp5 = [1 4 6 4 1] >> 4 per axis, every pass truncated;
//   4. Y += trunc(core(hp3) * A + .5) + trunc(core(hp5) * B + .5), where core() is a soft
//      coring with threshold X = min(trunc(20 nv + .5), 10), A = min(.4 + 2 nv, 1.5),
//      B = min(.2 + 2 nv, 1);
//   5. chroma C1 = (2G-R-B)/4, C2 = (B-R)/2 (+128) of the unblurred image, blurred with the
//      same 3x3 kernel, then smoothed by m = trunc(10 nv + .5): m < 3 [1 3 1]/5, 3 -> [1 2 1] >> 2,
//      4..5 -> [1 4 6 4 1] >> 4, >= 6 -> binomial 9 >> 8 (horizontal then vertical, truncated);
//   6. R = Y - (4 c1 + 3)/6 - c2, G = Y + (8 c1 + 3)/6, B = Y - (4 c1 + 3)/6 + c2 (C division).
// Left out: the DLL's first output column/row of the 5-tap filter weighs 12:3:1 instead of
// 11:4:1 (no measurable effect).

fn conv_sep(src: &[i32], w: usize, h: usize, co: &[i32], norm: impl Fn(i32) -> i32 + Sync) -> Vec<i32> {
    conv_sep_v(src, w, h, co, co, norm)
}

/// [`conv_sep`] with its own taps `cv` for the vertical pass.
fn conv_sep_v(src: &[i32], w: usize, h: usize, co: &[i32], cv: &[i32], norm: impl Fn(i32) -> i32 + Sync) -> Vec<i32> {
    let k = co.len();
    let l = (k - 1) / 2;
    let mut tmp = vec![0i32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut acc = 0;
            for (i, &c) in co.iter().enumerate() {
                let xx = (x as isize + i as isize - l as isize).clamp(0, w as isize - 1) as usize;
                acc += c * s[xx];
            }
            row[x] = norm(acc);
        }
    });
    let mut out = vec![0i32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut acc = 0;
            for (i, &c) in cv.iter().enumerate() {
                let yy = (y as isize + i as isize - l as isize).clamp(0, h as isize - 1) as usize;
                acc += c * tmp[yy * w + x];
            }
            row[x] = norm(acc);
        }
    });
    out
}

fn binom_row(k: usize) -> Vec<i32> {
    let mut r = vec![1i32];
    for _ in 1..k {
        let mut n = vec![1i32; r.len() + 1];
        for i in 1..r.len() {
            n[i] = r[i - 1] + r[i];
        }
        r = n;
    }
    r
}

/// 3x3 [1 1 1; 1 8 1; 1 1 1] >> 4 (truncating), edges replicated.
fn blur_bp_trunc(src: &[i32], w: usize, h: usize) -> Vec<i32> {
    let mut o = vec![0i32; w * h];
    o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (ya, yb) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            let (xa, xb) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let s = src[ya * w + xa] + src[ya * w + x] + src[ya * w + xb] + src[y * w + xa] + src[y * w + xb] + src[yb * w + xa] + src[yb * w + x] + src[yb * w + xb];
            row[x] = (s + 8 * src[y * w + x]) >> 4;
        }
    });
    o
}

fn nf_hp(y: &[i32], w: usize, h: usize, k: usize) -> Vec<usize> {
    // The DLL's vertical lp3 (FUN_10170b90, k == 3) overwrites its row buffer before reloading it, so it
    // weights rows (y-1, y, y+1) as [1, 2, 2]/5, not [1, 3, 1]/5 (a 0.2 px shift; horizontal stays [1, 3, 1]).
    let lp = if k == 3 { conv_sep_v(y, w, h, &[1, 3, 1], &[1, 2, 2], |a| a / 5) } else { conv_sep(y, w, h, &[1, 4, 6, 4, 1], |a| a >> 4) };
    y.iter().zip(&lp).map(|(&a, &b)| ((a - b + 128) & 255) as usize).collect()
}

fn nf_table(x: i32, gain: f64) -> [i32; 256] {
    std::array::from_fn(|k| {
        let d = k as i32 - 128;
        let v = if d >= x { d - x } else if d < -x { d + x } else { 0 };
        if v == 0 { 0 } else { (v as f64 * gain + 0.5) as i32 }
    })
}

/// The noise estimate `nv` (float32 in the DLL) of an 8-bit planar image.
pub fn noise_level(r: &[i32], b: &[i32], w: usize, h: usize) -> f32 {
    let d: Vec<i32> = r.iter().zip(b).map(|(&a, &c)| a - c).collect();
    let sum: i64 = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut s = 0i64;
            for x in 1..w - 1 {
                let i = y * w + x;
                let nb = d[i - w - 1] + d[i - w] + d[i - w + 1] + d[i - 1] + d[i + 1] + d[i + w - 1] + d[i + w] + d[i + w + 1];
                let v = 8 * d[i] - nb;
                s += ((v * v + 32) >> 6) as i64;
            }
            s
        })
        .sum();
    (sum as f64 / (((w - 2) * (h - 2)) as f64 * 64.0 - 64.0)) as f32
}

pub fn noise_filter(img: &mut Planes) {
    let (w, h) = (img.w, img.h);
    if w < 3 || h < 3 {
        return;
    }
    let raw: [Vec<i32>; 3] = std::array::from_fn(|c| img.p[c].iter().map(|&v| ((v >> 4) as i32).min(255)).collect());
    let nv = noise_level(&raw[0], &raw[2], w, h) as f64;
    let x = ((20.0 * nv + 0.5) as i32).min(10);
    let m = (10.0 * nv + 0.5) as i32;
    let a = (0.4 + 2.0 * nv).min(1.5);
    let b = (0.2 + 2.0 * nv).min(1.0);
    let n = w * h;
    let mut y: Vec<i32> = (0..n).map(|i| (raw[0][i] + raw[1][i] + raw[2][i]) / 3).collect();
    let c1: Vec<i32> = (0..n).map(|i| ((2 * raw[1][i] - raw[2][i] - raw[0][i]) / 4 + 128) & 255).collect();
    let c2: Vec<i32> = (0..n).map(|i| ((raw[2][i] - raw[0][i]) / 2 + 128) & 255).collect();
    if nv > 0.25 {
        y = blur_bp_trunc(&y, w, h);
    }
    let (h3, h5) = (nf_hp(&y, w, h, 3), nf_hp(&y, w, h, 5));
    let (ta, tb) = (nf_table(x, a), nf_table(x, b));
    for i in 0..n {
        y[i] = (y[i] + ta[h3[i]] + tb[h5[i]]).clamp(0, 255);
    }
    let sm = |p: &[i32]| -> Vec<i32> {
        let p = blur_bp_trunc(p, w, h);
        match m {
            i32::MIN..=2 => conv_sep(&p, w, h, &[1, 3, 1], |a| a / 5),
            3 => conv_sep(&p, w, h, &binom_row(3), |a| a >> 2),
            4 | 5 => conv_sep(&p, w, h, &binom_row(5), |a| a >> 4),
            _ => conv_sep(&p, w, h, &binom_row(9), |a| a >> 8),
        }
    };
    let (c1, c2) = (sm(&c1), sm(&c2));
    let [pr, pg, pb] = &mut img.p;
    pr.par_iter_mut().zip(pg.par_iter_mut()).zip(pb.par_iter_mut()).enumerate().for_each(|(i, ((r, g), bl))| {
        let (u, v) = (c1[i] - 128, c2[i] - 128);
        let t = (u * 4 + 3) / 6;
        *r = ((y[i] - t - v).clamp(0, 255) as u16) << 4;
        *g = ((y[i] + (u * 8 + 3) / 6).clamp(0, 255) as u16) << 4;
        *bl = ((y[i] - t + v).clamp(0, 255) as u16) << 4;
    });
}

// ImaEBPBlackPrintOp<uchar> defaults (EBP_FINDINGS.md section 3).
const BP_LUMA: f64 = 0.5;
const BP_CHROMA: f64 = 0.25;
const BP_LUMA_MAX: f64 = 50.0;
const BP_CHROMA_MAX: f64 = 20.0;
const BP_RATIO: f64 = 0.4;

/// Black Print on 8-bit data held as `v8 << 4`: in pixels darker than a
/// threshold (about a third of the mean) the colour blends to a 3x3-blurred,
/// and where the local chroma energy is small to a grey, version. The mean `M`
/// is the global mean over all samples of the whole image (the DLL runs the op in
/// 70 strips but takes the mean of the whole image); 100.00 % bit-exact on 8 Base-16 frames.
pub fn black_print(img: &mut Planes) {
    let (w, h) = (img.w, img.h);
    if w < 2 || h < 2 {
        return;
    }
    let raw: [Vec<i32>; 3] = std::array::from_fn(|c| img.p[c].iter().map(|&v| ((v as i32 + 8) >> 4).min(255)).collect());
    let m: f64 = raw.iter().map(|p| p.iter().map(|&v| v as i64).sum::<i64>()).sum::<i64>() as f64 / (3 * w * h) as f64;
    let lt = ((m * BP_LUMA + 0.5) as i32).clamp(1, BP_LUMA_MAX as i32) as f64;
    let ct = ((m * BP_CHROMA + 0.5) as i32).clamp(1, BP_CHROMA_MAX as i32) as f64;
    let lut1: Vec<i32> = (0..256).map(|v| (((lt - v as f64) * 4096.0 / (lt * BP_RATIO) + 0.5).floor() as i32).clamp(0, 4096)).collect();
    let n2 = (255.0 * 1.5) as usize + 1;
    let lut2: Vec<i32> = (0..n2).map(|v| (((ct - v as f64) * 4096.0 / (ct * BP_RATIO) + 0.5).floor() as i32).clamp(0, 4096)).collect();
    // 3x3 blur [1 1 1; 1 8 1; 1 1 1] / 16, edges replicated.
    let blur: [Vec<i32>; 3] = std::array::from_fn(|c| {
        let p = &raw[c];
        let mut o = vec![0i32; w * h];
        o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let (ya, yb) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let (xa, xb) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let s = p[ya * w + xa] + p[ya * w + x] + p[ya * w + xb] + p[y * w + xa] + p[y * w + xb] + p[yb * w + xa] + p[yb * w + x] + p[yb * w + xb];
                row[x] = (s + 8 * p[y * w + x] + 8) >> 4;
            }
        });
        o
    });
    let [pr, pg, pb] = &mut img.p;
    pr.par_chunks_mut(w).zip(pg.par_chunks_mut(w)).zip(pb.par_chunks_mut(w)).enumerate().for_each(|(y, ((r, g), b))| {
        for x in 0..w {
            let i = y * w + x;
            let l = (blur[0][i] + blur[1][i] + blur[2][i]) / 3;
            let wgt = lut1[l.clamp(0, 255) as usize];
            if wgt == 0 {
                continue;
            }
            let e: i32 = (0..3).map(|c| (l - raw[c][i]).abs()).sum();
            let q = lut2[(e as usize).min(n2 - 1)];
            let i13 = ((4096 - q) * wgt + 0x800) >> 12;
            let s2 = (((q * l + 0x800) >> 12) * wgt + 0x800) >> 12;
            let dst = [&mut r[x], &mut g[x], &mut b[x]];
            for (c, d) in dst.into_iter().enumerate() {
                let o = ((raw[c][i] * (4096 - wgt) + 0x800) >> 12) + s2 + ((blur[c][i] * i13 + 0x800) >> 12);
                *d = (o.clamp(0, 255) as u16) << 4;
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// High-bit-depth ("HQ") path. The same four 'E' stages in the sRGB domain, on float planes with
// the 0..255 scale, without any quantisation between the stages. Every *decision* (G&O lo/hi/lo2/
// hi2/k1/p, the Gamma exponent, the Noise Filter nv/X/m/A/B, the Black Print mean) is taken from
// the exact 8-bit chain ([`hq_params`]), so the look is the one of the bit-exact path; only the
// curves and filters are applied continuously. Thresholds stay on the 0..255 scale.

/// A float three-plane image, values on a 0..255 scale (not clamped between the stages except where
/// the 8-bit stage clamps).
#[derive(Debug, Clone)]
pub struct FPlanes {
    pub w: usize,
    pub h: usize,
    pub p: [Vec<f32>; 3],
}

/// Decisions of the exact 8-bit chain, the parameters of [`hq_apply`].
#[derive(Debug, Clone, Copy)]
pub struct HqParams {
    pub gain: GainInfo,
    pub gamma: f64,
    /// Noise estimate entering the Noise Filter (after Gamma).
    pub nv: f64,
    /// Black Print mean over all samples (after the Noise Filter).
    pub bp_mean: f64,
}

/// Run the exact 8-bit chain on `q8` (8-bit values held as `v8 << 4`) and
/// record its decisions. `q8` ends up as the exact 8-bit result.
pub fn hq_params(q8: &mut Planes, gamma: Gamma) -> HqParams {
    let gain = gain_and_offset(q8);
    let g = gamma_correction(q8, gain.p, gamma);
    let (w, h) = (q8.w, q8.h);
    let raw: [Vec<i32>; 3] = std::array::from_fn(|c| q8.p[c].iter().map(|&v| ((v >> 4) as i32).min(255)).collect());
    let nv = if w >= 3 && h >= 3 { noise_level(&raw[0], &raw[2], w, h) as f64 } else { 0.0 };
    noise_filter(q8);
    let tot: i64 = q8.p.iter().map(|p| p.iter().map(|&v| ((v as i64 + 8) >> 4).min(255)).sum::<i64>()).sum();
    HqParams { gain, gamma: g, nv, bp_mean: tot as f64 / (3 * w * h).max(1) as f64 }
}

/// Stage 1 + knee of Gain and Offset, continuous. `v` on the 0..255 scale.
fn go_hq(f: &mut FPlanes, gi: &GainInfo) {
    for c in 0..3 {
        let (off, s) = (gi.off[c] as f64, gi.s[c]);
        f.p[c].par_iter_mut().for_each(|v| {
            let y1 = (*v as f64 - off) * s * 16.0; // 12-bit units
            let y = match gi.knee {
                None => y1,
                Some([l2, h2, tl, th, k1, k2]) => {
                    if y1 < l2 {
                        y1 * if l2 != 0.0 { tl / l2 } else { 1.0 }
                    } else if y1 <= h2 {
                        tl + (y1 - l2) * k1
                    } else {
                        th + (y1 - h2) * k2
                    }
                }
            };
            // The DLL's stage-1-only LUT floors and the knee LUT rounds; the loss model found no net bias
            // to correct in either (fitted loss 0), so the curve is applied unbiased.
            *v = (y / 16.0).clamp(0.0, 255.0) as f32;
        });
    }
}

fn gamma_hq(f: &mut FPlanes, p: f64, g: f64, shrink: bool) {
    let cit = CITRAS;
    let [pr, pg, pb] = &mut f.p;
    pr.par_iter_mut().zip(pg.par_iter_mut()).zip(pb.par_iter_mut()).for_each(|((r, gg), b)| {
        let l = ((*r + *gg + *b) as f64 / 3.0).max(0.0);
        let ln = 255.0 * (l / 255.0).powf(g);
        let mut slope = (l.max(1.0) / (255.0 * p)).powf(g - 1.0) * p.powf(g) * g;
        if !slope.is_finite() {
            slope = 1.0;
        }
        let gk = ((slope - 1.0) * cit + 1.0) / p;
        for v in [r, gg, b] {
            let mut ch = (*v as f64 - l) * gk;
            if shrink {
                // the DLL's chroma term truncates toward zero: pulled toward 0 by up to 0.5 (review #5)
                ch -= 0.5 * ch.clamp(-1.0, 1.0);
            }
            *v = (ln + ch).clamp(0.0, 255.0) as f32;
        }
    });
}

/// Separable float convolution with edge replication, normalised by the sum of the taps.
fn conv_sep_f(src: &[f32], w: usize, h: usize, co: &[f32]) -> Vec<f32> {
    let l = (co.len() - 1) / 2;
    let norm = 1.0 / co.iter().sum::<f32>();
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut a = 0.0;
            for (i, &c) in co.iter().enumerate() {
                a += c * s[(x as isize + i as isize - l as isize).clamp(0, w as isize - 1) as usize];
            }
            row[x] = a * norm;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (i, &c) in co.iter().enumerate() {
            let t = &tmp[(y as isize + i as isize - l as isize).clamp(0, h as isize - 1) as usize * w..][..w];
            for x in 0..w {
                row[x] += c * t[x] * norm;
            }
        }
    });
    out
}

/// 3x3 `[1 1 1; 1 8 1; 1 1 1] / 16`, edges replicated.
fn blur_bp_f(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut o = vec![0f32; w * h];
    o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (ya, yb) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            let (xa, xb) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let s = src[ya * w + xa] + src[ya * w + x] + src[ya * w + xb] + src[y * w + xa] + src[y * w + xb] + src[yb * w + xa] + src[yb * w + x] + src[yb * w + xb];
            row[x] = (s + 8.0 * src[y * w + x]) * (1.0 / 16.0);
        }
    });
    o
}

fn noise_filter_hq(f: &mut FPlanes, nv: f64, loss: bool) {
    let (w, h) = (f.w, f.h);
    if w < 3 || h < 3 {
        return;
    }
    let x = ((20.0 * nv + 0.5) as i32).min(10) as f32;
    let m = (10.0 * nv + 0.5) as i32;
    let a = (0.4 + 2.0 * nv).min(1.5) as f32;
    let b = (0.2 + 2.0 * nv).min(1.0) as f32;
    let n = w * h;
    let mut y: Vec<f32> = (0..n).map(|i| (f.p[0][i] + f.p[1][i] + f.p[2][i]) / 3.0).collect();
    let mut c1: Vec<f32> = (0..n).map(|i| (2.0 * f.p[1][i] - f.p[2][i] - f.p[0][i]) / 4.0).collect();
    let mut c2: Vec<f32> = (0..n).map(|i| (f.p[2][i] - f.p[0][i]) / 2.0).collect();
    if loss {
        // the DLL's `/4` and `/2` truncate toward zero: chroma is pulled toward 0 by up to 0.375 / 0.25
        c1.par_iter_mut().for_each(|v| *v -= v.clamp(-1.0, 1.0) * 0.375);
        c2.par_iter_mut().for_each(|v| *v -= v.clamp(-1.0, 1.0) * 0.25);
    }
    if nv > 0.25 {
        y = blur_bp_f(&y, w, h);
    }
    let lp3 = conv_sep_f(&y, w, h, &[1.0, 3.0, 1.0]);
    let lp5 = conv_sep_f(&y, w, h, &[1.0, 4.0, 6.0, 4.0, 1.0]);
    let core = |d: f32| if d >= x { d - x } else if d < -x { d + x } else { 0.0 };
    y.par_iter_mut().zip(lp3.par_iter().zip(lp5.par_iter())).for_each(|(v, (&l3, &l5))| {
        let (h3, h5) = (*v - l3, *v - l5);
        *v = (*v + core(h3) * a + core(h5) * b).clamp(0.0, 255.0);
    });
    let sm = |p: &[f32]| -> Vec<f32> {
        let p = blur_bp_f(p, w, h);
        let k: Vec<f32> = match m {
            i32::MIN..=2 => vec![1.0, 3.0, 1.0],
            3 => binom_row(3).iter().map(|&v| v as f32).collect(),
            4 | 5 => binom_row(5).iter().map(|&v| v as f32).collect(),
            _ => binom_row(9).iter().map(|&v| v as f32).collect(),
        };
        conv_sep_f(&p, w, h, &k)
    };
    let (u, v) = (sm(&c1), sm(&c2));
    // Local texture weight for the blur floors only: they are zero on a flat patch (nothing to floor), while
    // the per-pixel algebraic truncations (the luma /3, the chroma /4 and /2, the (4u+3)/6 terms) happen on
    // flat patches too and stay unweighted. The weight saturates fast: any s0 in 0.5..1 gives the same.
    const TEXTURE_S0: f32 = 0.75;
    let tw: Vec<f32> = {
        let d: Vec<f32> = (0..n).map(|i| (c1[i] - u[i]).abs() + (c2[i] - v[i]).abs()).collect();
        blur_bp_f(&d, w, h).iter().map(|&t| (t / TEXTURE_S0).clamp(0.0, 1.0)).collect()
    };
    // Mean floor/truncation loss of the integer chain, only where it has texture to truncate
    // (flat patches lose nothing): luma (`/3`, optional 3x3 blur), chroma (3x3 blur + the
    // smoothing passes, two floors of `k_loss` each plus the 3x3 blur's own).
    let wt = ((nv / 0.1).min(1.0)) as f32;
    let k_loss = match m {
        i32::MIN..=2 => 0.4,
        3 => 3.0 / 8.0,
        4 | 5 => 15.0 / 32.0,
        _ => 255.0 / 512.0,
    };
    let (y_flat, y_blur, uv_blur) = if loss { (wt / 3.0, wt * if nv > 0.25 { 15.0 / 32.0 } else { 0.0 }, wt * (15.0 / 32.0 + 2.0 * k_loss)) } else { (0.0, 0.0, 0.0) };
    let [pr, pg, pb] = &mut f.p;
    pr.par_iter_mut().zip(pg.par_iter_mut()).zip(pb.par_iter_mut()).enumerate().for_each(|(i, ((r, g), bl))| {
        let (yy, uu, vv) = (y[i] - y_flat - y_blur * tw[i], u[i] - uv_blur * tw[i], v[i] - uv_blur * tw[i]);
        // `(4u+3)/6`, `(8u+3)/6` truncate toward zero: +1 for u < 0 (not weighted by wt: unweighted fits flat patches best, suite HQ 1.07/1.21 -> 0.94/0.97)
        let sn = if loss { (-uu).clamp(0.0, 1.0) } else { 0.0 };
        let t = uu * (4.0 / 6.0) + sn;
        *r = (yy - t - vv).clamp(0.0, 255.0);
        *g = (yy + uu * (8.0 / 6.0) + sn).clamp(0.0, 255.0);
        *bl = (yy - t + vv).clamp(0.0, 255.0);
    });
}

fn black_print_hq(f: &mut FPlanes, m: f64) {
    let (w, h) = (f.w, f.h);
    if w < 2 || h < 2 {
        return;
    }
    let lt = ((m * BP_LUMA + 0.5) as i32).clamp(1, BP_LUMA_MAX as i32) as f32;
    let ct = ((m * BP_CHROMA + 0.5) as i32).clamp(1, BP_CHROMA_MAX as i32) as f32;
    let blur: [Vec<f32>; 3] = std::array::from_fn(|c| blur_bp_f(&f.p[c], w, h));
    let raw = f.p.clone();
    let [pr, pg, pb] = &mut f.p;
    pr.par_chunks_mut(w).zip(pg.par_chunks_mut(w)).zip(pb.par_chunks_mut(w)).enumerate().for_each(|(y, ((r, g), b))| {
        for x in 0..w {
            let i = y * w + x;
            let l = (blur[0][i] + blur[1][i] + blur[2][i]) / 3.0;
            let wgt = ((lt - l) / (lt * BP_RATIO as f32)).clamp(0.0, 1.0);
            if wgt == 0.0 {
                continue;
            }
            let e: f32 = (0..3).map(|c| (l - raw[c][i]).abs()).sum();
            let q = ((ct - e) / (ct * BP_RATIO as f32)).clamp(0.0, 1.0);
            let i13 = (1.0 - q) * wgt;
            let s2 = q * l * wgt;
            for (c, d) in [&mut r[x], &mut g[x], &mut b[x]].into_iter().enumerate() {
                *d = (raw[c][i] * (1.0 - wgt) + s2 + blur[c][i] * i13).clamp(0.0, 255.0);
            }
        }
    });
}

/// Options of the HQ chain.
#[derive(Debug, Clone, Copy)]
pub struct HqOptions {
    /// Model the truncation loss of the DLL's integer chain (default). The float chain is *unbiased*; the
    /// DLL's is not (floors and truncations in the Noise Filter, Gamma's chroma term). The terms are derived
    /// from the C code (signed truncation toward zero of `/4`, `/2`, `(4u+3)/6`, `(8u+3)/6`, the blur floors,
    /// Gamma's chroma term) and applied at full strength; no fitted constants. Off gives the pure float chain.
    pub loss_model: bool,
}

impl Default for HqOptions {
    fn default() -> Self {
        HqOptions { loss_model: true }
    }
}

/// The HQ chain on float planes (0..255 scale): Gain and Offset, Gamma, Noise Filter, Black Print,
/// with the decisions in `p` ([`hq_params`]).
pub fn hq_apply(f: &mut FPlanes, p: &HqParams, opts: HqOptions) {
    go_hq(f, &p.gain);
    gamma_hq(f, p.gain.p, p.gamma, opts.loss_model);
    noise_filter_hq(f, p.nv, opts.loss_model);
    black_print_hq(f, p.bp_mean);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth gradient with per-channel ranges, `lo..hi` in `level << 4` units.
    fn ramp(w: usize, h: usize, lo: [u16; 3], hi: [u16; 3]) -> Planes {
        let mut p = Planes::new(w, h);
        for c in 0..3 {
            for y in 0..h {
                for x in 0..w {
                    let t = (x + y * 3) as f64 / (w + 3 * h) as f64;
                    p.p[c][y * w + x] = ((lo[c] as f64 + t * (hi[c] - lo[c]) as f64) as u16) & !15; // 8-bit levels, `<< 4`
                }
            }
        }
        p
    }

    #[test]
    fn blur_keeps_constants() {
        let v = vec![7i32; 40 * 30];
        assert!(blur_floor(&v, 40, 30, &[1, 4, 6, 4, 1], 4).iter().all(|&s| s == 7));
        assert!(blur_floor(&v, 40, 30, &[1, 2, 1], 2).iter().all(|&s| s == 7));
    }

    #[test]
    fn gain_and_offset_stretches_each_channel_to_full_range() {
        let mut im = ramp(400, 300, [400, 380, 360], [3500, 3600, 3700]);
        let gi = gain_and_offset(&mut im);
        for c in 0..3 {
            let (mn, mx) = (*im.p[c].iter().min().unwrap(), *im.p[c].iter().max().unwrap());
            assert!(mn < 40, "ch {c} min {mn}");
            assert!(mx > 3900, "ch {c} max {mx}");
        }
        assert!(gi.p > 1.0 && gi.p < 1.4, "p {}", gi.p);
        assert!(gi.hi[0] > gi.lo[0]);
    }

    #[test]
    fn short_histogram_gets_the_knee_and_stays_ordered() {
        // 8-bit range 100..140: stage 1 stretches by ~2x (capped), the luma
        // histogram is then wider than 63.75 bins so the knee follows.
        let mut im = ramp(400, 300, [1600, 1600, 1600], [2240, 2240, 2240]);
        let gi = gain_and_offset(&mut im);
        assert_eq!((gi.lo, gi.hi), ([100; 3], [138; 3]));
        assert!(gi.k1 > 1.0, "knee expected: {gi:?}");
        let row = &im.p[0][..400];
        assert!(row.windows(2).all(|w| w[0] <= w[1]));
        assert!(im.p[0].iter().all(|&v| v <= 4095));
    }

    #[test]
    fn gamma_one_is_identity_for_neutral_and_chroma() {
        let mut im = ramp(64, 48, [100, 500, 900], [3000, 3200, 3400]);
        let before = im.clone();
        let g = gamma_correction(&mut im, 1.0, Gamma::Fixed(1.0));
        assert_eq!(g, 1.0);
        for c in 0..3 {
            for (a, b) in im.p[c].iter().zip(&before.p[c]) {
                assert!((*a as i32 - *b as i32).abs() <= 1);
            }
        }
    }

    #[test]
    fn gamma_below_one_brightens_midtones() {
        let mut im = ramp(64, 48, [0, 0, 0], [4095, 4095, 4095]);
        let mid = im.p[1][24 * 64 + 32];
        gamma_correction(&mut im, 1.0, Gamma::Fixed(0.8));
        assert!(im.p[1][24 * 64 + 32] > mid + 100);
    }

    #[test]
    fn estimator_is_one_for_a_narrow_histogram_and_clamped_otherwise() {
        let narrow = ramp(200, 150, [1000; 3], [1400; 3]);
        assert_eq!(estimate_gamma(&narrow).gamma, 1.0);
        let dark = ramp(200, 150, [0; 3], [1500; 3]);
        let g = estimate_gamma(&dark).gamma;
        assert!((G_LO..=G_HI).contains(&g), "{g}");
        assert!(g < 1.0, "a dark image is brightened: {g}");
    }
}
