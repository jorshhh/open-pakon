//! Parity with the Python pipeline on a real roll. Needs reference outputs
//! from `rust/tools/golden.py RAW DIR`; set PAKON_GOLDEN=DIR to run, skipped
//! otherwise (the raws are gigabytes and stay out of the repo).

use std::path::PathBuf;

use ndarray::Array3;
use ndarray_npy::read_npy;
use pakon_image::{decode, ribbon};

fn golden() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("PAKON_GOLDEN")?);
    d.join("info.json").is_file().then_some(d)
}

#[test]
fn ribbon_matches_python() {
    let Some(dir) = golden() else {
        eprintln!("PAKON_GOLDEN not set: skipped");
        return;
    };
    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("info.json")).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let r = ribbon::build(&dir.join("slice.raw"), &ribbon::Options::default(), &mut |_, _| {}).unwrap();
    eprintln!("rust ribbon {:?} in {:?}", r.rgb.dim(), t.elapsed());

    assert_eq!(r.info.marker.map(|m| m as i64), info["marker"].as_i64());
    assert_eq!(r.info.leads.g as i64, info["leads"]["g"].as_i64().unwrap());
    assert_eq!(r.info.leads.b as i64, info["leads"]["b"].as_i64().unwrap());
    assert!(r.info.flat.is_some(), "flat field applied");

    let want: Array3<u16> = read_npy(dir.join("ribbon.npy")).unwrap();
    assert_eq!(r.rgb.dim(), want.dim());
    let diff = r.rgb.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
    let maxd = r.rgb.iter().zip(want.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    eprintln!("ribbon: {diff} of {} samples differ, max {maxd}", want.len());
    assert_eq!(diff, 0, "ribbon differs from Python");
}

#[test]
fn layout_detection() {
    let Ok(raw) = std::env::var("PAKON_GOLDEN_RAW") else { return };
    let r = ribbon::Raw::open(&raw).unwrap();
    let l = decode::detect_row_layout(r.samples()).unwrap();
    assert_eq!((l.row_samples, l.ir_lane), (8000, true));
}

/// Whole roll: PAKON_GOLDEN_RAW plus PAKON_GOLDEN_FULL (the Python ribbon of
/// that raw as .npy).
#[test]
fn full_roll_ribbon() {
    let (Ok(raw), Ok(want)) = (std::env::var("PAKON_GOLDEN_RAW"), std::env::var("PAKON_GOLDEN_FULL")) else { return };
    let t = std::time::Instant::now();
    let r = ribbon::build(std::path::Path::new(&raw), &ribbon::Options::default(), &mut |_, _| {}).unwrap();
    eprintln!("rust full ribbon {:?} in {:?}, leads {:?}", r.rgb.dim(), t.elapsed(), r.info.leads);
    let want: Array3<u16> = read_npy(want).unwrap();
    assert_eq!(r.rgb.dim(), want.dim());
    assert!(r.rgb == want, "full ribbon differs from Python");
}

mod colour {
    use super::*;
    use ndarray::{Array1, Array2};
    use pakon_image::colour::{self, Adjust, Balance, Matrix, Table};

    fn npy3(dir: &std::path::Path, n: &str) -> Array3<u16> {
        read_npy(dir.join(n)).unwrap()
    }

    fn negmat(dir: &std::path::Path) -> Matrix {
        let m: Array2<f32> = read_npy(dir.join("negmat.npy")).unwrap();
        std::array::from_fn(|r| std::array::from_fn(|c| m[(r, c)]))
    }

    fn report(name: &str, got: &[u16], want: &[u16]) -> usize {
        let diff = got.iter().zip(want).filter(|(a, b)| a != b).count();
        let maxd = got.iter().zip(want).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
        eprintln!("{name}: {diff} of {} differ, max {maxd}", want.len());
        diff
    }

