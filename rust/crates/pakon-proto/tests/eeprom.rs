//! EEPROM decoder against the in-repo fixtures (port of test/test_eeprom_c.c;
//! the values match test/test_eeprom.py for the Python decoder).

use std::path::PathBuf;

use pakon_proto::eeprom::{self, as_refs, crc32, decode, Base, CopyId};

fn fixture(name: &str) -> [Option<Vec<u8>>; 4] {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../test/fixtures/eeprom").join(name);
    let u = eeprom::load_dir(&dir);
    assert!(u.iter().all(Option::is_some), "fixture {}", dir.display());
    u
}

fn fix_crc(u: &mut [Option<Vec<u8>>; 4], c: CopyId) {
    let b = u[c as usize].as_mut().unwrap();
    let crc = crc32(&b[8..]);
    b[4..8].copy_from_slice(&crc.to_le_bytes());
}

fn buf(u: &mut [Option<Vec<u8>>; 4], c: CopyId) -> &mut Vec<u8> {
    u[c as usize].as_mut().unwrap()
}

#[test]
fn crc_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn f135_2233() {
    let u = fixture("F135-2233");
    let e = decode(as_refs(&u));
    assert!(e.decodable);
    assert_eq!(e.valid, [true; 4]);
    assert_eq!(e.copies_equal, [Some(true); 2]);
    assert_eq!(e.selected, [Some(CopyId::APrimary), Some(CopyId::BPrimary)]);
    assert!(e.warnings.is_empty());
    assert_eq!((e.serial, e.scanner_type, e.model_name()), (2233, eeprom::TYPE_F135, "F-135"));
    let b4 = e.base(Base::B4);
    assert_eq!((b4.offset, b4.motor_speed, b4.motor_speed_ir), (27, 8162, 6119));
    let b16 = e.base(Base::B16);
    assert_eq!((b16.offset, b16.motor_speed, b16.motor_speed_ir), (54, 2325, 1530));
    assert_eq!(e.adjust(Base::B4).ir, 1008);
    assert_eq!(e.pos_matrix[0][0], 0.25);
    assert!((e.neg_matrix[0][0] - 0.27680).abs() < 1e-4);
}

#[test]
fn f135_plus_16402_backup_fallback() {
    let u = fixture("F135plus-16402");
    let e = decode(as_refs(&u));
    assert!(e.decodable);
    assert_eq!(e.valid, [false, true, true, true]);
    assert_eq!(e.selected[0], Some(CopyId::ABackup));
    assert_eq!(e.copies_equal[0], Some(false));
    assert_eq!((e.serial, e.model_name()), (16402, "F-135+"));
    // per-mode motor speeds seen in serial 16402's captures
    assert_eq!(e.base(Base::B16).motor_speed, 5900);
    assert_eq!(e.base(Base::B8).motor_speed, 11434);
    assert_eq!(e.base(Base::B4).motor_speed, 25726);
    assert_eq!(e.base(Base::B4).motor_speed_ir, 19278);
    assert_eq!(e.pos_matrix[0][1], 0.0);
}

#[test]
fn section_b_backup() {
    let mut u = fixture("F135-2233");
    buf(&mut u, CopyId::BPrimary).fill(0);
    let e = decode(as_refs(&u));
    assert!(e.decodable);
    assert_eq!(e.selected[1], Some(CopyId::BBackup));
}

#[test]
fn missing_backup() {
    let mut u = fixture("F135-2233");
    u[CopyId::BBackup as usize] = None;
    let e = decode(as_refs(&u));
    assert!(e.decodable);
    assert_eq!(e.copies_equal[1], None);
    assert!(!e.valid[CopyId::BBackup as usize]);
    assert_eq!(e.selected[1], Some(CopyId::BPrimary));
}

#[test]
fn both_copies_bad() {
    for s in 0..2 {
        let mut u = fixture("F135-2233");
        buf(&mut u, CopyId::ALL[2 * s]).fill(0xFF);
        buf(&mut u, CopyId::ALL[2 * s + 1]).fill(0xFF);
        let e = decode(as_refs(&u));
        assert!(!e.decodable);
        assert_eq!(e.selected[s], None);
        assert!(e.selected[1 - s].is_some());
        if s == 0 {
            assert_eq!(e.serial, 0);
        } else {
            assert_eq!(e.adjust[0].ir, 0);
        }
    }
}

#[test]
fn corruption() {
    let mut u = fixture("F135-2233");
    let a = u[0].clone().unwrap();
    let n = a.len();
    assert!(eeprom::copy_valid(CopyId::APrimary, &a));
    assert!(!eeprom::copy_valid(CopyId::APrimary, &a[..7]));
    assert!(!eeprom::copy_valid(CopyId::APrimary, &a[..n - 1]));
    let mut big = a.clone();
    big.push(0);
    assert!(!eeprom::copy_valid(CopyId::APrimary, &big));
    assert!(!eeprom::copy_valid(CopyId::BPrimary, &a));
    buf(&mut u, CopyId::APrimary)[n - 1] ^= 1;
    assert!(!eeprom::copy_valid(CopyId::APrimary, u[0].as_ref().unwrap()));
    buf(&mut u, CopyId::APrimary)[n - 1] ^= 1;
    buf(&mut u, CopyId::APrimary)[0] = 0xFF;
    assert!(!eeprom::copy_valid(CopyId::APrimary, u[0].as_ref().unwrap()));
}

#[test]
fn differing_valid_copies() {
    let mut u = fixture("F135-2233");
    buf(&mut u, CopyId::ABackup)[0x10] = 0xE7;
    fix_crc(&mut u, CopyId::ABackup);
    let e = decode(as_refs(&u));
    assert!(e.decodable && e.valid[CopyId::ABackup as usize]);
    assert_eq!(e.copies_equal[0], Some(false));
    assert_eq!(e.selected[0], Some(CopyId::APrimary));
    assert_eq!(e.serial, 2233);
    assert_eq!(e.warnings, ["section A: valid copies differ, primary selected"]);
}

#[test]
fn unknown_type_and_nan() {
    let mut u = fixture("F135-2233");
    let a = buf(&mut u, CopyId::APrimary);
    a[0x0C] = 0x0F;
    a[0x0D] = 0x27; // 9999
    a[0x26..0x2A].copy_from_slice(&[0x00, 0x00, 0xC0, 0x7F]); // NaN
    fix_crc(&mut u, CopyId::APrimary);
    let e = decode(as_refs(&u));
    assert!(!e.decodable);
    assert_eq!(e.scanner_type, 9999);
    assert!(e.neg_matrix[0][0].is_nan());
    assert_eq!(e.model_name(), "unknown");
}
