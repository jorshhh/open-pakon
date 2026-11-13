//! Controller init, per-mode configure, calibration writes, scan start,
//! teardown and the event-service frames, built in code (port of
//! `include/pakon_setup.h`, `src/pakon_setup.c`).
//!
//! Pure: frames go into a [`Seq`]; nothing is sent here. Transcribed from five
//! captures (`resources/scan.pakscan`, F-135 serial 3054;
//! `resources/f135plus/base16/base8/base4/base4_ir.pakscan`, F-135+ serial
//! 16402); `tests/setup.rs` checks the generated frames against every one.
//!
//! Left out, because their timing is not part of these sequences: event
//! service traffic (host polls, event reads/acks, temperature and 0x90 reads)
//! and the front-panel status-LED word, which the captures write at different
//! points.

use std::fmt;

use crate::frame::{Address, Frame, PacketType};

/// FPGA bank 0x82: CCD timing and control.
const BANK_CCD: u8 = 0x82;
/// FPGA bank 0x84: CCD analog front end.
const BANK_AFE: u8 = 0x84;

/// LOW 0x82 period word sent with all-zero duties during init, as captured.
/// Constant across the four F-135+ modes; how it is derived is not known.
pub const LED_PERIOD_F135: u16 = 0x0997;
pub const LED_PERIOD_F135_PLUS: u16 = 0x03D6;

/// Front-panel status-LED words (bank 0x82 reg 9, docs/REGISTERS.md).
/// Middle LED green, right LED off: what init leaves and the OEM restores
/// after calibration.
pub const PANEL_IDLE: u16 = 0x0017;
/// Right LED blinking orange/off, as TLX shows before the first scan.
pub const PANEL_CALIBRATING: u16 = 0x00D4;
/// Motor running, feed the film: right LED solid green.
pub const PANEL_FEED: u16 = 0x0015;

/// Advance speed in the F-135 film-advance capture (serial 3054): 9500,
/// the top of the F-135 range. How the OEM derives it is not known.
pub const ADVANCE_SPEED_F135: u16 = 0x251C;

/// Film sensing threshold for the 0x93 levels: a sensor pair is clear when
/// both bytes are at or above this. The DX pots are per unit.
pub const SENSE_CLEAR_LEVEL: u8 = 170;

/// Host flags (`03 01 10` reply).
pub const HOST_FLAG_EVENT: u8 = 0x80;
/// Image FIFO overflow.
pub const HOST_FLAG_OVERFLOW: u8 = 0x02;

/// Lamp flags (LOW event status or `0x83`).
pub const LAMP_STABLE: u8 = 0x02;
pub const LAMP_WARNING: u8 = 0x20;
pub const LAMP_FAULT: u8 = 0x40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    F135,
    F135Plus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupError {
    /// No mode for this base / IR combination.
    UnknownMode { base: u32 },
    /// Integration above the FPGA limit 0xFFD.
    Integration(u16),
    /// LED current above the board's ceiling for this IR state.
    LedCurrent,
    /// Duty above period - 2 (or period < 2).
    Duty,
    /// LED enable byte with bits other than 0 and 1.
    Enable(u8),
    /// A/D gain above 0x3E or offset outside ±255.
    Afe,
    /// Motor speed outside the board's range.
    MotorSpeed(u16),
    /// Sequence would send a frame with no address or to a bootloader.
    Unsafe(usize),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMode { base } => write!(f, "no scan mode for base {base}"),
            Self::Integration(v) => write!(f, "integration {v:#x} above 0xffd"),
            Self::LedCurrent => write!(f, "LED current above the board ceiling"),
            Self::Duty => write!(f, "LED duty above period - 2"),
            Self::Enable(e) => write!(f, "LED enable {e:#x} has unknown bits"),
            Self::Afe => write!(f, "A/D gain or offset out of range"),
            Self::MotorSpeed(s) => write!(f, "motor speed {s} outside the board range"),
            Self::Unsafe(i) => write!(f, "frame {i} is unsafe to send"),
        }
    }
}

impl std::error::Error for SetupError {}