    #[test]
    fn tables_and_lut() {
        let Some(dir) = golden() else { return };
        let lut: Array1<f32> = read_npy(dir.join("lut.npy")).unwrap();
        assert_eq!(colour::c41_lut(), lut.to_vec(), "density LUT");
        let t: ndarray::Array4<f32> = read_npy(dir.join("rpd_table.npy")).unwrap();
        let ours = Table::from_u8(52, pakon_image::RPD_TABLE_U8).unwrap();
        assert_eq!(ours.data, t.iter().copied().collect::<Vec<_>>(), "rpd table");
    }

    #[test]
    fn render_oem_matches() {
        let Some(dir) = golden() else { return };
        let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("colour.json")).unwrap()).unwrap();
        let crop = npy3(&dir, "crop.npy");
        let (m, lut) = (negmat(&dir), colour::c41_lut());
        let table = Table::from_u8(52, pakon_image::RPD_TABLE_U8).unwrap();

        let t = std::time::Instant::now();
        let plain = colour::render_oem(crop.view(), &m, &lut, &table, None, None, false);
        eprintln!("render 3000x2000 in {:?}", t.elapsed());
        assert_eq!(report("plain", plain.as_slice().unwrap(), npy3(&dir, "render_plain.npy").as_slice().unwrap()), 0);

        let f = |k: &str| -> [f32; 3] { std::array::from_fn(|i| info[k][i].as_f64().unwrap() as f32) };
        let bal = Balance { gain: f("gain"), offset: f("offset") };
        let adj: Adjust = serde_json::from_value(info["adjust"].clone()).unwrap();
        let adj = adj.clean().unwrap();
        let got = colour::render_oem(crop.view(), &m, &lut, &table, Some(&bal), Some(&adj), false);
        assert_eq!(report("balanced+adjusted", got.as_slice().unwrap(), npy3(&dir, "render_adj.npy").as_slice().unwrap()), 0);

        let grey = colour::to_grey(colour::render_oem(crop.view(), &m, &lut, &table, None, None, true).view());
        let want: Array2<u16> = read_npy(dir.join("render_grey.npy")).unwrap();
        let maxd = grey.iter().zip(want.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        report("grey", grey.as_slice().unwrap(), want.as_slice().unwrap());
        assert!(maxd <= 1, "grey within 1 (numpy's matmul order)");
    }
}

#[test]
fn dmin_and_roll_balance() {
    use pakon_image::{balance, colour};
    let Some(dir) = golden() else { return };
    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("colour.json")).unwrap()).unwrap();
    let f = |k: &str| -> [f32; 3] { std::array::from_fn(|i| info[k][i].as_f64().unwrap() as f32) };
    let rgb: Array3<u16> = read_npy(dir.join("ribbon.npy")).unwrap();
    assert_eq!(balance::dmin(rgb.view()), f("dmin"));

    let ds = info["ds"].as_u64().unwrap() as usize;
    let small = rgb.slice(ndarray::s![..;ds, ..;ds, ..]).to_owned();
    let m: ndarray::Array2<f32> = read_npy(dir.join("negmat.npy")).unwrap();
    let m: colour::Matrix = std::array::from_fn(|r| std::array::from_fn(|c| m[(r, c)]));
    let b = balance::roll_balance(small.view(), &m, &colour::c41_lut(), balance::OEM_BALANCE);
    assert_eq!((b.gain, b.offset), (f("gain"), f("offset")));
}

