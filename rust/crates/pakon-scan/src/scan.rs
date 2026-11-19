//! The code-built scan (port of `do_scan_code`, `sm_scan_loop`,
//! `write_dx_json` and the end-of-roll detection in `tools/pakon_replay.c`):
//! setup, lamp gate, calibration, scan start, the driven image loop with
//! the event service, then the teardown, which runs whatever happens
//! ([`Teardown`] sends it on drop, including on panic).

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pakon_proto::dx;
use pakon_proto::eeprom::{Base, Eeprom};
use pakon_proto::lightcal::{self as lc, Film};
use pakon_proto::setup::{self, Seq, SetupState, HOST_FLAG_EVENT, HOST_FLAG_OVERFLOW, PANEL_FEED, PANEL_IDLE, PANEL_NEXT_STRIP};

use crate::calib::{self, CHUNK, IMG_TIMEOUT};
use crate::cmd;
use crate::link::Link;
use crate::session::{self, panel, Out};
use crate::ScanError;

/// End-of-roll thresholds of the replayed OEM scans (open gate ~48900).
pub const WHITE_THRESH: u16 = 40000;
pub const WHITE_FRAC_PCT: usize = 60;
/// The code-built scan's: its calibration drives the open gate to full scale.
pub const WHITE_THRESH_CODE: u16 = 55000;
pub const WHITE_FRAC_CODE_PCT: usize = 45;
/// Consecutive open-gate chunks after film => the roll is done.
const TRAIL_WHITE: u32 = 24;
/// Empty read windows (~5 s each) after film => end of roll.
const MAX_EMPTY: u32 = 2;
/// Empty windows before film => nobody fed the film (~2 min).
const LOAD_WAIT: u32 = 24;
/// No film within this long of the motor start => nobody fed it. Counted in
/// time, not empty reads: with acquire on the scanner streams the open gate
/// the whole while, so the reads are never empty (seen on an F-135+).
const FEED_WAIT: Duration = Duration::from_secs(120);
/// A sample above this is real film-band content.
const FILM_THRESH: u16 = 8000;
const FILM_FRAC_PCT: usize = 10;

fn samples(buf: &[u8]) -> impl Iterator<Item = u16> + '_ {
    buf.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b))
}

/// Is a 0x86 chunk dominated by open-gate white (no film in the gate)?
pub fn chunk_is_white(buf: &[u8], thresh: u16, frac_pct: usize) -> bool {
    let n = buf.len() / 2;
    n > 0 && samples(buf).filter(|&v| v > thresh).count() * 100 >= n * frac_pct
}

/// Does a 0x86 chunk carry real film (vs the dim pre-film leader)? Callers
/// test [`chunk_is_white`] first.
pub fn chunk_has_film(buf: &[u8]) -> bool {
    let n = buf.len() / 2;
    n > 0 && samples(buf).filter(|&v| v > FILM_THRESH).count() * 100 >= n * FILM_FRAC_PCT
}

/// Sends the code-built teardown when dropped, unless disarmed: the motor
/// and LEDs never stay on after an error, a stop or a panic.
pub struct Teardown<'a> {
    pub link: &'a mut dyn Link,
    st: SetupState,
    timeout: Duration,
    armed: bool,
}

impl<'a> Teardown<'a> {
    pub fn new(link: &'a mut dyn Link, st: SetupState, timeout: Duration) -> Self {
        Teardown { link, st, timeout, armed: true }
    }
    /// Tear down now; returns whether it went through.
    pub fn finish(mut self) -> Result<(), ScanError> {
        self.armed = false;
        session::teardown(self.link, &self.st, self.timeout)
    }
}

impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = session::teardown(self.link, &self.st, self.timeout);
        }
    }
}