/// Frames in send order, each with the delay to wait before sending it.
///
/// A [`PacketType::ReadStatus`] frame stands for the OEM busy poll: the
/// sender repeats it until bit 0 clears.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seq {
    frames: Vec<Frame>,
    delays_ms: Vec<u16>,
    pending_delay: u16,
}

impl Seq {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// Delay before frame `i`, in ms.
    pub fn delay_ms(&self, i: usize) -> u16 {
        self.delays_ms[i]
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Frame, u16)> {
        self.frames.iter().zip(self.delays_ms.iter().copied())
    }

    /// The safety check the sender runs over the whole sequence before
    /// sending any of it (as `pakon_cmd_run_seq`): every frame carries an
    /// address and none is a bootloader address. Type 0 cannot be built.
    pub fn check_safe(&self) -> Result<(), SetupError> {
        for (i, f) in self.frames.iter().enumerate() {
            let boot = Address::from_u8(f.addr()).is_some_and(Address::is_bootloader);
            if f.count() == 0 || boot {
                return Err(SetupError::Unsafe(i));
            }
        }
        Ok(())
    }

    /// Append a frame. Frames here are at most 15 bytes, far below the
    /// frame limit, so building cannot fail.
    pub fn push(&mut self, ty: PacketType, data: &[u8]) {
        let f = Frame::new(ty, data).expect("setup frames fit in a frame");
        self.frames.push(f);
        self.delays_ms.push(std::mem::take(&mut self.pending_delay));
    }

    /// Wait `ms` before the next frame pushed.
    fn delay_next(&mut self, ms: u16) {
        self.pending_delay = ms;
    }

    fn busy_poll(&mut self, addr: u8) {
        self.push(PacketType::ReadStatus, &[addr]);
    }

    /// Controller write `02 <n+3> <a> <n> <reg> <data>`, then the busy poll.
    fn write_reg(&mut self, addr: u8, reg: u8, val: &[u8]) {
        let mut d = vec![addr, val.len() as u8, reg];
        d.extend_from_slice(val);
        self.push(PacketType::Write, &d);
        self.busy_poll(addr);
    }

    fn write_u8(&mut self, addr: u8, reg: u8, v: u8) {
        self.write_reg(addr, reg, &[v]);
    }

    /// FPGA register write `02 06 SCN 03 <bank> <reg> <v16 LE>`.
    fn write_fpga(&mut self, scn: u8, bank: u8, reg: u8, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.write_reg(scn, bank, &[reg, lo, hi]);
    }

    fn command(&mut self, addr: u8, cmd: u8) {
        self.push(PacketType::Command, &[addr, 0x00, cmd]);
        self.busy_poll(addr);
    }

    /// Controller read `01 03 <a> <n> <reg>` (no busy poll).
    fn read_reg(&mut self, addr: u8, n: u8, reg: u8) {
        self.push(PacketType::Read, &[addr, n, reg]);
    }

    /// ResetFifos: host `02 04 10 01 84 02`, then LOW command 0x8A.
    fn reset_fifos(&mut self, low: u8) {
        self.push(PacketType::Write, &[Address::Host as u8, 0x01, 0x84, 0x02]);
        self.command(low, 0x8A);
    }
}

/// Values the FPGA and LOW controller hold after init; configure writes a
/// register only when its value changes, as the OEM does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupState {
    /// Controller addresses, from the probes.
    pub low: u8,
    pub scn: u8,
    /// Bank 0x82 registers.
    pub reg0: u16,
    pub reg4: u16,
    pub reg5: u16,
    pub reg6: u16,
    /// LOW 0x89.
    pub low89: u8,
}

impl SetupState {
    /// The board, from the LOW address (as the C driver: anything but the
    /// F-135 LOW address is the Plus).
    pub fn model(&self) -> Model {
        if self.low == Address::PicL as u8 { Model::F135 } else { Model::F135Plus }
    }

    fn ir_on(&self) -> bool {
        self.reg0 & 0x100 != 0
    }
}

