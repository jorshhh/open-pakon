//! Line statistics and calibration steps for light calibration (port of
//! `include/pakon_lightcal.h`, `src/pakon_lightcal.c`). Pure.
//!
//! During calibration (motor stopped, bank 0x82 reg 4 = 6, reg 5 = Offset +
//! 2000) a line is `line_px` R,G,B triplets and, with IR on, `line_px` IR
//! samples. Where a read starts within a line varies, so each read is aligned
//! from its own data: by the line marker (bit 0 set on the first word of each
//! line) or, IR on, by the black-to-lit step at the start of each block.
//! See the C header for the measurements behind each rule.

use std::fmt;
use std::str::FromStr;

/// Channel index into the 4-channel arrays (R, G, B, IR).
pub const R: usize = 0;
pub const G: usize = 1;
pub const B: usize = 2;
pub const IR: usize = 3;
pub const NCH: usize = 4;

/// Blocks per line: RGB triplets (3) plus the IR block when IR is on.
pub fn blocks(ir: bool) -> usize {
    if ir { 4 } else { 3 }
}

/// Fully black pixels at the start of a block (Base 16; scaled with the line
/// at Base 8 / 4).
pub const BLACK_PX: usize = 20;

pub const GAIN_START: i32 = 13;
pub const GAIN_MAX: i32 = 0x3E;
pub const OFFSET_START: i32 = 10;
pub const DARK_TARGET: f64 = 300.0;
pub const DARK_TOL: f64 = 32.0;
pub const DARK_ITERS: u32 = 8;
pub const REFINE_ITERS: u32 = 32;

/// Line origin (start of an RGB block, `0 <= origin < 4 * line_px`): the
/// offset maximising, over the mean of up to 64 lines, the contrast between
/// each block's first `black_px` pixels and the rest of that block (RGB block
/// plus IR block). `None` with fewer than 2 lines or contrast under
/// `min_contrast` (no light to align on).
pub fn find_origin(s: &[u16], line_px: usize, black_px: usize, min_contrast: f64) -> Option<usize> {
    let (l_len, b) = (4 * line_px, black_px);
    let lines = s.len().checked_div(l_len).unwrap_or(0);
    if b == 0 || line_px < 6 * b || lines < 2 {
        return None;
    }
    let lines = lines.min(64);
    // Mean line, then prefix sums over three copies so windows can wrap.
    let mut c = vec![0f64; 3 * l_len + 1];
    for l in 0..lines {
        for i in 0..l_len {
            c[1 + i] += s[l * l_len + i] as f64;
        }
    }
    for v in &mut c[1..=l_len] {
        *v /= lines as f64;
    }
    for k in 1..3 {
        for i in 0..l_len {
            c[1 + k * l_len + i] = c[1 + i];
        }
    }
    for i in 1..=3 * l_len {
        c[i] += c[i - 1];
    }
    let win = |o: usize, a: usize, b: usize| (c[o + b] - c[o + a]) / (b - a) as f64;
    let ir = 3 * line_px;
    let (mut best, mut arg) = (-1e18, None);
    for o in 0..l_len {
        // Whole-block active windows: a lit IR block alone cannot pass for
        // an RGB block, which is three times longer.
        let sc = win(o, 6 * b, ir - 30) - win(o, 0, 3 * b) + win(o, ir + 3 * b, l_len - 10) - win(o, ir, ir + b);
        if sc > best {
            best = sc;
            arg = Some(o);
        }
    }
    if best >= min_contrast { arg } else { None }
}

/// Line origin from the line marker: the phase, `0 <= origin < line_len`,
/// where bit 0 is set on at least 60% of the whole lines. Works lit or dark.
pub fn marker_origin(s: &[u16], line_len: usize) -> Option<usize> {
    let lines = s.len().checked_div(line_len).unwrap_or(0);
    if lines < 2 {
        return None;
    }
    let mut hits = vec![0usize; line_len];
    for (i, v) in s[..lines * line_len].iter().enumerate() {
        if v & 1 != 0 {
            hits[i % line_len] += 1;
        }
    }
    let mut arg = 0;
    for i in 1..line_len {
        if hits[i] > hits[arg] {
            arg = i;
        }
    }
    (hits[arg] * 10 >= lines * 6).then_some(arg)
}