/// Strip mode: the most strips in one session (the OEM's buffer count cap).
pub const MAX_STRIPS: usize = 15;
/// Strip mode: no new strip this long after the last one => the roll is
/// done. The OEM's NoFilmTimeOut default is not known.
pub const STRIP_WAIT: Duration = Duration::from_secs(300);
/// Open-gate rows kept in front of the film (the roll, or each strip); the
/// rest of the open gate before it is not written.
pub const STRIP_LEAD_ROWS: usize = 200;
/// Picture pitch in scan lines at Base 16 / 8 / 4 (TLB.dll framing
/// geometry, as `pakon_image::framing`).
pub fn pitch_rows(base: u32) -> usize {
    match base {
        4 => 1603,
        8 => 2405,
        _ => 3206,
    }
}

/// Strip mode: the longest strip the OEM takes (`MULTI_STRIPS_6_FRAMES`).
/// A longer one is most likely two strips fed with no gap between them.
pub const MAX_STRIP_FRAMES: usize = 6;

/// Strip mode settings for [`scan_loop`]. Each strip's length is measured,
/// not chosen.
#[derive(Debug, Clone, Copy)]
pub struct StripMode {
    /// Picture pitch in scan lines.
    pub pitch: usize,
    /// The LOW 0x91 trigger, resent before each strip after the first.
    pub trigger: u16,
}

impl StripMode {
    /// Longest strip the OEM's buffers take: 6 × pitch + pitch / 4 lines.
    pub fn max_rows(&self) -> usize {
        MAX_STRIP_FRAMES * self.pitch + self.pitch / 4
    }
}

/// One strip of a strip-mode scan: raw rows `[start, end)` (its open-gate
/// lead included) and the DX frame numbers read while it went through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StripRec {
    pub start: usize,
    pub film: usize,
    pub end: usize,
    pub dx: Vec<u8>,
}

/// What the image loop did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopStats {
    /// Image bytes read.
    pub bytes: u64,
    /// Image bytes written to the raw (less than `bytes` in strip mode,
    /// which drops the open gate between strips).
    pub written: u64,
    pub reads: u64,
    pub cmds: u64,
    pub errors: u64,
    pub events: u32,
    pub overflows: u32,
    pub film_seen: bool,
    /// Strip mode: the strips scanned, in feed order.
    pub strips: Vec<StripRec>,
}

/// The open gate before the film or between strips, kept to its last `cap`
/// bytes; dropped
/// only in whole rows, so the raw's row alignment holds.
struct Lead {
    buf: Vec<u8>,
    cap: usize,
    row: usize,
}

impl Lead {
    fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let excess = self.buf.len() - self.cap;
            let drop = excess - excess % self.row;
            self.buf.drain(..drop);
        }
    }
}

