//! Commands with the OEM reply rules (port of `pakon_cmd.c`): the one place
//! that uses both the protocol and the transport, through [`Link`].

use std::time::{Duration, Instant};

use pakon_proto::dx;
use pakon_proto::frame::{Address, Frame, PacketType};
use pakon_proto::setup::{self, Seq, HOST_FLAG_EVENT, HOST_FLAG_OVERFLOW, LAMP_FAULT, LAMP_STABLE};

use crate::link::Link;
use crate::{log, ScanError};

const HOST: u8 = Address::Host as u8;
const BUSY_POLL_TRIES: u64 = 44;
const REPLY_TRIES: u32 = 3;

// reply statuses
const PS_SUCCESS: u8 = 0;
const PS_NOT_ACKED: u8 = 1;
const PS_BAD_CHECKSUM: u8 = 3;
const PS_USB_6: u8 = 6;
const PS_SUCCESS_8: u8 = 8;
const PS_BUS_ERROR: u8 = 9;

fn is_bootloader(a: u8) -> bool {
    Address::from_u8(a).is_some_and(|a| a.is_bootloader())
}

/// Send `cmd` on EP1 OUT and, with `reply`, read and parse the reply.
pub fn cmd(link: &mut dyn Link, f: &Frame, reply: bool, timeout: Duration) -> Result<Option<Frame>, ScanError> {
    if f.raw_type() == 0 {
        return Err(ScanError::Param("frame type 0".into()));
    }
    let wire = f.to_bytes();
    let sent = link.send_cmd(&wire, timeout)?;
    if sent != wire.len() {
        return Err(ScanError::Usb(format!("sent {sent} of {} bytes", wire.len())));
    }
    if !reply {
        return Ok(None);
    }
    let mut buf = [0u8; pakon_proto::frame::PACKET_SIZE];
    let n = link.recv_cmd(&mut buf, timeout)?;
    Frame::parse(&buf[..n]).map(Some).map_err(|e| ScanError::Proto(e.to_string()))
}

fn cmd_reply(link: &mut dyn Link, f: &Frame, timeout: Duration) -> Result<Frame, ScanError> {
    cmd(link, f, true, timeout).map(|r| r.expect("a reply was asked for"))
}

fn frame(ty: PacketType, data: &[u8]) -> Frame {
    Frame::new(ty, data).expect("a short frame")
}

/// Best-effort bridge open: HostReset (`04 03 10 00 85`) + HostSetMode
/// (`02 04 10 01 8f 00`). Only the first open after power-on or a firmware
/// load gets replies; timeouts are expected after that and ignored.
pub fn bridge_open(link: &mut dyn Link, timeout: Duration) {
    let _ = cmd(link, &frame(PacketType::Command, &[HOST, 0x00, 0x85]), true, timeout);
    let _ = cmd(link, &frame(PacketType::Write, &[HOST, 0x01, 0x8F, 0x00]), true, timeout);
}

/// A PIC presence probe's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pic {
    /// Well-formed reply, status success.
    Present,
    /// Well-formed reply, status not acked.
    Absent,
    /// Anything else: no conclusion about the model may be drawn.
    Error(u8),
}

/// The OEM's model-detection primitive, `04 03 <addr> 00 00` (the one frame
/// allowed to the bootloader addresses: it is a presence probe, never a
/// flash command).
pub fn probe_pic(link: &mut dyn Link, addr: u8, timeout: Duration) -> Pic {
    let Ok(r) = cmd_reply(link, &frame(PacketType::Command, &[addr, 0x00, 0x00]), timeout) else { return Pic::Error(0xFF) };
    if r.raw_type() != PacketType::Response as u8 || r.count() < 2 || r.addr() != addr {
        return Pic::Error(0xFF);
    }
    match r.data()[1] {
        PS_SUCCESS => Pic::Present,
        PS_NOT_ACKED => Pic::Absent,
        s => Pic::Error(s),
    }
}

/// Flags byte of a READ/STATUS reply: error bits (0x04 does not count from
/// the host).
fn flags_error(addr: u8, flags: u8) -> bool {
    flags & 0x20 != 0 || (addr != HOST && flags & 0x04 != 0)
}

