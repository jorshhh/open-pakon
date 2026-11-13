//! `pakon_probe` in Rust: --list, --load-firmware PAKFW,
//! --load-firmware-hex STAGE1 MAIN, and the default classification.

use pakon_cli::say;
use pakon_usb::device::{self, DevClass, Device, UsbError};

fn usage() -> i32 {
    eprintln!("usage: pakon_probe [--list] [--load-firmware PAKFW] [--load-firmware-hex STAGE1 MAIN]");
    2
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let (mut list, mut pakfw, mut hex) = (false, None, None);
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return usage(),
            "--list" => list = true,
            "--load-firmware" if i + 1 < args.len() => {
                i += 1;
                pakfw = Some(args[i].clone());
            }
            "--load-firmware-hex" if i + 2 < args.len() => {
                hex = Some((args[i + 1].clone(), args[i + 2].clone()));
                i += 2;
            }
            "--timeout" if i + 1 < args.len() => i += 1,
            a => {
                eprintln!("unknown/incomplete argument: {a}");
                return usage();
            }
        }
        i += 1;
    }
    if list {
        return match device::list() {
            Ok(devs) => {
                for d in devs {
                    say(&format!("{:04x}:{:04x} bus {} addr {} class 0x{:02x}", d.vid, d.pid, d.bus, d.address, d.class));
                }
                0
            }
            Err(e) => {
                eprintln!("list: {e}");
                1
            }
        };
    }
    if let Some((s1, main)) = hex {
        let read = |p: &str| std::fs::read_to_string(p).map_err(|e| UsbError::Firmware(format!("cannot read '{p}': {e}")));
        let r = read(&s1).and_then(|a| read(&main).map(|b| (a, b))).and_then(|(a, b)| device::load_firmware_hex(&a, &b));
        return match r {
            Ok(()) => {
                say("load-firmware-hex: firmware loaded from HEX, device is warm");
                0
            }
            Err(e) => {
                eprintln!("load-firmware-hex: {e}");
                1
            }
        };
    }
    if let Some(p) = pakfw {
        let r = std::fs::read_to_string(&p).map_err(|e| UsbError::Firmware(format!("cannot read '{p}': {e}"))).and_then(|t| device::load_firmware_script(&t));
        return match r {
            Ok(()) => {
                say("load-firmware: firmware sent");
                0
            }
            Err(e) => {
                eprintln!("load-firmware: {e}");
                1
            }
        };
    }
    match device::find() {
        Err(UsbError::NoDevice) => {
            say("no device");
            0
        }
        Ok(DevClass::Cold) => {
            say("found cold FX2 device (needs firmware)");
            0
        }
        Ok(DevClass::Warm) => {
            say("found warm Pakon device");
            match Device::open() {
                Ok(d) => {
                    for e in d.endpoints() {
                        say(&format!("  ep 0x{:02x} attr 0x{:02x} maxpkt {} ifc {} alt {}", e.address, e.attributes, e.max_packet, e.interface, e.altsetting));
                    }
                    0
                }
                Err(e) => {
                    eprintln!("open: {e}");
                    1
                }
            }
        }
        Err(e) => {
            say(&format!("enumeration: {e}"));
            1
        }
    }
}

fn main() {
    std::process::exit(run());
}