/// One resolution / IR mode. See [`scan_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanMode {
    /// LOW 0x91 scan-line trigger value.
    pub trigger: u16,
    /// Bank 0x82 reg 0 bit 0x100.
    pub ir: bool,
    /// Bank 0x82 reg 0 bit 0x002.
    pub dual_tap: bool,
    /// Bank 0x82 reg 6, <= 0xFFD.
    pub integration: u16,
    /// Bank 0x82 reg 4.
    pub pixel_start: u16,
    /// Bank 0x82 reg 5.
    pub pixel_end: u16,
    /// LOW 0x89 per-resolution flag (1 at Base 8).
    pub low89: u8,
}

/// Pixel window for calibration: start = optically black pixels (6, or 3
/// dual-tap); end = EEPROM Offset for the base + line height (2000, or 1000
/// dual-tap). Matches all five captures.
pub fn pixel_window(eeprom_offset: u16, dual_tap: bool) -> (u16, u16) {
    let start = if dual_tap { 3 } else { 6 };
    let end = eeprom_offset.wrapping_add(if dual_tap { 1000 } else { 2000 });
    (start, end)
}

/// The mode for a resolution base (4, 8 or 16), pixel window from the EEPROM
/// Offset for that base.
///
/// TLB.dll builds one per-base table for both models: Base 16 height 2000;
/// Base 8 height 2000, output narrowed to 1500 px, LOW 0x89 = 1; Base 4
/// dual-tap, height 1000. On the F-135 only the integration times differ.
/// Trigger: round(23700 * clk * (0.01, or 0.005 with IR) / (integration *
/// output width)), clk 833333.3 on the F-135 and 2.5x that on the F-135+.
/// See `pakon_setup.h` for which values were captured and which checked on
/// hardware.
pub fn scan_mode(model: Model, base: u32, ir: bool, eeprom_offset: u16) -> Result<ScanMode, SetupError> {
    let (trigger, integration) = match (model, base) {
        (Model::F135Plus, 16) => if ir { (0x0031, 0x09C2) } else { (0x003C, 0x0FFD) },
        (Model::F135Plus, 8) => if ir { (0x004D, 0x0850) } else { (0x0075, 0x0AFD) },
        (Model::F135Plus, 4) => if ir { (0x00C5, 0x04E2) } else { (0x0107, 0x0753) },
        (Model::F135, 16) => if ir { (0x0010, 0x0C1A) } else { (24, 0x0FF0) },
        (Model::F135, 8) => if ir { (28, 0x0913) } else { (38, 0x0D9D) },
        (Model::F135, 4) => if ir { (64, 0x060D) } else { (85, 0x0913) },
        _ => return Err(SetupError::UnknownMode { base }),
    };
    let dual_tap = base == 4;
    let (pixel_start, pixel_end) = pixel_window(eeprom_offset, dual_tap);
    Ok(ScanMode {
        trigger,
        ir,
        dual_tap,
        integration,
        pixel_start,
        pixel_end,
        low89: (base == 8) as u8,
    })
}

/// Visible pixels per scan line: 2000 / 1500 / 1000 at Base 16 / 8 / 4.
pub fn scan_px(base: u32) -> u32 {
    match base {
        16 => 2000,
        8 => 1500,
        4 => 1000,
        _ => 0,
    }
}

/// LED period for an integration time, truncated: integration * 0.6 on the
/// F-135 (TLB.dll; 0x0C1A gives the captured 0x742), integration * 0.24 on
/// the F-135+ (0.6 over its 2.5x pixel clock; all four Plus captures:
/// 0xFFD -> 982, 0xAFD -> 675, 0x753 -> 450, 0x4E2 -> 300).
pub fn led_period(model: Model, integration: u16) -> u16 {
    match model {
        Model::F135 => (integration as u32 * 6 / 10) as u16,
        Model::F135Plus => (integration as u32 * 6 / 25) as u16,
    }
}

