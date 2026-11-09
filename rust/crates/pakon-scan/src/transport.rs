//! Moving the film without scanning (port of `do_transport` and
//! `timed_advance_on` in `tools/pakon_replay.c`): the F-135 eject / advance
//! built in code, reading the DX sensors to see the strip leave, and the
//! F-135+ timed advance from its capture. The motor is stopped on every way
//! out, a panic included.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pakon_proto::eeprom::Eeprom;
use pakon_proto::frame::{Address, Frame, PacketType};
use pakon_proto::setup::{self, Model, Seq, Sensor, SetupState, ADVANCE_SPEED_F135};

use crate::cmd::{self, Pic};
use crate::link::Link;
use crate::session::{self, Out};
use crate::ScanError;

/// Stops the motor when dropped, unless disarmed.
struct MotorStop<'a> {
    link: &'a mut dyn Link,
    st: SetupState,
    t: Duration,
    armed: bool,
}

impl MotorStop<'_> {
    fn finish(mut self) -> Result<(), ScanError> {
        self.armed = false;
        stop_motor(self.link, &self.st, self.t)
    }
}

impl Drop for MotorStop<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = stop_motor(self.link, &self.st, self.t);
        }
    }
}

fn stop_motor(link: &mut dyn Link, st: &SetupState, t: Duration) -> Result<(), ScanError> {
    let mut seq = Seq::new();
    setup::motor_stop(&mut seq, st);
    cmd::run_seq(link, &seq, t)
}

fn read_levels(link: &mut dyn Link, st: &SetupState, t: Duration) -> Option<[u8; 4]> {
    let r = cmd::cmd(link, &setup::sense_read_frame(st.low), true, t).ok()??;
    let d = r.data();
    (r.raw_type() == PacketType::Read as u8 && d.len() >= 6 && d[0] == st.low).then(|| [d[2], d[3], d[4], d[5]])
}

/// How a transport run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportEnd {
    TimeLimit,
    StripOut,
    Stalled,
    NoFilm,
    Stopped,
}

impl TransportEnd {
    pub fn text(self) -> &'static str {
        match self {
            TransportEnd::TimeLimit => "time limit reached",
            TransportEnd::StripOut => "strip is out",
            TransportEnd::Stalled => "strip stalled at the exit: pull it out by hand",
            TransportEnd::NoFilm => "no film in the transport",
            TransportEnd::Stopped => "stopped",
        }
    }
}

