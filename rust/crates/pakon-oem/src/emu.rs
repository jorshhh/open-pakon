//! What both DLL hosts share: mapping the image, binding the imports to
//! stubs, reading arguments and calling a function until it returns.

use std::collections::HashMap;

use unicorn_engine::{Prot, RegisterX86, Unicorn};

use crate::pe::{align, Pe};

/// Host state that can carry a fault out of a hook (hooks cannot return
/// errors: they record one and stop the emulation).
pub(crate) trait Host {
    fn fault(&mut self) -> &mut Option<String>;
}

pub(crate) fn uc_err(e: unicorn_engine::uc_error) -> String {
    format!("unicorn: {e:?}")
}

/// Map the image at its preferred base: headers, then each section.
pub(crate) fn map_image<D>(uc: &mut Unicorn<D>, pe: &Pe) -> Result<(), String> {
    let base = pe.image_base as u64;
    uc.mem_map(base, align(pe.size_of_image as u64, 0x1000), Prot::ALL).map_err(uc_err)?;
    uc.mem_write(base, &pe.headers).map_err(uc_err)?;
    for (va, data) in &pe.sections {
        uc.mem_write(base + *va as u64, data).map_err(uc_err)?;
    }
    Ok(())
}

/// Point every import at a 16-byte stub at `stub_base + 0x10 n` holding
/// `code(name)`. Returns stub address -> import name.
pub(crate) fn bind_imports<D>(
    uc: &mut Unicorn<D>,
    pe: &Pe,
    stub_base: u32,
    code: impl Fn(&str) -> Vec<u8>,
) -> Result<HashMap<u32, String>, String> {
    let mut stubs = HashMap::new();
    for (n, (name, iat)) in pe.imports.iter().enumerate() {
        let addr = stub_base + 0x10 * n as u32;
        uc.mem_write(*iat as u64, &addr.to_le_bytes()).map_err(uc_err)?;
        uc.mem_write(addr as u64, &code(name)).map_err(uc_err)?;
        stubs.insert(addr, name.clone());
    }
    Ok(stubs)
}

pub(crate) fn read<D>(uc: &Unicorn<D>, addr: u32, n: usize) -> Vec<u8> {
    uc.mem_read_as_vec(addr as u64, n).unwrap_or_else(|_| vec![0; n])
}

pub(crate) fn u32_at<D>(uc: &Unicorn<D>, addr: u32) -> u32 {
    let mut b = [0u8; 4];
    let _ = uc.mem_read(addr as u64, &mut b);
    u32::from_le_bytes(b)
}

pub(crate) fn esp<D>(uc: &Unicorn<D>) -> u32 {
    uc.reg_read(RegisterX86::ESP).unwrap_or(0) as u32
}

/// Argument `i` of the function whose return address is at [ESP].
pub(crate) fn arg<D>(uc: &Unicorn<D>, i: u32) -> u32 {
    u32_at(uc, esp(uc) + 4 + 4 * i)
}

/// Call `func(args...)` with a return address of `ret_magic` (a `hlt`) on a
/// fresh frame just under `stack_top`; returns EAX.
pub(crate) fn call<D: Host>(uc: &mut Unicorn<D>, func: u32, args: &[u32], stack_top: u32, ret_magic: u32) -> Result<u32, String> {
    let mut frame = ret_magic.to_le_bytes().to_vec();
    for a in args {
        frame.extend_from_slice(&a.to_le_bytes());
    }
    let esp = stack_top - 0x1000 - frame.len() as u32;
    uc.mem_write(esp as u64, &frame).map_err(uc_err)?;
    uc.reg_write(RegisterX86::ESP, esp as u64).map_err(uc_err)?;
    let r = uc.emu_start(func as u64, ret_magic as u64, 0, 0);
    if let Some(e) = uc.get_data_mut().fault().take() {
        return Err(e);
    }
    if let Err(e) = r {
        return Err(format!("emulation fault at {:#x}: {e:?}", uc.reg_read(RegisterX86::EIP).unwrap_or(0)));
    }
    Ok(uc.reg_read(RegisterX86::EAX).map_err(uc_err)? as u32)
}

pub(crate) fn set_eax<D>(uc: &mut Unicorn<D>, v: u32) {
    let _ = uc.reg_write(RegisterX86::EAX, v as u64);
}
