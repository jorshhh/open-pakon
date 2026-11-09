//! The USB transport on `nusb` (port of `pakon_usb.c`): enumeration and
//! classification, the FX2 firmware load, interface claim, bulk send /
//! receive and control transfers. No packet knowledge.
//!
//! Safety: vendor request 0xA2 is refused here, whatever asks for it.

use std::time::{Duration, Instant};

use nusb::transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient, TransferError};
use nusb::MaybeFuture;

use crate::fw;

/// One control transfer: `bmRequestType bRequest wValue wIndex wLength`
/// and the OUT data (the shape of a firmware-load step).
pub type Control = fw::Xfer;

/// Pakon's USB vendor.
pub const VID: u16 = 0x0F05;
/// Operational after the firmware load ("Pakon F135-USB Film Scanner").
pub const WARM_PID: u16 = 0xF135;
/// The FX2 EEPROM bootstrap: needs the firmware download.
pub const COLD_PID: u16 = 0xF235;

/// Command frames host -> device.
pub const EP_CMD_OUT: u8 = 0x01;
/// Command reply / status device -> host.
pub const EP_CMD_IN: u8 = 0x81;
/// Bulk image stream device -> host.
pub const EP_IMAGE_IN: u8 = 0x86;

/// The vendor request never to issue (pakon-reference safety rules).
const FORBIDDEN_REQUEST: u8 = 0xA2;
const FX2_TIMEOUT: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbError {
    NoDevice,
    Timeout,
    /// The endpoint stalled (the halt was cleared).
    Stall,
    /// A request the safety rules forbid, or a bad argument.
    Refused(String),
    Firmware(String),
    Usb(String),
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsbError::NoDevice => write!(f, "no Pakon scanner found"),
            UsbError::Timeout => write!(f, "USB timeout"),
            UsbError::Stall => write!(f, "endpoint stalled"),
            UsbError::Refused(s) => write!(f, "refused: {s}"),
            UsbError::Firmware(s) => write!(f, "firmware: {s}"),
            UsbError::Usb(s) => write!(f, "USB: {s}"),
        }
    }
}

impl std::error::Error for UsbError {}

fn usb(e: impl std::fmt::Display) -> UsbError {
    UsbError::Usb(e.to_string())
}

fn transfer_err(e: TransferError) -> UsbError {
    match e {
        TransferError::Cancelled => UsbError::Timeout,
        TransferError::Stall => UsbError::Stall,
        TransferError::Disconnected => UsbError::NoDevice,
        e => UsbError::Usb(e.to_string()),
    }
}

/// What a Pakon on the bus is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevClass {
    /// FX2 bootloader, needs the firmware download.
    Cold,
    /// 0F05:F135, ready for the protocol.
    Warm,
}

/// One device on the bus (the `pakon_probe --list` listing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevInfo {
    pub vid: u16,
    pub pid: u16,
    pub bus: String,
    pub address: u8,
    pub class: u8,
}

pub fn is_warm_id(vid: u16, pid: u16) -> bool {
    vid == VID && pid == WARM_PID
}

/// Everything on the bus.
pub fn list() -> Result<Vec<DevInfo>, UsbError> {
    Ok(nusb::list_devices()
        .wait()
        .map_err(usb)?
        .map(|d| DevInfo { vid: d.vendor_id(), pid: d.product_id(), bus: d.bus_id().to_string(), address: d.device_address(), class: d.class() })
        .collect())
}

/// Whether a warm or cold Pakon is present (warm wins).
pub fn find() -> Result<DevClass, UsbError> {
    let all = list()?;
    if all.iter().any(|d| is_warm_id(d.vid, d.pid)) {
        Ok(DevClass::Warm)
    } else if all.iter().any(|d| d.vid == VID && d.pid == COLD_PID) {
        Ok(DevClass::Cold)
    } else {
        Err(UsbError::NoDevice)
    }
}

