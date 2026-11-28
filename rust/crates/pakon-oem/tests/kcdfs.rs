//! Smoke test for PSI 2.4 Kodak KCDFS scene balance and SCP LUT generation.

use std::path::PathBuf;

use ndarray::Array3;
use pakon_oem::kcdfs::{default_dll, kcdfs_dir, roll_balance};
use pakon_oem::sba::{ANALYSIS_H, ANALYSIS_W};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn kcdfs_loads_and_balances_synthetic_roll() {
    let root = repo();
    let dll = default_dll(&root);
    if !dll.is_file() {
        eprintln!("skipped: no {}", dll.display());
        return;
    }
    let dir = kcdfs_dir(&root);

    let frame_offsets: [i16; 5] = [-80, -30, 0, 40, 90];
    let frames: Vec<Array3<i16>> = frame_offsets
        .iter()
        .map(|&ev| {
            let mut img = Array3::<i16>::zeros((ANALYSIS_H, ANALYSIS_W, 3));
            for y in 0..ANALYSIS_H {
                for x in 0..ANALYSIS_W {
                    let delta = ((x as i16) - 24) * 12 + ((y as i16) - 16) * 8 + ev;
                    img[(y, x, 0)] = (600 + delta).clamp(200, 3800);
                    img[(y, x, 1)] = (1050 + delta).clamp(200, 3800);
                    img[(y, x, 2)] = (1300 + delta).clamp(200, 3800);
                }
            }
            img
        })
        .collect();

    let dmin = Some([350, 750, 1000]);

    let res_no_scp = roll_balance(&dll, &dir, &frames, dmin, false).expect("roll_balance(do_scp_pass=false)");
    assert_eq!(res_no_scp.shifts.len(), frames.len());
    assert!(res_no_scp.scp_luts.is_none());
    for (i, s) in res_no_scp.shifts.iter().enumerate() {
        for &v in s {
            assert!(
                v.is_finite() && (-2000.0..2000.0).contains(&v),
                "no-scp shift[{i}] out of range: {s:?}"
            );
        }
    }
    eprintln!("KCDFS shifts (do_scp_pass=false): {:?}", res_no_scp.shifts);

    let res_scp = roll_balance(&dll, &dir, &frames, dmin, true).expect("roll_balance(do_scp_pass=true)");
    assert_eq!(res_scp.shifts.len(), frames.len());
    for (i, s) in res_scp.shifts.iter().enumerate() {
        for &v in s {
            assert!(
                v.is_finite() && (-2000.0..2000.0).contains(&v),
                "scp shift[{i}] out of range: {s:?}"
            );
        }
    }
    eprintln!("KCDFS shifts (do_scp_pass=true):  {:?}", res_scp.shifts);

    let luts = res_scp.scp_luts.expect("scp_luts should be Some when do_scp_pass=true");
    let sample_idx = [0usize, 512, 1024, 2048, 3072, 4095];
    for (ch, name) in [(0, "R"), (1, "G"), (2, "B")] {
        for &v in luts[ch].iter() {
            assert!(v <= 4095, "SCP LUT channel {name} entry {v} > 4095");
        }
        let samples: Vec<(usize, u16)> = sample_idx.iter().map(|&idx| (idx, luts[ch][idx])).collect();
        eprintln!("SCP LUT {name} samples: {samples:?}");
    }

    // Pinned: this synthetic roll's shifts on the real DLL (regression cover for the portrait
    // thumbnail layout and the frame-dmin wiring; a wrong stride or dmin moves these).
    let want: [[f32; 3]; 5] = [
        [-243.0, -693.0, -942.0],
        [-292.0, -743.0, -992.0],
        [-325.0, -775.0, -1024.0],
        [-371.0, -821.0, -1070.0],
        [-426.0, -876.0, -1125.0],
    ];
    assert_eq!(res_scp.shifts, want.to_vec());

    // A portrait thumbnail (rows >= cols) is the landscape one rotated 90 deg CCW, and must
    // give the same shifts as the landscape input.
    let portrait: Vec<Array3<i16>> = frames
        .iter()
        .map(|l| Array3::from_shape_fn((ANALYSIS_W, ANALYSIS_H, 3), |(yp, xp, c)| l[(xp, ANALYSIS_W - 1 - yp, c)]))
        .collect();
    let res_p = roll_balance(&dll, &dir, &portrait, dmin, true).expect("portrait roll_balance");
    assert_eq!(res_p.shifts, res_scp.shifts, "portrait and landscape layouts must agree");

    // Verify single-frame balance_against(roll) matches full roll_balance.
    let single = pakon_oem::kcdfs::roll_balance_against(&dll, &dir, &frames[2..3], Some(&frames), dmin, true).unwrap();
    assert_eq!(single.shifts[0], res_scp.shifts[2], "single-frame balance_against should match full roll");
}
