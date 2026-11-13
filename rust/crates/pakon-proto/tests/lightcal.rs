//! Line statistics and calibration steps (port of test/test_lightcal.c).
//!
//! Duty values come from resources/scan.pakscan's calibration (serial 3054,
//! period 0x742); our formulas land within 3 ticks of every captured value.

use pakon_proto::lightcal::{self as lc, Film, B, G, IR, NCH, R};

fn near(a: i64, b: i64, tol: i64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn stats_synthetic() {
    // 1 lead sample, an IR block, then lines of RGB + IR.
    // Pixel p of line l: R = 1000 + p, G = 2000 + p, B = 3000 + l, IR = 400.
    const PX: usize = 50;
    const LINES: usize = 5;
    let n = 1 + PX + LINES * 4 * PX;
    let mut s = vec![0u16; n];
    let o = 1 + PX;
    for l in 0..LINES {
        let at = o + l * 4 * PX;
        for p in 0..PX {
            s[at + 3 * p] = (1000 + p) as u16;
            s[at + 3 * p + 1] = (2000 + p) as u16;
            s[at + 3 * p + 2] = (3000 + l) as u16;
            s[at + 3 * PX + p] = 400;
        }
    }
    let st = lc::stats_range(&s, PX, true, o, 10, 20).unwrap();
    assert_eq!(st.lines, LINES);
    assert_eq!((st.mean[R], st.peak[R]), (1014.5, 1019));
    assert_eq!(st.mean[G], 2014.5);
    assert_eq!(st.mean[B], 3002.0);
    assert_eq!(st.mean[IR], 400.0);
    assert!(lc::stats_range(&s, PX, true, o, 20, 20).is_none());
    assert!(lc::stats_range(&s[..10], PX, true, o, 0, 5).is_none());
}

#[test]
fn origin_search() {
    const PX: usize = 2037;
    const L: usize = 4 * PX;
    const LINES: usize = 8;
    const START: usize = 1234;
    let mut line: Vec<u16> = (0..L)
        .map(|i| {
            let ir = i >= 3 * PX;
            let p = if ir { i - 3 * PX } else { i / 3 };
            if p < 20 { 300 } else if ir { 40000 } else { 60000 }
        })
        .collect();
    let stream = |line: &[u16]| (0..LINES * L).map(|i| line[(START + i) % L]).collect::<Vec<_>>();
    assert_eq!(lc::find_origin(&stream(&line), PX, 20, 3000.0), Some(L - START));
    for (i, v) in line.iter_mut().enumerate() {
        *v = if i >= 3 * PX && i - 3 * PX >= 20 { 40000 } else { 300 };
    }
    assert_eq!(lc::find_origin(&stream(&line), PX, 20, 3000.0), Some(L - START), "IR block alone");
    assert_eq!(lc::find_origin(&vec![300; LINES * L], PX, 20, 3000.0), None, "no light");
}

#[test]
fn line_width() {
    let pxs = [2037usize, 1528, 1027]; // Base 16, 8, 4 shapes
    let win = [2037usize, 2037, 1027]; // register window
    // k = 3..5: the same widths with only the IR LED lit
    for k in 0..6 {
        let px = pxs[k % 3];
        let l_len = 4 * px;
        let lines = 400000 / l_len;
        let line: Vec<u16> = (0..l_len)
            .map(|i| {
                let ir = i >= 3 * px;
                let p = if ir { i - 3 * px } else { i / 3 };
                let lit = if p < 15 || (k >= 3 && !ir) {
                    300.0
                } else {
                    300.0 + 600.0 * p.min(100) as f64 * if ir { 0.6 } else { 1.0 } + 20.0 * (p % 37) as f64
                };
                lit as u16
            })
            .collect();
        let mut seed: u32 = 12345;
        let s: Vec<u16> = (0..lines * l_len)
            .map(|i| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                (line[(777 + i) % l_len] as u32 + (seed >> 16) % 101) as u16
            })
            .collect();
        let got = lc::find_line_px(&s, win[k % 3] * 5 / 8, win[k % 3] + 8, true);
        assert_eq!(got, Some(px), "width {px} k {k}");
    }
    assert_eq!(lc::find_line_px(&[0; 100], 10, 20, true), None);
}

