//! The OEM dust removal (Applied Science Fiction Digital ICE,
//! `oem/DMLDICELib.dll`) under Unicorn; port of `tools/oem_dice.py`.
//!
//! TLB.dll calls it on the 16-bit planar picture before the density LUT:
//! `DMLDICEBegin(0x65, static {0x20, 2, 1, 14, 2, 0, 0, 0})`, then per picture
//! `DMLDICEProcess(handle, dynamic {0x30, planes, d0, d1, 300, negative, 0,
//! 0, 0, d0 - 1, d1 - 1, 0})`. The planes are R, G, B, IR back to back,
//! 14-bit, so the frame goes in as (raw - 300) / 4.
//!
//! The DLL links the MSVC runtime statically and imports only KERNEL32: every
//! import is a stdcall stub, the runtime's DllMain runs first, fs:[0] points
//! at a thread block, and its worker threads run on the spot when
//! WaitForSingleObject waits for them.
//!
//! Only the pixels DICE changes are taken back into the 16-bit frame. One
//! `Dice` per thread (Unicorn is not `Send`); see [`with_thread_dice`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ndarray::{Array2, Array3, ArrayView2, ArrayView3, Axis, Zip};
use unicorn_engine::{Arch, Mode, Prot, RegisterX86, Unicorn};

use crate::emu::{self, arg, set_eax, u32_at, uc_err, Host};
use crate::pe::{align, Pe};

const F_DLLMAIN: u32 = 0x1002_C2C0;
const F_BEGIN: u32 = 0x1002_B640;
const F_PROCESS: u32 = 0x1002_B6F0;
const F_COUNT: u32 = 0x1002_B780;

const STACK_TOP: u32 = 0x0F00_0000;
const STACK_SIZE: u32 = 0x0080_0000;
const TEB: u32 = 0x0F10_0000;
const GDT: u32 = 0x0F11_0000;
const STUB_BASE: u32 = 0x0E00_0000;
const STUB_SPAN: u32 = 0x0080_0000;
const HEAP_BASE: u32 = 0x2000_0000;
const HEAP_SIZE: u32 = 0x6000_0000; // 1.5 GB, committed lazily
const RET_MAGIC: u32 = 0x0E7F_FFF0;
/// Where a thread function run by a wait returns.
const THREAD_RET: u32 = 0x0E7F_FF00;

/// stdcall argument counts of the KERNEL32 imports.
fn stdcall_args(name: &str) -> u16 {
    match name {
        "CreateThread" => 6,
        "WaitForSingleObject" | "SetThreadPriority" | "SetPriorityClass" | "TlsSetValue" | "TerminateProcess" | "GetCPInfo" | "IsBadWritePtr"
        | "IsBadReadPtr" | "SetStdHandle" | "GetProcAddress" => 2,
        "RtlUnwind" | "HeapReAlloc" | "VirtualAlloc" | "SetFilePointer" | "RaiseException" | "GetStringTypeW" => 4,
        "VirtualFree" | "HeapFree" | "HeapAlloc" | "GetModuleFileNameA" | "HeapCreate" => 3,
        "GetModuleHandleA" | "TlsFree" | "SetLastError" | "TlsGetValue" | "EnterCriticalSection"
        | "LeaveCriticalSection" | "ExitProcess" | "SetHandleCount" | "GetStdHandle" | "GetFileType"
        | "GetStartupInfoA" | "DeleteCriticalSection" | "FreeEnvironmentStringsA" | "FreeEnvironmentStringsW"
        | "HeapDestroy" | "InitializeCriticalSection" | "InterlockedDecrement" | "InterlockedIncrement"
        | "SetUnhandledExceptionFilter" | "IsBadCodePtr" | "LoadLibraryA" | "FlushFileBuffers" | "CloseHandle" => 1,
        "WideCharToMultiByte" => 8,
        "WriteFile" | "GetStringTypeA" => 5,
        "MultiByteToWideChar" | "LCMapStringA" | "LCMapStringW" => 6,
        _ => 0,
    }
}

/// `oem/DMLDICELib.dll` under the repo, or `PAKON_DICE_DLL`.
pub fn default_dll(repo: &Path) -> PathBuf {
    std::env::var_os("PAKON_DICE_DLL").map(PathBuf::from).unwrap_or_else(|| repo.join("oem").join("DMLDICELib.dll"))
}