/// One frame with the reply rules; returns the reply.
fn run_one(link: &mut dyn Link, f: &Frame, timeout: Duration) -> Result<Frame, ScanError> {
    let addr = f.addr();
    if f.raw_type() == PacketType::ReadStatus as u8 {
        for t in 1..=BUSY_POLL_TRIES {
            let r = cmd_reply(link, f, timeout)?;
            if r.raw_type() != PacketType::ReadStatus as u8 || r.count() < 2 || r.addr() != addr {
                return Err(ScanError::Proto(format!("busy poll {addr:#04x}: reply {r:?}")));
            }
            let flags = r.data()[1];
            if flags_error(addr, flags) {
                return Err(ScanError::Status(flags));
            }
            if flags & 0x01 == 0 {
                return Ok(r);
            }
            std::thread::sleep(Duration::from_millis(t)); // growing delay
        }
        log(0, &format!("busy poll {addr:#04x}: still busy after {BUSY_POLL_TRIES} tries"));
        return Err(ScanError::Timeout);
    }
    let mut last = 0;
    for t in 1..=REPLY_TRIES {
        let r = cmd_reply(link, f, timeout)?;
        if r.count() < 2 || r.addr() != addr {
            return Err(ScanError::Proto(format!("frame to {addr:#04x}: reply {r:?}")));
        }
        let st = r.data()[1];
        last = st;
        if f.raw_type() == PacketType::Read as u8 {
            if r.raw_type() != PacketType::Read as u8 {
                return Err(ScanError::Proto(format!("read {addr:#04x}: reply {r:?}")));
            }
            if flags_error(addr, st) {
                return Err(ScanError::Status(st));
            }
            if st & 0x11 == 0 {
                return Ok(r);
            }
        } else {
            if r.raw_type() != PacketType::Response as u8 {
                return Err(ScanError::Proto(format!("frame to {addr:#04x}: reply {r:?}")));
            }
            if st == PS_SUCCESS || st == PS_SUCCESS_8 {
                return Ok(r);
            }
            if st != PS_BAD_CHECKSUM && st != PS_USB_6 && st != PS_BUS_ERROR {
                return Err(ScanError::Status(st));
            }
        }
        log(1, &format!("frame to {addr:#04x}: status {st:#04x}, retry {t}"));
    }
    Err(ScanError::Status(last))
}

fn is_dx_read(f: &Frame, low: u8) -> bool {
    let d = f.data();
    f.raw_type() == PacketType::Read as u8 && d.len() >= 3 && d[0] == low && d[1] as usize == dx::REPLY_LEN && d[2] == 0x90
}

fn take_dx_block(reply: &Frame, roll: Option<&mut dx::Roll>) {
    let d = reply.data();
    match (d.len() >= 2 + dx::REPLY_LEN).then(|| dx::parse(&d[2..])).flatten() {
        Some(b) => {
            log(2, &format!("dx: {b}"));
            if let Some(r) = roll {
                r.add(&b);
            }
        }
        None => log(1, &format!("dx: unparsed sensor block {d:02x?}")),
    }
}

fn run_seq_dx(link: &mut dyn Link, seq: &Seq, timeout: Duration, low: u8, mut roll: Option<&mut dx::Roll>) -> Result<(), ScanError> {
    // Safety rules: check the whole sequence before sending any of it.
    seq.check_safe().map_err(|e| ScanError::Param(e.to_string()))?;
    if seq.frames().iter().any(|f| f.raw_type() == 0 || f.count() == 0 || is_bootloader(f.addr())) {
        return Err(ScanError::Param("unsafe frame in sequence".into()));
    }
    for (i, (f, delay)) in seq.iter().enumerate() {
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay as u64));
        }
        let r = run_one(link, f, timeout).inspect_err(|e| log(0, &format!("sequence stopped at frame {i}: {e}")))?;
        if low != 0 && is_dx_read(f, low) {
            take_dx_block(&r, roll.as_deref_mut());
        }
    }
    Ok(())
}

/// Send a generated sequence with the OEM reply rules:
///   - WRITE/COMMAND: reply `07 02 <a> <status>`; 0/8 OK, 3/6/9 retried (3
///     tries), anything else an error;
///   - STATUS poll `03 01 <a>`: repeated until flags bit 0 (busy) clears, up
///     to 44 tries with a growing delay;
///   - READ/STATUS replies echo the type and address; flags 0x20, or 0x04
///     from a controller, are errors.
///
/// Refuses type 0 frames and bootloader addresses before sending anything.
pub fn run_seq(link: &mut dyn Link, seq: &Seq, timeout: Duration) -> Result<(), ScanError> {
    run_seq_dx(link, seq, timeout, 0, None)
}

