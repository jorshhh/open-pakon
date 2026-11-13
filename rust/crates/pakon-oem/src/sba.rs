//! OEM scene and roll balance (Kodak Ansel SBA + FOS) from
//! `oem/PakonIMAu.dll` under Unicorn; port of `tools/oem_sba.py`.
//!
//! The balance code is plain 32-bit x86 C: the DLL is mapped at its
//! preferred base, every import goes through a cdecl stub (calloc/free,
//! setjmp, the x87 maths helpers), and the C functions are called with the
//! structures the OEM C++ wrapper (AnsSbaCapabilityImpl) would build.
//!
//! Per roll:
//!   1. each frame: createAlgData (24x36 sample grid) + Sba() pass 1;
//!   2. SbaCalcFosResults over the first `maxFramesToFos` frames;
//!   3. each frame: Dmin lowered to the roll's, Sba() pass 2 with the FOS
//!      results, then Preference() -> the frame's RPD shift (R, G, B).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ndarray::{s, Array3, ArrayView3, Axis};
use pakon_image::colour::{self, Matrix};
use pakon_image::stats;
use unicorn_engine::{Arch, Mode, Prot, RegisterX86, Unicorn};

use crate::emu::{self, arg, set_eax, uc_err, Host};
use crate::pe::Pe;

const F_DECODE_PCODE: u32 = 0x1028_84B0;
const F_MAKESFS: u32 = 0x102A_C820;
const F_CREATE_ALG: u32 = 0x1028_CEB0;
const F_SBA: u32 = 0x1028_B8D0;
const F_FOS: u32 = 0x1028_F570;
const F_PREFERENCE: u32 = 0x1028_C780;

// AnsSbaCapabilityImpl layout (only what the C core touches).
const IMPL_SIZE: u32 = 0x5A00;
const ALG: u32 = 0x1A;
const DMIN_STATS: u32 = 0x289C;
const WORK: u32 = 0x290C;
const INFO: u32 = 0x388C;
const RESULT: u32 = 0x38A2;
const PREF_OUT: u32 = 0x3A30;
const SHIFTS: u32 = 0x3A38;
const FRAME_DMIN: u32 = 0x3BC8;
const DPI: u32 = 0x4CF0;
const PCODE_PTR: u32 = 0x5978;

/// The landscape SBA analysis image.
pub const ANALYSIS_W: usize = 192;
pub const ANALYSIS_H: usize = 128;

/// SbaParameterReader field offsets: (key, offset, count of shorts).
const DPI_FIELDS: &[(&str, usize, usize)] = &[
    ("fpa", 0x00, 3),
    ("neu", 0x06, 3),
    ("neo", 0x0C, 3),
    ("foson", 0x12, 1),
    ("usehistory", 0x14, 1),
    ("fmt", 0x16, 1),
    ("dmd", 0x18, 3),
    ("fpo", 0x1E, 3),
    ("pcls", 0x24, 1),
    ("pcwf", 0x26, 1),
    ("ix_pcwf", 0x28, 1),
    ("nonflashadj", 0x2A, 1),
    ("cmm", 0x2C, 1),
    ("fog", 0x2E, 1),
    ("fxr", 0x30, 1),
    ("blk", 0x32, 1),
    ("bxr", 0x34, 1),
    ("ll1", 0x36, 1),
    ("ll2", 0x38, 1),
    ("ll3", 0x3A, 1),
    ("mff", 0x3C, 1),
    ("maxframestofos", 0x3E, 1),
    ("splicepct", 0x40, 1),
    ("orderfpolowsatlimit", 0x42, 1),
    ("orderfpohighsatlimit", 0x44, 1),
    ("orderfpominweight", 0x46, 1),
    ("orderfpomaxweight", 0x48, 1),
    ("orderfpouseexpandedsatlimit", 0x4A, 1),
    ("minpeakheight", 0x4C, 1),
    ("minpeakdist", 0x4E, 1),
    ("neutralbutton", 0x60, 1),
    ("neutralbalancepoint", 0x62, 1),
    ("writedebugimages", 0x64, 1),
    ("mindmin", 0x68, 3),
];
const DPI_DOUBLES: &[(&str, usize)] = &[("neutralunderconstraint", 0x50), ("neutraloverconstraint", 0x58)];