/// The driven image phase: read 0x86 and serve events after every read
/// (no re-arm); stop on `TRAIL_WHITE` open-gate chunks after film, empty
/// windows after film, the byte cap, or `stop`.
///
/// The open gate before the film is not written but its last
/// [`STRIP_LEAD_ROWS`] rows (`row_bytes` per scan line): writing starts
/// with the first chunk that is not open gate. A dim or dense leader is
/// written as it comes.
///
/// In strip mode (`strips`) the end of a strip does not stop it: the strip
/// is recorded, LOW 0x91 is resent and the film LED blinks green until the
/// next strip comes; the open gate in between is not written. It stops on
/// `stop`, [`MAX_STRIPS`], [`STRIP_WAIT`] with no new strip, or the cap.
#[allow(clippy::too_many_arguments)]
pub fn scan_loop(
    link: &mut dyn Link,
    st: &SetupState,
    img: &mut dyn Write,
    row_bytes: usize,
    timeout: Duration,
    max_bytes: u64,
    white: (u16, usize),
    stop: &AtomicBool,
    roll: &mut dx::Roll,
    strips: Option<StripMode>,
    out: Out,
) -> LoopStats {
    let mut s = LoopStats::default();
    let mut buf = vec![0u8; CHUNK];
    let (mut trail_white, mut idle) = (0u32, 0u32);
    let started = std::time::Instant::now();
    // strip mode: the strip being scanned (film seen in it), waiting for
    // the next one, and where the current strip's DX codes begin
    let mut in_strip = false;
    let mut waiting: Option<std::time::Instant> = None;
    let mut strip_start = 0usize;
    let mut film_row = 0usize;
    let mut code_mark = 0usize;
    let mut warned_long = false;
    let row = row_bytes.max(1);
    // before the film: open gate held back (only its tail is written)
    let mut holding = true;
    let mut lead = Lead { buf: Vec::new(), cap: STRIP_LEAD_ROWS * row, row };
    let rows_written = |s: &LoopStats| s.written as usize / row;
    out(&format!(
        "  [sm] driven image phase: read 0x86 + event service (no re-arm); stop on {TRAIL_WHITE} trailing open-gate chunks, {MAX_EMPTY} empty windows after film, {} MB cap, or Ctrl-C / SIGTERM",
        max_bytes >> 20
    ));
    if strips.is_some() {
        out(&format!("  [sm] strip mode: up to {MAX_STRIPS} strips; ends on Stop or {} s with no new strip", STRIP_WAIT.as_secs()));
    }
    let new_codes = |roll: &dx::Roll, from: usize| -> Vec<u8> { roll.code[from.min(roll.code.len())..].iter().map(|c| c.frame_raw).collect() };
    loop {
        if stop.load(Ordering::Relaxed) {
            out(&format!("  [sm] stop requested -> end of roll ({} bytes{})", s.bytes, if s.film_seen { "" } else { ", no film seen" }));
            break;
        }
        if let Some(t) = waiting {
            if t.elapsed() >= STRIP_WAIT {
                out(&format!("  [sm] no new strip within {} s -> end of roll", STRIP_WAIT.as_secs()));
                break;
            }
        }
        let r = link.recv_image(&mut buf, IMG_TIMEOUT);
        let got = *r.as_ref().unwrap_or(&0);
        if got > 0 {
            let chunk = &buf[..got];
            s.bytes += got as u64;
            s.reads += 1;
            idle = 0;
            let white_chunk = chunk_is_white(chunk, white.0, white.1);
            let film_chunk = !white_chunk && chunk_has_film(chunk);
            // a new strip: its lead goes into the raw first
            if waiting.is_some() && film_chunk {
                waiting = None;
                strip_start = rows_written(&s);
                if img.write_all(&lead.buf).is_err() {
                    s.errors += 1;
                }
                s.written += lead.buf.len() as u64;
                lead.buf.clear();
                panel(link, st, PANEL_FEED, timeout, out);
            }
            if holding && !white_chunk {
                holding = false;
                if img.write_all(&lead.buf).is_err() {
                    s.errors += 1;
                }
                s.written += lead.buf.len() as u64;
                lead.buf.clear();
            }
            let pre = rows_written(&s);
            if waiting.is_some() || holding {
                lead.push(chunk);
            } else {
                if img.write_all(chunk).is_err() {
                    s.errors += 1;
                }
                s.written += got as u64;
            }
            // the OEM polls every 1 ms while scanning
            match cmd::service_events(link, st.low, st.scn, timeout, Some(roll)) {
                Ok(ev) => {
                    s.events += ev.serviced;
                    if ev.host_flags & HOST_FLAG_OVERFLOW != 0 {
                        s.overflows += 1;
                    }
                }
                Err(_) => s.errors += 1,
            }
            s.cmds += 1;
            if white_chunk {
                if in_strip || (strips.is_none() && s.film_seen) {
                    trail_white += 1;
                    if trail_white >= TRAIL_WHITE {
                        let Some(m) = strips else {
                            out(&format!("  [sm] end-of-roll: {trail_white} open-gate chunks after film -> film fully scanned ({} bytes)", s.bytes));
                            break;
                        };
                        // the end of a strip: record it, re-arm the DX
                        // reader as the OEM does, wait for the next
                        let end = rows_written(&s);
                        let n = s.strips.len() + 1;
                        let dx = new_codes(roll, code_mark);
                        code_mark = roll.code.len();
                        out(&format!("  [sm] strip {n} done: rows {strip_start}-{end}, {} DX codes", dx.len()));
                        s.strips.push(StripRec { start: strip_start, film: film_row, end, dx });
                        in_strip = false;
                        trail_white = 0;
                        if n >= MAX_STRIPS {
                            out(&format!("  [sm] {MAX_STRIPS} strips -> end of roll"));
                            break;
                        }
                        let mut seq = Seq::new();
                        setup::dx_start(&mut seq, st, m.trigger);
                        if cmd::run_seq(link, &seq, timeout).is_err() {
                            s.errors += 1;
                            out("  [sm] DX start (0x91) for the next strip failed");
                        }
                        panel(link, st, PANEL_NEXT_STRIP, timeout, out);
                        out(&format!("  [sm] feed strip {}", n + 1));
                        waiting = Some(std::time::Instant::now());
                    }
                }
            } else if film_chunk {
                if !in_strip && strips.is_some() {
                    film_row = pre;
                    warned_long = false;
                    out(&format!("  [sm] film detected · strip {} at {} bytes", s.strips.len() + 1, s.bytes));
                } else if !s.film_seen {
                    out(&format!("  [sm] film detected at {} bytes", s.bytes));
                }
                s.film_seen = true;
                in_strip = true;
                trail_white = 0;
            }
            if let (Some(m), true) = (strips, in_strip) {
                let len = rows_written(&s).saturating_sub(film_row);
                if !warned_long && len > m.max_rows() {
                    warned_long = true;
                    out(&format!("  [sm] warning: strip {} is longer than {MAX_STRIP_FRAMES} frames (two strips with no gap?)", s.strips.len() + 1));
                }
            }
            if max_bytes > 0 && s.written >= max_bytes {
                out(&format!("  [sm] hit safety cap {} MB -> stopping", max_bytes >> 20));
                break;
            }
            if !s.film_seen && started.elapsed() >= FEED_WAIT {
                out(&format!("  [sm] no film fed within {} s -> aborting", FEED_WAIT.as_secs()));
                break;
            }
            continue;
        }
        if !matches!(r, Ok(_) | Err(ScanError::Timeout)) {
            s.errors += 1;
        }
        // An empty window: poll the host (read-only). No re-arm: kicking 8a
        // into a live stream wedges the bus.
        let mut hf = 0xFF;
        match cmd::service_events(link, st.low, st.scn, timeout, Some(roll)) {
            Ok(ev) => {
                hf = ev.host_flags;
                s.events += ev.serviced;
            }
            Err(_) => s.errors += 1,
        }
        s.cmds += 1;
        if hf & HOST_FLAG_EVENT != 0 {
            continue;
        }
        idle += 1;
        if !s.film_seen {
            if idle >= LOAD_WAIT {
                out(&format!("  [sm] no film fed within ~{} s -> aborting", idle as u64 * IMG_TIMEOUT.as_secs()));
                break;
            }
            out(&format!("  [sm] waiting for film... ({idle}/{LOAD_WAIT}, HOST st=0x{hf:02x})"));
            continue;
        }
        if waiting.is_some() {
            // between strips: STRIP_WAIT decides
            continue;
        }
        out(&format!("  [sm] empty read after film, HOST st=0x{hf:02x} (idle {idle}/{MAX_EMPTY})"));
        if idle >= MAX_EMPTY {
            out(&format!("  [sm] stream ended (no data for {idle} windows) -> end of roll"));
            break;
        }
    }
    // no film came: the open gate's tail, so the raw is not empty
    if holding && !lead.buf.is_empty() {
        if img.write_all(&lead.buf).is_err() {
            s.errors += 1;
        }
        s.written += lead.buf.len() as u64;
    }
    // a strip cut short by Stop, the cap or the stream: keep what came
    if strips.is_some() && in_strip {
        let end = rows_written(&s);
        let dx = new_codes(roll, code_mark);
        out(&format!("  [sm] strip {} ended early: rows {strip_start}-{end}", s.strips.len() + 1));
        s.strips.push(StripRec { start: strip_start, film: film_row, end, dx });
    }
    out(&format!("  [sm] event service: {} events acknowledged, {} FIFO-overflow flags", s.events, s.overflows));
    out(&format!("  [sm] dx: {roll}"));
    s
}

