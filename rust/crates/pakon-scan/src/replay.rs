//! Replaying a captured scan (`.pakscan`; port of `do_scan`,
//! `sm_replay_teardown` and `drain_image` in `tools/pakon_replay.c`), the
//! F-135+ scan until its setup is built in code. With `autostop` the end of
//! the film ends the replay early (armed at motor start), and then the
//! capture's own teardown tail is replayed so the engines stop.
//!
//! Stricter than the C: every captured frame passes the safety rules before
//! it is sent (no type 0; to a bootloader address only the presence probe
//! `04 03 <addr> 00 00`), and the transport refuses vendor request 0xA2.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pakon_proto::frame::{Address, PacketType, PACKET_SIZE};
use pakon_proto::script::Line;
use pakon_usb::device::Control;

use crate::calib::IMG_TIMEOUT;
use crate::cmd;
use crate::link::Link;
use crate::scan::{chunk_has_film, chunk_is_white, WHITE_FRAC_PCT, WHITE_THRESH};
use crate::session::Out;
use crate::ScanError;

const TRAIL_WHITE: u32 = 24;
const MAX_READ: usize = 65536;

/// A captured frame the safety rules allow.
pub fn frame_is_safe(b: &[u8]) -> bool {
    if b.len() < 3 || b[0] == 0 || b[1] as usize + 2 != b.len() {
        return false;
    }
    let boot = Address::from_u8(b[2]).is_some_and(|a| a.is_bootloader());
    !boot || b == [PacketType::Command as u8, 0x03, b[2], 0x00, 0x00]
}

/// A script's `C` line: `bmReqType bReq wValue wIndex wLen [data]`.
pub fn parse_control(text: &str) -> Option<Control> {
    let w: Vec<&str> = text.split_whitespace().collect();
    if w.len() < 5 {
        return None;
    }
    let h = |s: &str| u16::from_str_radix(s, 16).ok();
    let (bm, rq, wv, wi, wl) = (h(w[0])? as u8, h(w[1])? as u8, h(w[2])?, h(w[3])?, h(w[4])?);
    let s = w.get(5).copied().unwrap_or("");
    let data = if bm & 0x80 == 0 { (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16)).collect::<Result<Vec<u8>, _>>().ok()? } else { vec![] };
    Some(Control { request_type: bm, request: rq, value: wv, index: wi, length: wl, data })
}

