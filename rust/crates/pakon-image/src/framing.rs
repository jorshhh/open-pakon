//! OEM (TLX) picture framing (TLB.dll FN_iFramePicturesSub and friends; port
//! of `tlx_geometry`, `tlx_profile`, `_tlx_threshold`, `tlx_place`,
//! `tlx_frame_grid` in `tools/pakon_image.py`, docs/TLB_FINDINGS.md
//! "Picture framing").

use std::collections::BTreeMap;

use ndarray::{ArrayView3, Axis};

use crate::stats;

/// Per sensor width: framing block (lines), picture length, pitch.
const GEOMETRY: [(usize, (usize, usize, usize)); 3] = [(2000, (8, 3000, 3206)), (1500, (6, 2250, 2405)), (1000, (4, 1500, 1603))];

/// Picture formats: (length, pitch) as fractions of the full-frame values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Full,
    Half,
    Pano,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Full, Format::Half, Format::Pano];

    fn fractions(self) -> (f64, f64) {
        match self {
            Format::Full => (1.0, 1.0),
            Format::Half => (0.5, 0.5),
            Format::Pano => (5530.0 / 3000.0, 5736.0 / 3206.0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Full => "full",
            Format::Half => "half",
            Format::Pano => "pano",
        }
    }
}

/// (block, length, pitch) in ribbon rows; a width with no OEM entry takes
/// the nearest resolution's (the first on a tie, as Python's `min`).
pub fn geometry(width: usize, fmt: Format) -> (usize, usize, usize) {
    let (_, (block, length, pitch)) = *GEOMETRY.iter().min_by_key(|(w, _)| w.abs_diff(width)).unwrap();
    let (kl, kp) = fmt.fractions();
    (block, (length as f64 * kl).round_ties_even() as usize, (pitch as f64 * kp).round_ties_even() as usize)
}

/// FUN_10006870: one 8-bit (R+G+B)/3 value per block of lines (the first
/// line of each block), inverted for negatives.
pub fn profile(ribbon: ArrayView3<u16>, block: usize, positive: bool) -> Vec<i32> {
    let n = ribbon.len_of(Axis(0)) / block;
    (0..n)
        .map(|i| {
            let row = ribbon.index_axis(Axis(0), i * block);
            let v: Vec<f32> = row.iter().map(|&x| x as f32).collect();
            let m = stats::np_sum(&v) / v.len() as f32 / 257.0f32;
            let m = m.clamp(0.0, 255.0) as i32;
            if positive { m } else { 255 - m }
        })
        .collect()
}

/// FUN_10005d20 mode 0: the first significant histogram peak (the gap
/// level) and a cut just above it.
fn threshold(prof: &[i32]) -> i32 {
    let mut h = [0i64; 256];
    for &v in prof {
        h[v as usize] += 1;
    }
    let floor = (prof.len() / 150) as i64;
    let (mut peak, mut pk) = (0usize, 0i64);
    for (i, &hv) in h.iter().enumerate().take(0xfa + 2) {
        if pk > floor && hv < pk * 3 / 5 {
            break;
        }
        if hv > pk {
            pk = hv;
            peak = i;
        }
    }
    let mut lo = peak;
    while lo > 0 && h[lo] >= pk / 20 {
        lo -= 1;
    }
    let (mut hi, top) = (peak, (peak + 50).min(255));
    while hi < top && h[hi] >= pk / 10 {
        hi += 1;
    }
    if (peak - lo) * 4 < hi - peak {
        let mut t = peak;
        while t < 255 && h[t] >= pk / 2 {
            t += 1;
        }
        return t as i32;
    }
    (hi + (hi - peak) / 2) as i32
}

/// Runs of ones: (start, length).
fn runs(ones: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ones.len() {
        if ones[i] {
            let s = i;
            while i < ones.len() && ones[i] {
                i += 1;
            }
            out.push((s, i - s));
        } else {
            i += 1;
        }
    }
    out
}

