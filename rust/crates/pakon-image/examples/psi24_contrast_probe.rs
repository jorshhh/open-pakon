//! Where does the contrast slider move a neutral patch? Median RPD of a real frame, contrast applied the
//! way `rpd_adjusted` does (about 1550 per channel, BEFORE the scene-balance shifts) vs about the balanced
//! neutral (after the shifts); rendered through the PSI 2.4 HQ chain at fixed gamma, 8-bit levels.
//!   psi24_contrast_probe <lut.u16> <in.raw> <shifts r,g,b>      (PSI24_W / PSI24_H)
use pakon_image::colour::CONTRAST_PIVOT;
use pakon_image::ebp::Gamma;
use pakon_image::psi24;

fn u16s(p: &str) -> Vec<u16> {
    std::fs::read(p).unwrap().chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()
}

fn main() {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    if a.first().map(String::as_str) == Some("frame") {
        a.remove(0);
        return frame_spread(&a);
    }
    let (lut, raw) = (u16s(&a[0]), u16s(&a[1]));
    let sh: Vec<f32> = a[2].split(',').map(|v| v.parse().unwrap()).collect();
    let ev = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (ev("PSI24_W", 2250), ev("PSI24_H", 1500));
    // per-channel median of the SCP output (before the shifts) over a sparse grid
    let mut med = [0f32; 3];
    for c in 0..3 {
        let mut v: Vec<f32> = (0..h).step_by(7).flat_map(|y| (0..w).step_by(7).map(move |x| (y, x))).map(|(y, x)| lut[c * 4096 + raw[c * w * h + y * w + x].min(4095) as usize] as f32).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        med[c] = v[v.len() / 2];
    }
    println!("median SCP output R/G/B {:.0} {:.0} {:.0}; shifts {:?}; balanced {:.0} {:.0} {:.0}", med[0], med[1], med[2], sh, med[0] + sh[0], med[1] + sh[1], med[2] + sh[2]);
    println!("{:>8} | {:^27} | {:^27}", "contrast", "as is (pivot before shifts)", "pivot after shifts");
    for contrast in [-500.0f32, -250.0, 0.0, 250.0, 500.0] {
        let slope = if contrast != 0.0 { 1000.0 + contrast / 2.0 } else { 0.0 };
        let mut line = format!("{contrast:>8}");
        for after in [false, true] {
            let rpd: Vec<f32> = (0..3)
                .map(|c| {
                    let k = |o: f32| if slope != 0.0 { ((o - CONTRAST_PIVOT) * slope / 1000.0 + CONTRAST_PIVOT).clamp(0.0, 4095.0) } else { o };
                    if after { k(med[c] + sh[c]) } else { (k(med[c]) + sh[c]).clamp(0.0, 4095.0) }
                })
                .collect();
            let rows: Vec<Vec<f32>> = (0..16).map(|_| (0..16).flat_map(|_| rpd.clone()).collect()).collect();
            let (r, _, _, _) = psi24::render_hq_f32(&rows, Gamma::Fixed(1.0));
            let p = &r[8][3 * 8..3 * 8 + 3];
            line += &format!(" | {:6.1} {:6.1} {:6.1}  R-B {:+5.1}", p[0], p[1], p[2], p[0] - p[2]);
        }
        println!("{line}");
    }
}

/// Whole frame through the PSI 2.4 HQ chain: luma p10 / p50 / p90 (8-bit levels) of the render with the
/// contrast slider applied after EBP (`apply_contrast`) vs on the RPD field, balance first (what `rpd_adjusted`
/// does now). `psi24_contrast_probe frame <lut> <raw> <shifts>`
fn frame_spread(a: &[String]) {
    let (lut, raw) = (u16s(&a[0]), u16s(&a[1]));
    let sh: Vec<f32> = a[2].split(',').map(|v| v.parse().unwrap()).collect();
    let ev = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (ev("PSI24_W", 2250), ev("PSI24_H", 1500));
    for c in [-500i32, -250, 0, 250, 500] {
        let slope = if c != 0 { 1000.0 + c as f32 / 2.0 } else { 0.0 };
        let field = |rpd_contrast: bool| -> Vec<Vec<f32>> {
            (0..h)
                .step_by(2)
                .map(|y| {
                    (0..w)
                        .step_by(2)
                        .flat_map(|x| {
                            let (lut, raw, sh) = (&lut, &raw, &sh);
                            (0..3).map(move |ch| {
                                let mut o = (lut[ch * 4096 + raw[ch * w * h + y * w + x].min(4095) as usize] as f32 + sh[ch]).clamp(0.0, 4095.0);
                                if rpd_contrast && slope != 0.0 {
                                    o = ((o - CONTRAST_PIVOT) * slope / 1000.0 + CONTRAST_PIVOT).clamp(0.0, 4095.0);
                                }
                                o
                            })
                        })
                        .collect()
                })
                .collect()
        };
        let stats = |img: &ndarray::Array3<u16>| {
            let mut l: Vec<f32> = img.outer_iter().flat_map(|r| r.outer_iter().map(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0 / 257.0).collect::<Vec<_>>()).collect();
            l.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let q = |f: f32| l[(l.len() as f32 * f) as usize];
            (q(0.1), q(0.5), q(0.9))
        };
        let (a1, _) = psi24::render_hq(&field(true), Gamma::Auto { cap: 2.0 });
        let (mut b1, _) = psi24::render_hq(&field(false), Gamma::Auto { cap: 2.0 });
        psi24::apply_contrast(&mut b1, c);
        let (s1, s2) = (stats(&a1), stats(&b1));
        println!("contrast {c:>5}  RPD-domain p10/50/90 {:5.1} {:5.1} {:5.1} (span {:5.1})   post-EBP {:5.1} {:5.1} {:5.1} (span {:5.1})", s1.0, s1.1, s1.2, s1.2 - s1.0, s2.0, s2.1, s2.2, s2.2 - s2.0);
    }
}