#[test]
fn slide_path() {
    use pakon_image::{colour, slide};
    let Some(dir) = golden() else { return };
    let profiles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../profiles");
    let sp = slide::SlidePath::load(colour::pos_matrix_default(), &profiles).unwrap();

    let t: ndarray::Array4<f32> = read_npy(dir.join("romm_table.npy")).unwrap();
    assert_eq!(sp.romm.data, t.iter().copied().collect::<Vec<_>>(), "romm table");

    let want: Array3<f32> = read_npy(dir.join("link_grid.npy")).unwrap();
    let g: Vec<f32> = (0..23).map(|i| if i == 22 { 1.0 } else { (i as f64 * (1.0 / 22.0)) as f32 }).collect();
    let mut k = 0;
    let mut bad = 0;
    for r in 0..23 {
        for gg in 0..23 {
            for b in 0..23 {
                let got = sp.link.apply([g[r], g[gg], g[b]]);
                let w = [want[(0, k, 0)], want[(0, k, 1)], want[(0, k, 2)]];
                bad += (got != w) as usize;
                k += 1;
            }
        }
    }
    assert_eq!(bad, 0, "link differs on {bad} grid points");

    let crop: Array3<u16> = read_npy(dir.join("crop.npy")).unwrap();
    let crop = crop.slice(ndarray::s![..600, .., ..]);
    let got = slide::render_slide(crop, &sp, Some([10.5, 7.25, 3.0]));
    let want: Array3<u16> = read_npy(dir.join("render_slide.npy")).unwrap();
    let diff = got.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
    let maxd = got.iter().zip(want.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    eprintln!("slide render: {diff} of {} differ, max {maxd}", want.len());
    assert_eq!(diff, 0);

    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("slide.json")).unwrap()).unwrap();
    let rgb: Array3<u16> = read_npy(dir.join("ribbon.npy")).unwrap();
    for k in [3.0, 6.0, 12.0] {
        let got = slide::find_slide_gaps(rgb.view(), 80, k);
        let want: Vec<(usize, usize)> = serde_json::from_value(info["gaps"][format!("{k:.1}")].clone()).unwrap();
        assert_eq!(got, want, "gaps k={k}");
    }
    let gaps: Vec<(usize, usize)> = serde_json::from_value(info["gaps"]["12.0"].clone()).unwrap();
    let fl = slide::measure_slide_flare(rgb.view(), slide::SLIDE_FLARE, Some(&gaps));
    let want: Vec<f32> = serde_json::from_value(info["flare12"].clone()).unwrap();
    assert_eq!(fl.to_vec(), want);
}

/// TLX framing on the whole roll: PAKON_GOLDEN_FULL plus framing.json next
/// to it (rust/tools/golden_framing.py).
#[test]
fn framing_matches() {
    use pakon_image::framing::{self, Format};
    let Ok(full) = std::env::var("PAKON_GOLDEN_FULL") else { return };
    let jf = std::path::Path::new(&full).with_file_name("framing.json");
    let Ok(text) = std::fs::read_to_string(&jf) else { return };
    let want: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rgb: Array3<u16> = read_npy(&full).unwrap();
    let prof = framing::profile(rgb.view(), 8, false);
    let wp: Vec<i32> = serde_json::from_value(want["profile"].clone()).unwrap();
    let bad = prof.iter().zip(&wp).filter(|(a, b)| a != b).count();
    assert_eq!(bad, 0, "profile differs in {bad} of {} blocks", wp.len());
    for fmt in Format::ALL {
        for pos in [false, true] {
            let got = framing::frame_grid(rgb.view(), pos, fmt);
            let w: (Vec<usize>, usize, usize) = serde_json::from_value(want[format!("{}-{}", fmt.name(), pos as u8)].clone()).unwrap();
            assert_eq!(got, w, "{} positive={pos}", fmt.name());
        }
    }
}