#[test]
fn ir_off_marker() {
    const PX: usize = 1527;
    const LINES: usize = 60;
    const START: usize = 2345;
    let l_len = 3 * PX;
    let mut line: Vec<u16> = (0..l_len)
        .map(|i| {
            let p = i / 3;
            (if p < 35 { 300 } else { 60000 + 40 * (p % 37) }) as u16 & !1
        })
        .collect();
    line[0] |= 1;
    let mut s: Vec<u16> = (0..LINES * l_len).map(|i| line[(START + i) % l_len]).collect();
    let o = lc::marker_origin(&s, l_len).unwrap();
    assert_eq!(o, l_len - START);
    assert_eq!(lc::find_line_px(&s, 2036 * 5 / 8, 2036 + 8, false), Some(PX));
    let st = lc::stats_range(&s, PX, false, o, 0, 20).unwrap();
    assert_eq!(st.lines, LINES - 1);
    assert!((st.mean[0] - 300.0).abs() < 0.1);
    assert_eq!((st.mean[1], st.mean[IR], st.peak[IR]), (300.0, 0.0, 0));
    let mut sum = vec![0f64; NCH * PX];
    let mut lines = 0;
    assert_eq!(lc::column_accum(&s, PX, false, o, &mut sum, &mut lines), LINES - 1);
    assert_eq!(sum[PX + 100], (LINES - 1) as f64 * line[3 * 100 + 1] as f64);
    assert_eq!(sum[3 * PX + 100], 0.0);

    for (i, v) in s.iter_mut().enumerate() {
        *v = if (START + i).is_multiple_of(l_len) { 301 } else { 300 }; // dark
    }
    assert_eq!(lc::marker_origin(&s, l_len), Some(l_len - START));
    assert_eq!(lc::marker_line_len(&s), Some(l_len));
    s.fill(300);
    assert_eq!(lc::marker_origin(&s, l_len), None);
    assert_eq!(lc::marker_line_len(&s), None);
}

#[test]
fn ir_on_short_black_ir_block() {
    // the shape behind serial 1101's failure: 18 black IR pixels, not 20
    const PX: usize = 2052;
    const L: usize = 4 * PX;
    const LINES: usize = 40;
    const START: usize = 1234;
    let dark = [272u16, 284, 286];
    let mut line: Vec<u16> = (0..L)
        .map(|i| if i < 3 * PX { dark[i % 3] } else if i - 3 * PX < 18 { 300 } else { 9000 })
        .collect();
    line[0] |= 1;
    let mut s: Vec<u16> = (0..LINES * L).map(|i| line[(START + i) % L]).collect();
    let edge = lc::find_origin(&s, PX, 20, 3000.0).unwrap();
    let o = lc::origin(&s, PX, 20, true, 3000.0).unwrap();
    assert!(edge < L - START && o == L - START, "edge {edge}");
    let st = lc::stats_range(&s, PX, true, o, 0, 20).unwrap();
    assert!(lc::dark_ok(st.mean[0]) && lc::dark_ok(st.mean[1]) && lc::dark_ok(st.mean[2]));
    let st = lc::stats_range(&s, PX, true, edge, 0, 20).unwrap();
    assert!(!lc::dark_ok(st.mean[0]));
    for v in &mut s {
        *v &= !1;
    }
    assert_eq!(lc::origin(&s, PX, 20, true, 3000.0), Some(edge), "falls back to contrast");
    assert_eq!(lc::origin(&s, PX, 20, false, 3000.0), None);
}

#[test]
fn dark_offset() {
    assert_eq!(lc::offset_next(-38, 300.0), -38);
    assert_eq!(lc::offset_next(10, 3884.0), -38);
    assert_eq!(lc::offset_next(-250, 0.0), -246);
    assert_eq!(lc::offset_next(-250, 60000.0), -255);
    assert!(lc::dark_ok(332.0) && lc::dark_ok(268.0) && !lc::dark_ok(333.0));
    assert_eq!(lc::offset_next(-38, 334.0), -39, "2026-10-09 stall");
    assert_eq!(lc::offset_next(-38, 262.0), -37);
    assert_eq!(lc::offset_next(-38, 320.0), -38);
}

#[test]
fn caps_and_windows() {
    assert!(!lc::peak_over_cap(G, 64000) && lc::peak_over_cap(G, 64001));
    assert!(!lc::peak_over_cap(B, 65500) && lc::peak_over_cap(IR, 40001));
    assert!(lc::peak_ok(R, 63936) && lc::peak_ok(R, 64000));
    assert!(!lc::peak_ok(R, 63935) && !lc::peak_ok(R, 64001));
    assert!(lc::peak_ok(IR, 39936) && !lc::peak_ok(IR, 40001));
}

