//! The DX sensor block (port of test/test_dx.c). Fixtures in
//! test/fixtures/dx are every 0x90 reply payload in three OEM captures from
//! serial 3054.

use std::path::PathBuf;

use pakon_proto::dx::{self, frame_label, Roll};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

const FIVE: &str = "73a9053301045418050153dbcc030108541805016031cc03010d54180000";

#[test]
fn blocks() {
    let b = dx::parse(&unhex(FIVE)).unwrap();
    assert_eq!((b.position, b.entries.len()), (0x73a9, 5));
    assert_eq!((b.entries[0].ty, b.entries[0].hi, b.entries[1].ty), (3, 3, 5));
    assert_eq!(b.entries[1].position(), 0x53db);
    let c = b.entries[0].code().unwrap();
    assert_eq!((c.frame_raw, c.product, c.generation, c.slot, c.valid), (1, 97, 10, 1, true));
    assert!(b.entries[1].code().is_none());

    let mut b = dx::parse(&unhex("37b205b5021343cc030231861405021e9ecc03022c861405022b5bcc0000")).unwrap();
    let c = b.entries[1].code().unwrap();
    assert_eq!((c.frame_raw, c.product, c.generation, c.slot, c.valid), (12, 82, 3, 2, true));
    b.entries[1].b[2] ^= 0x01;
    let c = b.entries[1].code().unwrap();
    assert!(!c.parity_ok && !c.valid, "flipped bit fails parity");
    b.entries[1].b[2] ^= 0x01;
    b.entries[1].b[1] ^= 0x20;
    b.entries[1].b[0] ^= 0x01;
    let c = b.entries[1].code().unwrap();
    assert!(c.parity_ok && !c.valid, "invalid bit rejects");

    let b = dx::parse(&unhex("be7401000000000000000000000000000000000000000000000000000000")).unwrap();
    assert_eq!(b.entries.len(), 1);
    assert_eq!(b.entries[0].ty, dx::NONE);
    let b = dx::parse(&unhex("08ff01370208f7cc00000000000000000000000000000000000000000000")).unwrap();
    assert_eq!((b.entries[0].ty, b.entries[0].flag, b.entries[0].position()), (dx::EDGE_A, 2, 0x08f7));
    let six = unhex("00ff06000000000000000000000000000000000000000000000000000000");
    assert!(dx::parse(&six).is_none());
    assert!(dx::parse(&six[..29]).is_none());
}

#[test]
fn labels() {
    assert_eq!(frame_label(73), "36A");
    assert_eq!(frame_label(2), "1");
    assert_eq!(frame_label(126), "00");
    assert_eq!(frame_label(124), "X");
}

#[test]
fn rolls() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../test/fixtures/dx");
    for (name, reads, codes, product, generation, first, last) in [
        ("36frames", 38, 69, 97, 10, 1, 73),
        ("pakon_fullroll", 25, 45, 96, 12, 1, 49),
        ("pakon_scan", 9, 5, 82, 3, 12, 8),
    ] {
        let text = std::fs::read_to_string(dir.join(format!("{name}.hex"))).unwrap();
        let mut r = Roll::new();
        let mut n = 0;
        for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            r.add(&dx::parse(&unhex(&line[..60])).unwrap());
            n += 1;
        }
        assert_eq!(n, reads, "{name} reads");
        assert_eq!((r.codes, r.rejected, r.films.len()), (codes, 0, 1), "{name}");
        assert_eq!(r.film(), Some((product, generation)), "{name}");
        assert_eq!((r.code[0].frame_raw, r.code.last().unwrap().frame_raw), (first, last), "{name}");
        assert!(r.code.iter().any(|c| c.position.is_some()), "{name}: some code follows a type 5");
        println!("{name}: {r}");
    }
}

#[test]
fn code_positions() {
    let mut r = Roll::new();
    r.add(&dx::parse(&unhex(FIVE)).unwrap());
    let pos: Vec<_> = r.code.iter().map(|c| c.position).collect();
    assert_eq!(pos, [None, Some(0x53db), Some(0x6031)]);
    // log lines as the C formatter prints them
    assert_eq!(r.to_string(), "film 97/10, 3 codes (0 rejected), frames 0A..1A, 1 sensor reads");
    assert_eq!(Roll::new().to_string(), "no code read (0 sensor reads, 0 entries, 0 rejected)");
    assert_eq!(
        dx::parse(&unhex(FIVE)).unwrap().to_string(),
        "pos 0x73a9, 5 entries; code 0A 97/10 slot 1; type 5 flag 0x01 pos 0x53db; \
         code 1 97/10 slot 1; type 5 flag 0x01 pos 0x6031; code 1A 97/10 slot 1"
    );
}
