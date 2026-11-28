//! OEM scene balance, roll chaining, and SCP LUT generation (Kodak KCDFS)
//! from PSI 2.4's `oem/psi24/PakonIMAu.dll` under Unicorn.
//!
//! Mirrors `sba.rs` by running the pure-C KCDFS core directly in a 32-bit x86
//! Unicorn machine with stubbed MSVCRT alloc/math/stdio imports, redirecting
//! `C:/KCDFS/*` reads to `oem/psi24/KCDFS/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ndarray::{Array3, ArrayView3, Axis};
use unicorn_engine::{Arch, Mode, Prot, RegisterX86, Unicorn};

use crate::emu::{self, arg, set_eax, uc_err, Host};
use crate::pe::Pe;

// Pure-C KCDFS core entry points in PSI 2.4 PakonIMAu.dll (ImageBase 0x10000000).
const F_CALC_DENS: u32 = 0x1000_1000;
const F_READ_SETUP: u32 = 0x1022_6710;
const F_DECODE_PCODE: u32 = 0x1022_7F30;
const F_SBA: u32 = 0x1022_86C0;
const F_CREATE_ALG: u32 = 0x1022_A040;
const F_PREFERENCE: u32 = 0x1022_AD90;
const F_BUILD_SCP_LUTS: u32 = 0x1022_B250;
const F_FOS: u32 = 0x1022_BB20;

// Static data addresses inside PakonIMAu.dll (.data section).
const G_CALC_DENS_A: u32 = 0x102D_2728;
const G_CALC_DENS_B: u32 = 0x102D_273A;
const G_SCP_OUT_INFO: u32 = 0x1030_BE90;
const G_SCP_PARAMS: u32 = 0x1030_BED8;
const G_MINDMIN: u32 = 0x1030_BEF0;
const G_FOS_OUT: u32 = 0x1030_BEF8;
const G_FOS_WORK_A: u32 = 0x1030_BF1C;
const G_FOS_WORK_B: u32 = 0x1030_BF34;
const G_GEOMETRY: u32 = 0x1030_BF48;
const G_SFS_PTR: u32 = 0x1030_BF74;
const G_DMIN_BUF: u32 = 0x1030_BF78;
const G_SETUP_REC: u32 = 0x1030_BF80;
const G_PREF_BLOCK: u32 = 0x1030_C2C8;
const G_SBA_BLOCK: u32 = 0x1030_C30C;
const G_PCODE_PTR: u32 = 0x1030_C334;

/// Default 20-byte camera/command-switch block from `CiListKcdfs::CiListKcdfs`
/// (`0x1002b330`): same as Ansel's `CMD_DEFAULTS` except byte 1 = 4 (35mm neg).
const CMD_DEFAULTS: [u8; 20] =
    [0xFE, 0x04, 0xFE, 0xFF, 0xFF, 0xFE, 0x00, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0x5F, 0x08, 0x1D, 0x0C, 0x1E];

