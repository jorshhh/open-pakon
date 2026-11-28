//! Render the PakonX PSI 2.4 reference frames and compare them with the real
//! DLL's output: `cargo run --release -p pakon-image --example psi24_ref -- <1.look dir> [gamma]`
//! (`gamma`: `auto` (default, capped at `GAMMA_CAP`), `uncapped`, or a number).
use pakon_image::ebp::Gamma;
use pakon_image::psi24;

const SH: [[f32; 3]; 4] = [[714.0, 312.0, -101.0], [562.0, 162.0, -236.0], [796.0, 405.0, 3.0], [810.0, 434.0, 45.0]];

fn u16s(p: &str) -> Vec<u16> {
    std::fs::read(p).unwrap().chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = &a[1];
    let mode = a.get(2).map(String::as_str).unwrap_or("auto");
    let gm = match mode {
        "auto" => Gamma::Auto { cap: psi24::GAMMA_CAP },
        "uncapped" => Gamma::Auto { cap: 9.0 },
        v => Gamma::Fixed(v.parse().unwrap()),
    };
    let d: Vec<f32> = std::env::var("DSHIFT").unwrap_or("0,0,0".into()).split(',').map(|v| v.parse().unwrap()).collect();
    // SHIFTS="r,g,b;r,g,b;..." replaces the real shifts (e.g. open-pakon's own KCDFS ones).
    let sh: Vec<[f32; 3]> = match std::env::var("SHIFTS") {
        Ok(v) => v.split(';').map(|f| { let p: Vec<f32> = f.split(',').map(|x| x.parse().unwrap()).collect(); [p[0], p[1], p[2]] }).collect(),
        Err(_) => SH.to_vec(),
    };
    let (w, h) = (2250usize, 1500usize);
    let lut = u16s(&format!("{dir}/scp_lut.u16"));
    for f in 1..=4 {
        let raw = u16s(&format!("{dir}/psi0{f}.raw"));
        let rpd: Vec<Vec<f32>> = (0..h)
            .map(|y| {
                let mut row = Vec::with_capacity(3 * w);
                for x in 0..w {
                    for c in 0..3 {
                        let v = raw[c * w * h + y * w + x].min(4095) as usize;
                        row.push((lut[c * 4096 + v] as f32 + sh[f - 1][c] + d[c]).clamp(0.0, 4095.0));
                    }
                }
                row
            })
            .collect();
        let t = std::time::Instant::now();
        let (out, g) = if std::env::var("PSI24_HQ").is_ok() { psi24::render_hq(&rpd, gm) } else { psi24::render(&rpd, gm) };
        let el = t.elapsed();
        let real = std::fs::read(format!("{dir}/look0{f}.rgb")).unwrap();
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
        println!(
            "frame {f}: gamma {g:.3} MAE {:.2} (R {:.2} G {:.2} B {:.2}) bias {:+.1} {:+.1} {:+.1}  {:.1}s",
            (mae[0] + mae[1] + mae[2]) / 3.0 / n, mae[0] / n, mae[1] / n, mae[2] / n, bias[0] / n, bias[1] / n, bias[2] / n, el.as_secs_f64()
        );
    }
}