/// FUN_10214e30: Sba()'s 56-byte input block, (block offset, DPI offset, shorts).
const SBA_BLOCK: &[(usize, usize, usize)] = &[
    (0x00, 0x1E, 3),
    (0x06, 0x18, 3),
    (0x0C, 0x26, 1),
    (0x0E, 0x28, 1),
    (0x10, 0x2A, 1),
    (0x12, 0x2E, 1),
    (0x14, 0x30, 1),
    (0x16, 0x32, 1),
    (0x18, 0x34, 1),
    (0x1A, 0x3C, 1),
    (0x1C, 0x42, 1),
    (0x1E, 0x44, 1),
    (0x20, 0x46, 1),
    (0x22, 0x48, 1),
    (0x24, 0x4A, 1),
];

/// FUN_10214f20: Preference()'s parameter block.
const PREF_BLOCK: &[(usize, usize, usize)] = &[
    (0x00, 0x1E, 3),
    (0x06, 0x00, 3),
    (0x0C, 0x06, 3),
    (0x12, 0x0C, 3),
    (0x18, 0x18, 3),
    (0x1E, 0x24, 1),
    (0x20, 0x26, 1),
    (0x22, 0x28, 1),
    (0x24, 0x2A, 1),
    (0x26, 0x16, 1),
    (0x30, 0x2C, 1),
    (0x32, 0x2E, 1),
    (0x34, 0x30, 1),
    (0x36, 0x32, 1),
    (0x38, 0x34, 1),
    (0x3A, 0x36, 1),
    (0x3C, 0x38, 1),
    (0x40, 0x3C, 1),
];

/// Constructor defaults for the 20 command-switch bytes at INFO with no
/// camera metadata (FUN_10217d00): unknowns, then 1995-08-29 12:30.
const CMD_DEFAULTS: [u8; 20] =
    [0xFE, 0xFE, 0xFE, 0xFF, 0xFF, 0xFE, 0x00, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0x5F, 0x08, 0x1D, 0x0C, 0x1E];

/// Ansel sourceType for 35 mm colour neg and B&W (sba.map).
pub const SOURCE_NEGATIVE_35: i32 = 1;

// -- x87 80-bit values -----------------------------------------------------------

