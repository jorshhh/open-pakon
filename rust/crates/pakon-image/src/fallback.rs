//! The colour path without an EEPROM archive (port of `invert_c41` and
//! `render_jpeg` in `tools/pakon_image.py`): no NegMatrix, so the frame is
//! divided by the film base, run through the ColNeg density LUT and sRGB
//! encoded; the JPEG gets the "vibrant" look through rpd.pf.

use ndarray::{Array3, ArrayView3, Axis, Zip};

use crate::colour::Table;
use crate::stats;

/// sRGB transfer function on [0, 1], numpy's float32 steps.
fn srgb_encode(x: f32) -> f32 {
    if x <= 0.0031308f32 {
        x * 12.92f32
    } else {
        1.055f32 * x.clamp(0.0, 1.0).powf((1.0f64 / 2.4) as f32) - 0.055f32
    }
}

/// The OEM C-41 ColNeg inversion of one frame (`invert_c41` with its
/// defaults: no crosstalk matrix, no grey world, sRGB encoded). `base` is
/// the per-channel Dmin (`balance::dmin`). 16-bit positive.
pub fn invert_c41(rgb: ArrayView3<u16>, base: [f32; 3], lut: &[f32]) -> Array3<u16> {
    let mut out = Array3::<u16>::zeros(rgb.dim());
    Zip::from(out.lanes_mut(Axis(2))).and(rgb.lanes(Axis(2))).par_for_each(|mut o, p| {
        for c in 0..3 {
            let in14 = (p[c] as f32 / base[c] * 16383.0f32).clamp(0.0, 16383.0);
            let pos = lut[in14 as u16 as usize];
            let pos = srgb_encode(pos / 16383.0f32) * 16383.0f32;
            o[c] = (pos * 4.0f32).clamp(0.0, 65535.0) as u16;
        }
    });
    out
}

/// The "vibrant JPEG" look from a 16-bit sRGB positive (`render_jpeg`):
/// the 8-bit picture through rpd.pf (`rpd`, the 52³ table; Python uses
/// LittleCMS's 8-bit transform, so values can differ by a code), then per
/// channel auto-levels between the `lo_p` / `hi_p` percentiles, a midtone
/// lift (`1 / gamma`) and a soft shoulder above `knee` instead of clipping.
pub fn render_jpeg(rgb: ArrayView3<u16>, rpd: &Table, gamma: f32, lo_p: f64, hi_p: f64, knee: f32) -> Array3<u8> {
    let (h, w, _) = rgb.dim();
    let mut a = Array3::<f32>::zeros((h, w, 3));
    Zip::from(a.lanes_mut(Axis(2))).and(rgb.lanes(Axis(2))).par_for_each(|mut o, p| {
        let x = [0, 1, 2].map(|c| (p[c] >> 8) as f32 / 255.0);
        let s = rpd.lookup(x);
        for c in 0..3 {
            // the transform's 8-bit output
            o[c] = (s[c] * 255.0 + 0.5).clamp(0.0, 255.0).floor() / 255.0f32;
        }
    });
    let mut out = Array3::<u8>::zeros((h, w, 3));
    for c in 0..3 {
        let ch = a.index_axis(Axis(2), c);
        let mut v: Vec<f32> = ch.iter().copied().collect();
        let lo = stats::percentile(&mut v, lo_p);
        let hi = stats::percentile(&mut v, hi_p);
        let span = (hi - lo).max(1e-3);
        Zip::from(out.index_axis_mut(Axis(2), c)).and(ch).par_for_each(|o, &x| {
            let mut x = ((x - lo) / span).max(0.0);
            x = if x <= 0.0 { 0.0 } else { x.powf(1.0 / gamma) };
            if x > knee {
                x = knee + (1.0 - knee) * (1.0 - (-(x - knee) / (1.0 - knee)).exp());
            }
            *o = (x * 255.0).clamp(0.0, 255.0) as u8;
        });
    }
    out
}

/// `render_jpeg`'s defaults: gamma 1.5, levels 0.5 / 99.7 %, knee 0.85.
pub fn render_jpeg_default(rgb: ArrayView3<u16>, rpd: &Table) -> Array3<u8> {
    render_jpeg(rgb, rpd, 1.5, 0.5, 99.7, 0.85)
}
