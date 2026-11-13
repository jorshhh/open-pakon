//! Opening the scanner (port of `do_open`, `load_eeprom_archive` and
//! `setup_session` in `tools/pakon_replay.c`): the bridge open, the model
//! from the PIC probes, the unit's EEPROM archive, and the code-built
//! controller init + mode configure.

use std::path::Path;
use std::time::Duration;

use pakon_proto::eeprom::{self, Base, Eeprom, TYPE_F135};
use pakon_proto::frame::{Address, Frame, PacketType};
use pakon_proto::setup::{self, Model, ScanMode, Seq, SetupState, LED_PERIOD_F135, LED_PERIOD_F135_PLUS};

use crate::cmd::{self, Pic};
use crate::link::Link;
use crate::ScanError;

/// Lines of progress, worded as `pakon_replay` prints them (the web app
/// shows some of them).
pub type Out<'a> = &'a mut dyn FnMut(&str);

/// The unit's EEPROM archive (`tools/pakon_eeprom.py backup DIR`).
pub fn load_eeprom_archive(dir: &Path) -> Option<Eeprom> {
    let copies = eeprom::load_dir(dir);
    let e = eeprom::decode(eeprom::as_refs(&copies));
    e.decodable.then_some(e)
}

/// What `--open` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenReport {
    pub model: Option<Model>,
    /// Steps that failed or answered unexpectedly.
    pub failures: u32,
}

/// The open handshake (HostReset, HostSetMode) and the OEM presence probes
/// (PICM Plus 0x44, its boot PIC 0x46, PICM 0x24): the model, and whether
/// anything is wrong (a boot PIC answering means a PIC is in its firmware
/// update state; such a scanner is not driven).
pub fn open_check(link: &mut dyn Link, timeout: Duration, out: Out) -> OpenReport {
    let mut failures = 0;
    let steps: [(&str, PacketType, &[u8]); 2] =
        [("open", PacketType::Command, &[0x10, 0x00, 0x85]), ("open-2", PacketType::Write, &[0x10, 0x01, 0x8F, 0x00])];
    for (label, ty, data) in steps {
        let f = Frame::new(ty, data).expect("short frame");
        let hex = |b: &[u8]| b.iter().map(|x| format!(" {x:02x}")).collect::<String>();
        match cmd::cmd(link, &f, true, timeout) {
            Err(ScanError::Timeout) => out(&format!("[{label:<20}] send{}  -> no reply (normal after first open)", hex(&f.to_bytes()))),
            Err(e) => {
                out(&format!("[{label:<20}] send{}  -> ERROR {e}", hex(&f.to_bytes())));
                failures += 1;
            }
            Ok(r) => {
                let got = r.map(|r| r.to_bytes()).unwrap_or_default();
                if got == [0x07, 0x02, 0x10, 0x00] {
                    out(&format!("[{label:<20}] send{}  recv{}  OK", hex(&f.to_bytes()), hex(&got)));
                } else {
                    out(&format!("[{label:<20}] send{}  recv{}  (expected 07 02 10 00)  MISMATCH", hex(&f.to_bytes()), hex(&got)));
                    failures += 1;
                }
            }
        }
    }
    let probes = [("PICM_PLUS (0x44)", Address::PicMPlus as u8), ("BOOT_PICM_PLUS (0x46)", Address::BootPicMPlus as u8), ("PICM (0x24)", Address::PicM as u8)];
    let mut state = [Pic::Error(0xFF); 3];
    for (i, (label, addr)) in probes.iter().enumerate() {
        state[i] = cmd::probe_pic(link, *addr, timeout);
        let what = match state[i] {
            Pic::Present => "present".to_string(),
            Pic::Absent => "absent".to_string(),
            Pic::Error(0xFF) => {
                failures += 1;
                "ERROR (no/malformed reply)".to_string()
            }
            Pic::Error(s) => {
                failures += 1;
                format!("ERROR (status {s:#04x})")
            }
        };
        out(&format!("[probe {label:<14}] {what}"));
    }
    let model = match (state[0], state[2]) {
        (Pic::Present, Pic::Absent) => Some(Model::F135Plus),
        (Pic::Absent, Pic::Present) => Some(Model::F135),
        _ => None,
    };
    if state[1] == Pic::Present {
        out("\nERROR: PICM boot PIC (0x46) answered — device in firmware-update state, not driving it");
        failures += 1;
    }
    match model {
        Some(Model::F135Plus) => out("\nmodel detected: F-135+ (Plus PICs at 0x40/0x44)"),
        Some(Model::F135) => out("\nmodel detected: F-135 (PICs at 0x20/0x24)"),
        None => {
            out("\nmodel detection FAILED");
            failures += 1;
        }
    }
    out(&format!("open handshake: {}", if failures > 0 { "INCOMPLETE" } else { "reached Idle" }));
    OpenReport { model, failures }
}

