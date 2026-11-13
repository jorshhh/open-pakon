//! Light calibration in code (port of `light_cal_run`, `scan_calibrate`,
//! `lc_write_flat` and their helpers in `tools/pakon_replay.c`): TLB.dll
//! FN_bCalibrateLEDs on the open gate, motor stopped. Dark offsets, the LED
//! current search, the duty refine, then the fixed-pattern tables; reused
//! from the saved calibration as often as TLB.dll does (`calcache`).

use std::path::Path;
use std::time::Duration;

use pakon_proto::calcache::{self, Action, Record};
use pakon_proto::frame::{Address, PacketType};
use pakon_proto::lightcal::{self as lc, Film, NCH};
use pakon_proto::setup::{self, LedValues, Seq, SetupState, PANEL_CALIBRATING, PANEL_IDLE};

use crate::cmd;
use crate::link::Link;
use crate::session::{panel, Out, Session};
use crate::ScanError;

/// Image read size (40 x 512-byte packets).
pub const CHUNK: usize = 20480;
/// Chunks per measurement (~51 lines).
const PROBE_CHUNKS: usize = 40;
/// Image read window.
pub const IMG_TIMEOUT: Duration = Duration::from_millis(5000);
/// Whole lines skipped after a re-arm before measuring.
const PROBE_SKIP_LINES: usize = 4;
/// Minimum black-to-lit step (counts) for a line alignment to count.
const MIN_CONTRAST: f64 = 3000.0;
/// Wait after an A/D offset write.
const AFE_SETTLE: Duration = Duration::from_millis(300);
/// Lines per fixed-pattern table (TLB.dll averages 128).
const TABLE_LINES: usize = 128;
/// TLB.dll FN_bLampTemperatureStable.
pub const LAMP_WAIT: Duration = Duration::from_secs(300);

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// A calibration: A/D, LEDs, the calibration line and its tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cal {
    pub gain: [i32; 3],
    pub off: [i32; 3],
    /// R, G, B, IR.
    pub cur: [u32; 4],
    /// Open-gate duties, R, G, B, IR.
    pub duty: [u16; 4],
    pub line_px: usize,
    pub px0: usize,
    pub black_px: usize,
    pub ir: bool,
    pub lines_dark: usize,
    pub lines_bright: usize,
    /// Per-pixel means, [channel][pixel], R, G, B, IR; empty when not captured.
    pub dark: Vec<f64>,
    pub bright: Vec<f64>,
}

impl Cal {
    fn from_record(r: Record) -> Cal {
        Cal {
            gain: r.gain,
            off: r.off,
            cur: r.cur,
            duty: r.duty,
            line_px: r.line_px,
            lines_dark: r.lines_dark,
            lines_bright: r.lines_bright,
            dark: r.dark,
            bright: r.bright,
            ..Default::default()
        }
    }

    fn table_mean(t: &[f64], line_px: usize, c: usize, p0: usize, p1: usize) -> f64 {
        if p1 > p0 { t[c * line_px + p0..c * line_px + p1].iter().sum::<f64>() / (p1 - p0) as f64 } else { 0.0 }
    }
}

fn leds(cur: [u32; 4]) -> LedValues {
    LedValues { r: cur[0] as u8, g: cur[1] as u8, b: cur[2] as u8, ir: cur[3] as u8 }
}

fn run(link: &mut dyn Link, seq: &Seq, t: Duration) -> Result<(), ScanError> {
    cmd::run_seq(link, seq, t)
}

/// ResetFifos re-arm: the read that follows starts at a fixed line phase.
fn rearm(link: &mut dyn Link, st: &SetupState, t: Duration) -> Result<(), ScanError> {
    let mut seq = Seq::new();
    seq.push(PacketType::Write, &[Address::Host as u8, 0x01, 0x84, 0x02]);
    seq.push(PacketType::Command, &[st.low, 0x00, 0x8A]);
    seq.push(PacketType::ReadStatus, &[st.low]);
    run(link, &seq, t)
}