/// Check a whole script before anything is sent.
pub fn check_script(lines: &[Line]) -> Result<(), ScanError> {
    for (i, l) in lines.iter().enumerate() {
        match l {
            Line::Out(b) if !frame_is_safe(b) => return Err(ScanError::Param(format!("script line {i}: unsafe frame {b:02x?}"))),
            Line::Control(c) => {
                let x = parse_control(c).ok_or_else(|| ScanError::Param(format!("script line {i}: bad C line")))?;
                if (x.request_type >> 5) & 3 == 2 && x.request == 0xA2 {
                    return Err(ScanError::Param(format!("script line {i}: vendor request 0xa2")));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayStats {
    pub cmds: u64,
    pub reads: u64,
    pub bytes: u64,
    pub errors: u64,
    pub stopped_early: bool,
    pub interrupted: bool,
    pub teardown_sent: bool,
}

fn send_raw(link: &mut dyn Link, b: &[u8], timeout: Duration) -> Result<Vec<u8>, ScanError> {
    link.send_cmd(b, timeout)?;
    let mut r = [0u8; PACKET_SIZE];
    let n = link.recv_cmd(&mut r, timeout)?;
    Ok(r[..n].to_vec())
}

fn is_motor_start(b: &[u8]) -> bool {
    b.len() >= 5 && b[0] == 0x04 && b[1] == 0x03 && (b[2] == Address::PicM as u8 || b[2] == Address::PicMPlus as u8) && b[3] == 0 && b[4] == 0xA0
}

/// Replay the captured teardown tail: every O/C line after the last image
/// read (the stop and engine reset). Returns the commands replayed.
pub fn replay_teardown(link: &mut dyn Link, lines: &[Line], timeout: Duration, st: &mut ReplayStats) -> u64 {
    let Some(last) = lines.iter().rposition(|l| matches!(l, Line::Image(_))) else { return 0 };
    let mut n = 0;
    for l in &lines[last + 1..] {
        let ok = match l {
            Line::Out(b) => send_raw(link, b, timeout).is_ok(),
            Line::Control(c) => parse_control(c).is_some_and(|x| link.control(&x, timeout).is_ok()),
            Line::Image(_) => continue,
        };
        if !ok {
            st.errors += 1;
        }
        st.cmds += 1;
        n += 1;
    }
    n
}

/// Replay `lines` (checked first), writing image reads to `img`.
#[allow(clippy::too_many_arguments)]
pub fn replay_scan(link: &mut dyn Link, lines: &[Line], img: &mut dyn Write, timeout: Duration, autostop: bool, drain: bool, stop: &AtomicBool, out: Out) -> Result<ReplayStats, ScanError> {
    check_script(lines)?;
    let mut st = ReplayStats::default();
    let mut buf = vec![0u8; MAX_READ];
    let (mut film_seen, mut armed, mut trail_white) = (false, false, 0u32);
    if autostop {
        out(&format!("  [autostop] will arm at motor-start, then stop at end-of-roll white ({TRAIL_WHITE} trailing-white chunks after film)"));
    }
    for l in lines {
        if stop.load(Ordering::Relaxed) {
            out("  [interrupt] Ctrl-C: stopping scan, replaying captured teardown...");
            st.stopped_early = true;
            st.interrupted = true;
            break;
        }
        match l {
            Line::Out(b) => {
                if autostop && !armed && is_motor_start(b) {
                    armed = true;
                    out(&format!("  [autostop] motor started at read {} -> arming end-of-roll", st.reads));
                }
                if send_raw(link, b, timeout).is_err() {
                    st.errors += 1;
                }
                st.cmds += 1;
            }
            Line::Image(want) => {
                let want = (*want).min(MAX_READ);
                let r = link.recv_image(&mut buf[..want], IMG_TIMEOUT);
                let got = *r.as_ref().unwrap_or(&0);
                if got > 0 {
                    if img.write_all(&buf[..got]).is_err() {
                        st.errors += 1;
                    }
                    st.bytes += got as u64;
                }
                if !matches!(r, Ok(_) | Err(ScanError::Timeout)) {
                    st.errors += 1;
                }
                st.reads += 1;
                if st.reads % 1000 == 0 {
                    out(&format!("  ... {} image reads, {} bytes", st.reads, st.bytes));
                }
                if autostop && armed && got > 0 {
                    let chunk = &buf[..got];
                    if chunk_is_white(chunk, WHITE_THRESH, WHITE_FRAC_PCT) {
                        if film_seen {
                            trail_white += 1;
                            if trail_white >= TRAIL_WHITE {
                                out(&format!("  [autostop] end-of-roll white ({trail_white} chunks) after {} reads / {} bytes -> stopping early", st.reads, st.bytes));
                                st.stopped_early = true;
                                break;
                            }
                        }
                    } else if chunk_has_film(chunk) {
                        if !film_seen {
                            out(&format!("  [autostop] film detected at read {}", st.reads));
                        }
                        film_seen = true;
                        trail_white = 0;
                    }
                }
            }
            Line::Control(c) => {
                let x = parse_control(c).expect("checked");
                if link.control(&x, timeout).is_err() {
                    st.errors += 1;
                }
            }
        }
    }
    if st.stopped_early {
        out("  [teardown] replaying captured teardown (stop + engine reset)...");
        let n = replay_teardown(link, lines, timeout, &mut st);
        out(&format!("  [teardown] {n} commands replayed"));
        st.teardown_sent = true;
    } else if drain {
        out("  [drain] script exhausted, draining 0x86 until scanner signals done...");
        let poll = pakon_proto::frame::Frame::new(PacketType::ReadStatus, &[Address::Host as u8]).expect("short");
        let mut chunk = vec![0u8; 20480];
        while !stop.load(Ordering::Relaxed) {
            if cmd::cmd(link, &poll, true, timeout).is_err() {
                st.errors += 1;
            }
            st.cmds += 1;
            let r = link.recv_image(&mut chunk, IMG_TIMEOUT);
            let got = *r.as_ref().unwrap_or(&0);
            if got > 0 {
                let _ = img.write_all(&chunk[..got]);
                st.bytes += got as u64;
            }
            st.reads += 1;
            if got == 0 {
                break;
            }
        }
        if stop.load(Ordering::Relaxed) {
            out("  [interrupt] Ctrl-C during drain: replaying captured teardown...");
            let n = replay_teardown(link, lines, timeout, &mut st);
            out(&format!("  [teardown] {n} commands replayed"));
            st.interrupted = true;
            st.teardown_sent = true;
        }
    }
    let _ = img.flush();
    out(&format!(
        "\nscan replay done: {} commands, {} image reads, {} image bytes ({} transfer errors){}",
        st.cmds,
        st.reads,
        st.bytes,
        st.errors,
        if !st.interrupted {
            ""
        } else if st.teardown_sent {
            " [interrupted by Ctrl-C, teardown sent]"
        } else {
            " [interrupted by Ctrl-C]"
        }
    ));
    if st.interrupted {
        return Err(ScanError::Proto("interrupted: the raw is truncated".into()));
    }
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_of_captured_frames() {
        assert!(frame_is_safe(&[4, 3, 0x46, 0, 0]), "the presence probe");
        assert!(!frame_is_safe(&[4, 3, 0x46, 0, 0x0C]), "a flash command");
        assert!(!frame_is_safe(&[2, 4, 0x22, 1, 0x80, 0]));
        assert!(!frame_is_safe(&[0, 3, 0x20, 0, 0]), "type 0");
        assert!(!frame_is_safe(&[4, 4, 0x20, 0, 0]), "length mismatch");
        assert!(frame_is_safe(&[2, 4, 0x10, 1, 0x8F, 0]));
    }

    #[test]
    fn every_capture_passes_the_check() {
        let res = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources");
        let mut n = 0;
        let dirs = [res.clone(), res.join("f135plus")];
        for p in dirs.iter().filter_map(|d| std::fs::read_dir(d).ok()).flat_map(|r| r.flatten().map(|e| e.path())) {
            if p.extension().is_some_and(|e| e == "pakscan") {
                let lines = pakon_proto::script::parse_file(&p).unwrap();
                check_script(&lines).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                n += 1;
            }
        }
        assert!(n >= 6, "{n} scripts");
    }
}
