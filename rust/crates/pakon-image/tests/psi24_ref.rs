//! Gated: renders the PakonX PSI 2.4 reference strip (real `PakonIMAu.dll`
//! output) and checks the mean absolute error. Set `PAKON_PSI24_REF` to the
//! reference's `1.look` folder (needs `psiNN.raw`, `scp_lut.u16`,
//! `lookNN.rgb`); skipped otherwise.
use pakon_image::{ebp::Gamma, psi24};

const SH: [[f32; 3]; 4] = [[714.0, 312.0, -101.0], [562.0, 162.0, -236.0], [796.0, 405.0, 3.0], [810.0, 434.0, 45.0]];
// Measured with the Kodak CMM + EBP render: 0.694 0.737 1.218 1.209 (mean over R, G, B, 8-bit levels),
// plus a modest margin.
const MAX_MAE: [f64; 4] = [0.85, 0.9, 1.4, 1.4];

fn u16s(p: &str) -> Vec<u16> {
    std::fs::read(p).unwrap().chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()
}

#[test]
fn reference_frames_match_the_real_dll() {
    let Ok(dir) = std::env::var("PAKON_PSI24_REF") else { return };
    let (w, h) = (2250usize, 1500usize);
    let lut = u16s(&format!("{dir}/scp_lut.u16"));
    for f in 1..=4 {
        let raw = u16s(&format!("{dir}/psi0{f}.raw"));
        let rpd: Vec<Vec<f32>> = (0..h)
            .map(|y| (0..w).flat_map(|x| (0..3).map(move |c| (c, x))).map(|(c, x)| (lut[c * 4096 + raw[c * w * h + y * w + x].min(4095) as usize] as f32 + SH[f - 1][c]).clamp(0.0, 4095.0)).collect::<Vec<_>>())
            .map(|v: Vec<f32>| v)
            .collect();
        let (out, _) = psi24::render(&rpd, Gamma::Auto { cap: psi24::GAMMA_CAP });
        let real = std::fs::read(format!("{dir}/look0{f}.rgb")).unwrap();
        let mut sum = 0f64;
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    sum += (((out[(y, x, c)] as u32 + 128) / 257) as f64 - real[c * w * h + y * w + x] as f64).abs();
                }
            }
        }
        let mae = sum / (3 * w * h) as f64;
        assert!(mae < MAX_MAE[f - 1], "frame {f}: MAE {mae}");
    }
}