/// Line origin of a calibration read, IR on or off: the line marker when the
/// read has one, else (IR on only) [`find_origin`]. The marker is exact; the
/// contrast search lands early on a unit whose IR block has fewer than
/// `black_px` black pixels (serial 1101).
pub fn origin(s: &[u16], line_px: usize, black_px: usize, ir: bool, min_contrast: f64) -> Option<usize> {
    marker_origin(s, blocks(ir) * line_px).or_else(|| {
        if ir { find_origin(s, line_px, black_px, min_contrast) } else { None }
    })
}

/// Line length in samples from the line marker: the most common distance
/// between consecutive marked words (seen at least twice). Works lit or dark.
pub fn marker_line_len(s: &[u16]) -> Option<usize> {
    // (distance, count), first-seen order, at most 64 distinct distances
    let mut seen: Vec<(usize, u32)> = Vec::new();
    let mut prev = None;
    for (i, v) in s.iter().enumerate() {
        if v & 1 == 0 {
            continue;
        }
        if let Some(p) = prev {
            let d = i - p;
            match seen.iter().position(|(l, _)| *l == d) {
                Some(k) => seen[k].1 += 1,
                None if seen.len() < 64 => seen.push((d, 1)),
                None => {}
            }
        }
        prev = Some(i);
    }
    let mut best: Option<(usize, u32)> = None;
    for &(d, n) in &seen {
        if n >= 2 && best.is_none_or(|(_, bn)| n > bn) {
            best = Some((d, n));
        }
    }
    best.map(|(d, _)| d)
}

/// Line width (pixels) of a stream of identical lines, as on the open gate
/// with the motor stopped: the width in `[lo, hi]` whose mean over up to 48
/// lines has the largest variance (at a wrong width the mean blurs).
/// `None` if fewer than 4 lines of `hi` fit.
pub fn find_line_px(s: &[u16], lo: usize, hi: usize, ir: bool) -> Option<usize> {
    let nb = blocks(ir);
    if lo == 0 || hi < lo {
        return None;
    }
    let lines = s.len() / (nb * hi);
    if lines < 4 {
        return None;
    }
    let lines = lines.min(48);
    let mut m = vec![0f64; nb * hi];
    let (mut best, mut arg) = (-1.0, 0);
    for px in lo..=hi {
        let l_len = nb * px;
        let m = &mut m[..l_len];
        m.fill(0.0);
        for l in 0..lines {
            for (i, v) in m.iter_mut().enumerate() {
                *v += s[l * l_len + i] as f64;
            }
        }
        let (mut sum, mut sq) = (0.0, 0.0);
        for &v in m.iter() {
            sum += v;
            sq += v * v;
        }
        let n = l_len as f64;
        let var = sq / n - (sum / n) * (sum / n);
        if var > best {
            best = var;
            arg = px;
        }
    }
    Some(arg)
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// Whole lines used.
    pub lines: usize,
    /// Mean over lines and pixels, per channel.
    pub mean: [f64; NCH],
    /// Max over pixels of the per-pixel mean.
    pub peak: [u16; NCH],
}