fn open_ids(vid: u16, pid: u16) -> Result<nusb::Device, UsbError> {
    let info = nusb::list_devices().wait().map_err(usb)?.find(|d| d.vendor_id() == vid && d.product_id() == pid).ok_or(UsbError::NoDevice)?;
    info.open().wait().map_err(usb)
}

fn claim(dev: &nusb::Device, ifc: u8) -> Result<nusb::Interface, UsbError> {
    #[cfg(target_os = "linux")]
    let r = dev.detach_and_claim_interface(ifc).wait();
    #[cfg(not(target_os = "linux"))]
    let r = dev.claim_interface(ifc).wait();
    r.map_err(|e| UsbError::Usb(format!("claim interface {ifc}: {e}")))
}

fn control_type(bm: u8) -> (ControlType, Recipient) {
    let ct = match (bm >> 5) & 3 {
        0 => ControlType::Standard,
        1 => ControlType::Class,
        _ => ControlType::Vendor,
    };
    let rc = match bm & 0x1F {
        0 => Recipient::Device,
        1 => Recipient::Interface,
        2 => Recipient::Endpoint,
        _ => Recipient::Other,
    };
    (ct, rc)
}

fn check_request(bm: u8, request: u8) -> Result<(), UsbError> {
    if (bm >> 5) & 3 == 2 && request == FORBIDDEN_REQUEST {
        return Err(UsbError::Refused(format!("vendor request {FORBIDDEN_REQUEST:#04x}")));
    }
    Ok(())
}

/// A control transfer on a claimed interface: IN returns up to `length`
/// bytes, OUT sends `data`.
fn control(intf: &nusb::Interface, x: &Control, timeout: Duration) -> Result<Vec<u8>, UsbError> {
    check_request(x.request_type, x.request)?;
    let (control_type, recipient) = control_type(x.request_type);
    let (request, value, index) = (x.request, x.value, x.index);
    if x.request_type & 0x80 != 0 {
        intf.control_in(ControlIn { control_type, recipient, request, value, index, length: x.length }, timeout).wait().map_err(transfer_err)
    } else {
        intf.control_out(ControlOut { control_type, recipient, request, value, index, data: &x.data }, timeout).wait().map_err(transfer_err)?;
        Ok(vec![])
    }
}

/// A device-level control transfer, no interface claimed (macOS, Linux):
/// the cold FX2 may have no active configuration, so no interface 0.
#[cfg(not(windows))]
fn control_dev(dev: &nusb::Device, x: &Control, timeout: Duration) -> Result<Vec<u8>, UsbError> {
    check_request(x.request_type, x.request)?;
    let (control_type, recipient) = control_type(x.request_type);
    let (request, value, index) = (x.request, x.value, x.index);
    if x.request_type & 0x80 != 0 {
        dev.control_in(ControlIn { control_type, recipient, request, value, index, length: x.length }, timeout).wait().map_err(transfer_err)
    } else {
        dev.control_out(ControlOut { control_type, recipient, request, value, index, data: &x.data }, timeout).wait().map_err(transfer_err)?;
        Ok(vec![])
    }
}