/// ~51 lines of samples, serving events after every read.
fn probe_read(link: &mut dyn Link, st: &SetupState, t: Duration, out: Out) -> Result<Vec<u16>, ScanError> {
    let mut buf = vec![0u8; CHUNK];
    let mut s = Vec::with_capacity(PROBE_CHUNKS * CHUNK / 2);
    for i in 0..PROBE_CHUNKS {
        let got = match link.recv_image(&mut buf, IMG_TIMEOUT) {
            Ok(n) if n > 0 => n,
            r => {
                out(&format!("probe: image read {i}: {r:?}"));
                return Err(ScanError::Timeout);
            }
        };
        if cmd::service_events(link, st.low, st.scn, t, None).is_err() {
            out(&format!("probe: event service failed at read {i}"));
        }
        s.extend(buf[..got].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)));
    }
    Ok(s)
}

/// Line origin of a read: on the line marker; with IR on and no marker in
/// the read, on the lit IR / RGB blocks.
fn origin(s: &[u16], line_px: usize, black_px: usize, ir: bool, out: Out) -> Option<usize> {
    let o = lc::origin(s, line_px, black_px, ir, MIN_CONTRAST);
    if ir && o.is_some() && lc::marker_origin(s, lc::blocks(true) * line_px).is_none() {
        out("light-cal: no line marker in this read; aligned on the IR block");
    }
    o
}

fn stats(s: &[u16], g: &Cal, out: Out) -> Result<(lc::Stats, lc::Stats), ScanError> {
    let Some(o) = origin(s, g.line_px, g.black_px, g.ir, out) else {
        out(if g.ir { "light-cal: no light to align lines on" } else { "light-cal: no line marker to align on" });
        return Err(ScanError::Proto("no line alignment".into()));
    };
    let origin = o + PROBE_SKIP_LINES * lc::blocks(g.ir) * g.line_px;
    let act0 = if g.px0 > 0 { g.px0 + 3 } else { g.black_px + 20 };
    let black = lc::stats_range(s, g.line_px, g.ir, origin, 0, g.black_px);
    let active = lc::stats_range(s, g.line_px, g.ir, origin, act0, g.line_px - 10);
    black.zip(active).ok_or_else(|| ScanError::Proto("not enough data".into()))
}

fn measure(link: &mut dyn Link, st: &SetupState, t: Duration, g: &Cal, out: Out) -> Result<(lc::Stats, lc::Stats), ScanError> {
    rearm(link, st, t)?;
    let s = probe_read(link, st, t, out)?;
    stats(&s, g, out)
}

/// The calibration line's geometry: the register window at Base 16 and 4,
/// measured at Base 8 (LOW 0x89 = 1 narrows the line); the scan window and
/// black pixels scale with it.
fn geometry_set(st: &SetupState, ir: bool, offset: u16, lp: usize, res: &mut Cal, out: Out) -> Result<(), ScanError> {
    let win = (st.reg5 - st.reg4) as usize;
    let scan_win = st.reg5 as usize - offset as usize;
    if lp == 0 || offset >= st.reg5 || offset < st.reg4 {
        out("light-cal: could not measure the line width");
        return Err(ScanError::Proto("line width".into()));
    }
    let scan_px = (scan_win * lp + win / 2) / win;
    res.ir = ir;
    res.line_px = lp;
    res.px0 = lp.saturating_sub(scan_px);
    res.black_px = (lc::BLACK_PX * scan_px + 1000) / 2000;
    out(&format!(
        "calibration line: {lp} px (register window {win} px); scan {scan_px} px from pixel {}, {} black{}",
        res.px0,
        res.black_px,
        if ir { "" } else { "; IR off, no IR block" }
    ));
    Ok(())
}

fn geometry(st: &SetupState, ir: bool, offset: u16, s: &[u16], res: &mut Cal, out: Out) -> Result<(), ScanError> {
    let win = (st.reg5 - st.reg4) as usize;
    let lp = if st.low89 != 0 && ir {
        lc::find_line_px(s, win * 5 / 8, win + 8, true).unwrap_or(0)
    } else if st.low89 != 0 {
        lc::marker_line_len(s).map_or(0, |l| l / lc::blocks(false))
    } else {
        win
    };
    geometry_set(st, ir, offset, lp, res, out)
}