/// An x87 register (10 bytes, mantissa then sign/exponent) as the nearest
/// f64 (`_f80_to_float`).
fn f80_to_f64(b: &[u8]) -> f64 {
    let mant = u64::from_le_bytes(b[..8].try_into().unwrap());
    let se = u16::from_le_bytes([b[8], b[9]]);
    let (neg, exp) = (se >> 15 != 0, (se & 0x7FFF) as i32);
    if exp == 0 && mant == 0 {
        return if neg { -0.0 } else { 0.0 };
    }
    if exp == 0x7FFF {
        return if mant << 1 != 0 {
            f64::NAN
        } else if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let x = ldexp(mant as f64, exp - 16383 - 63);
    if neg {
        -x
    } else {
        x
    }
}

fn ldexp(mut x: f64, mut e: i32) -> f64 {
    while e > 1000 {
        x *= 2f64.powi(1000);
        e -= 1000;
    }
    while e < -1000 {
        x *= 2f64.powi(-1000);
        e += 1000;
    }
    x * 2f64.powi(e)
}

fn f64_to_f80(x: f64) -> [u8; 10] {
    let (mant, se): (u64, u16) = if x == 0.0 {
        (0, if x.is_sign_negative() { 0x8000 } else { 0 })
    } else if x.is_nan() {
        (3 << 62, 0x7FFF)
    } else if x.is_infinite() {
        (1 << 63, if x < 0.0 { 0xFFFF } else { 0x7FFF })
    } else {
        // |x| = m 2^e with 0.5 <= m < 1 (frexp)
        let bits = x.abs().to_bits();
        let (mut e, mut frac) = (((bits >> 52) & 0x7FF) as i32, bits & ((1 << 52) - 1));
        if e == 0 {
            // subnormal: normalise
            let shift = frac.leading_zeros() as i32 - 11;
            frac <<= shift;
            e = 1 - shift;
            frac &= (1 << 52) - 1;
        }
        let m = (1u64 << 63) | (frac << 11);
        let exp = (e - 1023 + 16383) as u16;
        (m, exp | if x < 0.0 { 0x8000 } else { 0 })
    };
    let mut b = [0u8; 10];
    b[..8].copy_from_slice(&mant.to_le_bytes());
    b[8..].copy_from_slice(&se.to_le_bytes());
    b
}

// -- the emulator ----------------------------------------------------------------

const STUB_BASE: u32 = 0x7F00_0000;
const STUB_SPAN: u32 = 0x0100_0000;
const STACK_TOP: u32 = 0x7E00_0000;
const STACK_SIZE: u32 = 0x0080_0000;
const HEAP_BASE: u32 = 0x6000_0000;
const HEAP_SIZE: u32 = 0x1000_0000;
const RET_MAGIC: u32 = 0x7F7F_FFF0;
/// The f64 a floor/ceil stub loads.
const SCRATCH: u32 = STUB_BASE + 0x00F0_1000;

#[derive(Default)]
struct State {
    stubs: HashMap<u32, String>,
    heap: u32,
    fault: Option<String>,
}

impl Host for State {
    fn fault(&mut self) -> &mut Option<String> {
        &mut self.fault
    }
}

/// The code at each import's stub. The hook runs before it and leaves the
/// x87 stack as the instructions expect: `_CIpow` and `_ftol` pop one
/// value, floor/ceil push the result the hook put at SCRATCH.
fn stub_code(name: &str) -> Vec<u8> {
    match name {
        "_CIpow" | "_ftol" => vec![0xDD, 0xD8, 0xC3], // fstp st(0); ret
        "floor" | "ceil" => {
            let mut v = vec![0xDD, 0x05]; // fld qword [SCRATCH]; ret
            v.extend_from_slice(&SCRATCH.to_le_bytes());
            v.push(0xC3);
            v
        }
        _ => vec![0xC3], // ret (cdecl: the caller cleans)
    }
}

fn st<D>(uc: &Unicorn<D>, i: i32) -> f64 {
    let r = RegisterX86::ST0 as i32 + i;
    uc.reg_read_long(r).map(|b| f80_to_f64(&b)).unwrap_or(f64::NAN)
}

fn set_st<D>(uc: &Unicorn<D>, i: i32, x: f64) {
    let _ = uc.reg_write_long(RegisterX86::ST0 as i32 + i, &f64_to_f80(x));
}

fn bump(uc: &mut Unicorn<State>, n: u32, data: Option<&[u8]>) -> Result<u32, String> {
    let st = uc.get_data_mut();
    let p = (st.heap + 15) & !15;
    st.heap = p + n.max(1);
    if st.heap > HEAP_BASE + HEAP_SIZE - 0x40000 {
        return Err("emulated heap exhausted".into());
    }
    match data {
        Some(d) => uc.mem_write(p as u64, d),
        None => uc.mem_write(p as u64, &vec![0u8; n.max(1) as usize]),
    }
    .map_err(uc_err)?;
    Ok(p)
}

fn fail(uc: &mut Unicorn<State>, msg: String) {
    uc.get_data_mut().fault = Some(msg);
    let _ = uc.emu_stop();
}

fn stub(uc: &mut Unicorn<State>, addr: u32) {
    let Some(name) = uc.get_data().stubs.get(&addr).cloned() else { return };
    let r: u32 = match name.as_str() {
        "calloc" => match bump(uc, arg(uc, 0).wrapping_mul(arg(uc, 1)), None) {
            Ok(p) => p,
            Err(e) => return fail(uc, e),
        },
        "malloc" | "??2@YAPAXI@Z" => match bump(uc, arg(uc, 0), None) {
            Ok(p) => p,
            Err(e) => return fail(uc, e),
        },
        "free" | "??3@YAXPAX@Z" => 0,
        "memset" => {
            let (d, c, n) = (arg(uc, 0), arg(uc, 1), arg(uc, 2));
            let _ = uc.mem_write(d as u64, &vec![c as u8; n as usize]);
            d
        }
        "memcpy" | "memmove" => {
            let (d, s, n) = (arg(uc, 0), arg(uc, 1), arg(uc, 2));
            let b = emu::read(uc, s, n as usize);
            let _ = uc.mem_write(d as u64, &b);
            d
        }
        "_setjmp3" => 0, // the normal path
        "longjmp" => return fail(uc, "OEM code took its longjmp error path".into()),
        "_CIacos" | "_CIasin" | "_CItanh" => {
            // argument and result in ST0
            let x = st(uc, 0);
            let y = match name.as_str() {
                "_CIacos" => x.acos(),
                "_CIasin" => x.asin(),
                _ => x.tanh(),
            };
            set_st(uc, 0, y);
            return;
        }
        "_CIpow" => {
            // ST1 ** ST0; the stub pops ST0, leaving the result
            let (y, x) = (st(uc, 0), st(uc, 1));
            set_st(uc, 1, x.powf(y));
            return;
        }
        "floor" | "ceil" => {
            // double on the stack -> ST0 (pushed by the stub)
            let b = emu::read(uc, emu::esp(uc) + 4, 8);
            let x = f64::from_le_bytes(b.try_into().unwrap());
            let y = if name == "floor" { x.floor() } else { x.ceil() };
            let _ = uc.mem_write(SCRATCH as u64, &y.to_le_bytes());
            return;
        }
        "_ftol" => {
            // ST0 -> EDX:EAX, truncated; the stub pops
            let v = st(uc, 0) as i64;
            let _ = uc.reg_write(RegisterX86::EDX, ((v >> 32) as u32) as u64);
            v as u32
        }
        _ => return fail(uc, format!("OEM code called an unhandled import: {name}")),
    };
    set_eax(uc, r);
}

/// PakonIMAu.dll mapped into a 32-bit Unicorn machine. Functions are called
/// cdecl; memory comes from a bump heap (one emulator per roll, so nothing
/// is freed).
struct Emulator {
    uc: Unicorn<'static, State>,
}

impl Emulator {
    fn new(dll: &Path) -> Result<Emulator, String> {
        let pe = Pe::read(dll)?;
        let mut uc = Unicorn::new_with_data(Arch::X86, Mode::MODE_32, State::default()).map_err(uc_err)?;
        emu::map_image(&mut uc, &pe)?;
        uc.mem_map((STACK_TOP - STACK_SIZE) as u64, STACK_SIZE as u64, Prot::ALL).map_err(uc_err)?;
        uc.mem_map(HEAP_BASE as u64, HEAP_SIZE as u64, Prot::ALL).map_err(uc_err)?;
        uc.mem_map(STUB_BASE as u64, STUB_SPAN as u64, Prot::ALL).map_err(uc_err)?;
        let stubs = emu::bind_imports(&mut uc, &pe, STUB_BASE, stub_code)?;
        let st = uc.get_data_mut();
        st.stubs = stubs;
        st.heap = HEAP_BASE;
        uc.mem_write(RET_MAGIC as u64, &[0xF4]).map_err(uc_err)?; // hlt
        uc.add_code_hook(STUB_BASE as u64, (STUB_BASE + STUB_SPAN - 1) as u64, |uc, addr, _| stub(uc, addr as u32))
            .map_err(uc_err)?;
        Ok(Emulator { uc })
    }

    fn alloc(&mut self, n: u32, data: Option<&[u8]>) -> Result<u32, String> {
        bump(&mut self.uc, n, data)
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        self.uc.mem_write(addr as u64, data).map_err(uc_err)
    }

    fn u32(&self, addr: u32) -> u32 {
        emu::u32_at(&self.uc, addr)
    }

    fn s16(&self, addr: u32, n: usize) -> Vec<i16> {
        emu::read(&self.uc, addr, 2 * n).chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, String> {
        emu::call(&mut self.uc, func, args, STACK_TOP, RET_MAGIC)
    }
}

// -- parameter files ---------------------------------------------------------------

/// `oem/ansel/sba` under the repo.
pub fn sba_dir(repo: &Path) -> PathBuf {
    repo.join("oem").join("ansel").join("sba")
}

/// `oem/PakonIMAu.dll` under the repo.
pub fn default_dll(repo: &Path) -> PathBuf {
    repo.join("oem").join("PakonIMAu.dll")
}

/// An Ansel .dpi file as {lower-case key: values}.
pub fn read_dpi(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text: String = d.iter().map(|&b| b as char).collect(); // latin-1
    let mut vals = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            vals.insert(k.trim().to_lowercase(), v.split_whitespace().map(String::from).collect());
        }
    }
    Ok(vals)
}