/// `<raw>.strips.json`: each strip's raw rows and the DX frame numbers read
/// on it, in feed order.
pub fn strips_json(row_samples: usize, strips: &[StripRec]) -> String {
    let mut j = format!("{{\n  \"version\": 1,\n  \"row_samples\": {row_samples},\n  \"strips\": [\n");
    for (i, st) in strips.iter().enumerate() {
        let dx: Vec<String> = st.dx.iter().map(|v| v.to_string()).collect();
        j += &format!(
            "    {{\"start\": {}, \"film\": {}, \"end\": {}, \"dx\": [{}]}}{}\n",
            st.start,
            st.film,
            st.end,
            dx.join(", "),
            if i + 1 < strips.len() { "," } else { "" }
        );
    }
    j += "  ]\n}\n";
    j
}

/// The film's DX code next to the raw (`<raw>.dx.json`); nothing when no
/// code was read.
pub fn dx_json(roll: &dx::Roll) -> Option<String> {
    let (prod, gen) = roll.film()?;
    Some(format!(
        "{{\n  \"version\": 1,\n  \"product\": {prod},\n  \"generation\": {gen},\n  \"codes\": {},\n  \"rejected\": {},\n  \"reads\": {}\n}}\n",
        roll.codes, roll.rejected, roll.blocks
    ))
}