/// Per-pixel means over >= 128 aligned lines.
fn capture_columns(link: &mut dyn Link, st: &SetupState, ir: bool, t: Duration, line_px: usize, black_px: usize, out: Out) -> Result<(Vec<f64>, usize), ScanError> {
    let mut mean = vec![0.0f64; NCH * line_px];
    let mut lines = 0usize;
    for read in 0..8 {
        if lines >= TABLE_LINES {
            break;
        }
        rearm(link, st, t)?;
        let s = probe_read(link, st, t, out)?;
        let Some(o) = origin(&s, line_px, black_px, ir, out) else {
            let mx = s.iter().copied().max().unwrap_or(0);
            let avg = if s.is_empty() { 0.0 } else { s.iter().map(|&v| v as f64).sum::<f64>() / s.len() as f64 };
            out(&format!("light-cal: could not align lines (table read {read}: {} samples, mean {avg:.0}, max {mx})", s.len()));
            return Err(ScanError::Proto("table alignment".into()));
        };
        lc::column_accum(&s, line_px, ir, o + PROBE_SKIP_LINES * lc::blocks(ir) * line_px, &mut mean, &mut lines);
    }
    if lines < TABLE_LINES {
        return Err(ScanError::Proto("too few table lines".into()));
    }
    for v in mean.iter_mut() {
        *v /= lines as f64;
    }
    Ok((mean, lines))
}

/// Calibrate only on a stable lamp, as the OEM does.
pub fn lamp_wait(link: &mut dyn Link, st: &SetupState, t: Duration, who: &str, out: Out) -> Result<(), ScanError> {
    let r = cmd::wait_lamp(link, st.low, st.scn, t, LAMP_WAIT, &mut |lamp, ms| {
        if ms < 1000 || (ms / 1000) % 5 == 0 {
            out(&format!("lamp: warming up, {lamp:.1} C ({} s)", ms / 1000));
        }
    });
    match r {
        Ok(()) => {
            match cmd::lamp_temps(link, st.low, t) {
                Ok((lamp, _)) => out(&format!("lamp: stable, {lamp:.1} C")),
                Err(_) => out("lamp: stable"),
            }
            Ok(())
        }
        Err(ScanError::Timeout) => {
            out(&format!("{who}: the lamp did not settle in {} s; switch the scanner off and on and try again", LAMP_WAIT.as_secs()));
            Err(ScanError::Timeout)
        }
        Err(ScanError::Status(s)) => {
            out(&format!("{who}: the scanner reports a lamp fault"));
            Err(ScanError::Status(s))
        }
        Err(e) => {
            out(&format!("{who}: lamp check failed ({e})"));
            Err(e)
        }
    }
}

/// The calibration itself on a configured session, for `film`'s base
/// densities. Leaves acquire on and the LEDs at the open-gate values. With
/// `prior`, skips the current search and starts from its values, as TLB.dll
/// does between full calibrations.
pub fn light_cal_run(link: &mut dyn Link, s: &Session, t: Duration, film: Film, with_tables: bool, prior: Option<&Cal>, out: Out) -> Result<Cal, ScanError> {
    let st = &s.st;
    let r = light_cal_inner(link, s, t, film, with_tables, prior, out);
    panel(link, st, PANEL_IDLE, t, out);
    r
}

