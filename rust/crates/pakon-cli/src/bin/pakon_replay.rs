//! `pakon_replay`: the same flags and output lines as the C tool it
//! replaced (`tools/pakon_replay.c`, git tag python-final) for --open,
//! --scan (replay, --autostop, --drain, --advance), --scan-code,
//! --light-cal, --setup, --eject, --advance-code and --advance. The C
//! tool's research modes (--read-params, --calib-probe, --film-sense,
//! --trace-status, advance scripts) were not ported.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use pakon_cli::{say, stop_flag};
use pakon_proto::eeprom::Eeprom;
use pakon_proto::lightcal::{self as lc, Film};
use pakon_scan::link::Link;
use pakon_scan::{calib, cmd, replay, scan, session, transport, ScanError};
use pakon_usb::Device;

#[derive(Default)]
struct Args {
    open: bool,
    setup: bool,
    teardown: bool,
    light_cal: bool,
    scan_code: bool,
    recalibrate: bool,
    eject: bool,
    eject_s: u64,
    advance_code_s: u64,
    advance: bool,
    advance_s: u32,
    scan: Option<PathBuf>,
    drain: bool,
    autostop: bool,
    film: Film,
    ir: Option<bool>,
    base: u32,
    eeprom_dir: Option<PathBuf>,
    flat_out: Option<PathBuf>,
    image: PathBuf,
    max_mb: u64,
    timeout_ms: u64,
}

fn usage() {
    eprintln!(
        "usage: pakon_replay --open\n\
         \x20      pakon_replay --scan FILE [--image OUT] [--drain] [--autostop] [--advance]\n\
         \x20      pakon_replay --setup --eeprom-dir DIR [--base N] [--teardown]\n\
         \x20      pakon_replay --light-cal --eeprom-dir DIR [--base N] [--ir on|off] [--flat-out FILE]\n\
         \x20      pakon_replay --scan-code --eeprom-dir DIR [--base N] [--ir on|off] [--image OUT] [--film c41|bw|bwc41|slide] [--recalibrate] [--max-mb N]\n\
         \x20      pakon_replay --eject [--eject-seconds N] --eeprom-dir DIR\n\
         \x20      pakon_replay --advance-code SECONDS --eeprom-dir DIR\n\
         \x20      pakon_replay --advance [--advance-seconds N]          (F-135+)\n\
         \x20 --timeout MS  USB per-transfer timeout (default 1000); PAKON_DEBUG=0..4 for traces"
    );
}

fn parse() -> Result<Args, i32> {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args { eject_s: 60, advance_s: 15, base: 16, image: "pakon_scan.raw".into(), max_mb: 4096, timeout_ms: 1000, ..Default::default() };
    let mut i = 0;
    let num = |s: &str| -> Result<u64, i32> {
        let s = s.trim();
        let r = if let Some(h) = s.strip_prefix("0x") { u64::from_str_radix(h, 16) } else { s.parse() };
        r.map_err(|_| {
            eprintln!("not a number: {s}");
            2
        })
    };
    while i < v.len() {
        let has = i + 1 < v.len();
        match v[i].as_str() {
            "-h" | "--help" => {
                usage();
                return Err(0);
            }
            "--open" => a.open = true,
            "--setup" => a.setup = true,
            "--teardown" => a.teardown = true,
            "--light-cal" => a.light_cal = true,
            "--scan-code" => a.scan_code = true,
            "--recalibrate" => a.recalibrate = true,
            "--eject" => a.eject = true,
            "--advance" => a.advance = true,
            "--drain" => a.drain = true,
            "--autostop" => a.autostop = true,
            "--eject-seconds" if has => {
                i += 1;
                a.eject_s = num(&v[i])?;
            }
            "--advance-code" if has => {
                i += 1;
                a.advance_code_s = num(&v[i])?;
            }
            "--advance-seconds" if has => {
                i += 1;
                a.advance_s = num(&v[i])? as u32;
            }
            "--film" if has => {
                i += 1;
                a.film = v[i].parse().map_err(|_| {
                    eprintln!("--film: c41, bw, bwc41 or slide, got '{}'", v[i]);
                    2
                })?;
            }
            "--ir" if has => {
                i += 1;
                a.ir = Some(match v[i].as_str() {
                    "on" => true,
                    "off" => false,
                    _ => {
                        eprintln!("--ir must be on or off");
                        return Err(2);
                    }
                });
            }
            "--base" if has => {
                i += 1;
                a.base = num(&v[i])? as u32;
                if ![4, 8, 16].contains(&a.base) {
                    eprintln!("--base must be 4, 8 or 16");
                    return Err(2);
                }
            }
            "--eeprom-dir" if has => {
                i += 1;
                a.eeprom_dir = Some(v[i].clone().into());
            }
            "--flat-out" if has => {
                i += 1;
                a.flat_out = Some(v[i].clone().into());
            }
            "--scan" if has => {
                i += 1;
                a.scan = Some(v[i].clone().into());
            }
            "--image" if has => {
                i += 1;
                a.image = v[i].clone().into();
            }
            "--max-mb" if has => {
                i += 1;
                a.max_mb = num(&v[i])?;
            }
            "--timeout" if has => {
                i += 1;
                a.timeout_ms = num(&v[i])?;
            }
            "--read-params" | "--calib-probe" | "--film-sense" | "--trace-status" | "--steps" | "--limit" | "--params-out" => {
                eprintln!("{}: not ported from the C pakon_replay (git tag python-final)", v[i]);
                return Err(2);
            }
            s if !s.starts_with('-') => {
                eprintln!("{s}: advance scripts were not ported from the C pakon_replay (git tag python-final); use --advance-code");
                return Err(2);
            }
            s => {
                eprintln!("unknown/incomplete argument: {s}");
                usage();
                return Err(2);
            }
        }
        i += 1;
    }
    Ok(a)
}