/// sba.map's selector rows: (path, scanner, sourceType, product, gen, key).
fn map_rows(dir: &Path) -> Result<Vec<[String; 6]>, String> {
    let p = dir.join("SbaDPI").join("sba.map");
    let d = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let text: String = d.iter().map(|&b| b as char).collect();
    let (mut rows, mut on) = (Vec::new(), false);
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with("mapping") {
            on = true;
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        if on && w.len() == 7 && !w[0].starts_with('_') {
            rows.push([w[0], w[1], w[2], w[3], w[4], w[6]].map(String::from));
        }
    }
    Ok(rows)
}

/// Each .dpi's own key -> its file name.
fn dpi_files(dir: &Path) -> Result<HashMap<String, String>, String> {
    let folder = dir.join("SbaDPI");
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".dpi"))
        .collect();
    names.sort();
    let mut out = HashMap::new();
    for n in names {
        if let Some(k) = read_dpi(&folder.join(&n))?.get("key").and_then(|k| k.first()) {
            out.insert(k.clone(), n);
        }
    }
    Ok(out)
}

/// The SBA parameter file sba.map picks for a film: the first row whose
/// sourceType, product code and generation match (`any` matches anything;
/// no DX code matches only `any`), on the default path and scanner.
pub fn dpi_for(dir: &Path, product: Option<i32>, generation: Option<i32>, source_type: i32) -> Result<String, String> {
    let ok = |sel: &str, val: Option<i32>| sel == "any" || val.is_some_and(|v| v >= 0 && sel == v.to_string());
    let files = dpi_files(dir)?;
    for [path, scanner, src, prod, gen, key] in map_rows(dir)? {
        if path == "any" && scanner == "any" && ok(&src, Some(source_type)) && ok(&prod, product) && ok(&gen, generation) {
            return Ok(files.get(&key).cloned().unwrap_or_else(|| "sba-CN-default.dpi".into()));
        }
    }
    Ok("sba-CN-default.dpi".into())
}

