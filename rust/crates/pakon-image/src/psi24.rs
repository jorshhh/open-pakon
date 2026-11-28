//! The PSI 2.4 C-41 render after the scene balance: 12-bit RPD (SCP LUT + shifts applied, see
//! `colour::rpd_field`) -> the Kodak CMM (`rpd.pf` combined with `srgb.pf`, sampled on a 65^3 grid) -> 8-bit
//! sRGB -> EnhancedBlackPrinting (Gain and Offset, Gamma, Noise Filter, Black Print; see `ebp`). The real
//! DLL does exactly this; without the EBP stages the renders have ~1.3x too much contrast and a shadow toe.

use std::sync::OnceLock;

use ndarray::Array3;
use rayon::prelude::*;

use crate::colour::Table;
use crate::ebp::{self, FPlanes, Gamma, HqOptions, Planes};

/// The real Kodak CMM, `SpCombineXforms(rpd.pf, srgb.pf)` sampled with `SpEvaluate` (output type 5,
/// u16 full range) on a 65^3 grid of 12-bit inputs: cmm_in (SCP LUT + shifts) -> sRGB, R-major
/// (`assets/make_psi24_tables.py`, Windows `psi24-cmm`). Used for the float planes of the HQ path.
pub const CMM65_U16: &[u8] = include_bytes!("../assets/cmm65.u16");

/// The same grid with `SpEvaluate` output type 1 (u8, what the DLL's render uses). Type 5 lifts the
/// shadows by +0.5..+0.9 u8 levels against it, so the exact 8-bit path and the 8-bit decisions of the
/// HQ path use this one.
pub const CMM65_U8: &[u8] = include_bytes!("../assets/cmm65_u8.u8");

pub struct Tables {
    /// type 5 (u16), float output
    pub cmm: Table,
    /// `cmm` in linear light, with the nodes the sRGB clip cut at 0 extended below 0 ([`linear_with_cliff`])
    pub cmm_lin: Table,
    /// per node and channel: 1 where the node is clipped to 0 or next to such a node, else 0 (a table so
    /// that the weight is interpolated and therefore continuous across cells)
    pub cliff_w: Table,
    /// type 1 (u8), the DLL's 8-bit output
    pub cmm8: Table,
    /// `cmm8` in linear light; nodes the u8 table rounds to 0 take the type-5 table's (signed) linear value,
    /// capped at 0, so a node that is only rounded down is not mistaken for a clipped one
    pub cmm8_lin: Table,
}

pub fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let cmm = Table::from_u16_le(65, CMM65_U16).expect("cmm65.u16");
        let cmm_lin = linear_with_cliff(&cmm);
        let cliff_w = cliff_weights(&cmm);
        let cmm8 = Table::from_u8(65, CMM65_U8).expect("cmm65_u8.u8");
        let cmm8_lin = Table {
            n: cmm8.n,
            data: cmm8.data.iter().zip(&cmm_lin.data).map(|(&v, &l)| if v > 0.0 { srgb_to_lin(v) } else { l.min(0.0) }).collect(),
        };
        Tables {
            cmm,
            cmm_lin,
            cliff_w,
            cmm8,
            cmm8_lin,
        }
    })
}

/// Node indicator of the 0-clip edge, per channel: 1 on every node clipped to 0 and on its 6 axis
/// neighbours (so the linear-light
/// interpolation covers the cells on each side of the edge), else 0. (26 cube neighbours fix the C1D red ramp
/// maximum, 4.6 -> 1.4, but cost 0.04 MAE on the real frames.)
fn cliff_weights(t: &Table) -> Table {
    let n = t.n;
    let idx = |r: usize, g: usize, b: usize, c: usize| 3 * ((r * n + g) * n + b) + c;
    let mut data = vec![0f32; t.data.len()];
    for r in 0..n {
        for g in 0..n {
            for b in 0..n {
                for c in 0..3 {
                    if t.data[idx(r, g, b, c)] > 0.0 {
                        continue;
                    }
                    data[idx(r, g, b, c)] = 1.0;
                    for ax in 0..3 {
                        for s in [-1i32, 1] {
                            let mut q = [r as i32, g as i32, b as i32];
                            q[ax] += s;
                            if q[ax] >= 0 && (q[ax] as usize) < n {
                                data[idx(q[0] as usize, q[1] as usize, q[2] as usize, c)] = 1.0;
                            }
                        }
                    }
                }
            }
        }
    }
    Table { n, data }
}

