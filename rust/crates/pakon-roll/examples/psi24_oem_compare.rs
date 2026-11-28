//! Our PSI 2.4 chain on the OEM's per-frame RPD raws against the OEM's own finals.
//!   psi24_oem_compare <oem raw dir> <tif dir (N.rgb8 = the final, interleaved u8, 2000x3000)> <repo> <frames csv>
//! The OEM raw is a 16-byte header then planar u16 3 x 2000 x 3000 (12-bit RPD before the SCP LUT).
//! The balance (KCDFS) runs on the whole roll (every `*A.raw` of the directory), area-averaged to the
//! analysis grid in RPD. Then, per chosen frame: our render with that balance vs the OEM final, and the
//! per-channel RPD shift that would make the render agree (coordinate descent on MAE, exact path).
use ndarray::Array3;
use pakon_image::colour::{smooth_scp, ScpTable};
use pakon_image::ebp::Gamma;
use pakon_image::psi24;
use pakon_oem::{kcdfs, sba};
use rayon::prelude::*;

const W: usize = 3000;
const H: usize = 2000;

fn load(p: &std::path::Path) -> Vec<u16> {
    let b = std::fs::read(p).unwrap();
    b[16..].chunks_exact(2).map(|v| u16::from_le_bytes([v[0], v[1]])).collect()
}

fn analysis(raw: &[u16]) -> Array3<i16> {
    let (ah, aw) = if std::env::var("SMALL").is_ok() { (sba::ANALYSIS_H, sba::ANALYSIS_W) } else { (sba::KCDFS_H, sba::KCDFS_W) };
    let (sy, sx) = (H / ah, W / aw);
    let (y0, x0) = ((H - sy * ah) / 2, (W - sx * aw) / 2);
    let mut out = Array3::<i16>::zeros((ah, aw, 3));
    for i in 0..ah {
        for j in 0..aw {
            for c in 0..3 {
                let mut s = 0f32;
                for y in 0..sy {
                    for x in 0..sx {
                        s += raw[c * H * W + (y0 + i * sy + y) * W + x0 + j * sx + x] as f32;
                    }
                }
                out[(i, j, c)] = (s / (sy * sx) as f32).round_ties_even().clamp(0.0, 4095.0) as i16;
            }
        }
    }
    out
}

fn field<S: ScpTable + ?Sized>(raw: &[u16], lut: &S, sh: [f32; 3]) -> Vec<Vec<f32>> {
    (0..H)
        .into_par_iter()
        .map(|y| {
            let mut v = Vec::with_capacity(3 * W);
            for x in 0..W {
                for c in 0..3 {
                    v.push((lut.at(c, raw[c * H * W + y * W + x].min(4095) as usize) + sh[c]).clamp(0.0, 4095.0));
                }
            }
            v
        })
        .collect()
}

fn q(v: u16) -> i32 {
    ((v as u32 + 128) / 257) as i32
}