/// Statistics over pixels `[px0, px1)` of every whole line from `origin`
/// (RGB block, then the IR block when `ir`; with IR off the IR mean and peak
/// are 0). `None` if the range is empty or no whole line fits.
pub fn stats_range(s: &[u16], line_px: usize, ir: bool, origin: usize, px0: usize, px1: usize) -> Option<Stats> {
    if px1 <= px0 || px1 > line_px {
        return None;
    }
    let line = blocks(ir) * line_px;
    let lines = s.len().saturating_sub(origin) / line;
    if lines == 0 {
        return None;
    }
    let npx = px1 - px0;
    let mut col = vec![0f64; NCH * npx];
    for l in 0..lines {
        let rgb = &s[origin + l * line..];
        for p in px0..px1 {
            let k = p - px0;
            col[k] += rgb[3 * p] as f64;
            col[npx + k] += rgb[3 * p + 1] as f64;
            col[2 * npx + k] += rgb[3 * p + 2] as f64;
            if ir {
                col[3 * npx + k] += rgb[3 * line_px + p] as f64;
            }
        }
    }
    let mut out = Stats { lines, ..Default::default() };
    for c in 0..NCH {
        let (mut sum, mut best) = (0.0, 0.0);
        for &v in &col[c * npx..(c + 1) * npx] {
            let m = v / lines as f64;
            sum += m;
            if m > best {
                best = m;
            }
        }
        out.mean[c] = sum / npx as f64;
        out.peak[c] = (best + 0.5) as u16;
    }
    Some(out)
}

/// Per-column sums for the fixed-pattern tables (TLB.dll FUN_1001f550): adds
/// every pixel of every whole line from `origin` to `sum`, laid out
/// `[channel][pixel]` (`NCH * line_px`; the IR row is left alone when IR is
/// off), and the line count to `lines`. Returns the lines added.
pub fn column_accum(s: &[u16], line_px: usize, ir: bool, origin: usize, sum: &mut [f64], lines: &mut usize) -> usize {
    if line_px == 0 {
        return 0;
    }
    let line = blocks(ir) * line_px;
    let n = s.len().saturating_sub(origin) / line;
    for l in 0..n {
        let rgb = &s[origin + l * line..];
        for p in 0..line_px {
            sum[p] += rgb[3 * p] as f64;
            sum[line_px + p] += rgb[3 * p + 1] as f64;
            sum[2 * line_px + p] += rgb[3 * p + 2] as f64;
            if ir {
                sum[3 * line_px + p] += rgb[3 * line_px + p] as f64;
            }
        }
    }
    *lines += n;
    n
}

/// Flat-field gain for one column:
/// `64000 / ((bright - dark) - (black_bright - black_dark))`, capped at 0x3ffff / 65536 like the OEM's 16.16 table; 0
/// when the denominator is not positive.
pub fn flat_gain(bright: f64, dark: f64, black_bright: f64, black_dark: f64) -> f64 {
    let den = (bright - dark) - (black_bright - black_dark);
    if den <= 0.0 {
        return 0.0;
    }
    (64000.0 / den).min(0x3ffff as f64 / 65536.0)
}

/// Smear coefficient (16.16): `65536 * (black_bright - black_dark) /
/// (active_bright - active_dark)`, kept only within 1..699, else 0.
pub fn smear(black_bright: f64, black_dark: f64, active_bright: f64, active_dark: f64) -> u32 {
    let den = active_bright - active_dark;
    if den <= 0.0 {
        return 0;
    }
    let v = 65536.0 * (black_bright - black_dark) / den;
    if (1.0..=699.0).contains(&v) { v as u32 } else { 0 }
}

/// Film types the scan supports. The discriminant is stored in the
/// calibration cache: new types go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Film {
    #[default]
    C41 = 0,
    Bw = 1,
    Slide = 2,
    BwC41 = 3,
}

/// Colour-negative film-base densities (R, G, B, IR, TLB.dll FUN_10020230).
pub const DENSITY_C41: [f64; NCH] = [0.144, 0.40, 0.715, 0.0];
/// The OEM's "other" row, used for silver B&W.
pub const DENSITY_OTHER: [f64; NCH] = [0.0, 0.03, 0.0, 0.08];
/// Slide: departs from the OEM (see `pakon_lightcal.h`).
pub const DENSITY_SLIDE: [f64; NCH] = [0.12, 0.12, 0.12, 0.08];
/// Chromogenic B&W (TLB.dll film type 8).
pub const DENSITY_BWC41: [f64; NCH] = [0.10, 0.25, 0.25, 0.08];

impl Film {
    pub const ALL: [Film; 4] = [Film::C41, Film::Bw, Film::Slide, Film::BwC41];