fn srgb_to_lin(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn lin_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// The sampled CMM clips sRGB at 0 where the colour leaves the gamut. In sRGB the clipped edge is a cliff
/// (the gamma curve has an infinite slope at 0), so tetrahedral interpolation across it errs by up to ~22
/// levels on a ramp. In linear light the same surface crosses 0 almost linearly: this is the table in
/// linear light where every node clipped to 0 that has two positive neighbours along an axis gets the
/// signed value that line continues to (`2*v1 - v2`, <= 0), so interpolation across the edge follows the
/// true slope. Interior nodes are untouched.
fn linear_with_cliff(t: &Table) -> Table {
    let n = t.n;
    let mut data: Vec<f32> = t.data.iter().map(|&v| srgb_to_lin(v)).collect();
    let idx = |r: usize, g: usize, b: usize, c: usize| 3 * ((r * n + g) * n + b) + c;
    for r in 0..n {
        for g in 0..n {
            for b in 0..n {
                for c in 0..3 {
                    if t.data[idx(r, g, b, c)] > 0.0 {
                        continue;
                    }
                    let (mut sum, mut cnt) = (0f32, 0);
                    for ax in 0..3 {
                        for s in [-1i32, 1] {
                            let p = [r as i32, g as i32, b as i32];
                            let at = |k: i32| {
                                let mut q = p;
                                q[ax] += s * k;
                                (q[ax] >= 0 && (q[ax] as usize) < n).then(|| idx(q[0] as usize, q[1] as usize, q[2] as usize, c))
                            };
                            if let (Some(i1), Some(i2)) = (at(1), at(2)) {
                                if t.data[i1] > 0.0 && t.data[i2] > 0.0 {
                                    sum += 2.0 * data[i1] - data[i2];
                                    cnt += 1;
                                }
                            }
                        }
                    }
                    if cnt > 0 {
                        data[idx(r, g, b, c)] = (sum / cnt as f32).min(0.0);
                    }
                }
            }
        }
    }
    Table { n, data }
}

/// Above this sRGB value (0..1) a channel is far from the 0-clip edge: skip the extra lookups.
const CLIFF_SKIP: f32 = 0.5;

/// Type-5 CMM lookup (sRGB 0..1) of a 12-bit RPD triple: tetrahedral in sRGB, blended towards a
/// linear-light interpolation ([`linear_with_cliff`]) around the 0-clip edge.
#[inline]
pub fn cmm_float(rpd: [f32; 3], cliff: bool) -> [f32; 3] {
    let t = tables();
    blend_cliff(&t.cmm, &t.cmm_lin, &t.cliff_w, rpd, cliff)
}

/// The same lookup on the DLL's 8-bit table (type 1), 0..1 (not yet rounded).
#[inline]
pub fn cmm8_float(rpd: [f32; 3], cliff: bool) -> [f32; 3] {
    let t = tables();
    blend_cliff(&t.cmm8, &t.cmm8_lin, &t.cliff_w, rpd, cliff)
}

#[inline]
fn blend_cliff(plain: &Table, lin: &Table, wt: &Table, rpd: [f32; 3], cliff: bool) -> [f32; 3] {
    let x01 = [rpd[0] / 4095.0, rpd[1] / 4095.0, rpd[2] / 4095.0];
    let mut s = plain.lookup_tetra(x01);
    if !cliff {
        return s;
    }
    // Blend to the linear-light interpolation by the interpolated clip-edge indicator: continuous over the
    // whole cube, so there is no jump across cell faces (a per-cell switch left up to 7.8 levels).
    if s.iter().any(|&v| v < CLIFF_SKIP) {
        let w = wt.lookup_tetra(x01);
        if w.iter().any(|&v| v > 0.0) {
            let l = lin.lookup_tetra(x01);
            for c in 0..3 {
                s[c] = (1.0 - w[c]) * s[c] + w[c] * lin_to_srgb(l[c].max(0.0));
            }
        }
    }
    s
}


/// Gamma exponent ceiling passed to the automatic estimator (`Gamma::Auto { cap }`) by the roll
/// renderer and the reference test. It is the DLL's own upper clamp (2.0, `ebp::G_HI`), so the cap
/// changes nothing: the estimator reproduces the DLL's gamma exactly on the bright frames, and the DLL
/// does output gamma above 1 for high-key frames (1.04 and 1.22 on roll A, 1.29 on roll B). It used to
/// be 1.0 (a leftover from an approximate estimator that overshot on subsampled input), which made
/// bright full-resolution frames up to 14 levels too bright. Estimating on a downscaled preview still
/// differs from the full frame (the edge statistic depends on the grain), so previews should reuse the
/// full-resolution decisions.
pub const GAMMA_CAP: f64 = 2.0;

/// `v` 0..1 -> the 8-bit level on the 12-bit plane scale (`level << 4`), rounded like the DLL's u8 output.
#[inline]
fn q8(v: f32) -> u16 {
    ((v * 255.0 + 0.5).clamp(0.0, 255.0) as u16) << 4
}

/// The 8-bit sRGB image entering EBP (planar, `level << 4`): RPD field (rows of interleaved f32, 0..4095)
/// through the sampled Kodak CMM, rounded to 8 bit. `psi24_stage`/analysis code compares this with the
/// real `s0` dumps.
pub fn pre_ebp(rpd: &[Vec<f32>]) -> Planes {
    pre_ebp_planes(rpd, false).0
}

/// [`pre_ebp`] plus, when `float` is set, the same image from the type-5 table without rounding
/// (0..255 scale), for the HQ path.
fn pre_ebp_planes(rpd: &[Vec<f32>], float: bool) -> (Planes, Option<FPlanes>) {
    let h = rpd.len();
    let w = if h > 0 { rpd[0].len() / 3 } else { 0 };
    let mut q = Planes::new(w, h);
    let mut f = float.then(|| FPlanes { w, h, p: std::array::from_fn(|_| vec![0f32; w * h]) });
    if w == 0 {
        return (q, f);
    }
    let [qr, qg, qb] = &mut q.p;
    let rows = qr.par_chunks_mut(w).zip(qg.par_chunks_mut(w)).zip(qb.par_chunks_mut(w));
    match f.as_mut() {
        None => rows.enumerate().for_each(|(y, ((qr, qg), qb))| {
            for x in 0..w {
                let s8 = cmm8_float([rpd[y][3 * x], rpd[y][3 * x + 1], rpd[y][3 * x + 2]], true);
                (qr[x], qg[x], qb[x]) = (q8(s8[0]), q8(s8[1]), q8(s8[2]));
            }
        }),
        Some(f) => {
            let [fr, fg, fb] = &mut f.p;
            rows.zip(fr.par_chunks_mut(w).zip(fg.par_chunks_mut(w)).zip(fb.par_chunks_mut(w))).enumerate().for_each(|(y, (((qr, qg), qb), ((fr, fg), fb)))| {
                for x in 0..w {
                    let s = cmm_float([rpd[y][3 * x], rpd[y][3 * x + 1], rpd[y][3 * x + 2]], true);
                    let s8 = cmm8_float([rpd[y][3 * x], rpd[y][3 * x + 1], rpd[y][3 * x + 2]], true);
                    (qr[x], qg[x], qb[x]) = (q8(s8[0]), q8(s8[1]), q8(s8[2]));
                    (fr[x], fg[x], fb[x]) = ((s[0] * 255.0).clamp(0.0, 255.0), (s[1] * 255.0).clamp(0.0, 255.0), (s[2] * 255.0).clamp(0.0, 255.0));
                }
            });
        }
    }
    (q, f)
}

/// RPD field (rows of interleaved f32, 0..4095) -> 8-bit-equivalent 16-bit sRGB (H, W, 3), like
/// `colour::render_oem_scp`'s output. Returns the Gamma exponent used.
pub fn render(rpd: &[Vec<f32>], gamma: Gamma) -> (Array3<u16>, f64) {
    let h = rpd.len();
    let w = if h > 0 { rpd[0].len() / 3 } else { 0 };
    let mut pl = pre_ebp(rpd);
    let used = if w > 0 { ebp::enhanced_black_printing(&mut pl, gamma).1 } else { 1.0 };
    let mut out = Array3::<u16>::zeros((h, w, 3));
    out.axis_iter_mut(ndarray::Axis(0)).into_par_iter().enumerate().for_each(|(y, mut orow)| {
        for x in 0..w {
            for c in 0..3 {
                orow[(x, c)] = (pl.p[c][y * w + x] >> 4).min(255) * 257;
            }
        }
    });
    (out, used)
}

/// High-bit-depth render: same pipeline and decisions as [`render`], but the 8-bit EBP stages run
/// continuously on float planes ([`ebp::hq_apply`]) and nothing is quantised between the CMM and the
/// output. Returns sRGB on a 0..255 float scale plus the Gamma exponent. All parameter decisions come
/// from the exact 8-bit chain (run on the rounded planes).
pub fn render_hq_f32(rpd: &[Vec<f32>], gamma: Gamma) -> (Vec<Vec<f32>>, usize, usize, f64) {
    render_hq_f32_with(rpd, gamma, HqOptions::default())
}

pub fn render_hq_f32_with(rpd: &[Vec<f32>], gamma: Gamma, opts: HqOptions) -> (Vec<Vec<f32>>, usize, usize, f64) {
    let h = rpd.len();
    let w = if h > 0 { rpd[0].len() / 3 } else { 0 };
    if w == 0 {
        return (vec![], 0, h, 1.0);
    }
    let (mut q, f) = pre_ebp_planes(rpd, true);
    let mut f = f.unwrap();
    let params = ebp::hq_params(&mut q, gamma);
    drop(q);
    ebp::hq_apply(&mut f, &params, opts);
    let rows: Vec<Vec<f32>> = (0..h)
        .map(|y| {
            let mut row = Vec::with_capacity(3 * w);
            for x in 0..w {
                for c in 0..3 {
                    row.push(f.p[c][y * w + x]);
                }
            }
            row
        })
        .collect();
    (rows, w, h, params.gamma)
}

/// sRGB level of RPD 1550 (the 3.0 contrast pivot, 18 % grey): CMM65 of [1550; 3] is 119.8 / 118.1 / 117.6.
pub const CONTRAST_PIVOT_SRGB: f32 = 118.0;

/// Contrast slider (TLX units, -1000..1000) on a display-referred sRGB level 0..255, after EBP.
/// The DLL's own `ImaContrastOpT` is `110.08 * (v/110.08)^(1 + c/1000)`, which has no white anchor (white
/// 255 drops to 167 at -500, everything above 194 clips at +500). This keeps 0, the pivot and 255 fixed
/// and bends each side with the same exponent `k = 1 + c/1000`: below the pivot `p (v/p)^k`, above it the
/// mirror image from 255. Slope at the pivot is `k` on both sides. It is applied after EBP because
/// Gain & Offset re-stretches the histogram and absorbs about three quarters of an RPD-domain contrast.
pub fn contrast_curve(v: f32, c: i32) -> f32 {
    let k = (1.0 + c as f32 / 1000.0).max(0.05);
    let p = CONTRAST_PIVOT_SRGB;
    let v = v.clamp(0.0, 255.0);
    if v <= p { p * (v / p).powf(k) } else { 255.0 - (255.0 - p) * ((255.0 - v) / (255.0 - p)).powf(k) }
}

/// [`contrast_curve`] on a 16-bit render (`round(f * 257)` values); unchanged when `c == 0`.
pub fn apply_contrast(img: &mut Array3<u16>, c: i32) {
    if c == 0 {
        return;
    }
    let lut: Vec<u16> = (0..=65535u32).map(|v| (contrast_curve(v as f32 / 257.0, c) * 257.0 + 0.5).clamp(0.0, 65535.0) as u16).collect();
    img.as_slice_mut().expect("contiguous").par_iter_mut().for_each(|v| *v = lut[*v as usize]);
}

/// The decisions of the exact 8-bit chain (G&O gain, Gamma, noise level, Black Print mean) for an RPD
/// field, to be applied to another rendering of the same picture with [`render_hq_given`]: previews take
/// them from the full-resolution frame, because the estimators (above all Gamma's edge statistic) depend
/// on the grain, which a shrunk copy no longer has.
pub fn hq_decisions(rpd: &[Vec<f32>], gamma: Gamma) -> ebp::HqParams {
    let mut q = pre_ebp_planes(rpd, false).0;
    ebp::hq_params(&mut q, gamma)
}

/// [`render_hq`] with the decisions given instead of estimated ([`hq_decisions`]).
pub fn render_hq_given(rpd: &[Vec<f32>], params: &ebp::HqParams) -> Array3<u16> {
    let h = rpd.len();
    let w = if h > 0 { rpd[0].len() / 3 } else { 0 };
    let mut out = Array3::<u16>::zeros((h, w, 3));
    if w == 0 {
        return out;
    }
    let mut f = pre_ebp_planes(rpd, true).1.unwrap();
    ebp::hq_apply(&mut f, params, HqOptions::default());
    out.axis_iter_mut(ndarray::Axis(0)).into_par_iter().enumerate().for_each(|(y, mut orow)| {
        for x in 0..w {
            for c in 0..3 {
                orow[(x, c)] = (f.p[c][y * w + x] * 257.0 + 0.5).clamp(0.0, 65535.0) as u16;
            }
        }
    });
    out
}

/// 16-bit output of [`render_hq_f32`] (`v * 257`, so 8-bit levels map to the same codes as [`render`]).
pub fn render_hq(rpd: &[Vec<f32>], gamma: Gamma) -> (Array3<u16>, f64) {
    let (rows, w, h, g) = render_hq_f32(rpd, gamma);
    let mut out = Array3::<u16>::zeros((h, w, 3));
    out.axis_iter_mut(ndarray::Axis(0)).into_par_iter().enumerate().for_each(|(y, mut orow)| {
        for x in 0..w {
            for c in 0..3 {
                orow[(x, c)] = (rows[y][3 * x + c] * 257.0 + 0.5).clamp(0.0, 65535.0) as u16;
            }
        }
    });
    (out, g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_curve_keeps_the_anchors() {
        for c in [-1000, -500, -250, 250, 500, 1000] {
            assert!(contrast_curve(0.0, c).abs() < 1e-4 && (contrast_curve(255.0, c) - 255.0).abs() < 1e-3, "{c}");
            assert!((contrast_curve(CONTRAST_PIVOT_SRGB, c) - CONTRAST_PIVOT_SRGB).abs() < 1e-3, "{c}");
            let mut last = -1.0;
            for v in 0..=255 {
                let o = contrast_curve(v as f32, c);
                assert!(o >= last, "monotonic {c} {v}");
                last = o;
            }
        }
        let (lo, hi) = (contrast_curve(40.0, 500), contrast_curve(200.0, 500));
        assert!(lo < 40.0 && hi > 200.0, "more contrast spreads");
        let (lo, hi) = (contrast_curve(40.0, -500), contrast_curve(200.0, -500));
        assert!(lo > 40.0 && hi < 200.0, "less contrast squeezes");
        let mut a = Array3::from_shape_fn((2, 2, 3), |(y, x, c)| (y * 20000 + x * 7000 + c * 100) as u16);
        let b = a.clone();
        apply_contrast(&mut a, 0);
        assert_eq!(a, b);
    }

    #[test]
    fn cmm_tables_load_and_agree_on_the_neutral_ramp() {
        let t = tables();
        for i in 0..=60 {
            let x = i as f32 / 60.0 * 3000.0 / 4095.0;
            let (a, b) = (t.cmm8.lookup([x; 3]), t.cmm.lookup([x; 3]));
            for c in 0..3 {
                // type 5 is the same transform at higher precision: within ~3 u8 levels (measured -0.9..+2.5) of the u8 output
                assert!((a[c] - b[c]).abs() * 255.0 < 3.0, "ch {c} u8 {} vs t5 {} at {x}", a[c], b[c]);
            }
        }
        assert!(t.cmm8.lookup([0.0; 3]).iter().all(|v| *v * 255.0 < 3.0));
    }

    #[test]
    fn hq_stays_close_to_the_8bit_path_and_has_no_banding() {
        // smooth gradient with some texture, wide enough to give the stages real work
        let (w, h) = (600usize, 300usize);
        let rpd: Vec<Vec<f32>> = (0..h)
            .map(|y| {
                (0..3 * w)
                    .map(|i| {
                        let (x, c) = (i / 3, i % 3);
                        let t = x as f32 / (w - 1) as f32;
                        let tex = (((x * 7 + y * 13 + c * 5) % 17) as f32 - 8.0) * 3.0;
                        (700.0 + (2900.0 - c as f32 * 150.0) * t + tex).clamp(0.0, 4095.0)
                    })
                    .collect()
            })
            .collect();
        let (a, ga) = render(&rpd, Gamma::Auto { cap: GAMMA_CAP });
        let (b, gb) = render_hq(&rpd, Gamma::Auto { cap: GAMMA_CAP });
        assert!((ga - gb).abs() < 1e-9);
        let (mut mae, mut bias) = (0f64, 0f64);
        for (p, q) in a.iter().zip(b.iter()) {
            let d = (*q as f64 - *p as f64) / 257.0;
            mae += d.abs();
            bias += d;
        }
        let n = a.len() as f64;
        assert!(mae / n < 1.6, "MAE {}", mae / n);
        assert!((bias / n).abs() < 1.2, "bias {}", bias / n);
        // a smooth row must have many more distinct 16-bit values than the 256 of the 8-bit path
        let mut d8: Vec<u16> = (0..w).map(|x| a[(150, x, 1)]).collect();
        let mut d16: Vec<u16> = (0..w).map(|x| b[(150, x, 1)]).collect();
        d8.sort();
        d8.dedup();
        d16.sort();
        d16.dedup();
        assert!(d16.len() > d8.len() + d8.len() / 2, "hq {} vs 8-bit {}", d16.len(), d8.len());
    }

    #[test]
    fn render_of_a_flat_field_is_flat_and_sized() {
        let rpd: Vec<Vec<f32>> = (0..30).map(|_| vec![1500.0f32; 3 * 40]).collect();
        let (o, g) = render(&rpd, Gamma::Fixed(1.0));
        assert_eq!(o.dim(), (30, 40, 3));
        assert!(g == 1.0);
        for c in 0..3 {
            assert!(o.indexed_iter().all(|((_, _, k), &v)| k != c || v == o[(0, 0, c)]), "channel {c} not flat");
        }
    }

    /// Deterministic synthetic RPD fields (fixed xorshift) that between them reach every branch of the stages:
    /// a narrow dark noisy field (knee, Gamma g2, noisy Noise Filter, Black Print), a bright high-contrast one
    /// with sharp edges (Gamma g1 edge statistic) and a smooth full-range one (stage 1 only, flat Noise Filter).
    fn synth(kind: usize) -> Vec<Vec<f32>> {
        let (w, h) = (128usize, 96usize);
        let mut s = 0x9E3779B9u32 ^ (kind as u32).wrapping_mul(0x85EBCA6B);
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 8) as f32 / (1u32 << 24) as f32
        };
        (0..h)
            .map(|y| {
                (0..3 * w)
                    .map(|i| {
                        let (x, c) = (i / 3, i % 3);
                        let v = match kind {
                            0 => 500.0 + 700.0 * (x as f32 / w as f32) + (rnd() - 0.5) * 160.0 + c as f32 * 40.0,
                            1 => {
                                let blk = ((x / 16 + y / 16) % 2) as f32;
                                1500.0 + blk * 1400.0 + (rnd() - 0.5) * 40.0 - c as f32 * 60.0
                            }
                            _ => 300.0 + 2700.0 * ((x + y) as f32 / (w + h) as f32) + c as f32 * 30.0,
                        };
                        v.clamp(0.0, 4095.0)
                    })
                    .collect()
            })
            .collect()
    }

    fn fnv(v: impl Iterator<Item = u16>) -> u64 {
        v.fold(0xcbf29ce484222325u64, |h, x| (h ^ x as u64).wrapping_mul(0x100000001b3))
    }

    /// Guards refactors: the exact and HQ outputs of the synthetic fields, bit for bit. A deliberate change of
    /// the maths has to update these values (and say why).
    #[test]
    fn golden_hashes_of_synthetic_fields() {
        const GOLDEN: [(u64, u64); 3] = [
            (0xa92a057cbf7bc65d, 0xbd009a822ce4d475),
            (0x5c698b634dc4ec67, 0xb5f0d0dc78f85fe1),
            (0x5065b5d7e62b7422, 0x3cbd9e8e4cd1d832),
        ];
        let mut got = vec![];
        for kind in 0..3 {
            let rpd = synth(kind);
            let (e, ge) = render(&rpd, Gamma::Auto { cap: 9.0 });
            let (q, gq) = render_hq(&rpd, Gamma::Auto { cap: 9.0 });
            assert_eq!(ge, gq);
            got.push((fnv(e.iter().copied()), fnv(q.iter().copied())));
            eprintln!("kind {kind}: gamma {ge:.4} exact {:016x} hq {:016x}", got[kind].0, got[kind].1);
        }
        assert_eq!(got, GOLDEN);
    }

    /// The synthetic fields keep reaching the branches they were made for.
    /// No jump across the cell face that carried a 7.8-level step with a per-cell switch.
    #[test]
    fn cmm_float_has_no_seam_across_cell_faces() {
        let node = |i: f32| i * 4095.0 / 64.0;
        let (g, b) = (node(25.5), node(36.5));
        let mut prev = cmm_float([node(21.5), g, b], true)[0] * 255.0;
        let mut worst = 0f32;
        for k in 1..=200 {
            let r = node(21.5) + (node(22.5) - node(21.5)) * k as f32 / 200.0;
            let v = cmm_float([r, g, b], true)[0] * 255.0;
            worst = worst.max((v - prev).abs());
            prev = v;
        }
        assert!(worst < 1.0, "step {worst}");
    }

    /// Rendering with the decisions of the same field is the plain render.
    #[test]
    fn render_with_given_decisions_equals_render_hq() {
        for kind in 0..3 {
            let rpd = synth(kind);
            let gm = Gamma::Auto { cap: GAMMA_CAP };
            let d = hq_decisions(&rpd, gm);
            assert_eq!(render_hq_given(&rpd, &d), render_hq(&rpd, gm).0, "field {kind}");
        }
    }

    #[test]
    fn synthetic_fields_cover_the_stage_branches() {
        let mut pl = pre_ebp(&synth(0));
        assert!(ebp::gain_and_offset(&mut pl).knee.is_some(), "narrow field: knee");
        let mut pl = pre_ebp(&synth(2));
        assert!(ebp::gain_and_offset(&mut pl).knee.is_none(), "full-range field: stage 1 only");
        assert!(ebp::estimate_gamma(&pl).gamma > 1.0, "bright field: g1");
        let mut pl = pre_ebp(&synth(0));
        ebp::gain_and_offset(&mut pl);
        assert!(ebp::estimate_gamma(&pl).gamma < 1.0, "dark field: g2");
    }
}