#[derive(Default)]
struct State {
    stubs: HashMap<u32, String>,
    heap: u32,
    sizes: HashMap<u32, u32>,
    free_lists: HashMap<u32, Vec<u32>>,
    tls: HashMap<u32, u32>,
    /// handle -> (proc, param, ran)
    threads: HashMap<u32, (u32, u32, bool)>,
    /// ESP at each wait running a thread
    waits: Vec<u32>,
    wait_stub: u32,
    strings: u32,
    out: String,
    fault: Option<String>,
    t0: Option<Instant>,
}

impl Host for State {
    fn fault(&mut self) -> &mut Option<String> {
        &mut self.fault
    }
}

type Uc = Unicorn<'static, State>;

fn gdt_entry(base: u32, limit: u32, access: u8, flags: u8) -> [u8; 8] {
    let (base, limit) = (base as u64, limit as u64);
    let v = (limit & 0xFFFF)
        | (base & 0xFF_FFFF) << 16
        | (access as u64) << 40
        | ((limit >> 16) & 0xF) << 48
        | ((flags as u64) & 0xF) << 52
        | ((base >> 24) & 0xFF) << 56;
    v.to_le_bytes()
}

fn alloc(uc: &mut Unicorn<State>, n: u32, data: Option<&[u8]>) -> Result<u32, String> {
    let n = align(n.max(1) as u64, 16) as u32;
    let st = uc.get_data_mut();
    let p = match st.free_lists.get_mut(&n).and_then(|l| l.pop()) {
        Some(p) => p,
        None => {
            let p = st.heap;
            if (p as u64 + n as u64) > HEAP_BASE as u64 + HEAP_SIZE as u64 {
                return Err("emulated heap exhausted".into());
            }
            st.heap += n;
            p
        }
    };
    st.sizes.insert(p, n);
    match data {
        Some(d) => uc.mem_write(p as u64, d).map_err(uc_err)?,
        None => uc.mem_write(p as u64, &vec![0u8; n as usize]).map_err(uc_err)?,
    }
    Ok(p)
}

fn free(uc: &mut Unicorn<State>, p: u32) {
    let st = uc.get_data_mut();
    if let Some(n) = st.sizes.remove(&p) {
        st.free_lists.entry(n).or_default().push(p);
    }
}

fn fail(uc: &mut Unicorn<State>, msg: String) {
    uc.get_data_mut().fault = Some(msg);
    let _ = uc.emu_stop();
}