fn with_suffix(p: &Path, suffix: &str) -> std::path::PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// A code-built scan's options.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub base: u32,
    pub ir: bool,
    pub film: Film,
    pub recalibrate: bool,
    /// Image byte cap (0 = none); a full 36-exposure roll is up to ~2 GB.
    pub max_bytes: u64,
    /// Strip mode: cut strips fed one after another into one raw; off, the
    /// scan ends with the film (a full roll or one strip).
    pub strips: bool,
    pub timeout: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { base: 16, ir: true, film: Film::C41, recalibrate: false, max_bytes: 4096 << 20, strips: false, timeout: Duration::from_millis(1000) }
    }
}

/// The whole code-built scan into `image` (and `<image>.flat.json`,
/// `<image>.dx.json`): setup, lamp gate, calibration (reused when recent),
/// scan start, the image loop, teardown. Returns whether film
/// was seen.
pub fn scan_code(link: &mut dyn Link, eeprom: Eeprom, eeprom_dir: &Path, image: &Path, opt: &ScanOptions, stop: &AtomicBool, out: Out) -> Result<LoopStats, ScanError> {
    let s = session::setup_session(link, eeprom, opt.base, opt.ir, opt.timeout, out)?;
    let t = opt.timeout.max(Duration::from_millis(2000));
    let td = Teardown::new(&mut *link, s.st, t);
    let r = scan_body(&mut *td.link, &s, eeprom_dir, image, opt, stop, out);
    let fin = td.finish();
    out(&format!("teardown (code-built): {}", if fin.is_ok() { "OK" } else { "FAILED" }));
    panel(link, &s.st, PANEL_IDLE, t, out);
    let stats = r?;
    if stats.film_seen { Ok(stats) } else { Err(ScanError::Proto("no film seen".into())) }
}