/// Controller init after model detection.
pub fn init(seq: &mut Seq, low: u8, scn: u8, led_period: u16) -> SetupState {
    seq.write_u8(scn, 0x97, 0x01); // SCN controller init
    seq.write_u8(low, 0x03, 0x01);
    seq.read_reg(low, 0x0C, 0x07);
    seq.write_u8(scn, 0x03, 0x01);
    seq.read_reg(scn, 0x0C, 0x07);
    seq.reset_fifos(low);

    // Lamp and motherboard temperature warning / fault bands.
    seq.write_reg(low, 0x8F, &[0xE8, 0xFF, 0x18, 0x00]);
    seq.write_reg(low, 0x8C, &[0xE0, 0xFF, 0x20, 0x00]);
    seq.write_reg(low, 0x8B, &[0xF0, 0x00, 0x20, 0x03]);
    seq.write_reg(low, 0x8D, &[0xA0, 0x00, 0x70, 0x03]);
    // TEC setup: replay as-is, never sweep (safety rule).
    seq.write_u8(low, 0xD0, 0x00);
    seq.write_u8(low, 0xD1, 0x01);
    seq.write_reg(low, 0x87, &[0x00, 0x00]);

    // LEDs: enable, zero duties with the period word, then all off.
    seq.write_u8(low, 0x80, 0x01);
    let mut duties = [0u8; 12];
    duties[10..].copy_from_slice(&led_period.to_le_bytes());
    seq.write_reg(low, 0x82, &duties);
    seq.write_u8(low, 0x80, 0x00);
    seq.write_u8(low, 0x89, 0x00);

    // CCD FPGA defaults.
    let st = SetupState { low, scn, reg6: 0x0FFD, reg0: 0x0060, reg4: 0x003E, reg5: 0x080E, low89: 0 };
    seq.write_fpga(scn, BANK_CCD, 0x6, st.reg6);
    seq.write_fpga(scn, BANK_CCD, 0x0, st.reg0);
    seq.write_fpga(scn, BANK_CCD, 0xB, 0x0000);
    seq.write_fpga(scn, BANK_CCD, 0x4, st.reg4);
    seq.write_fpga(scn, BANK_CCD, 0x5, st.reg5);
    seq.write_fpga(scn, BANK_CCD, 0x1, 0x0000); // exposure R/G/B
    seq.write_fpga(scn, BANK_CCD, 0x2, 0x0000);
    seq.write_fpga(scn, BANK_CCD, 0x3, 0x0000);
    seq.write_fpga(scn, BANK_CCD, 0xA, 0x0400);
    seq.write_fpga(scn, BANK_AFE, 0x0, 0x0078);
    seq.write_fpga(scn, BANK_AFE, 0x1, 0x0080);

    seq.command(scn, 0xA2); // release the motor drive
    seq.write_fpga(scn, BANK_CCD, 0x9, 0x0014); // status LEDs
    seq.write_fpga(scn, BANK_CCD, 0x9, 0x0017);
    st
}

/// Mode configure after init (or a previous configure). Updates `st`.
pub fn configure(seq: &mut Seq, st: &mut SetupState, mode: &ScanMode) -> Result<(), SetupError> {
    if mode.integration > 0x0FFD {
        return Err(SetupError::Integration(mode.integration));
    }
    let (low, scn) = (st.low, st.scn);
    seq.reset_fifos(low);
    let [tl, th] = mode.trigger.to_le_bytes();
    seq.write_reg(low, 0x91, &[tl, th, 0x01]);

    // Order as captured: IR bit, integration, dual-tap bit, pixel window.
    // Unchanged registers are not rewritten.
    if mode.ir && st.reg0 & 0x100 == 0 {
        st.reg0 |= 0x100;
        seq.write_fpga(scn, BANK_CCD, 0x0, st.reg0);
    }
    if st.reg6 != mode.integration {
        st.reg6 = mode.integration;
        seq.write_fpga(scn, BANK_CCD, 0x6, st.reg6);
    }
    if mode.dual_tap && st.reg0 & 0x002 == 0 {
        st.reg0 |= 0x002;
        seq.write_fpga(scn, BANK_CCD, 0x0, st.reg0);
    }
    if st.reg4 != mode.pixel_start {
        st.reg4 = mode.pixel_start;
        seq.write_fpga(scn, BANK_CCD, 0x4, st.reg4);
    }
    if st.reg5 != mode.pixel_end {
        st.reg5 = mode.pixel_end;
        seq.write_fpga(scn, BANK_CCD, 0x5, st.reg5);
    }
    if st.low89 != mode.low89 {
        st.low89 = mode.low89;
        seq.write_u8(low, 0x89, st.low89);
    }
    seq.reset_fifos(low);
    Ok(())
}