/// Python's floor division for possibly negative values.
fn fdiv(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// FN_iFramePicturesSub + FN_iFramingFindPictures on a block profile:
/// picture starts in blocks, sorted, each `plen` blocks long.
pub fn place(prof: &[i32], plen: usize, pitch: usize) -> Vec<i64> {
    let (plen, pitch) = (plen as i64, pitch as i64);
    let (lo, hi) = (plen * 95 / 100, plen * 115 / 100);
    let good = |t: i32| runs(&prof.iter().map(|&v| v > t).collect::<Vec<_>>()).iter().filter(|(_, l)| lo < *l as i64 && (*l as i64) < hi).count();

    // Threshold sweep: ±2 steps while the count of picture-length runs does
    // not drop; keep the best.
    let t0 = threshold(prof);
    let (mut best_n, mut best_t) = (good(t0), t0);
    for step in [2, -2] {
        let mut t = t0;
        while 1 <= t + step && t + step <= 0xff {
            t += step;
            let g = good(t);
            if g < best_n {
                break;
            }
            if g > best_n {
                best_n = g;
                best_t = t;
            }
        }
    }
    let ones: Vec<bool> = prof.iter().map(|&v| v > best_t).collect();
    let n = ones.len() as i64;
    // slot (half pitch) -> start; first placed wins (dict.setdefault)
    let mut pics: BTreeMap<i64, i64> = BTreeMap::new();
    let put = |p: i64, pics: &mut BTreeMap<i64, i64>| {
        pics.entry(fdiv(p * 2, pitch)).or_insert(p);
    };

    // LookForNicePictures: whole pictures, and two with a lost gap.
    for (s, l) in runs(&ones) {
        let (s, l) = (s as i64, l as i64);
        if lo < l && l < hi {
            put(s + fdiv(l - plen, 3), &mut pics);
        } else if lo + pitch < l && l < hi + pitch {
            put(s + fdiv(l - pitch - plen, 3), &mut pics);
            put(s + pitch, &mut pics);
        }
    }
    if pics.is_empty() {
        // FramingBlindlyPlacePictures
        let off = fdiv(pitch - plen, 2);
        return (0..fdiv(n - 4, pitch).max(0)).map(|k| off + k * pitch).collect();
    }

    // LookInBetweenEnds: evenly fill the pictures missing between two found.
    let mut found: Vec<i64> = pics.values().copied().collect();
    found.sort();
    for w in found.windows(2) {
        let (a, b) = (w[0], w[1]);
        let span = b - a;
        let mut k = fdiv(span, pitch);
        if span - k * pitch < fdiv(pitch, 4) {
            k -= 1;
        }
        for j in 1..=k {
            put(a + fdiv(j * span, k + 1), &mut pics);
        }
    }

    // LookAtEnd / LookAtBeginning: one pitch outward at a time, where a plen
    // window holds the most film; stop when there is none left.
    let gap = (pitch - plen).max(1);
    let mut cum = vec![0i64; ones.len() + 1];
    for (i, &o) in ones.iter().enumerate() {
        cum[i + 1] = cum[i] + o as i64;
    }
    let estimate = |start: i64| -> Option<i64> {
        let mut best: Option<((i64, i64), i64)> = None;
        for k in 0..gap {
            let p = start + k;
            if p < 0 || p + plen > n {
                continue;
            }
            let key = (cum[(p + plen) as usize] - cum[p as usize], -(k - fdiv(gap, 2)).abs());
            if best.is_none_or(|(bk, _)| key > bk) {
                best = Some((key, p));
            }
        }
        match best {
            Some((key, p)) if key.0 >= fdiv(plen, 2) => Some(p),
            _ => None,
        }
    };
    let mut last = *pics.values().max().unwrap();
    while let Some(q) = estimate(last + plen + fdiv(gap, 2)).filter(|&q| q > last) {
        put(q, &mut pics);
        last = q;
    }
    let mut first = *pics.values().min().unwrap();
    while let Some(q) = estimate(first - pitch - fdiv(gap, 2)).filter(|&q| q < first) {
        put(q, &mut pics);
        first = q;
    }
    let mut out: Vec<i64> = pics.values().copied().collect();
    out.sort();
    out.dedup();
    out
}

/// OEM framing: (starts, length, pitch) in ribbon rows; each picture is
/// rows `[start, start + length)`.
pub fn frame_grid(ribbon: ArrayView3<u16>, positive: bool, fmt: Format) -> (Vec<usize>, usize, usize) {
    let (block, length, pitch) = geometry(ribbon.len_of(Axis(1)), fmt);
    let starts = place(&profile(ribbon, block, positive), length / block, pitch / block);
    let rows = ribbon.len_of(Axis(0)) as i64;
    let mut s: Vec<usize> = starts.iter().map(|&s| (s * block as i64).max(0).min((rows - length as i64).max(0)) as usize).collect();
    s.sort();
    s.dedup();
    (s, length, pitch)
}