/// A configured scanner: controllers initialised, the mode set, LEDs off,
/// not acquiring.
#[derive(Debug, Clone)]
pub struct Session {
    pub eeprom: Eeprom,
    pub st: SetupState,
    pub mode: ScanMode,
    pub base: u32,
}

impl Session {
    pub fn model(&self) -> Model {
        self.st.model()
    }
    /// The base's EEPROM Offset (the scan pixel start).
    pub fn offset(&self) -> u16 {
        self.eeprom.base(Base::from_base(self.base).expect("checked")).offset
    }
    /// IR channel on (CCD reg 0 bit 8).
    pub fn ir(&self) -> bool {
        self.st.reg0 & 0x100 != 0
    }
    pub fn led_period(&self) -> u16 {
        setup::led_period(self.st.model(), self.mode.integration)
    }
}

/// Detect the model, check it against the archive and send the code-built
/// setup for `base` (F-135: IR as asked; F-135+: IR off, the captured modes).
pub fn setup_session(link: &mut dyn Link, eeprom: Eeprom, base: u32, ir: bool, timeout: Duration, out: Out) -> Result<Session, ScanError> {
    let eb = Base::from_base(base).ok_or_else(|| ScanError::Param("--base must be 4, 8 or 16".into()))?;
    out(&format!("eeprom: {} serial {}, Base {base} Offset {}", eeprom.model_name(), eeprom.serial, eeprom.base(eb).offset));
    cmd::bridge_open(link, timeout);
    let plus = cmd::probe_pic(link, Address::PicMPlus as u8, timeout);
    let plain = cmd::probe_pic(link, Address::PicM as u8, timeout);
    let (low, scn, led_period) = match (plus, plain) {
        (Pic::Present, Pic::Absent) => {
            out(&format!("model: F-135+ -> Base {base}{}", if ir { " + IR" } else { ", IR off" }));
            (Address::PicLPlus as u8, Address::PicMPlus as u8, LED_PERIOD_F135_PLUS)
        }
        (Pic::Absent, Pic::Present) => {
            out(&format!("model: F-135 -> Base {base}{}", if ir { " + IR" } else { ", IR off" }));
            (Address::PicL as u8, Address::PicM as u8, LED_PERIOD_F135)
        }
        _ => return Err(ScanError::Proto(format!("model detection failed (plus={plus:?} base={plain:?})"))),
    };
    let model = if low == Address::PicL as u8 { Model::F135 } else { Model::F135Plus };
    if (model == Model::F135) != (eeprom.scanner_type == TYPE_F135) {
        return Err(ScanError::Param(format!("EEPROM archive is for a {}, scanner is not", eeprom.model_name())));
    }
    let mode = setup::scan_mode(model, base, ir, eeprom.base(eb).offset)
        .map_err(|_| ScanError::Param(format!("no Base {base}{} mode for this model", if ir { " + IR" } else { "" })))?;
    if model == Model::F135Plus || base != 16 || !mode.ir {
        out(&format!(
            "mode: trigger {}, integration 0x{:03X}{}{}, window {}..{}",
            mode.trigger,
            mode.integration,
            if mode.dual_tap { ", dual-tap" } else { "" },
            if mode.low89 != 0 { ", LOW 0x89 = 1" } else { "" },
            mode.pixel_start,
            mode.pixel_end
        ));
    }
    let mut seq = Seq::new();
    let mut st = setup::init(&mut seq, low, scn, led_period);
    setup::configure(&mut seq, &mut st, &mode).map_err(|e| ScanError::Param(format!("could not build the sequence: {e}")))?;
    out(&format!("sending {} frames...", seq.len()));
    let r = cmd::run_seq(link, &seq, timeout.max(Duration::from_millis(2000)));
    out(&format!("setup: {}", match &r {
        Ok(()) => "OK, configured (not acquiring)".to_string(),
        Err(e) => e.to_string(),
    }));
    r?;
    Ok(Session { eeprom, st, mode, base })
}

/// The code-built teardown: LEDs off, ResetFifos, `92`, 20 ms, `a2`.
pub fn teardown(link: &mut dyn Link, st: &SetupState, timeout: Duration) -> Result<(), ScanError> {
    let mut seq = Seq::new();
    setup::teardown(&mut seq, st);
    cmd::run_seq(link, &seq, timeout)
}

/// The front-panel status LED word; cosmetic, so a failure is only logged.
pub fn panel(link: &mut dyn Link, st: &SetupState, word: u16, timeout: Duration, out: Out) {
    let mut seq = Seq::new();
    setup::panel(&mut seq, st, word);
    if cmd::run_seq(link, &seq, timeout).is_err() {
        out(&format!("light-cal: panel LED word 0x{word:04x} not set"));
    }
}
