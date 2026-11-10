//! OEM Digital ICE against tools/oem_dice.py (rust/tools/golden_dice.py at git tag python-final
//! writes the reference into $PAKON_GOLDEN).

use std::path::PathBuf;
use std::time::Instant;

use ndarray::{s, Array2, Array3};
use ndarray_npy::read_npy;
use pakon_oem::dice;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn dice_matches_python() {
    let Some(dir) = std::env::var_os("PAKON_GOLDEN").map(PathBuf::from) else { return };
    if !dir.join("dice.json").is_file() {
        eprintln!("skipped: make the reference with rust/tools/golden_dice.py (git tag python-final)");
        return;
    }
    let rgb: Array3<u16> = read_npy(dir.join("ribbon.npy")).unwrap();
    let ir: Array2<u16> = read_npy(dir.join("ir_plane.npy")).unwrap();
    let part = rgb.slice(s![2000..5000, .., ..]).to_owned();
    let irc = ir.slice(s![2000..5000, ..]).to_owned();
    let t = Instant::now();
    let mut d = dice::Dice::new(&dice::default_dll(&repo())).unwrap();
    let t1 = Instant::now();
    let (out, mask) = dice::clean_frame(&mut d, part.view(), irc.view(), true).unwrap();
    eprintln!("dice: init {:?}, frame {:?}, {} rebuilt", t1 - t, t1.elapsed(), mask.iter().filter(|&&m| m).count());
    let wm: Array2<bool> = read_npy(dir.join("dice_mask.npy")).unwrap();
    let wo: Array3<u16> = read_npy(dir.join("dice_out.npy")).unwrap();
    assert_eq!(mask, wm);
    assert_eq!(out, wo);
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("dice.json")).unwrap()).unwrap();
    let (_, counts) = d.process(part.mapv(dice::to14).view(), irc.mapv(dice::to14).view(), true).unwrap();
    assert_eq!(serde_json::json!([counts.0, counts.1]), j["counts"]);
}