/// What one pass of the event service saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Events {
    /// The host flags (0x02 = FIFO overflow).
    pub host_flags: u8,
    /// Controller events acknowledged.
    pub serviced: u32,
    /// The bits of every LOW event status.
    pub low_status: u8,
}

/// One pass of the event service: poll the host `03 01 10`; if an event is
/// pending, read LOW then SCN event status and send the follow-up for each
/// pending one, adding DX sensor blocks to `roll`. The OEM polls every 1 ms
/// while scanning.
pub fn service_events(link: &mut dyn Link, low: u8, scn: u8, timeout: Duration, roll: Option<&mut dx::Roll>) -> Result<Events, ScanError> {
    let mut ev = Events::default();
    let r = cmd_reply(link, &frame(PacketType::ReadStatus, &[HOST]), timeout)?;
    if r.raw_type() != PacketType::ReadStatus as u8 || r.count() < 2 || r.addr() != HOST {
        return Err(ScanError::Proto(format!("host poll: reply {r:?}")));
    }
    ev.host_flags = r.data()[1];
    if ev.host_flags & HOST_FLAG_OVERFLOW != 0 {
        log(1, "event service: host reports FIFO overflow");
    }
    if ev.host_flags & HOST_FLAG_EVENT == 0 {
        return Ok(ev);
    }
    let mut roll = roll;
    for addr in [low, scn] {
        // LOW first, as the OEM does
        let r = cmd_reply(link, &setup::event_read_frame(addr), timeout)?;
        let Some(status) = setup::event_parse(&r, addr) else { continue };
        let mut seq = Seq::new();
        setup::event_followup(&mut seq, addr, status, addr == low);
        run_seq_dx(link, &seq, timeout, low, roll.as_deref_mut())?;
        log(2, &format!("event service: {addr:#04x} status {status:#04x} acknowledged"));
        ev.serviced += 1;
        if addr == low {
            ev.low_status |= status;
        }
    }
    Ok(ev)
}

/// LOW `0x88`: lamp and motherboard temperatures, degC.
pub fn lamp_temps(link: &mut dyn Link, low: u8, timeout: Duration) -> Result<(f64, f64), ScanError> {
    let r = cmd_reply(link, &setup::lamp_temps_frame(low), timeout)?;
    setup::lamp_temps_parse(&r, low).ok_or_else(|| ScanError::Proto(format!("lamp temperatures: reply {r:?}")))
}

/// Wait for a stable lamp before calibrating, as TLB.dll does
/// (FN_bLampTemperatureStable): service events every 200 ms and read the
/// lamp flags once a second; stable when a LOW event status or the flags
/// carry 0x02. `progress` gets (lamp degC, ms waited) about once a second.
pub fn wait_lamp(link: &mut dyn Link, low: u8, scn: u8, timeout: Duration, limit: Duration, progress: &mut dyn FnMut(f64, u64)) -> Result<(), ScanError> {
    let t0 = Instant::now();
    let mut next_flags = 0u64;
    loop {
        let ev = service_events(link, low, scn, timeout, None)?;
        if ev.low_status & LAMP_STABLE != 0 {
            log(1, &format!("lamp: stable (event status {:#04x})", ev.low_status));
            return Ok(());
        }
        let ms = t0.elapsed().as_millis() as u64;
        if ms >= next_flags {
            next_flags = ms + 1000;
            let r = cmd_reply(link, &setup::lamp_flags_frame(low), timeout)?;
            let flags = setup::lamp_flags_parse(&r, low).ok_or_else(|| ScanError::Proto(format!("lamp flags: reply {r:?}")))?;
            if flags & LAMP_FAULT != 0 {
                log(0, &format!("lamp: fault (flags {flags:#04x})"));
                return Err(ScanError::Status(flags));
            }
            if flags & LAMP_STABLE != 0 {
                log(1, &format!("lamp: stable (flags {flags:#04x})"));
                return Ok(());
            }
            if let Ok((lamp, board)) = lamp_temps(link, low, timeout) {
                log(1, &format!("lamp: {lamp:.2} degC, board {board:.2} degC, flags {flags:#04x}"));
                progress(lamp, ms);
            }
        }
        if ms >= limit.as_millis() as u64 {
            return Err(ScanError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200)); // the OEM's idle poll
    }
}