    pub fn density(self) -> &'static [f64; NCH] {
        match self {
            Film::C41 => &DENSITY_C41,
            Film::Bw => &DENSITY_OTHER,
            Film::Slide => &DENSITY_SLIDE,
            Film::BwC41 => &DENSITY_BWC41,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Film::C41 => "c41",
            Film::Bw => "bw",
            Film::Slide => "slide",
            Film::BwC41 => "bwc41",
        }
    }

    pub fn from_u32(v: u32) -> Option<Film> {
        Film::ALL.get(v as usize).copied()
    }
}

impl fmt::Display for Film {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Film {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Film::ALL.into_iter().find(|f| f.name() == s).ok_or_else(|| format!("unknown film {s:?}"))
    }
}

/// C `lround` (half away from zero) to a 16-bit value, wrapping as the C
/// casts do.
fn round_u16(v: f64) -> u16 {
    v.round() as i64 as u16
}

/// Within 300 ± 32.
pub fn dark_ok(black_mean: f64) -> bool {
    (black_mean - DARK_TARGET).abs() <= DARK_TOL
}

/// Next A/D offset from a black-pixel mean: `off + round((mean - 300) *
/// -0.0133929)`; when that rounds to no change while still outside the
/// window, one code toward the target. Clamped to ±255.
pub fn offset_next(offset: i32, black_mean: f64) -> i32 {
    let mut next = offset + ((black_mean - DARK_TARGET) * -0.0133929).round() as i32;
    if next == offset && !dark_ok(black_mean) {
        next += if black_mean > DARK_TARGET { -1 } else { 1 };
    }
    next.clamp(-255, 255)
}

/// Current-search stop level: column peak above R/G 64000, B 65500, IR 40000.
pub fn peak_over_cap(ch: usize, peak: u32) -> bool {
    peak > [64000, 64000, 65500, 40000][ch]
}

/// Duty-refine window: R/G/B [63936, 64000], IR [39936, 40000].
pub fn peak_ok(ch: usize, peak: u32) -> bool {
    let hi = if ch == IR { 40000 } else { 64000 };
    (hi - 64..=hi).contains(&peak)
}

/// Calibration duty cap: `(period - 2) / 10^D`.
pub fn duty_cap(period: u16, density: f64) -> u16 {
    round_u16((period as i32 - 2) as f64 / 10f64.powf(density))
}

/// Duty to start refining from after the current search ends at `current`:
/// `cap * (n - 1) / n` (cap / 2 when n is 1).
pub fn duty_start(cap: u16, current: u32) -> u16 {
    if current <= 1 {
        return cap / 2;
    }
    round_u16(cap as f64 * (current - 1) as f64 / current as f64)
}

/// Refine step: `duty * 63968 / peak` (IR 39968), clamped to `[1, period - 2]`.
pub fn duty_next(ch: usize, duty: u16, peak: u32, period: u16) -> u16 {
    let target = if ch == IR { 39968.0 } else { 63968.0 };
    let max = period as i64 - 2;
    let mut next = if peak != 0 { (duty as f64 * target / peak as f64).round() as i64 } else { max };
    if next < 1 {
        next = 1;
    }
    if next > max {
        next = max;
    }
    next as u16
}

/// Settled: peak in the refine window, or the refine step would move the
/// duty by at most one tick (at small duties one tick is wider than the
/// window).
pub fn duty_settled(ch: usize, duty: u16, peak: u32, period: u16) -> bool {
    if peak_ok(ch, peak) {
        return true;
    }
    let next = duty_next(ch, duty, peak, period) as i32;
    (next - duty as i32).abs() <= 1 && next < period as i32 - 2
}

/// Scan duty for film: open-gate duty * 10^D, clamped to period - 2.
pub fn scan_duty(open_duty: u16, density: f64, period: u16) -> u16 {
    let d = (open_duty as f64 * 10f64.powf(density)).round() as i64;
    d.min(period as i64 - 2) as u16
}