#[test]
fn duties_vs_capture() {
    let period = 0x742;
    let cap_cap = [1335, 741, 359, 1856];
    let cur = [2, 3, 3, 2];
    let start_cap = [668, 494, 239, 930];
    let conv = [896u16, 708, 278, 1353];
    let scan_cap = [1247, 1775, 1442, 1352];
    for c in 0..4 {
        let d = lc::DENSITY_C41[c];
        let cap = lc::duty_cap(period, d);
        assert!(near(cap as i64, cap_cap[c], 3), "ch {c} cap {cap}");
        let st = lc::duty_start(cap_cap[c] as u16, cur[c]);
        assert!(near(st as i64, start_cap[c], 3), "ch {c} start {st}");
        let sc = lc::scan_duty(conv[c], d, period);
        assert!(near(sc as i64, scan_cap[c], 3), "ch {c} scan {sc}");
    }
    assert_eq!(lc::duty_next(B, 239, 53830, period), 284);
    assert_eq!(lc::duty_next(IR, 1000, 39968, period), 1000);
    assert_eq!(lc::duty_next(G, 1800, 30000, period), period - 2);
    assert_eq!(lc::duty_next(G, 100, 0, period), period - 2);
    assert!(lc::duty_settled(B, 273, 63958, period));
    assert!(lc::duty_settled(B, 273, 63899, period));
    assert!(!lc::duty_settled(B, 239, 54966, period));
    assert!(!lc::duty_settled(G, period - 2, 50000, period));
}

#[test]
fn column_sums() {
    // Pixel p of line l: R = 100 + p, G = 200 + l, B = 300, IR = 50 + p.
    const PX: usize = 30;
    const LINES: usize = 3;
    let n = 7 + LINES * 4 * PX + 5;
    let mut s = vec![0u16; n];
    for l in 0..LINES {
        let at = 7 + l * 4 * PX;
        for p in 0..PX {
            s[at + 3 * p] = (100 + p) as u16;
            s[at + 3 * p + 1] = (200 + l) as u16;
            s[at + 3 * p + 2] = 300;
            s[at + 3 * PX + p] = (50 + p) as u16;
        }
    }
    let mut sum = vec![0f64; NCH * PX];
    let mut lines = 0;
    let a = lc::column_accum(&s, PX, true, 7, &mut sum, &mut lines);
    let b = lc::column_accum(&s, PX, true, 7, &mut sum, &mut lines);
    assert_eq!((a, b, lines), (LINES, LINES, 2 * LINES));
    assert_eq!(sum[4], 2.0 * LINES as f64 * 104.0);
    assert_eq!(sum[PX + 9], 2.0 * (200.0 + 201.0 + 202.0));
    assert_eq!(sum[3 * PX + 29], 2.0 * LINES as f64 * 79.0);
    assert_eq!(lc::column_accum(&s[..10], PX, true, 7, &mut sum, &mut lines), 0);
    assert_eq!(lines, 2 * LINES);
}

#[test]
fn gain_and_smear() {
    assert_eq!(lc::flat_gain(64300.0, 300.0, 300.0, 300.0), 1.0);
    assert_eq!(lc::flat_gain(32300.0, 300.0, 300.0, 300.0), 2.0);
    assert_eq!(lc::flat_gain(32300.0, 300.0, 310.0, 300.0), 64000.0 / 31990.0);
    assert_eq!(lc::flat_gain(10300.0, 300.0, 300.0, 300.0), 0x3ffff as f64 / 65536.0);
    assert_eq!(lc::flat_gain(300.0, 300.0, 300.0, 300.0), 0.0);
    assert_eq!(lc::smear(310.0, 300.0, 64300.0, 300.0), 10);
    assert_eq!(lc::smear(300.0, 300.0, 64300.0, 300.0), 0);
    assert_eq!(lc::smear(1300.0, 300.0, 64300.0, 300.0), 0);
}

#[test]
fn films() {
    assert_eq!(Film::C41.density(), &lc::DENSITY_C41);
    assert_eq!(Film::Bw.density(), &lc::DENSITY_OTHER);
    assert_eq!(Film::Slide.density(), &lc::DENSITY_SLIDE);
    assert_eq!(Film::BwC41.density(), &[0.10, 0.25, 0.25, 0.08]);
    assert_eq!(Film::ALL.map(Film::name), ["c41", "bw", "slide", "bwc41"]);
    for f in Film::ALL {
        assert_eq!(f.name().parse::<Film>(), Ok(f));
        assert_eq!(Film::from_u32(f as u32), Some(f));
    }
    assert!("e6".parse::<Film>().is_err());

    let period = 0x742;
    let bw_cap = [1856u16, 1732, 1856, 1544];
    let bw_scan = [896u16, 759, 278, 1627];
    let open = [896u16, 708, 278, 1353];
    for c in 0..4 {
        assert_eq!(lc::duty_cap(period, lc::DENSITY_OTHER[c]), bw_cap[c]);
        assert_eq!(lc::scan_duty(open[c], lc::DENSITY_OTHER[c], period), bw_scan[c]);
    }
    let cap = [1408u16, 1408, 1408, 1544];
    let open = [1400u16, 1100, 900, 1372];
    let scan = [1846u16, 1450, 1186, 1650];
    for c in 0..4 {
        assert_eq!(lc::duty_cap(period, lc::DENSITY_SLIDE[c]), cap[c]);
        assert_eq!(lc::scan_duty(open[c], lc::DENSITY_SLIDE[c], period), scan[c]);
        assert!(scan[c] <= period - 2);
    }
}