fn int_of(s: &str) -> Result<i64, String> {
    s.parse::<i64>().map_err(|_| format!("not an integer: {s:?}"))
}

// -- SBA / FOS -------------------------------------------------------------------

/// One roll's worth of OEM scene balance (a fresh emulator per instance).
pub struct Sba {
    e: Emulator,
    dpi: HashMap<String, Vec<String>>,
    dpi_bytes: [u8; 0x70],
    pcode: u32,
    sfs: u32,
    block: u32,
}

fn check(rc: u32, what: &str) -> Result<(), String> {
    if rc != 0 {
        return Err(format!("{what} failed, return code {rc:#x}"));
    }
    Ok(())
}

fn pack(src: &[u8], layout: &[(usize, usize, usize)], size: usize) -> Vec<u8> {
    let mut b = vec![0u8; size];
    for &(bo, so, n) in layout {
        b[bo..bo + 2 * n].copy_from_slice(&src[so..so + 2 * n]);
    }
    b
}

fn le_i16s(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

impl Sba {
    /// `dpi` is a file name under `<dir>/SbaDPI`.
    pub fn new(dll: &Path, dir: &Path, dpi: &str) -> Result<Sba, String> {
        let mut e = Emulator::new(dll)?;
        let dpi = read_dpi(&dir.join("SbaDPI").join(dpi))?;
        let first = |k: &str| dpi.get(k).and_then(|v| v.first()).cloned().ok_or(format!("{k} missing from the .dpi"));
        let pp = dir.join("Pcode").join(first("pcode")?);
        let pcode = std::fs::read(&pp).map_err(|e| format!("{}: {e}", pp.display()))?;
        let out = e.alloc(4, None)?;
        let src = e.alloc(pcode.len() as u32 + 2, Some(&pcode))?;
        check(e.call(F_DECODE_PCODE, &[src, pcode.len() as u32, out])?, "SbaDecodePcode")?;
        let pcode = e.u32(out);
        let sp = dir.join("Sfs").join(first("sfstable")?);
        let text = std::fs::read_to_string(&sp).map_err(|e| format!("{}: {e}", sp.display()))?;
        let mut rows = Vec::new();
        for line in text.lines() {
            for x in line.split_whitespace().take(4) {
                rows.push(int_of(x)? as i16);
            }
        }
        // Makesfs(rows, out): out is the table object's +0x2c, which Sba() reads.
        let sfs = e.alloc(0x8000, None)?;
        let rp = e.alloc(2 * rows.len() as u32 + 8, Some(&le_i16s(&rows)))?;
        check(e.call(F_MAKESFS, &[rp, sfs + 0x2C])?, "Makesfs")?;
        let mut b = [0u8; 0x70];
        for &(k, off, n) in DPI_FIELDS {
            if let Some(v) = dpi.get(k) {
                for i in 0..n {
                    let x = v.get(i).ok_or(format!("{k} needs {n} values"))?;
                    let f: f64 = x.parse().map_err(|_| format!("{k}: not a number: {x:?}"))?;
                    b[off + 2 * i..off + 2 * i + 2].copy_from_slice(&(f.trunc() as i16).to_le_bytes());
                }
            }
        }
        for &(k, off) in DPI_DOUBLES {
            if let Some(v) = dpi.get(k).and_then(|v| v.first()) {
                let f: f64 = v.parse().map_err(|_| format!("{k}: not a number: {v:?}"))?;
                b[off..off + 8].copy_from_slice(&f.to_le_bytes());
            }
        }
        let block = e.alloc(56, Some(&pack(&b, SBA_BLOCK, 56)))?;
        Ok(Sba { e, dpi, dpi_bytes: b, pcode, sfs, block })
    }

    fn dpi_short(&self, off: usize) -> i16 {
        i16::from_le_bytes([self.dpi_bytes[off], self.dpi_bytes[off + 1]])
    }

    fn dpi_double(&self, off: usize) -> f64 {
        f64::from_le_bytes(self.dpi_bytes[off..off + 8].try_into().unwrap())
    }

    fn mindmin(&self) -> Result<[i64; 3], String> {
        let v = self.dpi.get("mindmin").ok_or("mindmin missing from the .dpi")?;
        Ok([int_of(&v[0])?, int_of(&v[1])?, int_of(&v[2])?])
    }

    fn pref_block(&mut self) -> Result<u32, String> {
        let mut b = pack(&self.dpi_bytes, PREF_BLOCK, 0x48);
        b[0x28..0x2A].copy_from_slice(&0x32i16.to_le_bytes());
        b[0x2A..0x2C].copy_from_slice(&0x53i16.to_le_bytes());
        b[0x2C..0x30].copy_from_slice(&(self.sfs + 0x2C).to_le_bytes());
        b[0x3E..0x40].copy_from_slice(&0x8Ci16.to_le_bytes());
        let nb = self.dpi_short(0x60) as f64;
        let (lo, hi) = (self.dpi_double(0x50), self.dpi_double(0x58));
        b[0x42..0x44].copy_from_slice(&((nb * lo).trunc() as i16).to_le_bytes());
        b[0x44..0x46].copy_from_slice(&((nb * hi).trunc() as i16).to_le_bytes());
        let v = self.dpi_short(0x62) as i64 * 0x2A495; // x sqrt(3)
        let r = ((if v >= 0 { v + 50000 } else { v - 50000 }) as f64 / 100000.0).trunc() as i16;
        b[0x46..0x48].copy_from_slice(&r.to_le_bytes());
        self.e.alloc(0x48, Some(&b))
    }

    /// The frame's film base as the OEM wrapper sets it (FUN_10218110): the
    /// frame's own minimum, or with a scene dmin, per channel the lower of
    /// the two, a scene value at or below the .dpi's `mindmin` counting as
    /// none (0xfff).
    pub fn frame_dmin(&self, stats: [i16; 3], dmin: Option<[i64; 3]>) -> Result<[i16; 3], String> {
        let Some(d) = dmin else { return Ok(stats) };
        let aim = self.mindmin()?;
        Ok(std::array::from_fn(|c| {
            let v = if d[c] <= aim[c] { 0xFFF } else { d[c] };
            v.min(stats[c] as i64) as i16
        }))
    }

    /// createAlgData + Sba() for one frame. `rpd`: (rows, cols, 3) 12-bit
    /// RPD, landscape. Returns the frame's object address.
    pub fn pass1(&mut self, rpd: ArrayView3<i16>, fos_on: bool, dmin: Option<[i64; 3]>) -> Result<u32, String> {
        let (rows, cols, _) = rpd.dim();
        let e = &mut self.e;
        let imp = e.alloc(IMPL_SIZE, None)?;
        e.write(imp + 0x10, &(imp + ALG).to_le_bytes())?;
        e.write(imp + DPI, &self.dpi_bytes)?;
        e.write(imp + PCODE_PTR, &self.pcode.to_le_bytes())?;
        e.write(imp + INFO, &CMD_DEFAULTS)?;
        let mut planes = Vec::with_capacity(rows * cols * 6);
        for c in 0..3 {
            for v in rpd.index_axis(Axis(2), c).iter() {
                planes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let data = e.alloc(planes.len() as u32, Some(&planes))?;
        let n = (rows * cols * 2) as u32;
        let image = e.alloc(16, Some(&le_u32s(&[0, data, data + n, data + 2 * n])))?; // planar
        let g: Vec<u32> = [rows as i32, cols as i32, 99999, 99999, 99999, 99999, 24, 36, 99999, 99999, 0].map(|v| v as u32).to_vec();
        let geometry = e.alloc(64, Some(&le_u32s(&g)))?;
        let inner = e.alloc(8, None)?;
        let offsets = e.alloc(64, Some(&le_u32s(&[0, inner, 0, 0])))?;
        let aim = self.mindmin()?.map(|v| v as i16);
        let e = &mut self.e;
        let aim = e.alloc(8, Some(&le_i16s(&aim)))?;
        let scratch = e.alloc(64, None)?;
        check(
            e.call(F_CREATE_ALG, &[image, imp + 0x10, 0, 0, imp + DMIN_STATS, geometry, offsets, scratch, imp + 0x14, 0, 0, aim])?,
            "createAlgData",
        )?;
        let st = self.e.s16(imp + DMIN_STATS, 3);
        let fd = self.frame_dmin([st[0], st[1], st[2]], dmin)?;
        let e = &mut self.e;
        e.write(imp + FRAME_DMIN, &le_i16s(&fd))?;
        let scratch = e.alloc(64, None)?;
        let args = [
            imp + ALG,
            imp + FRAME_DMIN,
            imp + INFO,
            fos_on as u32,
            0,
            self.block,
            self.pcode,
            self.sfs + 0x2C,
            0,
            0,
            scratch,
            imp + WORK,
            imp + RESULT,
        ];
        check(e.call(F_SBA, &args)?, "Sba() pass 1")?;
        Ok(imp)
    }

    /// SbaCalcFosResults over the frames. Returns the result address.
    pub fn fos(&mut self, impls: &[u32]) -> Result<u32, String> {
        let n = impls.len() as u32;
        let fpo = self.dpi_bytes[0x1E..0x24].to_vec();
        let e = &mut self.e;
        let reference = e.alloc(8, Some(&fpo))?;
        let out = e.alloc(0x40, None)?;
        let mut arr = |off: u32| -> Result<u32, String> {
            let v: Vec<u32> = impls.iter().map(|p| p + off).collect();
            e.alloc(4 * n, Some(&le_u32s(&v)))
        };
        let (a, i, w) = (arr(ALG)?, arr(INFO)?, arr(WORK)?);
        check(self.e.call(F_FOS, &[n, 0, reference, self.pcode, 0, 0, a, i, w, out])?, "SbaCalcFosResults")?;
        Ok(out)
    }

    /// Sba() mode 2 with the FOS results, then Preference(). Returns the
    /// frame's RPD shift (R, G, B).
    pub fn pass2(&mut self, imp: u32, fos_out: u32) -> Result<[i16; 3], String> {
        let (a, b) = (self.e.s16(imp + FRAME_DMIN, 3), self.e.s16(fos_out + 0xC, 3));
        let fd: Vec<i16> = a.iter().zip(&b).map(|(x, y)| *x.min(y)).collect();
        self.e.write(imp + FRAME_DMIN, &le_i16s(&fd))?;
        let args =
            [imp + ALG, imp + FRAME_DMIN, imp + INFO, 2, 0, self.block, self.pcode, self.sfs + 0x2C, 0, 0, fos_out, imp + WORK, imp + RESULT];
        check(self.e.call(F_SBA, &args)?, "Sba() pass 2")?;
        self.preference(imp, fos_out)
    }

    pub fn preference(&mut self, imp: u32, fos_out: u32) -> Result<[i16; 3], String> {
        let pb = self.pref_block()?;
        check(self.e.call(F_PREFERENCE, &[imp + RESULT, fos_out, imp + PREF_OUT, pb, 0])?, "Preference()")?;
        let s = self.e.s16(imp + SHIFTS, 3);
        Ok([s[0], s[1], s[2]])
    }
}

/// One roll's OEM scene balance, frame by frame: the roll stage (FOS over
/// the first `maxFramesToFos` roll frames) runs once, then each frame's
/// shift costs one pass 1 + pass 2. With fewer than `mff` roll frames each
/// frame is balanced on its own (FOS off), as the OEM does.
pub struct RollBalance {
    s: Sba,
    dmin: Option<[i64; 3]>,
    fos_out: Option<u32>,
}

impl RollBalance {
    pub fn new(sba: Sba, roll: &[Array3<i16>], dmin: Option<[i64; 3]>) -> Result<RollBalance, String> {
        let mut s = sba;
        let (mff, max_fos) = (s.dpi_short(0x3C), s.dpi_short(0x3E));
        let fos_out = if roll.len() as i64 >= (mff as i64).max(1) {
            let mut impls = Vec::new();
            for f in roll.iter().take(max_fos.max(0) as usize) {
                impls.push(s.pass1(f.view(), true, dmin)?);
            }
            Some(s.fos(&impls)?)
        } else {
            None
        };
        Ok(RollBalance { s, dmin, fos_out })
    }

    /// The frame's RPD shift (R, G, B) for its analysis image.
    pub fn shift(&mut self, frame: ArrayView3<i16>) -> Result<[i16; 3], String> {
        let s = &mut self.s;
        match self.fos_out {
            None => {
                let imp = s.pass1(frame, false, self.dmin)?;
                s.preference(imp, 0)
            }
            Some(fo) => {
                let imp = s.pass1(frame, true, self.dmin)?;
                s.pass2(imp, fo)
            }
        }
    }
}

/// OEM per-frame RPD shifts for analysis images (12-bit RPD, landscape),
/// balanced against `roll` (the whole roll's analysis images; default the
/// frames). The OEM balances against the roll, not the frames picked.
pub fn roll_shifts(
    dll: &Path,
    dir: &Path,
    frames: &[Array3<i16>],
    dpi: &str,
    roll: Option<&[Array3<i16>]>,
    dmin: Option<[i64; 3]>,
) -> Result<Vec<[f32; 3]>, String> {
    let mut rb = RollBalance::new(Sba::new(dll, dir, dpi)?, roll.unwrap_or(frames), dmin)?;
    frames.iter().map(|f| rb.shift(f.view()).map(|s| s.map(|v| v as f32))).collect()
}

/// The strip's film base like TLB.dll's FN_bFindTmax: per channel the level
/// the brightest 0.1% of samples exceed, over the central 3/4 of the width,
/// on the clear film between the film's arrival (the first row below 90% of
/// the open gate) and the first frame. Raw 16-bit (R, G, B), or None when
/// that stretch is shorter than `min_mm`.
pub fn film_base(ribbon: ArrayView3<u16>, first_frame: usize, rows_per_mm: f64, margin_mm: f64, min_mm: f64) -> Option<[f64; 3]> {
    let w = ribbon.len_of(Axis(1));
    let (c0, c1) = (w / 8, w - w / 8);
    let ff = first_frame.min(ribbon.len_of(Axis(0)));
    if ff == 0 {
        return None;
    }
    let level: Vec<f32> = (0..ff)
        .map(|r| {
            let row: Vec<f32> = (c0..c1).map(|x| ribbon[(r, x, 1)] as f32).collect();
            stats::pairwise_sum(&row) / row.len() as f32
        })
        .collect();
    let k = ((2.0 * rows_per_mm) as usize).max(1);
    let gate = level[..k.min(level.len())].iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let thr = (0.9 * gate) as f32;
    let first = level.iter().position(|&v| v < thr)?;
    let m = (margin_mm * rows_per_mm) as i64;
    let (r0, r1) = (first as i64 + m, first_frame as i64 - m);
    if ((r1 - r0) as f64) < min_mm * rows_per_mm {
        return None;
    }
    let seg = ribbon.slice(s![r0 as usize..r1 as usize, c0..c1, ..]);
    Some(std::array::from_fn(|c| {
        let mut v: Vec<f32> = seg.index_axis(Axis(2), c).iter().map(|&x| x as f32).collect();
        stats::percentile(&mut v, 99.9) as f64
    }))
}

/// numpy's `np.rint` for f32 (round half to even).
fn rint(x: f32) -> f32 {
    x.round_ties_even()
}

/// A raw (R, G, B) film base -> 12-bit RPD, the frames' conversion.
pub fn film_base_rpd(base: [f64; 3], negmat: &Matrix, lut: &[f32]) -> [i64; 3] {
    let px = base.map(|v| rint(v as f32).clamp(0.0, 65535.0) as u16 as f32);
    colour::rpd(px, negmat, lut, false).map(|v| (v as f64).round_ties_even() as i64)
}

/// A frame crop (raw 16-bit RGB) -> the SBA analysis image: landscape,
/// area-averaged to ANALYSIS_H x ANALYSIS_W in raw space, then 12-bit RPD.
pub fn analysis_image(raw: ArrayView3<u16>, negmat: &Matrix, lut: &[f32]) -> Array3<i16> {
    // np.rot90: out[i, j] = a[j, W - 1 - i]
    let a = if raw.len_of(Axis(0)) > raw.len_of(Axis(1)) {
        let mut v = raw;
        v.swap_axes(0, 1);
        v.invert_axis(Axis(0));
        v
    } else {
        raw
    };
    let (h, w) = (ANALYSIS_H, ANALYSIS_W);
    let (sy, sx) = (a.len_of(Axis(0)) / h, a.len_of(Axis(1)) / w);
    let (y0, x0) = ((a.len_of(Axis(0)) - sy * h) / 2, (a.len_of(Axis(1)) - sx * w) / 2);
    let count = (sy * sx) as f32;
    let mut small = Array3::<u16>::zeros((h, w, 3));
    for i in 0..h {
        for j in 0..w {
            for c in 0..3 {
                // reshape(h, sy, w, sx, 3).mean(axis=(1, 3)): f32, y then x
                let mut s = 0.0f32;
                for y in 0..sy {
                    for x in 0..sx {
                        s += a[(y0 + i * sy + y, x0 + j * sx + x, c)] as f32;
                    }
                }
                small[(i, j, c)] = rint(s / count).clamp(0.0, 65535.0) as u16;
            }
        }
    }
    colour::rpd_image(small.view(), negmat, lut).mapv(|v| rint(v) as i16)
}