/// Teardown after a scan (OEM FN_bAfterScan): bank 0x82 reg 0 back to the
/// configured value (acquire bit clear; clearing it is what lets 0xA2 stop
/// the motor), LEDs off, ResetFifos, LOW 0x92 (end acquisition), 20 ms,
/// SCN 0xA2 (release the drive). Same in all seven captures.
pub fn teardown(seq: &mut Seq, st: &SetupState) {
    seq.write_fpga(st.scn, BANK_CCD, 0x0, st.reg0 & !1);
    seq.write_u8(st.low, 0x80, 0x00); // LEDs off
    seq.reset_fifos(st.low);
    seq.command(st.low, 0x92); // end acquisition
    seq.delay_next(20);
    seq.command(st.scn, 0xA2); // release the drive
}

/// Front-panel status-LED word (bank 0x82 reg 9).
pub fn panel(seq: &mut Seq, st: &SetupState, word: u16) {
    seq.write_fpga(st.scn, BANK_CCD, 0x9, word);
}

/// Bank 0x82 reg 0 = configured value | 1 (acquire on) or the value alone.
pub fn acquire(seq: &mut Seq, st: &SetupState, on: bool) {
    let v = if on { st.reg0 | 1 } else { st.reg0 & !1 };
    seq.write_fpga(st.scn, BANK_CCD, 0x0, v);
}

/// Sign-magnitude offset encoding.
fn enc_offset(v: i32) -> u16 {
    v.unsigned_abs() as u16 | if v < 0 { 0x100 } else { 0 }
}

/// A/D gain codes (bank 0x84 regs 2-4, <= 0x3E) and signed offsets (regs
/// 5-7, |v| <= 255), R/G/B, in the captured order.
pub fn afe(seq: &mut Seq, st: &SetupState, gain: [i32; 3], offset: [i32; 3]) -> Result<(), SetupError> {
    if gain.iter().any(|g| !(0..=0x3E).contains(g)) || offset.iter().any(|o| !(-255..=255).contains(o)) {
        return Err(SetupError::Afe);
    }
    for (c, g) in gain.iter().enumerate() {
        seq.write_fpga(st.scn, BANK_AFE, 2 + c as u8, *g as u16);
    }
    for (c, o) in offset.iter().enumerate() {
        seq.write_fpga(st.scn, BANK_AFE, 5 + c as u8, enc_offset(*o));
    }
    Ok(())
}

/// LED currents, order R, G, B, IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LedValues {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub ir: u8,
}

/// Current ceilings per board (docs/REGISTERS.md), indexed by IR on.
pub const LED_CEILING_F135: [LedValues; 2] = [
    LedValues { r: 6, g: 8, b: 8, ir: 0 },
    LedValues { r: 8, g: 8, b: 8, ir: 8 },
];
pub const LED_CEILING_F135_PLUS: [LedValues; 2] = [
    LedValues { r: 4, g: 20, b: 20, ir: 0 },
    LedValues { r: 8, g: 24, b: 24, ir: 8 },
];

pub fn led_ceiling(model: Model, ir_on: bool) -> LedValues {
    match model {
        Model::F135 => LED_CEILING_F135[ir_on as usize],
        Model::F135Plus => LED_CEILING_F135_PLUS[ir_on as usize],
    }
}

/// Duties `[R, G, B, IR]` + period, as the 12 wire bytes `[B, IR, R, 0, G,
/// period]` u16 LE.
fn duty_bytes(duty: [u16; 4], period: u16) -> Result<[u8; 12], SetupError> {
    if period < 2 || duty.iter().any(|&d| d > period - 2) {
        return Err(SetupError::Duty);
    }
    let words = [duty[2], duty[3], duty[0], 0, duty[1], period];
    let mut out = [0u8; 12];
    for (i, w) in words.iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&w.to_le_bytes());
    }
    Ok(out)
}