/// (MAE, mean signed error per channel) of an image (u16, level*257) against the interleaved u8 final.
fn score(img: &Array3<u16>, tif: &[u8]) -> (f64, [f64; 3]) {
    let (mut s, mut d) = (0f64, [0f64; 3]);
    for y in 0..H {
        for x in 0..W {
            for c in 0..3 {
                let e = q(img[(y, x, c)]) - tif[(y * W + x) * 3 + c] as i32;
                s += e.abs() as f64;
                d[c] += e as f64;
            }
        }
    }
    let n = (H * W) as f64;
    (s / (3.0 * n), d.map(|v| v / n))
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (dir, tdir, repo) = (std::path::Path::new(&a[0]), std::path::Path::new(&a[1]), std::path::Path::new(&a[2]));
    let chosen: Vec<String> = a[3].split(',').map(String::from).collect();
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with("A.raw") && !n.starts_with("._")).collect();
    names.sort();
    // ORDER=desc|asc: only the numbered frames (36..1 is the scan order: OEM frame k is the roll's 37-k-th); ALL keeps the extras
    match std::env::var("ORDER").as_deref() {
        Ok("desc") | Ok("asc") => {
            names.retain(|n| n[..n.len() - 5].parse::<u32>().is_ok_and(|k| k >= 1));
            names.sort_by_key(|n| n[..n.len() - 5].parse::<u32>().unwrap());
            if std::env::var("ORDER").as_deref() == Ok("desc") {
                names.reverse();
            }
        }
        _ => {}
    }
    eprintln!("{} frames in the roll", names.len());
    // THUMB=flipv|fliph|rot180|transpose: the thumbnail's orientation (the OEM's may differ from the raw's)
    let mode = std::env::var("THUMB").unwrap_or_default();
    let imgs: Vec<Array3<i16>> = names
        .iter()
        .map(|n| {
            let a = analysis(&load(&dir.join(n)));
            let (h, w, _) = a.dim();
            match mode.as_str() {
                "flipv" => Array3::from_shape_fn((h, w, 3), |(y, x, c)| a[(h - 1 - y, x, c)]),
                "fliph" => Array3::from_shape_fn((h, w, 3), |(y, x, c)| a[(y, w - 1 - x, c)]),
                "rot180" => Array3::from_shape_fn((h, w, 3), |(y, x, c)| a[(h - 1 - y, w - 1 - x, c)]),
                _ => a,
            }
        })
        .collect();
    let dmin: Option<[i64; 3]> = std::env::var("DMIN").ok().map(|v| {
        let p: Vec<i64> = v.split(',').map(|x| x.parse().unwrap()).collect();
        [p[0], p[1], p[2]]
    });
    let res = kcdfs::roll_balance_against(&kcdfs::default_dll(repo), &kcdfs::kcdfs_dir(repo), &imgs, None, dmin, true).expect("kcdfs");
    if std::env::var("SHIFTS_ONLY").is_ok() {
        for n in &chosen {
            let i = names.iter().position(|x| x == &format!("{n}A.raw")).expect("frame");
            println!("frame {n:>2}: shifts {:+.0} {:+.0} {:+.0}", res.shifts[i][0], res.shifts[i][1], res.shifts[i][2]);
        }
        return;
    }
    let luts = res.scp_luts.expect("scp luts");
    let smooth = smooth_scp(&luts, 9);
    let gm = Gamma::Auto { cap: psi24::GAMMA_CAP };
    for n in &chosen {
        let i = names.iter().position(|x| x == &format!("{n}A.raw")).expect("frame");
        let raw = load(&dir.join(&names[i]));
        let tif = std::fs::read(tdir.join(format!("{n}.rgb8"))).unwrap();
        let sh = res.shifts[i];
        let (hq, g) = psi24::render_hq(&field(&raw, &*smooth, sh), gm);
        let (ex, _) = psi24::render(&field(&raw, &*luts, sh), gm);
        if let Ok(d) = std::env::var("DUMP") {
            let b: Vec<u8> = (0..H * W * 3).map(|k| q(img_at(&hq, k)) as u8).collect();
            std::fs::write(format!("{d}/{n}.rgb8"), b).unwrap();
        }
        let (m_h, d_h) = score(&hq, &tif);
        let (m_e, _) = score(&ex, &tif);
        println!("frame {n:>2}: our shifts {:+.0} {:+.0} {:+.0}  gamma {g:.3}  MAE HQ {m_h:.2} exact {m_e:.2}  signed (ours - OEM) {:+.1} {:+.1} {:+.1}", sh[0], sh[1], sh[2], d_h[0], d_h[1], d_h[2]);
        if std::env::var("FIT").is_ok() {
            let mut best = sh;
            let mut cur = score(&psi24::render(&field(&raw, &*luts, best), gm).0, &tif).0;
            for step in [48.0f32, 24.0, 12.0, 6.0] {
                for _ in 0..2 {
                    for c in 0..3 {
                        for dir in [-1.0f32, 1.0] {
                            loop {
                                let mut t = best;
                                t[c] += dir * step;
                                let s = score(&psi24::render(&field(&raw, &*luts, t), gm).0, &tif).0;
                                if s + 1e-9 < cur {
                                    cur = s;
                                    best = t;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            println!("          fitted shifts {:+.0} {:+.0} {:+.0} (ours {:+.0} {:+.0} {:+.0}; delta {:+.0} {:+.0} {:+.0})  MAE exact {cur:.2}", best[0], best[1], best[2], sh[0], sh[1], sh[2], best[0] - sh[0], best[1] - sh[1], best[2] - sh[2]);
        }
    }
}

fn img_at(img: &Array3<u16>, k: usize) -> u16 {
    let (y, x, c) = (k / (W * 3), (k / 3) % W, k % 3);
    img[(y, x, c)]
}