// -- x87 80-bit helpers ---------------------------------------------------------

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
    if neg { -x } else { x }
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
        let bits = x.abs().to_bits();
        let (mut e, mut frac) = (((bits >> 52) & 0x7FF) as i32, bits & ((1 << 52) - 1));
        if e == 0 {
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

// -- emulator & CRT/stdio stubs -------------------------------------------------

const STUB_BASE: u32 = 0x7F00_0000;
const STUB_SPAN: u32 = 0x0100_0000;
const STACK_TOP: u32 = 0x7E00_0000;
const STACK_SIZE: u32 = 0x0080_0000;
const HEAP_BASE: u32 = 0x6000_0000;
const HEAP_SIZE: u32 = 0x1000_0000;
const RET_MAGIC: u32 = 0x7F7F_FFF0;
const SCRATCH: u32 = STUB_BASE + 0x00F0_1000;

struct VirtualFile {
    data: Vec<u8>,
    pos: usize,
}

#[derive(Default)]
struct State {
    stubs: HashMap<u32, String>,
    heap: u32,
    fault: Option<String>,
    kcdfs_dir: PathBuf,
    files: HashMap<u32, VirtualFile>,
}

impl Host for State {
    fn fault(&mut self) -> &mut Option<String> {
        &mut self.fault
    }
}

fn stub_code(name: &str) -> Vec<u8> {
    match name {
        "_CIpow" | "_ftol" => vec![0xDD, 0xD8, 0xC3], // fstp st(0); ret
        "floor" | "ceil" | "atof" => {
            let mut v = vec![0xDD, 0x05]; // fld qword [SCRATCH]; ret
            v.extend_from_slice(&SCRATCH.to_le_bytes());
            v.push(0xC3);
            v
        }
        _ => vec![0xC3], // cdecl ret
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

fn read_cstr(uc: &Unicorn<State>, addr: u32) -> String {
    if addr == 0 {
        return String::new();
    }
    let mut out = Vec::new();
    let mut cur = addr;
    for _ in 0..4096 {
        let mut b = [0u8; 1];
        if uc.mem_read(cur as u64, &mut b).is_err() || b[0] == 0 {
            break;
        }
        out.push(b[0]);
        cur += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn set_eof_flag(uc: &mut Unicorn<State>, fp: u32, eof: bool) {
    let mut flag = [0u8; 4];
    if uc.mem_read((fp + 0x0C) as u64, &mut flag).is_ok() {
        let mut v = u32::from_le_bytes(flag);
        if eof {
            v |= 0x10; // MSVCRT _IOEOF
        } else {
            v &= !0x10;
        }
        let _ = uc.mem_write((fp + 0x0C) as u64, &v.to_le_bytes());
    }
}

/// Minimal `sscanf` / `fscanf` engine supporting the format specifiers used by
/// `SbuReadSetupFile` (`%s`, `%d`, `%hd`, `%ld`, `%u`, `%f`, `%lf`, `%c`, literal matches).
fn scan_from_slice(uc: &mut Unicorn<State>, input: &[u8], fmt: &str, arg_base_idx: u32) -> (u32, usize) {
    let mut pos = 0usize;
    let mut assigned = 0u32;
    let mut arg_i = arg_base_idx;
    let fbytes = fmt.as_bytes();
    let mut fi = 0usize;

    let skip_ws = |p: &mut usize| {
        while *p < input.len() && input[*p].is_ascii_whitespace() {
            *p += 1;
        }
    };

    while fi < fbytes.len() {
        if fbytes[fi].is_ascii_whitespace() {
            while fi < fbytes.len() && fbytes[fi].is_ascii_whitespace() {
                fi += 1;
            }
            skip_ws(&mut pos);
            continue;
        }
        if fbytes[fi] != b'%' {
            if pos < input.len() && input[pos] == fbytes[fi] {
                pos += 1;
                fi += 1;
                continue;
            } else {
                break;
            }
        }
        fi += 1; // skip '%'
        if fi < fbytes.len() && fbytes[fi] == b'%' {
            if pos < input.len() && input[pos] == b'%' {
                pos += 1;
                fi += 1;
                continue;
            } else {
                break;
            }
        }
        let mut suppress = false;
        if fi < fbytes.len() && fbytes[fi] == b'*' {
            suppress = true;
            fi += 1;
        }
        let mut width: Option<usize> = None;
        while fi < fbytes.len() && fbytes[fi].is_ascii_digit() {
            width = Some(width.unwrap_or(0) * 10 + (fbytes[fi] - b'0') as usize);
            fi += 1;
        }
        let mut len_mod = 0u8; // b'h' or b'l'
        if fi < fbytes.len() && (fbytes[fi] == b'h' || fbytes[fi] == b'l') {
            len_mod = fbytes[fi];
            fi += 1;
        }
        if fi >= fbytes.len() {
            break;
        }
        let spec = fbytes[fi];
        fi += 1;

        match spec {
            b's' => {
                skip_ws(&mut pos);
                let start = pos;
                let max_end = width.map(|w| (start + w).min(input.len())).unwrap_or(input.len());
                while pos < max_end && !input[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                if pos == start {
                    break;
                }
                if !suppress {
                    let dst = arg(uc, arg_i);
                    arg_i += 1;
                    let mut out = input[start..pos].to_vec();
                    out.push(0);
                    let _ = uc.mem_write(dst as u64, &out);
                    assigned += 1;
                }
            }
            b'c' => {
                let n = width.unwrap_or(1);
                if pos + n > input.len() {
                    break;
                }
                if !suppress {
                    let dst = arg(uc, arg_i);
                    arg_i += 1;
                    let _ = uc.mem_write(dst as u64, &input[pos..pos + n]);
                    assigned += 1;
                }
                pos += n;
            }
            b'd' | b'i' | b'u' => {
                skip_ws(&mut pos);
                let start = pos;
                let max_end = width.map(|w| (start + w).min(input.len())).unwrap_or(input.len());
                if pos < max_end && (input[pos] == b'+' || input[pos] == b'-') {
                    pos += 1;
                }
                let digits_start = pos;
                while pos < max_end && input[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos == digits_start {
                    break;
                }
                let s = std::str::from_utf8(&input[start..pos]).unwrap_or("0");
                let Ok(val) = s.parse::<i64>() else { break };
                if !suppress {
                    let dst = arg(uc, arg_i);
                    arg_i += 1;
                    if len_mod == b'h' {
                        let _ = uc.mem_write(dst as u64, &(val as i16).to_le_bytes());
                    } else {
                        let _ = uc.mem_write(dst as u64, &(val as i32).to_le_bytes());
                    }
                    assigned += 1;
                }
            }
            b'f' | b'g' | b'e' | b'G' | b'E' => {
                skip_ws(&mut pos);
                let start = pos;
                let max_end = width.map(|w| (start + w).min(input.len())).unwrap_or(input.len());
                if pos < max_end && (input[pos] == b'+' || input[pos] == b'-') {
                    pos += 1;
                }
                let mut any_digit = false;
                while pos < max_end && input[pos].is_ascii_digit() {
                    pos += 1;
                    any_digit = true;
                }
                if pos < max_end && input[pos] == b'.' {
                    pos += 1;
                    while pos < max_end && input[pos].is_ascii_digit() {
                        pos += 1;
                        any_digit = true;
                    }
                }
                if any_digit && pos < max_end && (input[pos] == b'e' || input[pos] == b'E') {
                    let save = pos;
                    pos += 1;
                    if pos < max_end && (input[pos] == b'+' || input[pos] == b'-') {
                        pos += 1;
                    }
                    let exp_start = pos;
                    while pos < max_end && input[pos].is_ascii_digit() {
                        pos += 1;
                    }
                    if pos == exp_start {
                        pos = save;
                    }
                }
                if !any_digit {
                    break;
                }
                let s = std::str::from_utf8(&input[start..pos]).unwrap_or("0");
                let Ok(val) = s.parse::<f64>() else { break };
                if !suppress {
                    let dst = arg(uc, arg_i);
                    arg_i += 1;
                    if len_mod == b'l' {
                        let _ = uc.mem_write(dst as u64, &val.to_le_bytes());
                    } else {
                        let _ = uc.mem_write(dst as u64, &(val as f32).to_le_bytes());
                    }
                    assigned += 1;
                }
            }
            _ => break,
        }
    }
    (assigned, pos)
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
        "_setjmp3" => 0,
        "longjmp" => return fail(uc, "OEM KCDFS code took its longjmp error path".into()),
        "_CIacos" | "_CIasin" | "_CItanh" => {
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
            let (y, x) = (st(uc, 0), st(uc, 1));
            set_st(uc, 1, x.powf(y));
            return;
        }
        "floor" | "ceil" => {
            let b = emu::read(uc, emu::esp(uc) + 4, 8);
            let x = f64::from_le_bytes(b.try_into().unwrap());
            let y = if name == "floor" { x.floor() } else { x.ceil() };
            let _ = uc.mem_write(SCRATCH as u64, &y.to_le_bytes());
            return;
        }
        "atof" => {
            let s = read_cstr(uc, arg(uc, 0));
            let y = s.trim().parse::<f64>().unwrap_or(0.0);
            let _ = uc.mem_write(SCRATCH as u64, &y.to_le_bytes());
            return;
        }
        "atoi" => {
            let s = read_cstr(uc, arg(uc, 0));
            s.trim().parse::<i32>().unwrap_or(0) as u32
        }
        "_ftol" => {
            let v = st(uc, 0) as i64;
            let _ = uc.reg_write(RegisterX86::EDX, ((v >> 32) as u32) as u64);
            v as u32
        }
        "fopen" => {
            let raw_path = read_cstr(uc, arg(uc, 0));
            let file_name = raw_path.rsplit(['/', '\\']).next().unwrap_or(&raw_path);
            let host_path = uc.get_data().kcdfs_dir.join(file_name);
            match std::fs::read(&host_path) {
                Ok(data) => match bump(uc, 32, None) {
                    Ok(fp) => {
                        uc.get_data_mut().files.insert(fp, VirtualFile { data, pos: 0 });
                        fp
                    }
                    Err(e) => return fail(uc, e),
                },
                Err(_) => 0,
            }
        }
        "fclose" => {
            let fp = arg(uc, 0);
            uc.get_data_mut().files.remove(&fp);
            0
        }
        "fgets" => {
            let (buf, max_n, fp) = (arg(uc, 0), arg(uc, 1) as usize, arg(uc, 2));
            let (out, hit_eof) = match uc.get_data_mut().files.get_mut(&fp) {
                Some(f) if f.pos < f.data.len() && max_n > 0 => {
                    let mut line = Vec::new();
                    let limit = max_n - 1;
                    while f.pos < f.data.len() && line.len() < limit {
                        let b = f.data[f.pos];
                        f.pos += 1;
                        line.push(b);
                        if b == b'\n' {
                            break;
                        }
                    }
                    let eof = f.pos >= f.data.len();
                    line.push(0);
                    (Some(line), eof)
                }
                _ => (None, true),
            };
            if hit_eof {
                set_eof_flag(uc, fp, true);
            }
            match out {
                Some(line) => {
                    let _ = uc.mem_write(buf as u64, &line);
                    buf
                }
                None => 0,
            }
        }
        "fread" => {
            let (dst, size, count, fp) = (arg(uc, 0), arg(uc, 1) as usize, arg(uc, 2) as usize, arg(uc, 3));
            let want = size.saturating_mul(count);
            let (chunk, items, hit_eof) = match uc.get_data_mut().files.get_mut(&fp) {
                Some(f) if size > 0 && want > 0 => {
                    let avail = f.data.len().saturating_sub(f.pos);
                    let take = avail.min(want);
                    let bytes = f.data[f.pos..f.pos + take].to_vec();
                    f.pos += take;
                    (bytes, take / size, take < want)
                }
                _ => (Vec::new(), 0, true),
            };
            if !chunk.is_empty() {
                let _ = uc.mem_write(dst as u64, &chunk);
            }
            if hit_eof {
                set_eof_flag(uc, fp, true);
            }
            items as u32
        }
        "fseek" => {
            let (fp, offset, whence) = (arg(uc, 0), arg(uc, 1) as i32 as i64, arg(uc, 2));
            let ok = if let Some(f) = uc.get_data_mut().files.get_mut(&fp) {
                let base = match whence {
                    0 => 0i64,
                    1 => f.pos as i64,
                    2 => f.data.len() as i64,
                    _ => -1,
                };
                if base >= 0 && base + offset >= 0 {
                    f.pos = (base + offset) as usize;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if ok {
                set_eof_flag(uc, fp, false);
                0
            } else {
                u32::MAX
            }
        }
        "ftell" => {
            let fp = arg(uc, 0);
            uc.get_data().files.get(&fp).map(|f| f.pos as u32).unwrap_or(u32::MAX)
        }
        "rewind" => {
            let fp = arg(uc, 0);
            if let Some(f) = uc.get_data_mut().files.get_mut(&fp) {
                f.pos = 0;
            }
            set_eof_flag(uc, fp, false);
            0
        }
        "sscanf" => {
            let s = read_cstr(uc, arg(uc, 0));
            let fmt = read_cstr(uc, arg(uc, 1));
            let (n, _) = scan_from_slice(uc, s.as_bytes(), &fmt, 2);
            n
        }
        "fscanf" => {
            let fp = arg(uc, 0);
            let fmt = read_cstr(uc, arg(uc, 1));
            let remaining = uc
                .get_data()
                .files
                .get(&fp)
                .map(|f| if f.pos < f.data.len() { f.data[f.pos..].to_vec() } else { Vec::new() })
                .unwrap_or_default();
            if remaining.is_empty() {
                set_eof_flag(uc, fp, true);
                u32::MAX // EOF (-1)
            } else {
                let (n, consumed) = scan_from_slice(uc, &remaining, &fmt, 2);
                let hit_eof = if let Some(f) = uc.get_data_mut().files.get_mut(&fp) {
                    f.pos = (f.pos + consumed).min(f.data.len());
                    f.pos >= f.data.len()
                } else {
                    true
                };
                if hit_eof {
                    set_eof_flag(uc, fp, true);
                }
                if n == 0 && hit_eof { u32::MAX } else { n }
            }
        }
        "strncmp" => {
            let (a, b, n) = (read_cstr(uc, arg(uc, 0)), read_cstr(uc, arg(uc, 1)), arg(uc, 2) as usize);
            let ab = &a.as_bytes()[..a.len().min(n)];
            let bb = &b.as_bytes()[..b.len().min(n)];
            match ab.cmp(bb) {
                std::cmp::Ordering::Less => u32::MAX,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        }
        "_stricmp" => {
            let (a, b) = (read_cstr(uc, arg(uc, 0)).to_ascii_lowercase(), read_cstr(uc, arg(uc, 1)).to_ascii_lowercase());
            match a.cmp(&b) {
                std::cmp::Ordering::Less => u32::MAX,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        }
        "strncpy" => {
            let (dst, src, n) = (arg(uc, 0), read_cstr(uc, arg(uc, 1)), arg(uc, 2) as usize);
            let mut out = vec![0u8; n];
            let copy = src.len().min(n);
            out[..copy].copy_from_slice(&src.as_bytes()[..copy]);
            let _ = uc.mem_write(dst as u64, &out);
            dst
        }
        "strchr" => {
            let (base, ch) = (arg(uc, 0), (arg(uc, 1) & 0xFF) as u8);
            let s = read_cstr(uc, base);
            if ch == 0 {
                base + s.len() as u32
            } else {
                s.as_bytes().iter().position(|&b| b == ch).map(|i| base + i as u32).unwrap_or(0)
            }
        }
        "strstr" => {
            let (base, needle) = (arg(uc, 0), read_cstr(uc, arg(uc, 1)));
            let hay = read_cstr(uc, base);
            hay.find(&needle).map(|i| base + i as u32).unwrap_or(0)
        }
        "isspace" => ((arg(uc, 0) as u8).is_ascii_whitespace()) as u32,
        "tolower" => (arg(uc, 0) as u8).to_ascii_lowercase() as u32,
        "isupper" => ((arg(uc, 0) as u8).is_ascii_uppercase()) as u32,
        "isalpha" => ((arg(uc, 0) as u8).is_ascii_alphabetic()) as u32,
        "isalnum" => ((arg(uc, 0) as u8).is_ascii_alphanumeric()) as u32,
        "printf" | "fprintf" | "sprintf" => 0,
        _ => return fail(uc, format!("OEM KCDFS code called an unhandled import: {name}")),
    };
    set_eax(uc, r);
}

struct Emulator {
    uc: Unicorn<'static, State>,
}

impl Emulator {
    fn new(dll: &Path, kcdfs_dir: &Path) -> Result<Emulator, String> {
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
        st.kcdfs_dir = kcdfs_dir.to_path_buf();
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

    fn u16s(&self, addr: u32, n: usize) -> Vec<u16> {
        emu::read(&self.uc, addr, 2 * n).chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()
    }

    fn s16(&self, addr: u32, n: usize) -> Vec<i16> {
        emu::read(&self.uc, addr, 2 * n).chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, String> {
        emu::call(&mut self.uc, func, args, STACK_TOP, RET_MAGIC)
    }
}

fn check(rc: u32, what: &str) -> Result<(), String> {
    if rc != 0 {
        return Err(format!("{what} failed, return code {rc:#x}"));
    }
    Ok(())
}

fn le_i16s(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

// -- public paths & API ---------------------------------------------------------

/// `oem/psi24/PakonIMAu.dll` under the repo.
pub fn default_dll(repo: &Path) -> PathBuf {
    repo.join("oem").join("psi24").join("PakonIMAu.dll")
}

/// `oem/psi24/KCDFS` under the repo.
pub fn kcdfs_dir(repo: &Path) -> PathBuf {
    repo.join("oem").join("psi24").join("KCDFS")
}

/// Result of running KCDFS roll balance: per-frame RPD shifts `(R, G, B)` and
/// the optional roll-adapted 12-bit SCP LUTs (`[R, G, B]`, 4096 entries each).
pub struct KcdfsRollResult {
    pub shifts: Vec<[f32; 3]>,
    pub scp_luts: Option<Box<[[u16; 4096]; 3]>>,
}

struct FrameSlot {
    alg: u32,
    dens: u32,
    info: u32,
    work: u32,
    result: u32,
}

/// One roll's PSI 2.4 KCDFS scene balance instance.
pub struct Kcdfs {
    e: Emulator,
    pcode: u32,
    sfs: u32,
}

impl Kcdfs {
    /// Load PSI 2.4 `PakonIMAu.dll` and initialize KCDFS from `kcdfs_dir`
    /// (`dls_1.7.pcode` + `kcdfs.su` and its referenced LUT/satdef files).
    pub fn new(dll: &Path, dir: &Path, mindmin: [i16; 3]) -> Result<Kcdfs, String> {
        let mut e = Emulator::new(dll, dir)?;

        // Static constructor defaults from `CiColorCorrectionKcdfs::CiColorCorrectionKcdfs` (0x10001410).
        e.write(G_SETUP_REC + 4, &G_DMIN_BUF.to_le_bytes())?;
        e.write(G_SCP_PARAMS, &2i16.to_le_bytes())?;
        e.write(G_SCP_PARAMS + 2, &1i16.to_le_bytes())?;
        e.write(G_SCP_PARAMS + 8, &1.0f64.to_le_bytes())?;
        e.write(G_SCP_PARAMS + 16, &0.199f64.to_le_bytes())?;

        // Decode dls_1.7.pcode.
        let pcode_path = dir.join("dls_1.7.pcode");
        let pcode_bytes = std::fs::read(&pcode_path).map_err(|err| format!("{}: {err}", pcode_path.display()))?;
        let pcode_src = e.alloc(pcode_bytes.len() as u32 + 2, Some(&pcode_bytes))?;
        check(e.call(F_DECODE_PCODE, &[pcode_src, pcode_bytes.len() as u32, G_PCODE_PTR])?, "SbaDecodePcode")?;
        let pcode = e.u32(G_PCODE_PTR);

        // Parse C:/KCDFS/kcdfs.su (remapped by fopen stub to <dir>/kcdfs.su).
        let su_path = e.alloc(32, Some(b"C:/KCDFS/kcdfs.su\0"))?;
        check(
            e.call(F_READ_SETUP, &[su_path, G_SBA_BLOCK, G_PREF_BLOCK, G_SETUP_REC, G_SFS_PTR])?,
            "SbuReadSetupFile",
        )?;
        let sfs = e.u32(G_SFS_PTR);

        // Initialize geometry defaults and mindmin (from bInit 0x100015ad).
        let geom: [u32; 11] = [0, 0, 99999, 99999, 99999, 99999, 24, 36, 99999, 0x360, 0];
        e.write(G_GEOMETRY, &le_u32s(&geom))?;
        e.write(G_MINDMIN, &le_i16s(&mindmin))?;

        Ok(Kcdfs { e, pcode, sfs })
    }

    fn zero_fos_globals(&mut self) -> Result<(), String> {
        self.e.write(G_FOS_OUT, &[0u8; 36])?;
        self.e.write(G_FOS_WORK_A, &[0u8; 24])?;
        self.e.write(G_FOS_WORK_B, &[0u8; 20])?;
        Ok(())
    }

    /// Run `createAlgData` + `_sbaCalcDens` + `Sba(mode=1)` for one frame
    /// (mirrors `CiColorCorrectionKcdfs::bStep1` at `0x10001940`).
    fn step1(&mut self, thumb: ArrayView3<i16>, dmin: [i16; 3]) -> Result<FrameSlot, String> {
        let (rows, cols, _) = thumb.dim();
        // The PSI 2.4 createAlgData wants a PORTRAIT image (row stride = G_GEOMETRY[0] = the short
        // edge, height = G_GEOMETRY[1]) and rotates it itself. Callers hand us an upright landscape
        // image (rows < cols): serialise it rotated 90 deg CCW, port[yp, xp] = land[xp, cols-1-yp].
        // A portrait input (rows >= cols) is serialised as is.
        let landscape = rows < cols;
        let (hp, wp) = if landscape { (cols, rows) } else { (rows, cols) };
        let e = &mut self.e;

        let alg = e.alloc(0x2880, None)?;
        let dens = e.alloc(16, None)?;
        let info = e.alloc(24, Some(&CMD_DEFAULTS))?;
        let work = e.alloc(0xF80, None)?;
        let result = e.alloc(0x190, None)?;

        let mut planes = Vec::with_capacity(rows * cols * 6);
        for c in 0..3 {
            for yp in 0..hp {
                for xp in 0..wp {
                    let v = if landscape { thumb[(xp, cols - 1 - yp, c)] } else { thumb[(yp, xp, c)] };
                    planes.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let data = e.alloc(planes.len() as u32, Some(&planes))?;
        let plane_bytes = (rows * cols * 2) as u32;
        let image = e.alloc(16, Some(&le_u32s(&[0, data, data + plane_bytes, data + 2 * plane_bytes])))?;

        let geom: [u32; 11] = [wp as u32, hp as u32, 99999, 99999, 99999, 99999, 24, 36, 99999, 0x360, 0];
        e.write(G_GEOMETRY, &le_u32s(&geom))?;
        e.write(G_SETUP_REC + 0x0C, &0u32.to_le_bytes())?;
        // bStep1 does `*DAT_1030bf84 = dmin`: the pointer at setup_rec+4 is the buffer
        // SbuReadSetupFile allocated (it overwrites the constructor's &G_DMIN_BUF), so the
        // frame's dmin must go through it. Writing only G_DMIN_BUF left the file's 0,0,0 in
        // place and lifted every shift by ~+35..+60 (equal on R,G,B).
        let dmin_ptr = e.u32(G_SETUP_REC + 4);
        e.write(dmin_ptr, &le_i16s(&dmin))?;
        e.write(G_DMIN_BUF, &le_i16s(&dmin))?;

        let dmin_a = e.alloc(8, Some(&le_i16s(&dmin)))?;
        let dmin_b = e.alloc(8, Some(&le_i16s(&dmin)))?;
        let zero_dmin = e.alloc(8, Some(&le_i16s(&[0, 0, 0])))?;
        // 3x3 identity scaled by 1000 (0x3E8), matching stack layout at 10001951..10001a95.
        let id_mat: [i16; 9] = [1000, 0, 0, 0, 1000, 0, 0, 0, 1000];
        let mat_buf = e.alloc(20, Some(&le_i16s(&id_mat)))?;
        let alg_ptr = e.alloc(4, Some(&alg.to_le_bytes()))?;

        // createAlgData(image, &alg_ptr, 0, 0, out_stats, G_GEOMETRY, G_SETUP_REC, dmin_b, zero_dmin, 0, info[1], G_MINDMIN)
        // Matching exact push order at 10001a1c..10001a9b (dmin_a is NOT passed here; it is read untouched by _sbaCalcDens).
        let out_stats = e.alloc(16, None)?;
        check(
            e.call(
                F_CREATE_ALG,
                &[image, alg_ptr, 0, 0, out_stats, G_GEOMETRY, G_SETUP_REC, dmin_b, zero_dmin, 0, CMD_DEFAULTS[1] as u32, G_MINDMIN],
            )?,
            "createAlgData",
        )?;

        // _sbaCalcDens (16 args, matching 10001b05..10001b57).
        check(
            e.call(
                F_CALC_DENS,
                &[
                    dmin_a,
                    dmin_a + 2,
                    dmin_a + 4,
                    1,
                    G_CALC_DENS_A,
                    G_CALC_DENS_B,
                    1,
                    mat_buf,
                    mat_buf,
                    0,
                    dens,
                    dens + 2,
                    dens + 4,
                    dens + 6,
                    dens + 8,
                    dens + 10,
                ],
            )?,
            "_sbaCalcDens",
        )?;

        // Sba(alg, dens, info, mode=1, 0, G_SBA_BLOCK, pcode, sfs, G_FOS_WORK_B, G_FOS_WORK_A, G_FOS_OUT, work, result)
        check(
            e.call(
                F_SBA,
                &[alg, dens, info, 1, 0, G_SBA_BLOCK, self.pcode, self.sfs, G_FOS_WORK_B, G_FOS_WORK_A, G_FOS_OUT, work, result],
            )?,
            "Sba() pass 1",
        )?;

        Ok(FrameSlot { alg, dens, info, work, result })
    }

    /// Run `SbaCalcFosResults` + `Sba(mode=2)` + either `SbaBuildSCPLuts` or `Preference`
    /// (mirrors `CiColorCorrectionKcdfs::bStep2` at `0x10001c20`).
    fn step2(&mut self, slots: &[FrameSlot], build_scp: bool, pref_flag: bool) -> Result<(Vec<[i16; 3]>, Option<Box<[[u16; 4096]; 3]>>), String> {
        let n = slots.len() as u32;
        let algs: Vec<u32> = slots.iter().map(|s| s.alg).collect();
        let infos: Vec<u32> = slots.iter().map(|s| s.info).collect();
        let works: Vec<u32> = slots.iter().map(|s| s.work).collect();

        let p_algs = self.e.alloc(4 * n, Some(&le_u32s(&algs)))?;
        let p_infos = self.e.alloc(4 * n, Some(&le_u32s(&infos)))?;
        let p_works = self.e.alloc(4 * n, Some(&le_u32s(&works)))?;

        check(
            self.e.call(
                F_FOS,
                &[n, 0, G_SBA_BLOCK, self.pcode, G_FOS_WORK_B, G_FOS_WORK_A, p_algs, p_infos, p_works, G_FOS_OUT],
            )?,
            "SbaCalcFosResults",
        )?;

        let pref_mode: u32 = if pref_flag { 5 } else { 0 };
        let pref_out = self.e.alloc(0x1A0, None)?;
        let mut shifts = Vec::with_capacity(slots.len());

        for slot in slots {
            check(
                self.e.call(
                    F_SBA,
                    &[
                        slot.alg,
                        slot.dens,
                        slot.info,
                        2,
                        0,
                        G_SBA_BLOCK,
                        self.pcode,
                        self.sfs,
                        G_FOS_WORK_B,
                        G_FOS_WORK_A,
                        G_FOS_OUT,
                        slot.work,
                        slot.result,
                    ],
                )?,
                "Sba() pass 2",
            )?;
            if !build_scp {
                self.e.write(pref_out, &[0u8; 0x198])?;
                check(
                    self.e.call(F_PREFERENCE, &[slot.result, G_FOS_OUT, pref_out, G_PREF_BLOCK, pref_mode])?,
                    "Preference()",
                )?;
                let s = self.e.s16(pref_out + 8, 3);
                shifts.push([s[0], s[1], s[2]]);
            }
        }

        let scp_luts = if build_scp {
            let fos = self.e.s16(G_FOS_OUT, 14);
            // Exact args from 10001db5..10001dfd:
            // SbaBuildSCPLuts(4096, fos[9], fos[10], fos[11], fos[12], fos[6], fos[7], fos[8], G_SCP_PARAMS, G_SCP_OUT_INFO, lut_r, lut_g, lut_b)
            let lut_r = self.e.alloc(4096 * 2, None)?;
            let lut_g = self.e.alloc(4096 * 2, None)?;
            let lut_b = self.e.alloc(4096 * 2, None)?;
            check(
                self.e.call(
                    F_BUILD_SCP_LUTS,
                    &[
                        4096,
                        fos[9] as u16 as u32,
                        fos[10] as u16 as u32,
                        fos[11] as u16 as u32,
                        fos[12] as u16 as u32,
                        fos[6] as u16 as u32,
                        fos[7] as u16 as u32,
                        fos[8] as u16 as u32,
                        G_SCP_PARAMS,
                        G_SCP_OUT_INFO,
                        lut_r,
                        lut_g,
                        lut_b,
                    ],
                )?,
                "SbaBuildSCPLuts",
            )?;
            let mut luts = Box::new([[0u16; 4096]; 3]);
            luts[0].copy_from_slice(&self.e.u16s(lut_r, 4096));
            luts[1].copy_from_slice(&self.e.u16s(lut_g, 4096));
            luts[2].copy_from_slice(&self.e.u16s(lut_b, 4096));
            Some(luts)
        } else {
            None
        };

        Ok((shifts, scp_luts))
    }

    /// Balance a roll of landscape 12-bit RPD analysis images (`128 x 192 x 3`),
    /// optionally running the KCDFS Stage-1 SCP LUT pass (`do_scp_pass = true`)
    /// before the Stage-2 shift pass, matching `PIKcdfsChainLoop`.
    pub fn balance_roll(
        &mut self,
        frames: &[Array3<i16>],
        dmin: [i16; 3],
        do_scp_pass: bool,
        pref_flag: bool,
    ) -> Result<KcdfsRollResult, String> {
        if frames.is_empty() {
            return Ok(KcdfsRollResult { shifts: Vec::new(), scp_luts: None });
        }

        let mut working: Vec<Array3<i16>> = frames.to_vec();
        let mut cur_dmin = dmin;
        let mut scp_luts = None;

        if do_scp_pass {
            self.zero_fos_globals()?;
            let mut slots = Vec::with_capacity(working.len().min(40));
            for f in working.iter().take(40) {
                slots.push(self.step1(f.view(), cur_dmin)?);
            }
            let (_, luts) = self.step2(&slots, true, pref_flag)?;
            if let Some(luts) = luts {
                // Apply the roll SCP LUT to every thumbnail and to dmin (mirrors 10001877..100018d2).
                for f in &mut working {
                    for v in f.index_axis_mut(Axis(2), 0).iter_mut() {
                        *v = luts[0][(*v).clamp(0, 4095) as usize] as i16;
                    }
                    for v in f.index_axis_mut(Axis(2), 1).iter_mut() {
                        *v = luts[1][(*v).clamp(0, 4095) as usize] as i16;
                    }
                    for v in f.index_axis_mut(Axis(2), 2).iter_mut() {
                        *v = luts[2][(*v).clamp(0, 4095) as usize] as i16;
                    }
                }
                for c in 0..3 {
                    cur_dmin[c] = luts[c][cur_dmin[c].clamp(0, 4095) as usize] as i16;
                }
                scp_luts = Some(luts);
            }
        }

        // Stage 2: final per-frame shifts in batches of up to 40 (matching TLB.dll 10034870).
        let mut all_shifts = Vec::with_capacity(working.len());
        for chunk in working.chunks(40) {
            self.zero_fos_globals()?;
            let mut slots = Vec::with_capacity(chunk.len());
            for f in chunk {
                slots.push(self.step1(f.view(), cur_dmin)?);
            }
            let (shifts, _) = self.step2(&slots, false, pref_flag)?;
            all_shifts.extend(shifts.into_iter().map(|s| s.map(|v| v as f32)));
        }

        Ok(KcdfsRollResult { shifts: all_shifts, scp_luts })
    }
    /// Balance `frames` against `roll` (defaults to `frames`), matching
    /// `sba::roll_shifts` so single-frame Adjust previews get the exact same
    /// roll SCP LUT and FOS chaining as full-roll export.
    pub fn balance_frames(
        &mut self,
        frames: &[Array3<i16>],
        roll: Option<&[Array3<i16>]>,
        dmin: [i16; 3],
        do_scp_pass: bool,
        pref_flag: bool,
    ) -> Result<KcdfsRollResult, String> {
        if frames.is_empty() {
            return Ok(KcdfsRollResult { shifts: Vec::new(), scp_luts: None });
        }
        let Some(roll_frames) = roll else {
            return self.balance_roll(frames, dmin, do_scp_pass, pref_flag);
        };
        if roll_frames.is_empty() {
            return self.balance_roll(frames, dmin, do_scp_pass, pref_flag);
        }

        let mut roll_work: Vec<Array3<i16>> = roll_frames.iter().take(40).cloned().collect();
        let mut frame_work: Vec<Array3<i16>> = frames.to_vec();
        let mut cur_dmin = dmin;
        let mut scp_luts = None;

        let apply_luts = |imgs: &mut [Array3<i16>], luts: &[[u16; 4096]; 3]| {
            for f in imgs {
                for c in 0..3 {
                    for v in f.index_axis_mut(Axis(2), c).iter_mut() {
                        *v = luts[c][(*v).clamp(0, 4095) as usize] as i16;
                    }
                }
            }
        };

        if do_scp_pass {
            self.zero_fos_globals()?;
            let mut slots = Vec::with_capacity(roll_work.len());
            for f in &roll_work {
                slots.push(self.step1(f.view(), cur_dmin)?);
            }
            let (_, luts) = self.step2(&slots, true, pref_flag)?;
            if let Some(luts) = luts {
                apply_luts(&mut roll_work, &luts);
                apply_luts(&mut frame_work, &luts);
                for c in 0..3 {
                    cur_dmin[c] = luts[c][cur_dmin[c].clamp(0, 4095) as usize] as i16;
                }
                scp_luts = Some(luts);
            }
        }

        // Compute FOS over the roll (populates G_FOS_OUT), then balance each requested frame.
        self.zero_fos_globals()?;
        let mut roll_slots = Vec::with_capacity(roll_work.len());
        for f in &roll_work {
            roll_slots.push(self.step1(f.view(), cur_dmin)?);
        }
        let n = roll_slots.len() as u32;
        let algs: Vec<u32> = roll_slots.iter().map(|s| s.alg).collect();
        let infos: Vec<u32> = roll_slots.iter().map(|s| s.info).collect();
        let works: Vec<u32> = roll_slots.iter().map(|s| s.work).collect();
        let p_algs = self.e.alloc(4 * n, Some(&le_u32s(&algs)))?;
        let p_infos = self.e.alloc(4 * n, Some(&le_u32s(&infos)))?;
        let p_works = self.e.alloc(4 * n, Some(&le_u32s(&works)))?;
        check(
            self.e.call(
                F_FOS,
                &[n, 0, G_SBA_BLOCK, self.pcode, G_FOS_WORK_B, G_FOS_WORK_A, p_algs, p_infos, p_works, G_FOS_OUT],
            )?,
            "SbaCalcFosResults",
        )?;

        let pref_mode: u32 = if pref_flag { 5 } else { 0 };
        let pref_out = self.e.alloc(0x1A0, None)?;
        let mut shifts = Vec::with_capacity(frame_work.len());
        for f in &frame_work {
            let slot = self.step1(f.view(), cur_dmin)?;
            check(
                self.e.call(
                    F_SBA,
                    &[
                        slot.alg,
                        slot.dens,
                        slot.info,
                        2,
                        0,
                        G_SBA_BLOCK,
                        self.pcode,
                        self.sfs,
                        G_FOS_WORK_B,
                        G_FOS_WORK_A,
                        G_FOS_OUT,
                        slot.work,
                        slot.result,
                    ],
                )?,
                "Sba() pass 2",
            )?;
            self.e.write(pref_out, &[0u8; 0x198])?;
            check(
                self.e.call(F_PREFERENCE, &[slot.result, G_FOS_OUT, pref_out, G_PREF_BLOCK, pref_mode])?,
                "Preference()",
            )?;
            let s = self.e.s16(pref_out + 8, 3);
            shifts.push([s[0] as f32, s[1] as f32, s[2] as f32]);
        }

        Ok(KcdfsRollResult { shifts, scp_luts })
    }
}

/// Convenience entry point matching `sba::roll_shifts` for PSI 2.4 KCDFS.
pub fn roll_balance(
    dll: &Path,
    dir: &Path,
    frames: &[Array3<i16>],
    dmin: Option<[i64; 3]>,
    do_scp_pass: bool,
) -> Result<KcdfsRollResult, String> {
    roll_balance_against(dll, dir, frames, None, dmin, do_scp_pass)
}

/// Estimate roll film-base Dmin (12-bit RPD) from the roll's analysis
/// thumbnails when no leader Dmin was measured (e.g. pre-cropped TLX `.raw`
/// folders). Per channel, takes the 0.5th percentile across all roll frames,
/// which tracks the unexposed film base without being pulled down by isolated
/// dead pixels.
fn estimate_roll_dmin(frames: &[Array3<i16>]) -> [i16; 3] {
    if frames.is_empty() {
        return [0, 0, 0];
    }
    let mut out = [0i16; 3];
    for (c, slot) in out.iter_mut().enumerate() {
        let mut vals: Vec<i16> = Vec::with_capacity(frames.len() * frames[0].dim().0 * frames[0].dim().1);
        for im in frames {
            vals.extend(im.index_axis(Axis(2), c).iter().copied());
        }
        if vals.is_empty() {
            continue;
        }
        let idx = vals.len() / 200; // 0.5th percentile
        vals.select_nth_unstable(idx);
        *slot = vals[idx].clamp(0, 4095);
    }
    out
}

/// Like [`roll_balance`], balancing `frames` against a separate `roll` slice
/// when rendering a single-frame preview.
pub fn roll_balance_against(
    dll: &Path,
    dir: &Path,
    frames: &[Array3<i16>],
    roll: Option<&[Array3<i16>]>,
    dmin: Option<[i64; 3]>,
    do_scp_pass: bool,
) -> Result<KcdfsRollResult, String> {
    let dmin_i16 = match dmin {
        Some(d) => d.map(|v| v.clamp(0, 4095) as i16),
        None => estimate_roll_dmin(roll.filter(|r| !r.is_empty()).unwrap_or(frames)),
    };
    let mut k = Kcdfs::new(dll, dir, [0, 0, 0])?;
    k.balance_frames(frames, roll, dmin_i16, do_scp_pass, false)
}