/// LEDs: LOW 0x80 enable (bit 0 visible, bit 1 IR), 0x81 currents
/// `[B, IR, R, 0, G]`, 0x82 duties. Refuses (nothing added) a current above
/// the board's ceiling for this IR state or a duty above period - 2.
pub fn leds(
    seq: &mut Seq,
    st: &SetupState,
    enable: u8,
    current: LedValues,
    duty: [u16; 4],
    period: u16,
) -> Result<(), SetupError> {
    if enable & !3 != 0 {
        return Err(SetupError::Enable(enable));
    }
    let c = led_ceiling(st.model(), st.ir_on());
    if current.r > c.r || current.g > c.g || current.b > c.b || current.ir > c.ir {
        return Err(SetupError::LedCurrent);
    }
    let bytes = duty_bytes(duty, period)?;
    seq.write_u8(st.low, 0x80, enable);
    seq.write_reg(st.low, 0x81, &[current.b, current.ir, current.r, 0, current.g]);
    seq.write_reg(st.low, 0x82, &bytes);
    Ok(())
}

fn speed_range(model: Model) -> std::ops::RangeInclusive<u16> {
    match model {
        Model::F135 => 400..=9500,
        Model::F135Plus => 1000..=32766,
    }
}

/// Motor speed from the EEPROM: MotorSpeed(_Ir) * adjust / 1000 (adjust
/// clamped 900-1100), clamped to the board's range.
pub fn motor_speed(eeprom_speed: u16, adjust: u16, model: Model) -> u16 {
    let adjust = adjust.clamp(900, 1100) as u32;
    let v = eeprom_speed as u32 * adjust / 1000;
    let r = speed_range(model);
    v.clamp(*r.start() as u32, *r.end() as u32) as u16
}

/// Scan start after calibration (OEM FN_bBeforeScan): acquire off,
/// ResetFifos, bank 0x82 reg 4 = the scan pixel start (EEPROM Offset), LOW
/// 0x80 = 0x03 (0x01 with IR off) and 0x82 = the scan duties, SCN 0xA5 =
/// motor speed, SCN 0xA0 (forward), acquire on, LOW 0x91 = trigger. Updates
/// `st.reg4`. Refuses a speed outside the board range or a bad duty.
pub fn scan_start(
    seq: &mut Seq,
    st: &mut SetupState,
    pixel_start: u16,
    duty: [u16; 4],
    period: u16,
    motor_speed: u16,
    trigger: u16,
) -> Result<(), SetupError> {
    if !speed_range(st.model()).contains(&motor_speed) {
        return Err(SetupError::MotorSpeed(motor_speed));
    }
    let bytes = duty_bytes(duty, period)?;
    let (low, scn) = (st.low, st.scn);

    seq.write_fpga(scn, BANK_CCD, 0x0, st.reg0 & !1);
    seq.reset_fifos(low);
    st.reg4 = pixel_start;
    seq.write_fpga(scn, BANK_CCD, 0x4, pixel_start);
    seq.write_u8(low, 0x80, if st.ir_on() { 0x03 } else { 0x01 });
    seq.write_reg(low, 0x82, &bytes);
    seq.write_reg(scn, 0xA5, &motor_speed.to_le_bytes());
    seq.command(scn, 0xA0);
    seq.write_fpga(scn, BANK_CCD, 0x0, st.reg0 | 1);
    let [tl, th] = trigger.to_le_bytes();
    seq.write_reg(low, 0x91, &[tl, th, 0x01]);
    Ok(())
}

/// Transport run without scanning (as in advance.pakscan): SCN 0xA5 = speed,
/// SCN 0xA0 (forward). Same speed range as [`scan_start`].
pub fn motor_run(seq: &mut Seq, st: &SetupState, speed: u16) -> Result<(), SetupError> {
    if !speed_range(st.model()).contains(&speed) {
        return Err(SetupError::MotorSpeed(speed));
    }
    seq.write_reg(st.scn, 0xA5, &speed.to_le_bytes());
    seq.command(st.scn, 0xA0);
    Ok(())
}