fn light_cal_inner(link: &mut dyn Link, s: &Session, t: Duration, film: Film, with_tables: bool, prior: Option<&Cal>, out: Out) -> Result<Cal, ScanError> {
    let st = &s.st;
    let (offset, period, ir) = (s.offset(), s.led_period(), s.ir());
    let density = film.density();
    let nm = ["R", "G", "B", "IR"];
    let mut res = Cal::default();
    let _ = cmd::service_events(link, st.low, st.scn, t, None);
    panel(link, st, PANEL_CALIBRATING, t, out);

    // 1. Dark offsets. IR on: the IR LED alone (current 1, half duty) so
    // every read can be aligned on the IR block; IR off: all LEDs off.
    let mut gain = prior.map_or([lc::GAIN_START; 3], |p| p.gain);
    let mut off = prior.map_or([lc::OFFSET_START; 3], |p| p.off);
    let ir_only = LedValues { r: 0, g: 0, b: 0, ir: ir as u8 };
    let dark_duty = [0, 0, 0, if ir { (period - 2) / 2 } else { 0 }];
    let dark_en = if ir { 0x02 } else { 0x00 };
    let bad = |e: setup::SetupError| ScanError::Param(e.to_string());
    let mut seq = Seq::new();
    setup::leds(&mut seq, st, dark_en, ir_only, dark_duty, period).map_err(bad)?;
    setup::acquire(&mut seq, st, true);
    run(link, &seq, t)?;
    let mut dark_ok = false;
    for it in 1..=lc::DARK_ITERS {
        if dark_ok {
            break;
        }
        let mut seq = Seq::new();
        setup::afe(&mut seq, st, gain, off).map_err(bad)?;
        run(link, &seq, t)?;
        std::thread::sleep(AFE_SETTLE);
        let (black, _) = if it == 1 {
            rearm(link, st, t)?;
            let smp = probe_read(link, st, t, out)?;
            geometry(st, ir, offset, &smp, &mut res, out)?;
            stats(&smp, &res, out)?
        } else {
            measure(link, st, t, &res, out)?
        };
        out(&format!(
            "dark {it}: offsets {}/{}/{} -> black means {:.0}/{:.0}/{:.0}",
            off[0], off[1], off[2], black.mean[0], black.mean[1], black.mean[2]
        ));
        dark_ok = true;
        for (o, &m) in off.iter_mut().zip(&black.mean[..3]) {
            if !lc::dark_ok(m) {
                dark_ok = false;
                *o = lc::offset_next(*o, m);
            }
        }
    }
    if !dark_ok {
        out("light-cal: dark offsets did not converge");
        return Err(ScanError::Proto("dark offsets did not converge".into()));
    }

    // 2. LED current search at the calibration duty caps (full calibration
    // only). With IR off the IR LED stays at 0 and only R, G, B calibrate.
    let nled = if ir { 4 } else { 3 };
    let lit_en = if ir { 0x03 } else { 0x01 };
    let mut cap = [0u16; 4];
    let mut duty = [0u16; 4];
    let mut curv: [u32; 4] = [1, 1, 1, ir as u32];
    let mut done = [false, false, false, !ir];
    let ceil = setup::led_ceiling(st.model(), ir);
    let ceilv = [ceil.r as u32, ceil.g as u32, ceil.b as u32, ceil.ir as u32];
    for c in 0..nled {
        cap[c] = lc::duty_cap(period, density[c]);
        duty[c] = cap[c];
    }
    if let Some(p) = prior {
        for c in 0..nled {
            curv[c] = p.cur[c].clamp(1, ceilv[c].max(1));
            duty[c] = p.duty[c].clamp(1, period - 2);
        }
        out(&format!("current: from the last calibration, R{} G{} B{} IR{}", curv[0], curv[1], curv[2], curv[3]));
    } else {
        // One pass per current step up to the board's highest ceiling (8 on
        // the F-135, 24 on the F-135+): each pass raises every unfinished
        // LED by one.
        let passes = ceilv.iter().copied().max().unwrap_or(8);
        for pass in 1..=passes {
            let mut seq = Seq::new();
            setup::leds(&mut seq, st, lit_en, leds(curv), duty, period).map_err(bad)?;
            run(link, &seq, t)?;
            sleep(100);
            let (_, active) = measure(link, st, t, &res, out)?;
            out(&format!(
                "current {pass}: R{} G{} B{} IR{} -> peaks {}/{}/{}/{}",
                curv[0], curv[1], curv[2], curv[3], active.peak[0], active.peak[1], active.peak[2], active.peak[3]
            ));
            let mut all = true;
            for c in 0..nled {
                if done[c] {
                    continue;
                }
                if lc::peak_over_cap(c, active.peak[c] as u32) || curv[c] >= ceilv[c] {
                    done[c] = true;
                } else {
                    curv[c] += 1;
                }
                all &= done[c];
            }
            if all {
                break;
            }
        }
        for c in 0..nled {
            duty[c] = lc::duty_start(cap[c], curv[c]);
        }
    }

    // 3. Duty refine: two converged passes 200 ms apart.
    let mut converged = 0;
    for it in 1..=lc::REFINE_ITERS {
        if converged >= 2 {
            break;
        }
        let mut seq = Seq::new();
        setup::leds(&mut seq, st, lit_en, leds(curv), duty, period).map_err(bad)?;
        run(link, &seq, t)?;
        sleep(if converged > 0 { 200 } else { 100 });
        let (_, active) = measure(link, st, t, &res, out)?;
        out(&format!(
            "refine {it}: duties {}/{}/{}/{} -> peaks {}/{}/{}/{}",
            duty[0], duty[1], duty[2], duty[3], active.peak[0], active.peak[1], active.peak[2], active.peak[3]
        ));
        let mut ok = true;
        for c in 0..nled {
            let peak = active.peak[c] as u32;
            if lc::duty_settled(c, duty[c], peak, period) {
                continue;
            }
            ok = false;
            let next = lc::duty_next(c, duty[c], peak, period);
            if next == duty[c] && next == period - 2 {
                // Duty at its maximum and still short: more current, else gain.
                if curv[c] < ceilv[c] {
                    curv[c] += 1;
                } else if c < 3 && gain[c] < lc::GAIN_MAX {
                    gain[c] += 1;
                    let mut seq = Seq::new();
                    setup::afe(&mut seq, st, gain, off).map_err(bad)?;
                    let _ = run(link, &seq, t);
                } else {
                    out(&format!("light-cal: insufficient light on {}", nm[c]));
                    return Err(ScanError::Proto("insufficient light".into()));
                }
            }
            duty[c] = next;
        }
        converged = if ok { converged + 1 } else { 0 };
    }
    if converged < 2 {
        out("light-cal: duties did not converge");
        return Err(ScanError::Proto("duties did not converge".into()));
    }

    // 4. Fixed-pattern tables: dark (RGB LEDs off, the IR LED on for the
    // alignment), then bright at the calibrated values, which also leaves
    // the LEDs where the scan start expects them.
    if with_tables {
        let line_px = res.line_px;
        let mut seq = Seq::new();
        setup::leds(&mut seq, st, dark_en, ir_only, dark_duty, period).map_err(bad)?;
        run(link, &seq, t)?;
        sleep(200);
        let (dark, ld) = capture_columns(link, st, ir, t, line_px, res.black_px, out).inspect_err(|_| out("light-cal: dark table capture failed"))?;
        let mut seq = Seq::new();
        setup::leds(&mut seq, st, lit_en, leds(curv), duty, period).map_err(bad)?;
        run(link, &seq, t)?;
        sleep(200);
        let (bright, lb) = capture_columns(link, st, ir, t, line_px, res.black_px, out).inspect_err(|_| out("light-cal: bright table capture failed"))?;
        let act0 = if res.px0 > 0 { res.px0 } else { res.black_px + 20 };
        for (c, n) in ["R", "G", "B"].iter().enumerate() {
            let bd = Cal::table_mean(&dark, line_px, c, 0, res.black_px);
            let ad = Cal::table_mean(&dark, line_px, c, act0, line_px);
            let ab = Cal::table_mean(&bright, line_px, c, act0, line_px);
            out(&format!("tables {n}: {ld}/{lb} lines, dark black {bd:.0} active {ad:.0} (IR leak {:+.0}), bright active {ab:.0}", ad - bd));
        }
        (res.dark, res.bright, res.lines_dark, res.lines_bright) = (dark, bright, ld, lb);
    }
    res.cur = curv;
    res.duty = duty;
    res.gain = gain;
    res.off = off;
    Ok(res)
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Calibrate before a scan only as often as TLB.dll does: reuse the saved
/// calibration while under an hour old, recalibrate from its currents when
/// older, from scratch when there is none, the last full one is over 20 h
/// old, or `force`. Leaves the A/D and LEDs at the open-gate values.
pub fn scan_calibrate(link: &mut dyn Link, s: &Session, t: Duration, eeprom_dir: &Path, film: Film, force: bool, out: Out) -> Result<Cal, ScanError> {
    let st = &s.st;
    let (offset, period) = (s.offset(), s.led_period());
    let line_px = if st.low89 != 0 { 0 } else { (st.reg5 - st.reg4) as usize };
    let path = calcache::path(eeprom_dir, film, s.base, s.ir());
    let rec = path.as_ref().and_then(|p| Record::load(p, s.eeprom.serial, offset as u32, film, line_px));
    let now = now_s();
    let (when, when_full) = rec.as_ref().map_or((0, 0), |r| (r.when, r.when_full));
    let act = if force { Action::Full } else { calcache::needed(now, when, when_full) };
    if force {
        out("calibration: full (forced)");
    } else if rec.is_none() {
        out(&format!("calibration: full (none saved for {}, Base {})", film.name(), s.base));
    } else {
        out(&format!("calibration: {} (last {} min ago, last full {} min ago)", act.name(), (now - when) / 60, (now - when_full) / 60));
    }

    if act == Action::Reuse {
        let mut res = Cal::from_record(rec.expect("reuse needs a record"));
        let _ = cmd::service_events(link, st.low, st.scn, t, None);
        let lp = res.line_px;
        geometry_set(st, s.ir(), offset, lp, &mut res, out)?;
        let mut seq = Seq::new();
        let bad = |e: setup::SetupError| ScanError::Param(e.to_string());
        setup::afe(&mut seq, st, res.gain, res.off).map_err(bad)?;
        setup::leds(&mut seq, st, if s.ir() { 0x03 } else { 0x01 }, leds(res.cur), res.duty, period).map_err(bad)?;
        run(link, &seq, t).inspect_err(|_| out("scan-code: applying the saved calibration failed"))?;
        sleep(200);
        return Ok(res);
    }

    let mut rec = rec;
    let mut result = None;
    let (mut when, mut when_full) = (when, when_full);
    if act == Action::Refresh {
        let prior = rec.take().map(Cal::from_record);
        match light_cal_run(link, s, t, film, true, prior.as_ref(), out) {
            Ok(r) => {
                result = Some(r);
                when = now;
            }
            Err(_) => out("calibration: refresh failed, calibrating from scratch"),
        }
    }
    let res = match result {
        Some(r) => r,
        None => {
            let r = light_cal_run(link, s, t, film, true, None, out)?;
            when = now;
            when_full = now;
            r
        }
    };
    let save = Record {
        serial: s.eeprom.serial,
        offset: offset as u32,
        film,
        when,
        when_full,
        gain: res.gain,
        off: res.off,
        cur: res.cur,
        duty: res.duty,
        line_px: res.line_px,
        lines_dark: res.lines_dark,
        lines_bright: res.lines_bright,
        dark: res.dark.clone(),
        bright: res.bright.clone(),
    };
    if let Some(p) = &path {
        if save.save(p).is_err() {
            out(&format!("scan-code: could not save the calibration to {}", p.display()));
        }
    }
    Ok(res)
}

/// "YYYY-MM-DDTHH:MM:SSZ" (UTC; the C wrote local time without a zone).
fn iso_now() -> String {
    let s = now_s();
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// The flat-field file the decoder reads (`<raw>.flat.json`, or the
/// archive's `flatfield*.json`): the calibration-line tables, black-pixel
/// means, smear, where the scan window sits, the mode and the row layout.
/// `scan_duty` are the duties the scan ran at (None for a plain light cal).
pub fn flat_json(serial: u32, offset: u16, base: u32, film: Film, res: &Cal, scan_duty: Option<[u16; 4]>) -> String {
    let (lp, px0) = (res.line_px, res.px0);
    let mut f = String::new();
    f += "{\n  \"version\": 1,\n  \"source\": \"pakon_replay light-cal\",\n";
    f += &format!("  \"date\": \"{}\",\n  \"serial\": {serial},\n  \"mode\": \"base{base}{}\",\n", iso_now(), if res.ir { "-ir" } else { "" });
    f += &format!("  \"film\": \"{}\",\n", film.name());
    f += &format!("  \"base\": {base},\n  \"row_samples\": {},\n  \"ir_lane\": {},\n", lc::blocks(res.ir) * (lp - px0), res.ir);
    f += &format!("  \"eeprom_offset\": {offset},\n  \"line_px\": {lp},\n  \"px0\": {px0},\n  \"black_px\": {},\n", res.black_px);
    f += &format!("  \"lines_dark\": {},\n  \"lines_bright\": {},\n", res.lines_dark, res.lines_bright);
    let (c, d) = (res.cur, res.duty);
    f += &format!("  \"currents\": [{}, {}, {}, {}],\n  \"duties\": [{}, {}, {}, {}],\n", c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]);
    if let Some(sd) = scan_duty {
        f += &format!("  \"scan_duties\": [{}, {}, {}, {}],\n", sd[0], sd[1], sd[2], sd[3]);
    }
    f += &format!(
        "  \"afe_gains\": [{}, {}, {}],\n  \"afe_offsets\": [{}, {}, {}],\n",
        res.gain[0], res.gain[1], res.gain[2], res.off[0], res.off[1], res.off[2]
    );
    let smear: Vec<String> = (0..3)
        .map(|c| {
            let bd = Cal::table_mean(&res.dark, lp, c, 0, res.black_px);
            let bb = Cal::table_mean(&res.bright, lp, c, 0, res.black_px);
            let ad = Cal::table_mean(&res.dark, lp, c, px0, lp);
            let ab = Cal::table_mean(&res.bright, lp, c, px0, lp);
            lc::smear(bb, bd, ab, ad).to_string()
        })
        .collect();
    f += &format!("  \"smear\": [{}],\n", smear.join(", "));
    for (k, (key, tab)) in [("dark", &res.dark), ("bright", &res.bright)].iter().enumerate() {
        f += &format!("  \"{key}\": [\n");
        for c in 0..NCH {
            let row: Vec<String> = (0..lp).map(|p| format!("{:.1}", tab[c * lp + p])).collect();
            f += &format!("    [{}]{}\n", row.join(","), if c + 1 < NCH { "," } else { "" });
        }
        f += &format!("  ]{}\n", if k == 1 { "" } else { "," });
    }
    f += "}\n";
    f
}

#[allow(clippy::too_many_arguments)]
pub fn write_flat(path: &Path, serial: u32, offset: u16, base: u32, film: Film, res: &Cal, scan_duty: Option<[u16; 4]>, out: Out) -> Result<(), ScanError> {
    std::fs::write(path, flat_json(serial, offset, base, film, res, scan_duty)).map_err(|e| {
        out(&format!("write error on '{}'", path.display()));
        ScanError::Usb(e.to_string())
    })?;
    out(&format!("flat field -> {}", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_json_reads_back() {
        let res = Cal {
            gain: [13, 13, 13],
            off: [10, 11, 12],
            cur: [2, 3, 3, 2],
            duty: [896, 708, 278, 1353],
            line_px: 30,
            px0: 4,
            black_px: 2,
            ir: true,
            lines_dark: 128,
            lines_bright: 130,
            dark: (0..NCH * 30).map(|i| i as f64 * 0.25).collect(),
            bright: (0..NCH * 30).map(|i| 1000.0 + i as f64).collect(),
        };
        let j = flat_json(3054, 154, 16, Film::C41, &res, Some([1, 2, 3, 4]));
        assert!(j.contains("\"row_samples\": 104,"));
        assert!(j.contains("\"scan_duties\": [1, 2, 3, 4],"));
        // C's %.1f: ties of the exact binary value round to even
        assert!(j.contains("\"dark\": [\n    [0.0,0.2,0.5,0.8,"));
        assert!(j.ends_with("  ]\n}\n"));
    }

    #[test]
    fn iso_date_shape() {
        let s = iso_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T');
    }
}
