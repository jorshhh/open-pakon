//! Generated init, configure, teardown and event frames vs the captures
//! (port of test/test_setup.c).
//!
//! For each capture, takes the frames from the SCN 0x97 controller init up to
//! the first acquire (bank 0x82 reg 0 with bit 0 set), drops the traffic
//! `setup` leaves out, and requires the generated sequence to match byte for
//! byte. Same for the teardown after the last image read.

use std::path::PathBuf;

use pakon_proto::script::{self, Line};
use pakon_proto::setup::{self, LedValues, Model, ScanMode, Sensor, Seq, SetupError, SetupState};
use pakon_proto::{Address, Frame, PacketType};

const LOW: u8 = Address::PicL as u8;
const SCN: u8 = Address::PicM as u8;
const LOW_P: u8 = Address::PicLPlus as u8;
const SCN_P: u8 = Address::PicMPlus as u8;

fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../resources")
}

fn load(name: &str) -> Vec<Line> {
    script::parse_file(resources().join(name)).unwrap()
}

/// Event-service frames that are not part of the sequence.
fn left_out(b: &[u8], low: u8, scn: u8) -> bool {
    if b.len() == 3 && b[0] == 3 && b[2] == Address::Host as u8 {
        return true; // host poll
    }
    b.len() == 5
        && b[0] == 1
        && (b[2] == low || b[2] == scn)
        && matches!((b[3], b[4]), (1, 0x02) | (0x1E, 0x90) | (1, 0x83) | (2, 0x84) | (4, 0x88))
}

/// Left-out writes whose busy poll goes too: the event ack and the
/// status-LED word 0x0313.
fn left_out_write(b: &[u8], scn: u8) -> bool {
    if b.len() == 7 && b[0] == 2 && b[3] == 2 && b[4] == 0x06 {
        return true;
    }
    b.len() == 8 && b[0] == 2 && b[2] == scn && b[4] == 0x82 && b[5] == 0x09 && b[6] == 0x13 && b[7] == 0x03
}

fn is_acquire_on(b: &[u8], scn: u8) -> bool {
    b.len() == 8 && b[0] == 2 && b[2] == scn && b[4] == 0x82 && b[5] == 0x00 && b[6] & 1 != 0
}

/// The comparable setup window of a capture.
fn load_window(lines: &[Line], low: u8, scn: u8) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let (mut started, mut after_ack) = (false, false);
    for l in lines {
        let b = match l {
            Line::Image(_) if started => break,
            Line::Out(b) => b,
            _ => continue,
        };
        if !started {
            if b.len() == 6 && b[0] == 2 && b[2] == scn && b[4] == 0x97 {
                started = true;
            } else {
                continue;
            }
        }
        if is_acquire_on(b, scn) {
            break;
        }
        if left_out_write(b, scn) {
            after_ack = true;
            continue;
        }
        if after_ack && b.len() == 3 && b[0] == 3 {
            after_ack = false;
            continue;
        }
        after_ack = false;
        if !left_out(b, low, scn) {
            out.push(b.clone());
        }
    }
    out
}

/// Teardown window: after the last image read, from the reg 0 write that
/// clears acquire through the busy poll after SCN 0xA2.
fn load_teardown(lines: &[Line], low: u8, scn: u8) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let (mut started, mut skip_poll, mut done) = (false, false, false);
    for l in lines {
        let b = match l {
            Line::Image(_) => {
                out.clear();
                started = false;
                done = false;
                continue;
            }
            Line::Out(b) if !done => b,
            _ => continue,
        };
        let fpga = b.len() == 8 && b[0] == 2 && b[2] == scn && b[4] == 0x82;
        if !started {
            if fpga && b[5] == 0x00 && b[6] & 1 == 0 {
                started = true;
            } else {
                continue;
            }
        }
        if left_out_write(b, scn) || (fpga && b[5] == 0x09) {
            skip_poll = true;
            continue;
        }
        if skip_poll && b.len() == 3 && b[0] == 3 {
            skip_poll = false;
            continue;
        }
        skip_poll = false;
        if left_out(b, low, scn) {
            continue;
        }
        out.push(b.clone());
        let n = out.len();
        if b.len() == 3 && b[0] == 3 && b[2] == scn && n >= 2 {
            let p = &out[n - 2];
            if p.len() == 5 && p[0] == 4 && p[4] == 0xA2 {
                done = true;
            }
        }
    }
    if done { out } else { Vec::new() }
}

