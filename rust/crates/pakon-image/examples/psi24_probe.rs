//! (`PSI24_HQ=1` renders with the high-bit-depth path, quantised to 8 bit for the comparison.)
//! Compare our render with a probe output: `psi24_probe <lut.u16> <in.raw> <ref.rgb> <shifts r,g,b|0> [gamma]`
use pakon_image::ebp::Gamma;
use pakon_image::psi24;
fn u16s(p: &str) -> Vec<u16> {
    std::fs::read(p)
        .unwrap()
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect()
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let lut = u16s(&a[1]);
    let raw = u16s(&a[2]);
    let real = std::fs::read(&a[3]).unwrap();
    let sh: Vec<f32> = a[4].split(',').map(|v| v.parse().unwrap()).collect();
    let gm = match a.get(5).map(String::as_str) {
        None | Some("auto") => Gamma::Auto { cap: 9.0 },
        Some(v) => Gamma::Fixed(v.parse().unwrap()),
    };
    let ev = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (ev("PSI24_W", 2250), ev("PSI24_H", 1500));
    let rpd: Vec<Vec<f32>> = (0..h)
        .map(|y| {
            let mut row = Vec::with_capacity(3 * w);
            for x in 0..w {
                for c in 0..3 {
                    let v = raw[c * w * h + y * w + x].min(4095) as usize;
                    row.push((lut[c * 4096 + v] as f32 + sh[c]).clamp(0.0, 4095.0));
                }
            }
            row
        })
        .collect();
    let (out, g) = if std::env::var("PSI24_HQ").is_ok() { psi24::render_hq(&rpd, gm) } else { psi24::render(&rpd, gm) };
    if let Ok(path) = std::env::var("PROBE_OUT") { // planar 8-bit RGB like the .rgb files
        let mut o = vec![0u8; 3 * w * h];
        for y in 0..h { for x in 0..w { for c in 0..3 { o[c * w * h + y * w + x] = ((out[(y, x, c)] as u32 + 128) / 257) as u8; } } }
        std::fs::write(path, o).unwrap();
    }
    let mut mae = [0f64; 3];
    let mut bias = [0f64; 3];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let o = ((out[(y, x, c)] as u32 + 128) / 257) as f64;
                let d = o - real[c * w * h + y * w + x] as f64;
                mae[c] += d.abs();
                bias[c] += d;
            }
        }
    }
    let n = (w * h) as f64;
    let cols = [0usize, 300, 600, 900, 1100, 1300, 1500, 1800, 2100, 2249];
    let mut line = String::new();
    for &x in &cols {
        line += &format!(
            " [{},{},{}|{},{},{}]",
            (out[(750, x, 0)] as u32 + 128) / 257,
            (out[(750, x, 1)] as u32 + 128) / 257,
            (out[(750, x, 2)] as u32 + 128) / 257,
            real[750 * w + x],
            real[w * h + 750 * w + x],
            real[2 * w * h + 750 * w + x]
        );
    }
    println!(
        "gamma {g:.3} MAE {:.2} bias {:+.1} {:+.1} {:+.1} |{line}",
        (mae[0] + mae[1] + mae[2]) / 3.0 / n,
        bias[0] / n,
        bias[1] / n,
        bias[2] / n
    );
}