/// Release the drive: SCN 0xA2. Only stops the motor while acquire is off
/// (use [`teardown`] after a scan).
pub fn motor_stop(seq: &mut Seq, st: &SetupState) {
    seq.command(st.scn, 0xA2);
}

fn read_frame(addr: u8, n: u8, reg: u8) -> Frame {
    Frame::new(PacketType::Read, &[addr, n, reg]).expect("3-byte frame")
}

/// `01 03 LOW 04 93`: read the four DX detector levels (the exit-side sensor
/// doubles as the film-presence sensor).
pub fn sense_read_frame(low: u8) -> Frame {
    read_frame(low, 0x04, 0x93)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Entry,
    Exit,
}

/// Bytes 0-1 are the entry DX sensor, 2-3 the exit one. Clear when both
/// bytes of the pair are at or above [`SENSE_CLEAR_LEVEL`].
pub fn sense_clear(level: [u8; 4], sensor: Sensor) -> bool {
    let p = match sensor {
        Sensor::Entry => &level[0..2],
        Sensor::Exit => &level[2..4],
    };
    p.iter().all(|&b| b >= SENSE_CLEAR_LEVEL)
}

/// `01 03 <a> 01 02`: read the 1-byte event status.
pub fn event_read_frame(addr: u8) -> Frame {
    read_frame(addr, 0x01, 0x02)
}

/// Reply `01 03 <a> <flags> <status>` to that read: the status if an event
/// is pending (flags bit 0x80).
pub fn event_parse(reply: &Frame, addr: u8) -> Option<u8> {
    let d = reply.data();
    (reply.raw_type() == PacketType::Read as u8 && d.len() >= 3 && d[0] == addr && d[1] & 0x80 != 0)
        .then(|| d[2])
}

/// Follow-up for a pending event: the ack `02 05 <a> 02 06 00 <status>` and
/// its busy poll; for LOW, then the sensor/DX block `01 03 LOW 1e 90` and,
/// if status & 0x5B, the temperatures `0x84` and `0x88`. Reproduces 91 of
/// the 95 acks in our seven captures (see `pakon_setup.h`).
pub fn event_followup(seq: &mut Seq, addr: u8, status: u8, is_low: bool) {
    seq.write_reg(addr, 0x06, &[0x00, status]);
    if is_low {
        seq.read_reg(addr, 0x1E, 0x90);
        if status & 0x5B != 0 {
            seq.read_reg(addr, 0x02, 0x84);
            seq.read_reg(addr, 0x04, 0x88);
        }
    }
}

/// `01 03 LOW 01 83`: read the lamp flags.
pub fn lamp_flags_frame(low: u8) -> Frame {
    read_frame(low, 0x01, 0x83)
}

/// `01 03 LOW 04 88`: read the lamp and motherboard temperatures.
pub fn lamp_temps_frame(low: u8) -> Frame {
    read_frame(low, 0x04, 0x88)
}

/// Reply `01 03 LOW <flags> <lamp flags>`: the lamp flags.
pub fn lamp_flags_parse(reply: &Frame, low: u8) -> Option<u8> {
    let d = reply.data();
    (reply.raw_type() == PacketType::Read as u8 && d.len() >= 3 && d[0] == low).then(|| d[2])
}

/// Reply `01 06 LOW <flags> <lamp lo hi> <board lo hi>`: signed 16-bit LE,
/// 1/16 degC. Returns (lamp, board) in degC.
pub fn lamp_temps_parse(reply: &Frame, low: u8) -> Option<(f64, f64)> {
    let d = reply.data();
    if reply.raw_type() != PacketType::Read as u8 || d.len() < 6 || d[0] != low {
        return None;
    }
    let t = |i: usize| i16::from_le_bytes([d[i], d[i + 1]]) as f64 / 16.0;
    Some((t(2), t(4)))
}