/// Run the F-135 transport at the OEM advance speed for at most `limit`;
/// with `eject`, until the DX sensors see the strip leave.
pub fn transport(link: &mut dyn Link, eeprom: Eeprom, eject: bool, limit: Duration, timeout: Duration, stop: &AtomicBool, out: Out) -> Result<TransportEnd, ScanError> {
    let s = session::setup_session(link, eeprom, 16, true, timeout, out)?;
    let t = timeout.max(Duration::from_millis(2000));
    if s.model() != Model::F135 {
        out("transport: F-135 only for now");
        return Err(ScanError::Param("F-135 only".into()));
    }
    let guard = MotorStop { link: &mut *link, st: s.st, t, armed: true };
    let mut seq = Seq::new();
    setup::motor_run(&mut seq, &s.st, ADVANCE_SPEED_F135).map_err(|e| ScanError::Param(e.to_string()))?;
    cmd::run_seq(guard.link, &seq, t).inspect_err(|_| out("transport: motor start failed"))?;
    out(&format!("motor running ({}, at most {} s)", if eject { "eject" } else { "advance" }, limit.as_secs()));

    let t0 = Instant::now();
    let (mut film_seen, mut exit_seen) = (false, false);
    let (mut exit_clear, mut flat, mut nothing) = (0u32, 0u32, 0u32);
    let mut prev = [0u8; 4];
    let mut end = TransportEnd::TimeLimit;
    while t0.elapsed() < limit {
        if stop.load(Ordering::Relaxed) {
            end = TransportEnd::Stopped;
            break;
        }
        let _ = cmd::service_events(guard.link, s.st.low, s.st.scn, t, None);
        if eject {
            if let Some(v) = read_levels(guard.link, &s.st, t) {
                let in_clear = setup::sense_clear(v, Sensor::Entry);
                let out_clear = setup::sense_clear(v, Sensor::Exit);
                film_seen |= !in_clear || !out_clear;
                exit_seen |= !out_clear;
                // out: the exit sensor had film and both are now clear
                exit_clear = if exit_seen && in_clear && out_clear { exit_clear + 1 } else { 0 };
                nothing = if in_clear && out_clear && !film_seen { nothing + 1 } else { 0 };
                let still = (v[2] as i32 - prev[2] as i32).abs() <= 4 && (v[3] as i32 - prev[3] as i32).abs() <= 4;
                flat = if in_clear && !out_clear && still { flat + 1 } else { 0 };
                prev = v;
                if exit_clear >= 5 {
                    end = TransportEnd::StripOut;
                    break;
                }
                if flat >= 15 {
                    end = TransportEnd::Stalled;
                    break;
                }
                if nothing >= 150 {
                    end = TransportEnd::NoFilm;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let r = guard.finish();
    out(&format!("motor stop: {}", if r.is_ok() { "OK" } else { "FAILED" }));
    out(&format!("{}: {}", if eject { "eject" } else { "advance" }, if eject { end.text() } else { "done" }));
    Ok(end)
}

/// The F-135+ timed advance, as captured (docs/F135_PLUS_CAPTURES.md):
/// motor speed 0x647E, engage, acquire on, a status keep-alive every
/// second, then acquire off before the drive is released (0xA2 alone
/// leaves the motor running) and the status LEDs as captured. Refused on
/// an F-135, whose speed limit the captured value exceeds.
pub fn timed_advance_plus(link: &mut dyn Link, seconds: u32, timeout: Duration, stop: &AtomicBool, out: Out) -> Result<(), ScanError> {
    cmd::bridge_open(link, timeout);
    let picm = [Address::PicMPlus as u8, Address::PicM as u8].into_iter().find(|&a| cmd::probe_pic(link, a, timeout) == Pic::Present);
    let Some(m) = picm else {
        out("advance: no motor PIC answered (0x44/0x24)");
        return Err(ScanError::NoDevice);
    };
    if m != Address::PicMPlus as u8 {
        out("advance: F-135 detected. The timed --advance replays F-135+ captured values\n  (motor speed 0x647E, above the F-135's 9500 limit) and is not used on this model.\n  Use the validated frame advance instead:\n    pakon_replay resources/advance.pakscan --steps N");
        return Err(ScanError::Param("F-135: use the frame advance".into()));
    }
    out(&format!("advance: motor PIC at 0x{m:02x} (F-135+), running transport for {seconds}s"));
    let mut errs = 0;
    let mut send = |link: &mut dyn Link, ty: PacketType, d: &[u8]| {
        if cmd::cmd(link, &Frame::new(ty, d).expect("short"), true, timeout).is_err() {
            errs += 1;
        }
    };
    send(link, PacketType::Write, &[m, 0x02, 0xA5, 0x7E, 0x64]);
    send(link, PacketType::Command, &[m, 0x00, 0xA0]);
    send(link, PacketType::Write, &[m, 0x03, 0x82, 0x00, 0x63, 0x00]);
    let mut interrupted = false;
    for _ in 0..seconds {
        if stop.load(Ordering::Relaxed) {
            interrupted = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
        let _ = cmd::cmd(link, &Frame::new(PacketType::ReadStatus, &[m]).expect("short"), true, timeout); // keep-alive
    }
    if interrupted {
        out("advance: interrupted — stopping the motor before exit");
    }
    send(link, PacketType::Write, &[m, 0x03, 0x82, 0x00, 0x60, 0x00]);
    send(link, PacketType::Command, &[m, 0x00, 0xA2]);
    send(link, PacketType::Write, &[m, 0x03, 0x82, 0x09, 0x17, 0x02]);
    send(link, PacketType::Write, &[m, 0x03, 0x82, 0x09, 0x17, 0x00]);
    out(&format!("advance: done ({errs} command errors)"));
    if errs > 0 || interrupted { Err(ScanError::Proto(format!("{errs} command errors"))) } else { Ok(()) }
}
