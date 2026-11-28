//! Run our EBP stages on a real per-stage dump: `psi24_stage <in.u8> <go|gamma|bp|all> <ref.u8> [out.u8]`
//! (planar 8-bit RGB 2250x1500; prints MAE per channel against the reference stage).
use pakon_image::ebp::{self, Gamma, Planes};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let ev = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (ev("PSI24_W", 2250), ev("PSI24_H", 1500));
    let inp = std::fs::read(&a[1]).unwrap();
    let rf = std::fs::read(&a[3]).unwrap();
    let mut pl = Planes::new(w, h);
    for c in 0..3 { for i in 0..w * h { pl.p[c][i] = (inp[c * w * h + i] as u16) << 4; } }
    
    let mut info = String::new();
    match a[2].as_str() {
        "go" => { let g = ebp::gain_and_offset(&mut pl); info = format!("{g:?}"); }
        "gamma" => { let est = ebp::estimate_gamma(&pl); info = format!("est gamma {:.3}", est.gamma); let p = std::env::var("P").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0); let gm = match std::env::var("GFIX").ok().and_then(|v| v.parse().ok()) { Some(v) => Gamma::Fixed(v), None => Gamma::Auto { cap: 9.0 } }; let g = ebp::gamma_correction(&mut pl, p, gm); info += &format!(" used {g:.3}"); }
        "nf" => ebp::noise_filter(&mut pl),
        "bp" => ebp::black_print(&mut pl),
        "all" => { let (g, gm) = ebp::enhanced_black_printing(&mut pl, Gamma::Auto { cap: 9.0 }); info = format!("{g:?} gamma {gm:.3}"); }
        m => panic!("{m}"),
    }
    let mut out = vec![0u8; 3 * w * h];
    let mut mae = [0f64; 3]; let mut bias = [0f64; 3]; let mut exact = 0usize;
    for c in 0..3 { for i in 0..w * h {
        let v = ((pl.p[c][i] + 8) >> 4).min(255) as u8; out[c * w * h + i] = v;
        let d = v as f64 - rf[c * w * h + i] as f64; mae[c] += d.abs(); bias[c] += d; if d == 0.0 { exact += 1; } } }
    let n = (w * h) as f64;
    println!("MAE {:.3} {:.3} {:.3} bias {:+.2} {:+.2} {:+.2} exact {:.1}% | {info}", mae[0] / n, mae[1] / n, mae[2] / n, bias[0] / n, bias[1] / n, bias[2] / n, 100.0 * exact as f64 / (3.0 * n));
    if let Some(p) = a.get(4) { std::fs::write(p, out).unwrap(); }
}