/// One KERNEL32 call (or a thread coming back to its wait).
fn stub(uc: &mut Unicorn<State>, addr: u32) {
    if addr == THREAD_RET {
        let st = uc.get_data_mut();
        let (esp, eip) = (st.waits.pop().unwrap_or(0), st.wait_stub);
        let _ = uc.reg_write(RegisterX86::ESP, esp as u64);
        let _ = uc.reg_write(RegisterX86::EIP, eip as u64);
        return;
    }
    let Some(name) = uc.get_data().stubs.get(&addr).cloned() else { return };
    let a = |uc: &Unicorn<State>, i| arg(uc, i);
    let w = |uc: &mut Unicorn<State>, p: u32, b: &[u8]| {
        let _ = uc.mem_write(p as u64, b);
    };
    let r: u32 = match name.as_str() {
        "HeapAlloc" => match alloc(uc, a(uc, 2), None) {
            Ok(p) => p,
            Err(e) => return fail(uc, e),
        },
        "HeapReAlloc" => {
            let (old, n) = (a(uc, 2), a(uc, 3));
            let r = match alloc(uc, n, None) {
                Ok(p) => p,
                Err(e) => return fail(uc, e),
            };
            let keep = n.min(uc.get_data().sizes.get(&old).copied().unwrap_or(0));
            if keep > 0 {
                let b = emu::read(uc, old, keep as usize);
                w(uc, r, &b);
            }
            free(uc, old);
            r
        }
        "HeapFree" => {
            free(uc, a(uc, 2));
            1
        }
        "VirtualAlloc" => {
            if a(uc, 0) != 0 {
                a(uc, 0)
            } else {
                match alloc(uc, align(a(uc, 1) as u64, 0x1000) as u32, None) {
                    Ok(p) => p,
                    Err(e) => return fail(uc, e),
                }
            }
        }
        "VirtualFree" => {
            if a(uc, 2) == 0x8000 {
                free(uc, a(uc, 0)); // MEM_RELEASE
            }
            1
        }
        "HeapCreate" => 0x00AB_0000,
        "HeapDestroy" | "SetHandleCount" | "SetStdHandle" | "FlushFileBuffers" | "CloseHandle" | "SetThreadPriority"
        | "SetPriorityClass" | "TlsFree" => 1,
        "GetVersion" => 0x0A28_0105, // Windows XP
        "GetCommandLineA" => uc.get_data().strings,
        "GetModuleFileNameA" => {
            let s = b"C:\\DMLDICELib.dll\0";
            let n = (a(uc, 2) as usize).min(s.len());
            let p = a(uc, 1);
            w(uc, p, &s[..n]);
            (n as u32).wrapping_sub(1)
        }
        "GetModuleHandleA" | "LoadLibraryA" | "GetProcAddress" | "GetEnvironmentStringsW" | "GetStringTypeA"
        | "GetStringTypeW" | "LCMapStringA" | "LCMapStringW" | "SetUnhandledExceptionFilter" | "IsBadWritePtr"
        | "IsBadReadPtr" | "IsBadCodePtr" | "GetLastError" | "SetLastError" | "FreeEnvironmentStringsA"
        | "FreeEnvironmentStringsW" | "InitializeCriticalSection" | "EnterCriticalSection" | "LeaveCriticalSection"
        | "DeleteCriticalSection" | "MultiByteToWideChar" | "WideCharToMultiByte" => 0,
        "GetEnvironmentStrings" => uc.get_data().strings + 5, // "\0\0"
        "GetStartupInfoA" => {
            let mut b = vec![0u8; 68];
            b[0] = 68;
            let p = a(uc, 0);
            w(uc, p, &b);
            0
        }
        "GetStdHandle" => match a(uc, 0) {
            0xFFFF_FFF6 => 3,
            0xFFFF_FFF5 => 7,
            0xFFFF_FFF4 => 11,
            _ => 0xFFFF_FFFF,
        },
        "GetFileType" => 2, // FILE_TYPE_CHAR
        "GetCPInfo" => {
            let mut b = vec![0u8; 18];
            b[0] = 1;
            b[4] = b'?';
            let p = a(uc, 1);
            w(uc, p, &b);
            1
        }
        "GetACP" | "GetOEMCP" => 1252,
        "GetTickCount" => {
            let st = uc.get_data_mut();
            st.t0.get_or_insert_with(Instant::now).elapsed().as_millis() as u32
        }
        "GetCurrentThreadId" | "TlsAlloc" => 1,
        "GetCurrentProcess" => 0xFFFF_FFFF,
        "TlsSetValue" => {
            let (k, v) = (a(uc, 0), a(uc, 1));
            uc.get_data_mut().tls.insert(k, v);
            1
        }
        "TlsGetValue" => {
            let k = a(uc, 0);
            uc.get_data().tls.get(&k).copied().unwrap_or(0)
        }
        "InterlockedIncrement" | "InterlockedDecrement" => {
            let p = a(uc, 0);
            let r = if name == "InterlockedIncrement" { u32_at(uc, p).wrapping_add(1) } else { u32_at(uc, p).wrapping_sub(1) };
            w(uc, p, &r.to_le_bytes());
            r
        }
        "WriteFile" => {
            let (p, n, done) = (a(uc, 1), a(uc, 2), a(uc, 3));
            let s: String = emu::read(uc, p, n as usize).iter().map(|&b| b as char).collect();
            uc.get_data_mut().out.push_str(&s);
            if done != 0 {
                w(uc, done, &n.to_le_bytes());
            }
            1
        }
        "CreateThread" => {
            let (proc_, param, id) = (a(uc, 2), a(uc, 3), a(uc, 5));
            let st = uc.get_data_mut();
            let h = 0x7000 + 4 * st.threads.len() as u32;
            st.threads.insert(h, (proc_, param, false));
            if id != 0 {
                w(uc, id, &h.to_le_bytes());
            }
            h
        }
        "WaitForSingleObject" => {
            let h = a(uc, 0);
            if let Some(t) = uc.get_data_mut().threads.get_mut(&h).filter(|t| !t.2) {
                // run the thread now: proc(param), returning to THREAD_RET
                t.2 = true;
                let (proc_, param) = (t.0, t.1);
                let sp = emu::esp(uc);
                uc.get_data_mut().waits.push(sp);
                let sp = sp - 0x100 - 8;
                let mut f = THREAD_RET.to_le_bytes().to_vec();
                f.extend_from_slice(&param.to_le_bytes());
                w(uc, sp, &f);
                let _ = uc.reg_write(RegisterX86::ESP, sp as u64);
                let _ = uc.reg_write(RegisterX86::EIP, proc_ as u64);
                return;
            }
            0 // WAIT_OBJECT_0
        }
        _ => return fail(uc, format!("DMLDICELib called an unhandled import: {name}")),
    };
    set_eax(uc, r);
}