#[test]
fn ice_matches() {
    use ndarray::{Array1, Array2};
    use pakon_image::ice;
    let Some(dir) = golden() else { return };
    if !dir.join("ice.json").is_file() {
        return;
    }
    let r = ribbon::build(&dir.join("slice.raw"), &ribbon::Options::default(), &mut |_, _| {}).unwrap();
    let ir = r.ir.as_ref().expect("IR plane");
    let want: Array2<u16> = read_npy(dir.join("ir_plane.npy")).unwrap();
    assert!(*ir == want, "IR plane differs");

    let cols = r.rgb.dim().1;
    let noise = ice::ir_noise_factor(r.flat.as_ref().unwrap(), cols);
    let wn: Array1<f32> = read_npy(dir.join("ice_noise.npy")).unwrap();
    assert_eq!(noise, wn.to_vec(), "noise factor");

    let part = r.rgb.slice(ndarray::s![2000..5000, .., ..]);
    let irc = ir.slice(ndarray::s![2000..5000, ..]);
    let tail = ice::ir_tail_factor(&irc.mapv(|v| v as f32));
    let wt: Array1<f32> = read_npy(dir.join("ice_tail.npy")).unwrap();
    assert_eq!(tail, wt.to_vec(), "tail factor");

    let t = std::time::Instant::now();
    let (out, mask) = ice::clean_frame(part, irc, &noise, ice::pakon_dpi(cols));
    eprintln!("ice frame in {:?}, {} rebuilt", t.elapsed(), mask.iter().filter(|&&m| m).count());
    let wm: Array2<bool> = read_npy(dir.join("ice_mask.npy")).unwrap();
    let mdiff = mask.iter().zip(wm.iter()).filter(|(a, b)| a != b).count();
    let wo: Array3<u16> = read_npy(dir.join("ice_out.npy")).unwrap();
    let odiff = out.iter().zip(wo.iter()).filter(|(a, b)| a != b).count();
    let maxd = out.iter().zip(wo.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    eprintln!("ice: mask {mdiff} differ (of {} rebuilt), out {odiff} differ, max {maxd}", wm.iter().filter(|&&m| m).count());
    assert_eq!((mdiff, odiff), (0, 0));
}

#[test]
fn adjustments_match() {
    use pakon_image::adjust;
    use pakon_image::colour::{self, Adjust, Table, Tone};
    use pakon_image::slide;
    let Some(dir) = golden() else { return };
    if !dir.join("adjust.json").is_file() {
        eprintln!("skipped: run rust/tools/golden_adjust.py first");
        return;
    }
    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("adjust.json")).unwrap()).unwrap();
    let profiles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../profiles");
    let rpd = Table::from_u8(52, pakon_image::RPD_TABLE_U8).unwrap();
    for case in info["cases"].as_array().unwrap() {
        let s = case[0].as_i64().unwrap() as i32;
        let tone: Tone = serde_json::from_value(case[1].clone()).unwrap();
        let t = std::time::Instant::now();
        let got = adjust::adjusted_table(&rpd, &profiles.join("rpd.pf"), s, tone).unwrap();
        let want: ndarray::Array4<f32> = read_npy(dir.join(format!("adj_rpd_{s}_{}.npy", case[1].as_str().unwrap()))).unwrap();
        let bad = got.data.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
        let maxd = got.data.iter().zip(want.iter()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("table sat {s} {tone:?}: {bad} differ (max {maxd:e}) in {:?}", t.elapsed());
        assert_eq!(bad, 0);
    }

    let crop: Array3<u16> = read_npy(dir.join("crop.npy")).unwrap();
    let crop = crop.slice(ndarray::s![..600, .., ..]);
    let sp = slide::SlidePath::load(colour::pos_matrix_default(), &profiles).unwrap();
    let adj: Adjust = serde_json::from_value(info["slide"].clone()).unwrap();
    let got = slide::render_slide_adjusted(crop, &sp, Some([10.5, 7.25, 3.0]), &adj.clean().unwrap(), &sp.romm);
    let want: Array3<u16> = read_npy(dir.join("render_slide_adj.npy")).unwrap();
    assert_eq!(got.iter().zip(want.iter()).filter(|(a, b)| a != b).count(), 0, "slide adjusted");

    let m: ndarray::Array2<f32> = read_npy(dir.join("negmat.npy")).unwrap();
    let m: colour::Matrix = std::array::from_fn(|r| std::array::from_fn(|c| m[(r, c)]));
    let lut = colour::c41_lut();
    let adj: Adjust = serde_json::from_value(info["oem"].clone()).unwrap();
    let adj = adj.clean().unwrap();
    let table = adjust::adjusted_table(&rpd, &profiles.join("rpd.pf"), adj.saturation, adj.tone).unwrap();
    let got = colour::render_oem(crop, &m, &lut, &table, None, Some(&adj), false);
    let want: Array3<u16> = read_npy(dir.join("render_oem_sat.npy")).unwrap();
    assert_eq!(got.iter().zip(want.iter()).filter(|(a, b)| a != b).count(), 0, "oem saturation + sepia");

    let adj = Adjust { tone: Tone::Warm, ..Default::default() };
    let table = adjust::adjusted_table(&rpd, &profiles.join("rpd.pf"), 0, Tone::Warm).unwrap();
    let got = colour::render_oem(crop, &m, &lut, &table, None, Some(&adj), true);
    let want: Array3<u16> = read_npy(dir.join("render_grey_tone.npy")).unwrap();
    let bad = got.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
    let maxd = got.iter().zip(want.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    eprintln!("grey + warm tone: {bad} differ, max {maxd}");
    assert!(maxd <= 1, "grey within 1 (the C's channel mean)");
}

#[test]
fn romm_matrix_trc_matches_its_table() {
    use pakon_image::adjust::{lab_to_srgb, Input};
    use pakon_image::colour::Table;
    let profiles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../profiles");
    let romm = Input::read(&profiles.join("romm.pf")).unwrap();
    assert!(matches!(romm, Input::MatrixTrc { .. }));
    let t = Table::from_u8(52, pakon_image::ROMM_TABLE_U8).unwrap();
    let mut worst = 0.0f32;
    for i in (0..52).step_by(3) {
        for j in (0..52).step_by(3) {
            for k in (0..52).step_by(3) {
                let x = [i, j, k].map(|v| v as f32 / 51.0);
                let a = lab_to_srgb(romm.to_lab(x));
                let b = t.lookup(x);
                worst = worst.max((0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max));
            }
        }
    }
    eprintln!("romm matrix/TRC vs LittleCMS table: max {worst}");
    assert!(worst < 0.02, "{worst}");
}

#[test]
fn fallback_matches() {
    use pakon_image::colour::{self, Table};
    use pakon_image::fallback;
    let Some(dir) = golden() else { return };
    if !dir.join("render_jpeg.npy").is_file() {
        eprintln!("skipped: run rust/tools/golden_fallback.py first");
        return;
    }
    let info: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("colour.json")).unwrap()).unwrap();
    let base: [f32; 3] = std::array::from_fn(|i| info["dmin"][i].as_f64().unwrap() as f32);
    let crop: Array3<u16> = read_npy(dir.join("crop.npy")).unwrap();
    let crop = crop.slice(ndarray::s![..800, .., ..]);
    let pos = fallback::invert_c41(crop, base, &colour::c41_lut());
    let want: Array3<u16> = read_npy(dir.join("invert_c41.npy")).unwrap();
    assert_eq!(pos, want, "invert_c41");

    let rpd = Table::from_u8(52, pakon_image::RPD_TABLE_U8).unwrap();
    let got = fallback::render_jpeg_default(pos.view(), &rpd);
    let want: Array3<u8> = read_npy(dir.join("render_jpeg.npy")).unwrap();
    let d: Vec<i32> = got.iter().zip(want.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).collect();
    let mean = d.iter().sum::<i32>() as f64 / d.len() as f64;
    let maxd = *d.iter().max().unwrap();
    let over2 = d.iter().filter(|&&x| x > 2).count() as f64 / d.len() as f64;
    eprintln!("render_jpeg vs LittleCMS: mean {mean:.3}, max {maxd}, {:.4}% over 2", 100.0 * over2);
    // the 8-bit LittleCMS transform alone differs from the table by 0.4 mean
    assert!(mean < 0.5 && over2 < 0.02, "render_jpeg drifted from Python");
}