fn out_fn() -> impl FnMut(&str) {
    |l: &str| say(l)
}

fn open_device() -> Result<Device, i32> {
    Device::open().map_err(|e| {
        eprintln!("open device failed: {e} (need operational 0F05:F135 on the host)");
        1
    })
}

fn archive(a: &Args) -> Result<(Eeprom, PathBuf), i32> {
    let dir = a.eeprom_dir.clone().ok_or_else(|| {
        eprintln!("setup: need --eeprom-dir with a decodable EEPROM archive (the app writes one when a warm scanner connects)");
        1
    })?;
    let e = session::load_eeprom_archive(&dir).ok_or_else(|| {
        eprintln!("setup: need --eeprom-dir with a decodable EEPROM archive (the app writes one when a warm scanner connects)");
        1
    })?;
    Ok((e, dir))
}

fn code(r: Result<(), ScanError>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn do_setup(a: &Args, dev: &mut Device, e: Eeprom, ir: bool) -> i32 {
    let mut out = out_fn();
    let t = Duration::from_millis(a.timeout_ms.max(2000));
    let s = match session::setup_session(dev, e, a.base, ir, Duration::from_millis(a.timeout_ms), &mut out) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    let mut r = Ok(());
    if a.teardown {
        r = session::teardown(dev, &s.st, t);
        say(&format!("teardown: {}", if r.is_ok() { "OK".to_string() } else { format!("{:?}", r) }));
    }
    for pass in 1..=2 {
        if r.is_err() {
            break;
        }
        match cmd::service_events(dev, s.st.low, s.st.scn, t, None) {
            Ok(ev) => say(&format!("events (pass {pass}): host flags 0x{:02x}, {} acknowledged", ev.host_flags, ev.serviced)),
            Err(err) => r = Err(err),
        }
    }
    code(r)
}

fn do_light_cal(a: &Args, dev: &mut Device, e: Eeprom, dir: &Path, ir: bool) -> i32 {
    let mut out = out_fn();
    let t = Duration::from_millis(a.timeout_ms.max(2000));
    let s = match session::setup_session(dev, e, a.base, ir, Duration::from_millis(a.timeout_ms), &mut out) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    let td = scan::Teardown::new(dev, s.st, t);
    let mut rc = 1;
    if calib::lamp_wait(&mut *td.link, &s.st, t, "light-cal", &mut out).is_ok() {
        if let Ok(res) = calib::light_cal_run(&mut *td.link, &s, t, Film::C41, true, None, &mut out) {
            let period = s.led_period();
            say(&format!("\n=== light calibration (serial {}, Base {}{}) ===", s.eeprom.serial, a.base, if s.mode.ir { " + IR" } else { ", IR off" }));
            say("        current  open-gate duty  scan duty (C-41)");
            for (c, n) in ["R", "G", "B", "IR"].iter().enumerate() {
                say(&format!("  {n:<3}   {:>7}  {:>14}  {:>16}", res.cur[c], res.duty[c], lc::scan_duty(res.duty[c], lc::DENSITY_C41[c], period)));
            }
            say(&format!("  gains {}/{}/{}, offsets {}/{}/{}", res.gain[0], res.gain[1], res.gain[2], res.off[0], res.off[1], res.off[2]));
            let flat = a.flat_out.clone().unwrap_or_else(|| {
                let b = if a.base != 16 { format!("-base{}", a.base) } else { String::new() };
                dir.join(format!("flatfield{b}{}.json", if s.mode.ir { "" } else { "-noir" }))
            });
            rc = code(calib::write_flat(&flat, s.eeprom.serial, s.offset(), a.base, Film::C41, &res, None, &mut out));
        }
    }
    let r = td.finish();
    say(&format!("teardown: {}", if r.is_ok() { "OK" } else { "FAILED" }));
    rc
}

fn run() -> i32 {
    let a = match parse() {
        Ok(a) => a,
        Err(c) => return c,
    };
    let stop = stop_flag();
    let timeout = Duration::from_millis(a.timeout_ms);
    let ir = a.ir.unwrap_or(!(a.scan_code && a.film == Film::Bw));
    let mut out = out_fn();
    let wants_session = a.setup || a.light_cal || a.scan_code || a.eject || a.advance_code_s > 0;
    if !(a.open || wants_session || a.scan.is_some() || a.advance) {
        usage();
        return 2;
    }
    if a.eject || a.advance_code_s > 0 {
        let Ok((e, _)) = archive(&a) else { return 1 };
        let Ok(mut dev) = open_device() else { return 1 };
        let secs = if a.eject { a.eject_s } else { a.advance_code_s }.min(120);
        return match transport::transport(&mut dev, e, a.eject, Duration::from_secs(secs), timeout, &stop, &mut out) {
            Ok(_) => 0,
            Err(_) => 1,
        };
    }
    if a.scan_code {
        let Ok((e, dir)) = archive(&a) else { return 1 };
        let Ok(mut dev) = open_device() else { return 1 };
        let opt = scan::ScanOptions { base: a.base, ir, film: a.film, recalibrate: a.recalibrate, max_bytes: a.max_mb << 20, timeout };
        return match scan::scan_code(&mut dev, e, &dir, &a.image, &opt, &stop, &mut out) {
            Ok(_) => 0,
            Err(_) => 1,
        };
    }
    if a.light_cal {
        let Ok((e, dir)) = archive(&a) else { return 1 };
        let Ok(mut dev) = open_device() else { return 1 };
        return do_light_cal(&a, &mut dev, e, &dir, ir);
    }
    if a.setup {
        let Ok((e, _)) = archive(&a) else { return 1 };
        let Ok(mut dev) = open_device() else { return 1 };
        return do_setup(&a, &mut dev, e, ir);
    }
    if let Some(script) = &a.scan {
        let lines = match pakon_proto::script::parse_file(script) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cannot open scan script '{}': {e}", script.display());
                return 1;
            }
        };
        let Ok(mut dev) = open_device() else { return 1 };
        let mut img = match std::fs::File::create(&a.image) {
            Ok(f) => std::io::BufWriter::with_capacity(1 << 20, f),
            Err(_) => {
                eprintln!("cannot open image '{}'", a.image.display());
                return 1;
            }
        };
        let mut rc = match replay::replay_scan(&mut dev, &lines, &mut img, timeout, a.autostop, a.drain, &stop, &mut out) {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
        if a.advance {
            say("\n[advance] pushing the film out of the transport...");
            let r = advance(&mut dev, a.advance_s, timeout, &stop, &mut out);
            if rc == 0 {
                rc = r;
            }
        }
        return rc;
    }
    let Ok(mut dev) = open_device() else { return 1 };
    if a.advance {
        return advance(&mut dev, a.advance_s, timeout, &stop, &mut out);
    }
    let (vid, pid) = dev.ids();
    say(&format!("device {vid:04x}:{pid:04x}"));
    let rep = session::open_check(&mut dev, timeout, &mut out);
    if rep.failures > 0 { 1 } else { 0 }
}

fn advance(dev: &mut dyn Link, seconds: u32, timeout: Duration, stop: &AtomicBool, out: &mut dyn FnMut(&str)) -> i32 {
    match transport::timed_advance_plus(dev, seconds, timeout, stop, out) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn main() {
    std::process::exit(run());
}