fn scan_body(link: &mut dyn Link, s: &crate::session::Session, eeprom_dir: &Path, image: &Path, opt: &ScanOptions, stop: &AtomicBool, out: Out) -> Result<LoopStats, ScanError> {
    let t = opt.timeout.max(Duration::from_millis(2000));
    let (period, offset) = (s.led_period(), s.offset());
    let eb = Base::from_base(opt.base).expect("checked by the session");
    out(&format!(
        "film: {}",
        match opt.film {
            Film::Bw => "B&W (silver)",
            Film::Slide => "slide (colour reversal)",
            Film::BwC41 => "B&W C-41 (chromogenic)",
            _ => "colour negative",
        }
    ));
    calib::lamp_wait(link, &s.st, t, "scan-code", out)?;
    let res = calib::scan_calibrate(link, s, t, eeprom_dir, opt.film, opt.recalibrate, out).inspect_err(|_| out("scan-code: calibration failed"))?;
    let density = opt.film.density();
    let scan_duty: [u16; 4] = std::array::from_fn(|c| lc::scan_duty(res.duty[c], density[c], period));
    calib::write_flat(&with_suffix(image, ".flat.json"), s.eeprom.serial, offset, opt.base, opt.film, &res, Some(scan_duty), out)?;
    // The EEPROM keeps a transport speed per base for IR on and off.
    let (b, a) = (s.eeprom.base(eb), s.eeprom.adjust(eb));
    let speed = if s.mode.ir { setup::motor_speed(b.motor_speed_ir, a.ir, s.model()) } else { setup::motor_speed(b.motor_speed, a.normal, s.model()) };
    out(&format!(
        "calibrated: gains {}/{}/{}, offsets {}/{}/{}, currents {}/{}/{}/{}, scan duties {}/{}/{}/{}, motor speed {speed}",
        res.gain[0], res.gain[1], res.gain[2], res.off[0], res.off[1], res.off[2], res.cur[0], res.cur[1], res.cur[2], res.cur[3], scan_duty[0], scan_duty[1], scan_duty[2], scan_duty[3]
    ));
    let mut img = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(image).map_err(|e| {
        out(&format!("cannot open image '{}'", image.display()));
        ScanError::Usb(e.to_string())
    })?);
    let mut st = s.st;
    let mut seq = Seq::new();
    setup::scan_start(&mut seq, &mut st, offset, scan_duty, period, speed, s.mode.trigger).map_err(|e| ScanError::Param(e.to_string()))?;
    cmd::run_seq(link, &seq, t).inspect_err(|_| out("scan-code: scan start failed"))?;
    panel(link, &st, PANEL_FEED, t, out); // right LED green: feed now
    out("  [sm] motor start (a0) sent -- feed the film now");
    let mut roll = dx::Roll::new();
    let row_samples = lc::blocks(res.ir) * (res.line_px - res.px0);
    let strips = opt.strips.then_some(StripMode { pitch: pitch_rows(opt.base), trigger: s.mode.trigger });
    let stats = scan_loop(link, &st, &mut img, row_samples * 2, opt.timeout, opt.max_bytes, (WHITE_THRESH_CODE, WHITE_FRAC_CODE_PCT), stop, &mut roll, strips, out);
    let _ = img.flush();
    if strips.is_some() {
        let p = with_suffix(image, ".strips.json");
        if std::fs::write(&p, strips_json(row_samples, &stats.strips)).is_err() {
            out(&format!("cannot write '{}'", p.display()));
        }
    }
    if let Some(j) = dx_json(&roll) {
        let p = with_suffix(image, ".dx.json");
        if std::fs::write(&p, j).is_err() {
            out(&format!("cannot write '{}'", p.display()));
        }
    }
    out(&format!(
        "scan-code: {} image reads, {} bytes -> {} ({} errors){}",
        stats.reads,
        stats.written,
        image.display(),
        stats.errors,
        if stats.film_seen { "" } else { " -- no film seen" }
    ));
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(v: u16, frac: f64) -> Vec<u8> {
        let n = CHUNK / 2;
        (0..n).flat_map(|i| if (i as f64) < frac * n as f64 { v } else { 1000 }.to_le_bytes()).collect()
    }

    #[test]
    fn white_and_film_chunks() {
        assert!(chunk_is_white(&chunk(60000, 0.5), WHITE_THRESH_CODE, WHITE_FRAC_CODE_PCT));
        assert!(!chunk_is_white(&chunk(60000, 0.4), WHITE_THRESH_CODE, WHITE_FRAC_CODE_PCT));
        assert!(chunk_has_film(&chunk(15000, 0.2)));
        assert!(!chunk_has_film(&chunk(2400, 1.0)));
        assert!(!chunk_is_white(&[], 1, 1));
    }
}