/// Wait for the warm device after a firmware load (polls every 200 ms).
fn wait_for_warm(limit: Duration) -> Result<(), UsbError> {
    let t0 = Instant::now();
    loop {
        if find() == Ok(DevClass::Warm) {
            return Ok(());
        }
        if t0.elapsed() > limit {
            return Err(UsbError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn run_fw(xfers: &[fw::Xfer], personality_at: Option<usize>) -> Result<usize, UsbError> {
    let dev = open_ids(VID, COLD_PID)?;
    // As the C driver: claim interface 0 when there is one, else (an
    // unconfigured cold FX2) send on the device itself.
    let intf = claim(&dev, 0).ok();
    #[cfg(windows)]
    if intf.is_none() {
        return Err(UsbError::Usb("claim interface 0 of the cold device failed (is WinUSB installed?)".into()));
    }
    let mut late_errors = 0;
    for (i, x) in xfers.iter().enumerate() {
        let r = match &intf {
            Some(intf) => control(intf, x, FX2_TIMEOUT),
            #[cfg(not(windows))]
            None => control_dev(&dev, x, FX2_TIMEOUT),
            #[cfg(windows)]
            None => unreachable!(),
        };
        match r {
            Err(e) => {
                // Only the final run (CPUCS = 0) may fail as the device renumerates.
                if i + 2 >= xfers.len() {
                    late_errors += 1;
                    continue;
                }
                return Err(UsbError::Firmware(format!("transfer {i} req={:#04x} val={:#06x}: {e}", x.request, x.value)));
            }
            Ok(data) => {
                if Some(i) == personality_at {
                    tracing_hex("personality", &data);
                    if data.len() != 8 || !fw::personality_is_f135(&data) {
                        return Err(UsbError::Firmware(
                            "personality is not F235_AA07 (F-135/F-135+); not loading Pakon7.hex. Power-cycle.".into(),
                        ));
                    }
                }
            }
        }
    }
    Ok(late_errors)
}

fn tracing_hex(tag: &str, b: &[u8]) {
    if crate::debug_level() >= 2 {
        eprintln!("{tag}: {}", b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
    }
}

/// Load the firmware from Intel HEX (the stage-1 loader and the main image)
/// into the cold FX2, refusing the main image unless the loader reports the
/// F-135/F-135+ personality; waits for the warm device.
pub fn load_firmware_hex(stage1: &str, main_image: &str) -> Result<(), UsbError> {
    let seq = fw::build(stage1, main_image).map_err(|e| UsbError::Firmware(e.to_string()))?;
    let late = run_fw(&seq.xfers, Some(seq.personality_at))?;
    if crate::debug_level() >= 1 {
        eprintln!("sent {} firmware transfers from HEX ({late} late USB errors); waiting for {VID:04x}:{WARM_PID:04x}", seq.xfers.len());
    }
    wait_for_warm(Duration::from_secs(5))
}

/// Replay a captured firmware-load script (`.pakfw`).
pub fn load_firmware_script(text: &str) -> Result<(), UsbError> {
    let xfers = fw::parse_pakfw(text);
    if xfers.is_empty() {
        return Err(UsbError::Firmware("no transfers in the script".into()));
    }
    run_fw(&xfers, None)?;
    wait_for_warm(Duration::from_secs(5))
}

/// One endpoint's description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointInfo {
    pub address: u8,
    pub attributes: u8,
    pub max_packet: u16,
    pub interface: u8,
    pub altsetting: u8,
}

/// The open warm scanner, interface 0 claimed.
pub struct Device {
    dev: nusb::Device,
    intf: nusb::Interface,
    vid: u16,
    pid: u16,
    cmd_out: nusb::Endpoint<Bulk, Out>,
    cmd_in: nusb::Endpoint<Bulk, In>,
    image_in: Option<nusb::Endpoint<Bulk, In>>,
}

impl Device {
    /// Open the warm scanner and claim interface 0, alt setting 0.
    pub fn open() -> Result<Device, UsbError> {
        let info = nusb::list_devices().wait().map_err(usb)?.find(|d| is_warm_id(d.vendor_id(), d.product_id())).ok_or(UsbError::NoDevice)?;
        let (vid, pid) = (info.vendor_id(), info.product_id());
        let dev = info.open().wait().map_err(usb)?;
        let intf = claim(&dev, 0)?;
        intf.set_alt_setting(0).wait().map_err(|e| UsbError::Usb(format!("set alt setting 0: {e}")))?;
        let cmd_out = intf.endpoint::<Bulk, Out>(EP_CMD_OUT).map_err(usb)?;
        let cmd_in = intf.endpoint::<Bulk, In>(EP_CMD_IN).map_err(usb)?;
        let image_in = intf.endpoint::<Bulk, In>(EP_IMAGE_IN).ok();
        Ok(Device { dev, intf, vid, pid, cmd_out, cmd_in, image_in })
    }

    pub fn ids(&self) -> (u16, u16) {
        (self.vid, self.pid)
    }

    /// The active configuration's endpoints.
    pub fn endpoints(&self) -> Vec<EndpointInfo> {
        let Ok(cfg) = self.dev.active_configuration() else { return vec![] };
        cfg.interface_alt_settings()
            .flat_map(|alt| {
                let (i, a) = (alt.interface_number(), alt.alternate_setting());
                alt.endpoints()
                    .map(move |e| EndpointInfo { address: e.address(), attributes: e.transfer_type() as u8, max_packet: e.max_packet_size() as u16, interface: i, altsetting: a })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Write `data` on an OUT endpoint (only EP 0x01 here).
    pub fn send(&mut self, ep: u8, data: &[u8], timeout: Duration) -> Result<usize, UsbError> {
        if ep != EP_CMD_OUT {
            return Err(UsbError::Refused(format!("no OUT endpoint {ep:#04x}")));
        }
        tracing_hex_level(4, "send", data);
        let mut buf = Buffer::new(data.len());
        buf.extend_from_slice(data);
        let c = self.cmd_out.transfer_blocking(buf, timeout);
        let n = c.actual_len;
        match c.status {
            Ok(()) => Ok(n),
            Err(TransferError::Stall) => {
                let _ = self.cmd_out.clear_halt().wait();
                Err(UsbError::Stall)
            }
            Err(e) => Err(transfer_err(e)),
        }
    }

    /// Read up to `buf.len()` bytes from an IN endpoint (0x81 or 0x86). The
    /// request is rounded up to whole packets as the host controller wants;
    /// a device sending more than `buf` holds is an error.
    pub fn recv(&mut self, ep: u8, buf: &mut [u8], timeout: Duration) -> Result<usize, UsbError> {
        let endpoint = match ep {
            EP_CMD_IN => &mut self.cmd_in,
            EP_IMAGE_IN => self.image_in.as_mut().ok_or(UsbError::Refused("no image endpoint".into()))?,
            _ => return Err(UsbError::Refused(format!("no IN endpoint {ep:#04x}"))),
        };
        let mps = endpoint.max_packet_size().max(1);
        let want = buf.len().div_ceil(mps).max(1) * mps;
        let c = endpoint.transfer_blocking(Buffer::new(want), timeout);
        match c.status {
            Ok(()) => {
                let n = c.actual_len;
                if n > buf.len() {
                    return Err(UsbError::Usb(format!("endpoint {ep:#04x}: {n} bytes for a {} byte buffer", buf.len())));
                }
                buf[..n].copy_from_slice(&c.buffer[..n]);
                if n > 0 {
                    tracing_hex_level(4, "recv", &buf[..n.min(64)]);
                }
                Ok(n)
            }
            Err(TransferError::Stall) => {
                let _ = endpoint.clear_halt().wait();
                Err(UsbError::Stall)
            }
            Err(e) => Err(transfer_err(e)),
        }
    }

    /// An EP0 control transfer; IN returns up to `length` bytes.
    pub fn control(&self, x: &Control, timeout: Duration) -> Result<Vec<u8>, UsbError> {
        control(&self.intf, x, timeout)
    }
}

fn tracing_hex_level(level: u8, tag: &str, b: &[u8]) {
    if crate::debug_level() >= level {
        tracing_hex(tag, b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_vendor_request() {
        assert!(check_request(0x40, 0xA2).is_err());
        assert!(check_request(0xC0, 0xA2).is_err());
        assert!(check_request(0x40, 0xA0).is_ok());
        assert!(check_request(0x00, 0xA2).is_ok()); // a standard request numbered 0xA2 is not the vendor one
    }

    #[test]
    fn listing_does_not_fail() {
        // No scanner needed: the bus listing itself must work.
        let _ = list();
    }
}