/// DMLDICELib.dll in a 32-bit Unicorn machine, Begin'd once.
pub struct Dice {
    uc: Uc,
    handle: u32,
}

/// Defect counts DMLDICEDefectCount reports.
pub type Counts = (u32, u32);

impl Dice {
    pub fn new(dll: &Path) -> Result<Dice, String> {
        let pe = Pe::read(dll)?;
        let mut uc: Uc = Unicorn::new_with_data(Arch::X86, Mode::MODE_32, State::default()).map_err(uc_err)?;
        emu::map_image(&mut uc, &pe)?;
        let map = |uc: &mut Uc, a: u32, n: u32| uc.mem_map(a as u64, n as u64, Prot::ALL).map_err(uc_err);
        map(&mut uc, STACK_TOP - STACK_SIZE, STACK_SIZE)?;
        map(&mut uc, TEB, 0x20000)?;
        map(&mut uc, HEAP_BASE, HEAP_SIZE)?;
        map(&mut uc, STUB_BASE, STUB_SPAN)?;
        // fs -> the thread block: an exception list end, stack range, self
        let teb: Vec<u8> =
            [0xFFFF_FFFF, STACK_TOP, STACK_TOP - STACK_SIZE, 0, 0, 0, TEB].iter().flat_map(|v: &u32| v.to_le_bytes()).collect();
        uc.mem_write(TEB as u64, &teb).map_err(uc_err)?;
        // flat 32-bit code (1) and data (2), and the thread block (3); with a
        // GDT loaded, SS must be a 32-bit segment or pushes wrap at 64 KB
        uc.mem_write((GDT + 8) as u64, &gdt_entry(0, 0xFFFFF, 0x9A, 0xC)).map_err(uc_err)?;
        uc.mem_write((GDT + 16) as u64, &gdt_entry(0, 0xFFFFF, 0x92, 0xC)).map_err(uc_err)?;
        uc.mem_write((GDT + 24) as u64, &gdt_entry(TEB, 0xFFF, 0x92, 0x4)).map_err(uc_err)?;
        // uc_x86_mmr {u16 selector; u64 base; u32 limit; u32 flags}
        let mut mmr = [0u8; 24];
        mmr[8..16].copy_from_slice(&(GDT as u64).to_le_bytes());
        mmr[16..20].copy_from_slice(&(4 * 8 - 1u32).to_le_bytes());
        uc.reg_write_long(RegisterX86::GDTR, &mmr).map_err(uc_err)?;
        for r in [RegisterX86::SS, RegisterX86::DS, RegisterX86::ES] {
            uc.reg_write(r, 2 << 3).map_err(uc_err)?;
        }
        uc.reg_write(RegisterX86::FS, 3 << 3).map_err(uc_err)?;

        let stubs = emu::bind_imports(&mut uc, &pe, STUB_BASE, |name| {
            let k = 4 * stdcall_args(name);
            vec![0xC2, k as u8, (k >> 8) as u8] // ret 4k
        })?;
        let wait_stub = stubs.iter().find(|(_, n)| *n == "WaitForSingleObject").map(|(a, _)| *a).unwrap_or(0);
        {
            let st = uc.get_data_mut();
            st.stubs = stubs;
            st.wait_stub = wait_stub;
            st.heap = HEAP_BASE;
        }
        let mut s = b"dice\0\0\0C:\\DMLDICELib.dll\0".to_vec();
        s.resize(0x100, 0);
        let strings = alloc(&mut uc, 0x100, Some(&s))?;
        uc.get_data_mut().strings = strings;
        uc.mem_write(RET_MAGIC as u64, &[0xF4]).map_err(uc_err)?; // hlt
        uc.add_code_hook(STUB_BASE as u64, (STUB_BASE + STUB_SPAN - 1) as u64, |uc, addr, _| stub(uc, addr as u32))
            .map_err(uc_err)?;

        let mut d = Dice { uc, handle: 0 };
        if d.call(F_DLLMAIN, &[pe.image_base, 1, 0])? == 0 {
            return Err("DMLDICELib DllMain failed".into());
        }
        d.handle = d.call(F_BEGIN, &[0x65, 0x20, 2, 1, 0xE, 2, 0, 0, 0])?;
        if d.handle == 0 {
            return Err("DMLDICEBegin returned NULL".into());
        }
        Ok(d)
    }

    fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, String> {
        emu::call(&mut self.uc, func, args, STACK_TOP, RET_MAGIC)
    }

    /// Text the DLL wrote to its standard handles during the last picture.
    pub fn output(&self) -> &str {
        &self.uc.get_data().out
    }

    /// rgb14 (H, W, 3), ir14 (H, W): 14-bit. Returns the cleaned rgb14 and
    /// the defect counts.
    pub fn process(&mut self, rgb14: ArrayView3<u16>, ir14: ArrayView2<u16>, negative: bool) -> Result<(Array3<u16>, Counts), String> {
        let (h, w) = ir14.dim();
        let mut planes = Vec::with_capacity(8 * h * w);
        for c in 0..3 {
            for v in rgb14.index_axis(Axis(2), c).iter() {
                planes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in ir14.iter() {
            planes.extend_from_slice(&v.to_le_bytes());
        }
        let n = planes.len() as u32;
        let buf = alloc(&mut self.uc, n, Some(&planes))?;
        drop(planes);
        let (d0, d1) = (w as u32, h as u32);
        let dynamic = [0x30, buf, d0, d1, 300, negative as u32, 0, 0, 0, d0 - 1, d1 - 1, 0];
        self.uc.get_data_mut().out.clear();
        let mut args = vec![self.handle];
        args.extend_from_slice(&dynamic);
        let rc = self.call(F_PROCESS, &args)?;
        if rc != 0 {
            free(&mut self.uc, buf);
            return Err(format!("DMLDICEProcess returned {}", rc as i32));
        }
        let counts = alloc(&mut self.uc, 8, None)?;
        args.extend_from_slice(&[counts, counts + 4]);
        self.call(F_COUNT, &args)?;
        let n_counts = (u32_at(&self.uc, counts), u32_at(&self.uc, counts + 4));
        free(&mut self.uc, counts);
        let raw = self.uc.mem_read_as_vec(buf as u64, 6 * h * w).map_err(uc_err)?;
        free(&mut self.uc, buf);
        let plane = h * w;
        let out = Array3::from_shape_fn((h, w, 3), |(y, x, c)| {
            let i = 2 * (c * plane + y * w + x);
            u16::from_le_bytes([raw[i], raw[i + 1]])
        });
        Ok((out, n_counts))
    }
}

/// 16-bit samples (black at 300) -> the 14-bit values TLB hands DICE.
pub fn to14(v: u16) -> u16 {
    ((v as f32 - 300.0) / 4.0).clamp(0.0, 16383.0) as u16
}

pub fn from14(v: u16) -> u16 {
    (v as f32 * 4.0 + 300.0 + 0.5).clamp(0.0, 65535.0) as u16
}

/// `pakon_ice::clean_frame`'s interface: raw (H, W, 3) registered,
/// flat-fielded samples (black at 300), ir (H, W) the 16-bit IR plane ->
/// (cleaned frame, mask of rebuilt pixels). Rebuilt pixels come from DICE,
/// every other one is raw as it was. `negative = false` for slides.
pub fn clean_frame(dice: &mut Dice, raw: ArrayView3<u16>, ir: ArrayView2<u16>, negative: bool) -> Result<(Array3<u16>, Array2<bool>), String> {
    let rgb14 = raw.mapv(to14);
    // Python: to14(ir as f32 + 300); exact for u16
    let ir14 = ir.mapv(|v| (((v as f32 + 300.0) - 300.0) / 4.0).clamp(0.0, 16383.0) as u16);
    let (out14, _) = dice.process(rgb14.view(), ir14.view(), negative)?;
    let mut mask = Array2::<bool>::from_elem(ir.dim(), false);
    Zip::from(&mut mask).and(out14.lanes(Axis(2))).and(rgb14.lanes(Axis(2))).for_each(|m, o, r| *m = o != r);
    let mut out = raw.to_owned();
    Zip::from(out.lanes_mut(Axis(2))).and(out14.lanes(Axis(2))).and(&mask).for_each(|mut d, o, &m| {
        if m {
            for c in 0..3 {
                d[c] = from14(o[c]);
            }
        }
    });
    Ok((out, mask))
}

thread_local! {
    static DICE: RefCell<Option<Dice>> = const { RefCell::new(None) };
}

/// Run `f` with this thread's `Dice` (built on first use from `dll`).
pub fn with_thread_dice<R>(dll: &Path, f: impl FnOnce(&mut Dice) -> Result<R, String>) -> Result<R, String> {
    DICE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Dice::new(dll)?);
        }
        f(slot.as_mut().unwrap())
    })
}
