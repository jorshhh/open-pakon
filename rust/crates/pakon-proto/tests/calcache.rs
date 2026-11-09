//! Recalibration policy and the saved calibration (port of
//! test/test_calcache.c), plus compatibility with files the C driver wrote.

use std::path::PathBuf;

use pakon_proto::calcache::{self, needed, Action, Record};
use pakon_proto::lightcal::{Film, NCH};

const NOW: i64 = 1_800_000_000;

#[test]
fn policy() {
    assert_eq!(needed(NOW, 0, 0), Action::Full);
    assert_eq!(needed(NOW, NOW, NOW), Action::Reuse);
    assert_eq!(needed(NOW, NOW - 3600, NOW - 3600), Action::Reuse, "60 min exactly");
    assert_eq!(needed(NOW, NOW - 3601, NOW - 3601), Action::Refresh);
    assert_eq!(needed(NOW, NOW - 600, NOW - 72000), Action::Reuse);
    assert_eq!(needed(NOW, NOW - 600, NOW - 72001), Action::Full);
    assert_eq!(needed(NOW, NOW - 7200, NOW - 50000), Action::Refresh);
    assert_eq!(needed(NOW, NOW + 10, NOW - 100), Action::Full, "from the future");
    assert_eq!(Action::Refresh.name(), "refresh");
}

#[test]
fn paths() {
    let p = |film, base, ir| calcache::path("/d", film, base, ir).map(|p| p.to_string_lossy().into_owned());
    assert_eq!(p(Film::Bw, 16, false).as_deref(), Some("/d/calibration-bw-noir.bin"));
    assert_eq!(p(Film::Bw, 4, false).as_deref(), Some("/d/calibration-bw-base4-noir.bin"));
    assert_eq!(p(Film::Bw, 12, true), None);
    assert_eq!(p(Film::Bw, 16, true).as_deref(), Some("/d/calibration-bw.bin"));
    assert_eq!(p(Film::Bw, 8, true).as_deref(), Some("/d/calibration-bw-base8.bin"));
}

fn record(lp: usize) -> Record {
    Record {
        serial: 3054,
        offset: 43,
        film: Film::Bw,
        when: NOW - 5,
        when_full: NOW - 500,
        gain: [13, 14, 15],
        off: [-38, -31, -30],
        cur: [2, 3, 3, 2],
        duty: [896, 708, 278, 1353],
        line_px: lp,
        lines_dark: 128,
        lines_bright: 130,
        dark: (0..NCH * lp).map(|i| 300.5 + i as f64).collect(),
        bright: (0..NCH * lp).map(|i| 64000.25 - i as f64).collect(),
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pakon-calcache-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn round_trip() {
    const LP: usize = 37;
    let dir = tmpdir("rt");
    let path = calcache::path(&dir, Film::Bw, 8, true).unwrap();
    let mut rec = record(LP);
    rec.when = NOW - 50;
    rec.save(&path).unwrap();
    rec.when = NOW - 5;
    rec.save(&path).unwrap(); // replaces the file
    assert!(!path.with_extension("bin.tmp").exists());

    assert_eq!(Record::load(&path, 3054, 43, Film::Bw, LP).as_ref(), Some(&rec));
    assert!(Record::load(&path, 3055, 43, Film::Bw, LP).is_none(), "other serial");
    assert!(Record::load(&path, 3054, 44, Film::Bw, LP).is_none(), "other offset");
    assert!(Record::load(&path, 3054, 43, Film::C41, LP).is_none(), "other film");
    assert!(Record::load(&path, 3054, 43, Film::Bw, LP + 1).is_none(), "other line");
    let any = Record::load(&path, 3054, 43, Film::Bw, 0).unwrap();
    assert_eq!((any.line_px, any.dark[LP]), (LP, rec.dark[LP]));

    let len = std::fs::metadata(&path).unwrap().len();
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(len - 8).unwrap();
    assert!(Record::load(&path, 3054, 43, Film::Bw, LP).is_none(), "truncated");
    std::fs::remove_file(&path).unwrap();
    assert!(Record::load(&path, 3054, 43, Film::Bw, LP).is_none(), "missing");

    let mut bad = record(LP);
    bad.dark.clear();
    assert!(bad.save(&path).is_err(), "save without tables refused");
    assert!(!path.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// tests/fixtures/calibration-c.bin was written by the C driver's
/// pakon_cal_save (slide, line_px 5): Rust reads it and writes the same bytes.
#[test]
fn c_driver_compat() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/calibration-c.bin");
    let rec = Record::load(&fixture, 3054, 43, Film::Slide, 5).expect("C record loads");
    assert_eq!((rec.when, rec.when_full), (1_800_000_000, 1_799_990_000));
    assert_eq!((rec.gain, rec.off), ([13, 14, 15], [-38, -31, -30]));
    assert_eq!((rec.cur, rec.duty), ([2, 3, 3, 2], [896, 708, 278, 1353]));
    assert_eq!((rec.lines_dark, rec.lines_bright), (128, 130));
    assert_eq!(rec.dark[19], 319.5);
    assert_eq!(rec.bright[0], 64000.25);

    let dir = tmpdir("compat");
    let out = dir.join("out.bin");
    rec.save(&out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&fixture).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}