fn wire(seq: &Seq) -> Vec<Vec<u8>> {
    seq.frames().iter().map(Frame::to_bytes).collect()
}

fn assert_matches(label: &str, seq: &Seq, cap: &[Vec<u8>]) {
    assert!(!cap.is_empty(), "{label}: empty capture window");
    let got = wire(seq);
    if let Some(i) = got.iter().zip(cap).position(|(a, b)| a != b) {
        panic!("{label}: first difference at frame {i}: generated {:02x?}, captured {:02x?}", got[i], cap[i]);
    }
    assert_eq!(got.len(), cap.len(), "{label}: frame count");
}

/// Hex of the non-poll frames, space-separated.
fn seq_hex(seq: &Seq) -> String {
    seq.frames()
        .iter()
        .filter(|f| f.packet_type() != Some(PacketType::ReadStatus))
        .map(|f| f.to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn frame_hex(f: &Frame) -> String {
    f.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn check_capture(file: &str, low: u8, scn: u8, period: u16, mode: &ScanMode, has_setup: bool) {
    let lines = load(file);
    let mut seq = Seq::new();
    let mut st = setup::init(&mut seq, low, scn, period);
    setup::configure(&mut seq, &mut st, mode).unwrap();
    seq.check_safe().unwrap();
    if has_setup {
        assert_matches(&format!("{file} setup"), &seq, &load_window(&lines, low, scn));
    }

    let mut seq = Seq::new();
    setup::teardown(&mut seq, &st);
    // 20 ms between LOW 0x92 and SCN 0xA2 (FN_bAfterScan).
    assert_eq!(seq.delay_ms(seq.len() - 2), 20, "{file} teardown delay");
    assert_matches(&format!("{file} teardown"), &seq, &load_teardown(&lines, low, scn));
}

fn f135_state() -> SetupState {
    let mut seq = Seq::new();
    let mut st = setup::init(&mut seq, LOW, SCN, setup::LED_PERIOD_F135);
    st.reg0 = 0x0160; // after Base 16 + IR configure
    st
}

#[test]
fn delays_default_zero() {
    let mut seq = Seq::new();
    setup::init(&mut seq, LOW, SCN, setup::LED_PERIOD_F135);
    assert!((0..seq.len()).all(|i| seq.delay_ms(i) == 0));
}

#[test]
fn pixel_window() {
    // serial 16402's offsets 60 / 30 give the captured reg 4/5
    assert_eq!(setup::pixel_window(60, false), (6, 0x080C));
    assert_eq!(setup::pixel_window(30, true), (3, 0x0406));
}

#[test]
fn f135_captures() {
    let m = setup::scan_mode(Model::F135, 16, true, 43).unwrap();
    assert!(m.trigger == 0x0010 && m.integration == 0x0C1A && m.ir && !m.dual_tap && m.low89 == 0);
    let p = setup::LED_PERIOD_F135;
    check_capture("scan.pakscan", LOW, SCN, p, &m, true);
    check_capture("scan_fullroll.pakscan", LOW, SCN, p, &m, false); // starts mid-session
    check_capture("36frames.pakscan", LOW, SCN, p, &m, true);
}

#[test]
fn f135_plus_captures() {
    let p = setup::LED_PERIOD_F135_PLUS;
    for (file, base, ir, off) in [
        ("f135plus/base16.pakscan", 16, false, 60),
        ("f135plus/base8.pakscan", 8, false, 58),
        ("f135plus/base4.pakscan", 4, false, 30),
        ("f135plus/base4_ir.pakscan", 4, true, 30),
    ] {
        let m = setup::scan_mode(Model::F135Plus, base, ir, off).unwrap();
        check_capture(file, LOW_P, SCN_P, p, &m, true);
    }
}

#[test]
fn uncaptured_modes() {
    let m = setup::scan_mode(Model::F135, 8, true, 41).unwrap();
    assert!(m.trigger == 28 && m.integration == 0x0913 && m.low89 == 1 && !m.dual_tap);
    assert_eq!((m.pixel_start, m.pixel_end), (6, 41 + 2000));
    let m = setup::scan_mode(Model::F135, 4, true, 21).unwrap();
    assert!(m.trigger == 64 && m.integration == 0x060D && m.dual_tap && m.low89 == 0);
    assert_eq!((m.pixel_start, m.pixel_end), (3, 21 + 1000));
    let m = setup::scan_mode(Model::F135, 16, false, 43).unwrap();
    assert!(m.trigger == 24 && m.integration == 0x0FF0 && !m.ir && !m.dual_tap && m.low89 == 0);
    assert_eq!((m.pixel_start, m.pixel_end), (6, 43 + 2000));
    let m = setup::scan_mode(Model::F135, 8, false, 42).unwrap();
    assert!(m.trigger == 38 && m.integration == 0x0D9D && !m.ir && m.low89 == 1);
    let m = setup::scan_mode(Model::F135, 4, false, 21).unwrap();
    assert!(m.trigger == 85 && m.integration == 0x0913 && !m.ir && m.dual_tap);
    let m = setup::scan_mode(Model::F135Plus, 16, true, 60).unwrap();
    assert!(m.trigger == 0x0031 && m.integration == 0x09C2 && m.ir && !m.dual_tap && m.low89 == 0);
    let m = setup::scan_mode(Model::F135Plus, 8, true, 58).unwrap();
    assert!(m.trigger == 0x004D && m.integration == 0x0850 && m.ir && m.low89 == 1);
}

#[test]
fn triggers_follow_formula() {
    for model in [Model::F135, Model::F135Plus] {
        for base in [16, 8, 4] {
            for ir in [false, true] {
                let m = setup::scan_mode(model, base, ir, 40).unwrap();
                let clk = 833333.3 * if model == Model::F135Plus { 2.5 } else { 1.0 };
                let v = 23700.0 * clk * if ir { 0.005 } else { 0.01 }
                    / (m.integration as f64 * setup::scan_px(base) as f64);
                assert_eq!(m.trigger, (v + 0.5) as u16, "{model:?} base {base} ir {ir}: formula {v}");
            }
        }
    }
}

#[test]
fn unknown_modes_refused() {
    assert_eq!(setup::scan_mode(Model::F135Plus, 12, true, 60), Err(SetupError::UnknownMode { base: 12 }));
    assert!(setup::scan_mode(Model::F135, 12, true, 43).is_err());
}

#[test]
fn period_and_px() {
    assert_eq!(setup::led_period(0x0C1A), 0x742);
    assert_eq!(setup::led_period(0x0913), 1393);
    assert_eq!(setup::led_period(0x060D), 929);
    assert_eq!([16, 8, 4, 2].map(setup::scan_px), [2000, 1500, 1000, 0]);
}

#[test]
fn event_frames() {
    let f = setup::event_read_frame(LOW);
    assert_eq!(f.to_bytes(), [1, 3, 0x20, 1, 2]);
    let pending = Frame::parse(&[0x01, 0x03, 0x20, 0x80, 0x20]).unwrap();
    let idle = Frame::parse(&[0x01, 0x03, 0x20, 0x00, 0x00]).unwrap();
    assert_eq!(setup::event_parse(&pending, LOW), Some(0x20));
    assert_eq!(setup::event_parse(&idle, LOW), None);
    assert_eq!(setup::event_parse(&idle, SCN), None);

    // lamp gate reads, as the OEM sends them in calib_prelude.pakscan
    assert_eq!(frame_hex(&setup::lamp_flags_frame(LOW)), "0103200183");
    assert_eq!(frame_hex(&setup::lamp_temps_frame(LOW)), "0103200488");
    let prelude = load("calib_prelude.pakscan");
    for want in [setup::lamp_flags_frame(LOW), setup::lamp_temps_frame(LOW)] {
        assert!(prelude.contains(&Line::Out(want.to_bytes())));
    }

    let flags = Frame::parse(&[0x01, 0x03, 0x20, 0x00, 0x02]).unwrap();
    assert_eq!(setup::lamp_flags_parse(&flags, LOW), Some(setup::LAMP_STABLE));
    assert_eq!(setup::lamp_flags_parse(&flags, SCN), None);

    // 641/16 = 40.0625 C lamp, 461/16 = 28.8125 C board; -24/16 = -1.5
    let temps = Frame::parse(&[0x01, 0x06, 0x20, 0x00, 0x81, 0x02, 0xCD, 0x01]).unwrap();
    assert_eq!(setup::lamp_temps_parse(&temps, LOW), Some((40.0625, 28.8125)));
    let neg = Frame::parse(&[0x01, 0x06, 0x20, 0x00, 0xE8, 0xFF, 0x00, 0x00]).unwrap();
    assert_eq!(setup::lamp_temps_parse(&neg, LOW).unwrap().0, -1.5);
    assert_eq!(setup::lamp_temps_parse(&neg, SCN), None);
}

#[test]
fn event_followups_match_captures() {
    let (mut acks, mut matched) = (0, 0);
    for file in [
        "scan.pakscan",
        "scan_fullroll.pakscan",
        "36frames.pakscan",
        "f135plus/base16.pakscan",
        "f135plus/base8.pakscan",
        "f135plus/base4.pakscan",
        "f135plus/base4_ir.pakscan",
    ] {
        let fr: Vec<Vec<u8>> = load(file)
            .into_iter()
            .filter_map(|l| match l {
                Line::Out(b) if !(b.len() == 3 && b[0] == 3 && b[2] == Address::Host as u8) => Some(b),
                _ => None,
            })
            .collect();
        for i in 0..fr.len() {
            let b = &fr[i];
            if !(b.len() == 7 && b[0] == 2 && b[1] == 5 && b[3] == 2 && b[4] == 0x06) {
                continue;
            }
            let a = b[2];
            let mut seq = Seq::new();
            setup::event_followup(&mut seq, a, b[6], a == LOW || a == LOW_P);
            let gen = wire(&seq);
            let k = gen.iter().zip(&fr[i..]).take_while(|(g, c)| g == c).count();
            // matched if nothing else event-related (another read) follows
            let extra = fr.get(i + k).is_some_and(|n| n.len() == 5 && n[0] == 1 && n[2] == a && n[4] != 0x02);
            acks += 1;
            if k == gen.len() && !extra {
                matched += 1;
            }
        }
    }
    assert_eq!((matched, acks), (91, 95), "event follow-ups reproduced (expect 91 of 95)");
}

#[test]
fn calibration_writes() {
    let st = f135_state();

    let mut seq = Seq::new();
    setup::acquire(&mut seq, &st, true);
    assert_eq!(seq_hex(&seq), "0206240382006101");

    let mut seq = Seq::new();
    for w in [setup::PANEL_CALIBRATING, setup::PANEL_FEED, setup::PANEL_IDLE] {
        setup::panel(&mut seq, &st, w);
    }
    assert_eq!(seq_hex(&seq), "020624038209d400 0206240382091500 0206240382091700");

    let mut seq = Seq::new();
    setup::afe(&mut seq, &st, [13, 13, 13], [-51, -42, -43]).unwrap();
    assert_eq!(
        seq_hex(&seq),
        "0206240384020d00 0206240384030d00 0206240384040d00 \
         0206240384053301 0206240384062a01 0206240384072b01"
    );
    assert_eq!(setup::afe(&mut Seq::new(), &st, [0x3F, 0, 0], [0; 3]), Err(SetupError::Afe));
    assert_eq!(setup::afe(&mut Seq::new(), &st, [0; 3], [256, 0, 0]), Err(SetupError::Afe));

    let mut seq = Seq::new();
    let mut cur = LedValues { r: 2, g: 3, b: 3, ir: 2 };
    setup::leds(&mut seq, &st, 0x03, cur, [0x380, 0x2C4, 0x116, 0x549], 0x742).unwrap();
    assert_eq!(
        seq_hex(&seq),
        "020420018003 02082005810302020003 020f200c821601490580030000c4024207"
    );

    let mut seq = Seq::new();
    cur.r = 9;
    assert_eq!(setup::leds(&mut seq, &st, 3, cur, [1; 4], 0x742), Err(SetupError::LedCurrent));
    cur.r = 2;
    assert_eq!(setup::leds(&mut seq, &st, 3, cur, [0x741, 1, 1, 1], 0x742), Err(SetupError::Duty));
    assert_eq!(setup::leds(&mut seq, &st, 4, cur, [1; 4], 0x742), Err(SetupError::Enable(4)));
    assert!(seq.is_empty());
    let mut off = st;
    off.reg0 = 0x0060; // IR off: IR ceiling 0
    let one = LedValues { r: 1, g: 1, b: 1, ir: 1 };
    assert_eq!(setup::leds(&mut seq, &off, 1, one, [1; 4], 0x742), Err(SetupError::LedCurrent));
}

#[test]
fn scan_start() {
    // scan.pakscan lines 1688-1724; speed 0x0615 = serial 3054's Base 16
    // MotorSpeed_Ir 1557 x 1000/1000
    let mut st = f135_state();
    assert_eq!(setup::motor_speed(1557, 1000, Model::F135), 0x0615);
    assert_eq!(setup::motor_speed(1557, 1200, Model::F135), 1712);
    assert_eq!(setup::motor_speed(100, 1000, Model::F135), 400);
    assert_eq!(setup::motor_speed(25726, 1000, Model::F135Plus), 25726);

    let mut seq = Seq::new();
    setup::scan_start(&mut seq, &mut st, 43, [0x4DF, 0x6EF, 0x5A2, 0x548], 0x742, 0x0615, 0x0010).unwrap();
    assert_eq!(
        seq_hex(&seq),
        "0206240382006001 020410018402 040320008a 0206240382042b00 020420018003 \
         020f200c82a2054805df040000ef064207 02052402a51506 04032400a0 \
         0206240382006101 0206200391100001"
    );
    assert_eq!(st.reg4, 43);

    let mut seq = Seq::new();
    assert_eq!(
        setup::scan_start(&mut seq, &mut st, 43, [1; 4], 0x742, 9501, 0x10),
        Err(SetupError::MotorSpeed(9501))
    );
    assert!(seq.is_empty());
}

#[test]
fn advance() {
    let st = f135_state();
    let mut seq = Seq::new();
    setup::motor_run(&mut seq, &st, setup::ADVANCE_SPEED_F135).unwrap();
    assert_eq!(seq.len(), 4);
    assert_eq!(seq.frames()[1].packet_type(), Some(PacketType::ReadStatus));
    assert_eq!(seq.frames()[3].packet_type(), Some(PacketType::ReadStatus));
    assert_eq!(seq_hex(&seq), "02052402a51c25 04032400a0");

    let mut seq = Seq::new();
    setup::motor_stop(&mut seq, &st);
    assert_eq!((seq_hex(&seq).as_str(), seq.len()), ("04032400a2", 2));

    assert_eq!(setup::motor_run(&mut Seq::new(), &st, 9501), Err(SetupError::MotorSpeed(9501)));
    assert_eq!(setup::sense_read_frame(LOW).to_bytes(), [1, 3, 0x20, 4, 0x93]);
}

#[test]
fn film_sensing() {
    // readings from the 2026-10-09 film-sense run
    use setup::sense_clear as clear;
    let empty = [190, 216, 224, 202];
    let head_in = [89, 109, 223, 194];
    let both = [141, 164, 108, 69];
    let at_exit = [190, 216, 106, 110];
    let spike = [120, 8, 107, 102];
    assert!(clear(empty, Sensor::Entry) && clear(empty, Sensor::Exit));
    assert!(!clear(head_in, Sensor::Entry) && clear(head_in, Sensor::Exit));
    assert!(!clear(both, Sensor::Entry) && !clear(both, Sensor::Exit));
    assert!(clear(at_exit, Sensor::Entry) && !clear(at_exit, Sensor::Exit));
    assert!(!clear(spike, Sensor::Entry));
}

#[test]
fn integration_limit() {
    let mut seq = Seq::new();
    let mut st = setup::init(&mut seq, LOW, SCN, setup::LED_PERIOD_F135);
    let mut m = setup::scan_mode(Model::F135, 16, true, 43).unwrap();
    m.integration = 0x1000;
    assert_eq!(setup::configure(&mut seq, &mut st, &m), Err(SetupError::Integration(0x1000)));
}

#[test]
fn safety_check() {
    let mut seq = Seq::new();
    seq.push(PacketType::Command, &[Address::BootPicL as u8, 0x00, 0x0C]);
    assert_eq!(seq.check_safe(), Err(SetupError::Unsafe(0)));
    let mut seq = Seq::new();
    seq.push(PacketType::Write, &[]);
    assert_eq!(seq.check_safe(), Err(SetupError::Unsafe(0)));
}
