//! A scanned roll from raw to exported frames (port of `web/decode.py`):
//! `prescan` builds the ribbon, finds the frames and renders the preview
//! strip; `autoframe` reruns the OEM framing; `export_frames` crops, cleans,
//! balances, renders and writes each frame.
//!
//! The environment switches are the Python app's: PAKON_LINEWIDTH /
//! PAKON_IR_LANE, PAKON_FLAT, PAKON_EEPROM_DIR / PAKON_DATA, PAKON_OEM_SBA,
//! PAKON_SBA_DMIN, PAKON_OEM_DICE / PAKON_DICE_DLL, PAKON_EXPORT_THREADS
//! ([`Env::from_env`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ndarray::{s, Array2, Array3, ArrayView2, ArrayView3, Axis};
use ndarray_npy::{ViewNpyExt, WriteNpyExt};
use pakon_image::colour::{self, Adjust, Balance, Matrix, Table, Tone};
use pakon_image::decode::Layout;
use pakon_image::film::{Film, ProdTable};
use pakon_image::framing::{self, Format};
use pakon_image::output::{self, Picture16};
use pakon_image::{adjust, balance, fallback, film, ice, ribbon, slide};
use pakon_oem::{dice, sba};
use serde::{Deserialize, Serialize};

/// Progress: (step, fraction 0..1).
pub type Progress<'a> = &'a (dyn Fn(&str, f64) + Sync);

fn emit(p: Option<Progress>, step: &str, pct: f64) {
    if let Some(p) = p {
        p(step, (pct * 1000.0).round_ties_even() / 1000.0);
    }
}

/// Film kinds: colour negative, silver B&W, chromogenic B&W, slide (E-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilmKind {
    #[default]
    C41,
    Bw,
    Bwc41,
    Slide,
}

impl FilmKind {
    pub fn grey(self) -> bool {
        matches!(self, FilmKind::Bw | FilmKind::Bwc41)
    }
}

/// Export sharpening: the OEM's unsharp mask at 1, 2 and 3 TLX clicks.
pub const SHARPEN_LEVELS: [(&str, i32); 4] = [("off", 0), ("low", 67), ("medium", 133), ("high", 200)];

/// Target short-axis (frame height) of the preview.
const PREVIEW_H: f64 = 500.0;
/// Memory budget per frame being exported, without / with dust removal.
/// Measured on a 36-frame Base 16 roll with Digital ICE and sharpening, 6
/// frames at once: 0.48 GB a full frame, 0.68 GB a panorama crop (the
/// largest), 0.35 GB without ICE; upper bounds, as they include the cached
/// ribbon being paged in. (The Python app budgets 0.5 / 1.5 GB.)
const FRAME_RAM: [f64; 2] = [0.5 * (1u64 << 30) as f64, 0.75 * (1u64 << 30) as f64];

/// Frames exported at once with `ram` bytes of physical memory and `cpus`
/// cores: as many as there are cores, while each frame's budget fits in
/// half the memory; at least 2. A frame runs on one core (Digital ICE is
/// one emulated thread), so the cores are the ceiling: on a 14-core M4 Pro
/// 36 ICE frames take 16.1 s 6 at once, 10.6 s 10, 8.5 s 12, 8.6 s 14.
fn workers_for(ram: u64, ice: bool, cpus: usize) -> usize {
    let fit = (ram as f64 / 2.0 / FRAME_RAM[ice as usize]).floor() as usize;
    fit.min(cpus).max(2)
}

/// Where things are and which optional stages run.
#[derive(Debug, Clone)]
pub struct Env {
    /// The repo (or app bundle) root: `oem/`, `profiles/`.
    pub repo: PathBuf,
    /// The work folder: ribbon cache, preview, exported frames.
    pub work_dir: PathBuf,
    /// The scanner's EEPROM archive.
    pub eeprom_dir: Option<PathBuf>,
    pub layout: Option<Layout>,
    pub no_flat: bool,
    pub sba: bool,
    pub sba_dmin: bool,
    pub dice: bool,
    pub dice_dll: PathBuf,
    pub export_threads: Option<usize>,
}

/// `/tmp/pakon_web` (the user's temp folder on Windows).
pub fn default_work_dir() -> PathBuf {
    if cfg!(windows) { std::env::temp_dir() } else { PathBuf::from("/tmp") }.join("pakon_web")
}

/// The EEPROM archive root: `$PAKON_DATA/eeprom`, else `<repo>/backups/eeprom`.
pub fn eeprom_root(repo: &Path) -> PathBuf {
    match std::env::var_os("PAKON_DATA").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("eeprom"),
        None => repo.join("backups").join("eeprom"),
    }
}

fn report(d: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(d.join("report.json")).ok()?).ok()
}

/// Decodable archives under `root`, newest first (`eeprom.archives`).
pub fn archives(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|d| {
            d.is_dir()
                && !d.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
                && d.join("eeprom_0x52_sectionA_primary.bin").exists()
                && report(d).and_then(|r| r.get("decodable").and_then(|v| v.as_bool())).unwrap_or(true)
        })
        .map(|d| (d.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), d))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, d)| d).collect()
}

/// The serial an archive's report.json records.
pub fn archive_serial(d: &Path) -> Option<u64> {
    report(d)?.get("decoded")?.get("serial")?.as_u64()
}

/// The scanner's EEPROM archive (`find_eeprom_dir`): PAKON_EEPROM_DIR; else
/// the newest archive for the connected scanner's serial; else the newest.
pub fn find_eeprom_dir(repo: &Path, connected_serial: Option<u64>) -> Option<PathBuf> {
    if let Some(o) = std::env::var_os("PAKON_EEPROM_DIR").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(o);
        return p.is_dir().then_some(p);
    }
    let found = archives(&eeprom_root(repo));
    if let Some(s) = connected_serial {
        if let Some(d) = found.iter().find(|d| archive_serial(d) == Some(s)) {
            return Some(d.clone());
        }
    }
    found.into_iter().next()
}

impl Env {
    /// The Python app's configuration from the environment.
    pub fn from_env(repo: &Path, connected_serial: Option<u64>) -> Env {
        let var = |k: &str| std::env::var(k).ok();
        let layout = var("PAKON_LINEWIDTH").and_then(|v| v.parse::<usize>().ok()).map(|lw| Layout {
            row_samples: lw,
            ir_lane: match var("PAKON_IR_LANE") {
                Some(v) => v != "0",
                None => lw == 8000,
            },
        });
        Env {
            repo: repo.to_path_buf(),
            work_dir: default_work_dir(),
            eeprom_dir: find_eeprom_dir(repo, connected_serial),
            layout,
            no_flat: var("PAKON_FLAT").as_deref() == Some("0"),
            sba: var("PAKON_OEM_SBA").as_deref() != Some("0"),
            sba_dmin: var("PAKON_SBA_DMIN").as_deref() != Some("0"),
            dice: var("PAKON_OEM_DICE").as_deref() != Some("0"),
            dice_dll: dice::default_dll(repo),
            export_threads: var("PAKON_EXPORT_THREADS").and_then(|v| v.parse().ok()).filter(|&n| n > 0),
        }
    }

    pub fn ribbon_path(&self) -> PathBuf {
        self.work_dir.join("ribbon.npy")
    }
    pub fn ir_ribbon_path(&self) -> PathBuf {
        self.work_dir.join("ir_ribbon.npy")
    }
    pub fn ir_noise_path(&self) -> PathBuf {
        self.work_dir.join("ir_noise.npy")
    }
    pub fn preview_path(&self) -> PathBuf {
        self.work_dir.join("preview.jpg")
    }
    fn profiles(&self) -> PathBuf {
        self.repo.join("profiles")
    }
    pub fn rpd_profile(&self) -> PathBuf {
        self.profiles().join("rpd.pf")
    }

    /// The OEM scene balance can run: on, and its DLL and parameters exist.
    pub fn sba_on(&self) -> bool {
        self.sba && sba::default_dll(&self.repo).is_file() && sba::sba_dir(&self.repo).is_dir()
    }

    /// The OEM Digital ICE can run.
    pub fn dice_on(&self) -> bool {
        self.dice && self.dice_dll.is_file()
    }

    /// (NegMatrix, LUT, rpd.pf table) for the OEM colour path, or None
    /// without an EEPROM archive or rpd.pf (`_oem_path`).
    pub fn oem_path(&self) -> Option<Oem> {
        let dir = self.eeprom_dir.as_ref()?;
        if !self.rpd_profile().is_file() {
            return None;
        }
        let copies = pakon_proto::eeprom::load_dir(dir);
        let e = pakon_proto::eeprom::decode(pakon_proto::eeprom::as_refs(&copies));
        if !e.decodable {
            return None;
        }
        Some(Oem { negmat: e.neg_matrix, lut: colour::c41_lut(), table: Table::from_u8(52, pakon_image::RPD_TABLE_U8).ok()? })
    }

    /// The slide path: the unit's PosMatrix when its archive decodes, else
    /// the factory one (`_slide_path`).
    pub fn slide_path(&self) -> Result<slide::SlidePath, String> {
        let posmat = match &self.eeprom_dir {
            None => colour::pos_matrix_default(),
            Some(dir) => {
                let copies = pakon_proto::eeprom::load_dir(dir);
                let e = pakon_proto::eeprom::decode(pakon_proto::eeprom::as_refs(&copies));
                if !e.decodable {
                    return Err(format!("no usable positive_matrix in {}", dir.display()));
                }
                e.pos_matrix
            }
        };
        slide::SlidePath::load(posmat, &self.profiles())
    }

    /// Frames exported at once: one per core, as many as fit in half the
    /// physical memory ([`workers_for`]); PAKON_EXPORT_THREADS overrides it.
    pub fn export_workers(&self, ice: bool) -> usize {
        if let Some(n) = self.export_threads {
            return n;
        }
        match total_ram() {
            None => 2,
            Some(ram) => workers_for(ram, ice, std::thread::available_parallelism().map_or(2, |n| n.get())),
        }
    }

    /// Delete the exported frames, or with `everything` the whole work
    /// folder (`clear_work`).
    pub fn clear_work(&self, everything: bool) {
        let Ok(rd) = std::fs::read_dir(&self.work_dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let frame_file = name.starts_with("frame_")
                || name.starts_with("thumb_")
                || name == "contact_sheet.jpg"
                || (name.starts_with("export_") && name.ends_with(".zip"));
            if p.is_file() && (everything || frame_file) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

fn total_ram() -> Option<u64> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok();
        if let Some(v) = out.and_then(|o| String::from_utf8(o.stdout).ok()?.trim().parse().ok()) {
            return Some(v);
        }
        let mi = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = mi.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as the call asks.
        let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut s) } != 0).then_some(s.ullTotalPhys)
    }
    #[cfg(not(any(unix, windows)))]
    None
}

/// The OEM colour-negative path's inputs.
pub struct Oem {
    pub negmat: Matrix,
    pub lut: Vec<f32>,
    pub table: Table,
}

/// What `prescan` returns (the JSON the web app keeps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub ribbon: String,
    pub base: [f64; 3],
    pub rpd_balance: Option<[[f64; 3]; 2]>,
    pub rows: usize,
    pub cols: usize,
    pub pitch: usize,
    pub frame_w: usize,
    pub box_w: HashMap<String, usize>,
    pub format: Format,
    pub centres: Vec<i64>,
    pub preview_w: usize,
    pub preview_h: usize,
    pub scale: f64,
    pub film: FilmKind,
    pub flare: Option<[f64; 3]>,
    pub ice: bool,
    pub sba_dpi: Option<String>,
    pub scene_dmin: Option<[i64; 3]>,
}

/// The SBA parameter file for the raw's DX code, and the strip's film base
/// in RPD (`_sba_inputs`).
fn sba_inputs(env: &Env, raw: &Path, rgb: ArrayView3<u16>, first_frame: Option<usize>, oem: Option<&Oem>) -> (Option<String>, Option<[i64; 3]>) {
    let dir = sba::sba_dir(&env.repo);
    let code = film::code_for_raw(raw);
    let dpi = sba::dpi_for(&dir, code.map(|c| c.0), code.map(|c| c.1), sba::SOURCE_NEGATIVE_35).ok();
    let mut dmin = None;
    if let (Some(oem), Some(ff), true) = (oem, first_frame, env.sba_dmin) {
        let rows_per_mm = framing::geometry(rgb.len_of(Axis(1)), Format::Full).1 as f64 / 36.0;
        if let Some(base) = sba::film_base(rgb, ff, rows_per_mm, 1.0, 2.0) {
            dmin = Some(sba::film_base_rpd(base, &oem.negmat, &oem.lut));
        }
    }
    (dpi, dmin)
}

/// OEM SBA per-frame RPD shifts for raw crops, balanced against `roll`
/// (default the crops); None when it cannot run or fails (`_sba_shifts`).
fn sba_shifts(env: &Env, crops: &[ArrayView3<u16>], oem: Option<&Oem>, roll: Option<&[ArrayView3<u16>]>, dpi: Option<&str>, dmin: Option<[i64; 3]>) -> Option<Vec<[f32; 3]>> {
    let oem = oem?;
    if crops.is_empty() || !env.sba_on() {
        return None;
    }
    let analysis = |c: &ArrayView3<u16>| sba::analysis_image(c.view(), &oem.negmat, &oem.lut);
    let dir = sba::sba_dir(&env.repo);
    let run = || -> Result<Vec<[f32; 3]>, String> {
        let imgs: Vec<Array3<i16>> = crops.iter().map(analysis).collect();
        let roll: Option<Vec<Array3<i16>>> = roll.map(|r| r.iter().map(analysis).collect());
        let dpi = match dpi {
            Some(d) => d.to_string(),
            None => sba::dpi_for(&dir, None, None, sba::SOURCE_NEGATIVE_35)?,
        };
        sba::roll_shifts(&sba::default_dll(&env.repo), &dir, &imgs, &dpi, roll.as_deref(), dmin)
    };
    match run() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("OEM SBA failed, using the roll balance: {e}");
            None
        }
    }
}

/// `rgb[max(0, r1 - fw):r1]`, `r1 = min(rows, max(0, centre - fw // 2) + fw)`.
fn crop_rows(rows: usize, centre: i64, fw: usize) -> (usize, usize) {
    let r1 = ((centre - (fw as i64).div_euclid(2)).max(0) + fw as i64).min(rows as i64);
    ((r1 - fw as i64).max(0) as usize, r1.max(0) as usize)
}

/// `np.rot90(a, k)` as a view.
fn rot90<T>(a: ArrayView3<T>, k: usize) -> ArrayView3<T> {
    let mut v = a;
    match k % 4 {
        1 => {
            v.swap_axes(0, 1);
            v.invert_axis(Axis(0));
        }
        2 => {
            v.invert_axis(Axis(0));
            v.invert_axis(Axis(1));
        }
        3 => {
            v.swap_axes(0, 1);
            v.invert_axis(Axis(1));
        }
        _ => {}
    }
    v
}

/// A ribbon-sized array cached as .npy, memory-mapped.
pub struct Npy {
    map: memmap2::Mmap,
}

impl Npy {
    pub fn open(path: &Path) -> Result<Npy, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // SAFETY: the work folder's caches are only rewritten by a new prescan.
        let map = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Npy { map })
    }
    pub fn view3(&self) -> Result<ArrayView3<'_, u16>, String> {
        ArrayView3::<u16>::view_npy(&self.map).map_err(|e| e.to_string())
    }
    pub fn view2(&self) -> Result<ArrayView2<'_, u16>, String> {
        ArrayView2::<u16>::view_npy(&self.map).map_err(|e| e.to_string())
    }
}

fn write_npy<A: WriteNpyExt>(path: &Path, a: &A) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    a.write_npy(std::io::BufWriter::new(f)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Stage 1: build the ribbon, measure the film base, find the frames, cache
/// the ribbon (and IR plane), render the preview strip.
pub fn prescan(env: &Env, raw: &Path, film: FilmKind, fmt: Format, progress: Option<Progress>) -> Result<Meta, String> {
    let grey = film.grey();
    std::fs::create_dir_all(&env.work_dir).map_err(|e| format!("{}: {e}", env.work_dir.display()))?;
    env.clear_work(false);

    let opt = ribbon::Options { layout: env.layout, eeprom_dir: env.eeprom_dir.clone(), no_flat: env.no_flat };
    let mut prog = |s: &str, p: f64| emit(progress, s, p);
    let r = ribbon::build(raw, &opt, &mut prog)?;
    let rgb = r.rgb;
    let (rows, cols, _) = rgb.dim();
    // On slide film dust removal smoothed the grain over most of the frame
    // (2026-11-02), so slides get none.
    let ice_plane = if film == FilmKind::Slide { None } else { r.ir.zip(r.flat) };

    emit(progress, "Measuring film base", 0.62);
    let base = balance::dmin(rgb.view());

    emit(progress, "Detecting frames", 0.68);
    let box_w: HashMap<String, usize> = Format::ALL.iter().map(|f| (f.name().to_string(), framing::geometry(cols, *f).1)).collect();
    let (starts, mut frame_w, mut pitch) = framing::frame_grid(rgb.view(), film == FilmKind::Slide, fmt);
    let mut centres: Vec<i64> = starts.iter().map(|&s| (s + frame_w / 2) as i64).collect();
    if centres.is_empty() {
        centres = vec![(rows / 2) as i64];
        pitch = rows;
    }
    if frame_w == 0 {
        frame_w = framing::geometry(cols, fmt).1;
    }

    emit(progress, "Caching ribbon", 0.75);
    write_npy(&env.ribbon_path(), &rgb)?;
    match &ice_plane {
        Some((ir, flat)) => {
            write_npy(&env.ir_ribbon_path(), ir)?;
            write_npy(&env.ir_noise_path(), &ndarray::Array1::from(ice::ir_noise_factor(flat, cols)))?;
        }
        None => {
            let _ = std::fs::remove_file(env.ir_ribbon_path());
            let _ = std::fs::remove_file(env.ir_noise_path());
        }
    }

    emit(progress, "Rendering preview", 0.85);
    let ds = ((cols as f64 / PREVIEW_H).round_ties_even() as usize).max(1);
    let small = rgb.slice(s![..;ds, ..;ds, ..]).to_owned();
    let oem = env.oem_path();
    let (sba_dpi, scene_dmin) =
        if film != FilmKind::Slide { sba_inputs(env, raw, rgb.view(), starts.first().copied(), oem.as_ref()) } else { (None, None) };
    let mut rpd_balance = None;
    let mut flare = None;
    let pos: Picture16Owned = if film == FilmKind::Slide {
        let fl = slide::measure_slide_flare(rgb.view(), slide::SLIDE_FLARE, None);
        flare = Some(fl);
        Picture16Owned::Rgb(slide::render_slide(small.view(), &env.slide_path()?, Some(fl)))
    } else if let Some(o) = &oem {
        let bal = balance::roll_balance(small.view(), &o.negmat, &o.lut, balance::OEM_BALANCE);
        rpd_balance = Some(bal);
        let fw = frame_w.min(pitch) as i64;
        let crops: Vec<ArrayView3<u16>> =
            centres.iter().map(|&c| rgb.slice(s![(c - fw / 2).max(0) as usize..((c + fw / 2).max(0) as usize).min(rows), .., ..])).collect();
        let shifts = sba_shifts(env, &crops, Some(o), None, sba_dpi.as_deref(), scene_dmin);
        // The strip gets the roll's mean OEM shift; the export applies each
        // frame's own.
        let b = match &shifts {
            None => bal,
            Some(s) => {
                let mut sum = [0.0f32; 3];
                for v in s {
                    for c in 0..3 {
                        sum[c] += v[c];
                    }
                }
                Balance { gain: [1.0; 3], offset: sum.map(|v| v / s.len() as f32) }
            }
        };
        let out = colour::render_oem(small.view(), &o.negmat, &o.lut, &o.table, Some(&b), None, grey);
        if grey {
            Picture16Owned::Grey(colour::to_grey(out.view()))
        } else {
            Picture16Owned::Rgb(out)
        }
    } else {
        let p = fallback::invert_c41(small.view(), base, &colour::c41_lut());
        if grey {
            Picture16Owned::Grey(colour::to_grey(p.view()))
        } else {
            Picture16Owned::Rgb(p)
        }
    };
    let strip = output::preview_strip(output::to_u8(&pos.picture()).view());
    output::write_jpeg(&env.preview_path(), strip.view(), 85, None)?;
    let (prev_h, prev_w, _) = strip.dim();

    emit(progress, "Done", 1.0);
    Ok(Meta {
        ribbon: env.ribbon_path().to_string_lossy().into_owned(),
        base: base.map(|v| v as f64),
        rpd_balance: rpd_balance.map(|b| [b.gain.map(|v| v as f64), b.offset.map(|v| v as f64)]),
        rows,
        cols,
        pitch,
        frame_w,
        box_w,
        format: fmt,
        centres,
        preview_w: prev_w,
        preview_h: prev_h,
        scale: prev_w as f64 / rows as f64,
        film,
        flare: flare.map(|f| f.map(|v| v as f64)),
        ice: ice_plane.is_some(),
        sba_dpi,
        scene_dmin,
    })
}

enum Picture16Owned {
    Rgb(Array3<u16>),
    Grey(Array2<u16>),
}

impl Picture16Owned {
    fn picture(&self) -> Picture16<'_> {
        match self {
            Picture16Owned::Rgb(a) => Picture16::Rgb(a.view()),
            Picture16Owned::Grey(a) => Picture16::Grey(a.view()),
        }
    }
}

/// The OEM framing on the cached ribbon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Autoframe {
    pub centres: Vec<i64>,
    pub frame_w: usize,
    pub pitch: usize,
    pub format: Format,
}

pub fn autoframe(ribbon: &Path, film: FilmKind, fmt: Format) -> Result<Autoframe, String> {
    let npy = Npy::open(ribbon)?;
    let rgb = npy.view3()?;
    let (starts, frame_w, pitch) = framing::frame_grid(rgb, film == FilmKind::Slide, fmt);
    Ok(Autoframe { centres: starts.iter().map(|&s| (s + frame_w / 2) as i64).collect(), frame_w, pitch, format: fmt })
}

/// A frame's per-picture adjustments from the web app's JSON
/// (`clean_adjust`): numbers rounded half to even and clamped, an unknown
/// tone ignored; None when nothing changes.
pub fn adjust_from_json(v: &serde_json::Value) -> Option<Adjust> {
    let obj = v.as_object()?;
    let num = |k: &str| -> f64 {
        match obj.get(k) {
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0.0),
            Some(serde_json::Value::Bool(b)) => *b as i32 as f64,
            _ => 0.0,
        }
    };
    let r = |k: &str, lim: f64| num(k).round_ties_even().clamp(-lim, lim) as i32;
    let tone = match obj.get("tone").and_then(|t| t.as_str()) {
        Some("warm") => Tone::Warm,
        Some("cold") => Tone::Cold,
        Some("sepia") => Tone::Sepia,
        _ => Tone::None,
    };
    let a = Adjust {
        red: r("red", 1000.0),
        green: r("green", 1000.0),
        blue: r("blue", 1000.0),
        brightness: r("brightness", 1000.0),
        contrast: r("contrast", 1000.0),
        sharpness: r("sharpness", 1000.0),
        saturation: r("saturation", 5.0),
        tone,
    };
    a.clean()
}

/// What `export_frames` takes (the prescan's meta plus the operator's
/// boxes and choices).
#[derive(Debug, Clone, Default)]
pub struct ExportRequest {
    pub ribbon: PathBuf,
    pub base: [f32; 3],
    pub centres: Vec<i64>,
    /// Per-frame crop widths (full, half and pano boxes mixed), or None for
    /// `frame_w` everywhere.
    pub widths: Option<Vec<usize>>,
    /// Degrees, a multiple of 90 (default 90).
    pub rotate: i32,
    pub frame_w: usize,
    pub rpd_balance: Option<[[f32; 3]; 2]>,
    /// The prescan's frames: the scene balance weighs against the roll.
    pub roll_centres: Option<Vec<i64>>,
    pub film: FilmKind,
    pub flare: Option<[f32; 3]>,
    pub dx_film: Option<Film>,
    pub ice: bool,
    pub sba_dpi: Option<String>,
    pub scene_dmin: Option<[i64; 3]>,
    pub adjustments: Option<Vec<Option<Adjust>>>,
}

/// The files written for one frame.
#[derive(Debug, Clone, Serialize)]
pub struct FrameFiles {
    pub index: usize,
    pub raw_tiff: PathBuf,
    pub tiff: PathBuf,
    pub jpg: PathBuf,
    pub thumb: PathBuf,
}

/// The film named by a raw's DX code (`dx.for_raw`).
pub fn dx_for_raw(repo: &Path, raw: &Path) -> Option<Film> {
    let (p, g) = film::code_for_raw(raw)?;
    ProdTable::read(&ProdTable::path(repo)).lookup(p, g)
}

/// Stage 2: crop the cached ribbon at each centre, remove dust, balance,
/// render and write the TIFFs and JPEGs. Frames render in parallel, as many
/// as memory allows.
pub fn export_frames(env: &Env, req: &ExportRequest, progress: Option<Progress>) -> Result<Vec<FrameFiles>, String> {
    let grey = req.film.grey();
    let slide = if req.film == FilmKind::Slide { Some(env.slide_path()?) } else { None };
    std::fs::create_dir_all(&env.work_dir).map_err(|e| format!("{}: {e}", env.work_dir.display()))?;
    env.clear_work(false);

    let npy = Npy::open(&req.ribbon)?;
    let rgb = npy.view3()?;
    let rows = rgb.len_of(Axis(0));
    let lut = colour::c41_lut();
    let rpd_jpeg = env.rpd_profile().is_file().then(|| Table::from_u8(52, pakon_image::RPD_TABLE_U8).ok()).flatten();
    let oem = env.oem_path();
    let rot_k = (req.rotate.div_euclid(90)).rem_euclid(4) as usize;
    let n = req.centres.len();
    let width_of = |i: usize| req.widths.as_ref().and_then(|w| w.get(i).copied()).unwrap_or(req.frame_w);
    let crop_at = |c: i64, fw: usize| {
        let (a, b) = crop_rows(rows, c, fw);
        rgb.slice_move(s![a..b, .., ..])
    };
    let crop = |i: usize| crop_at(req.centres[i], width_of(i));

    let ir_npy = if req.ice && env.ir_ribbon_path().is_file() { Some(Npy::open(&env.ir_ribbon_path())?) } else { None };
    let ir_rib = ir_npy.as_ref().map(|n| n.view2()).transpose()?;
    let ir_noise: Vec<f32> = if ir_rib.is_some() {
        ndarray_npy::read_npy::<_, ndarray::Array1<f32>>(env.ir_noise_path()).map_err(|e| e.to_string())?.to_vec()
    } else {
        vec![]
    };
    let dpi = ice::pakon_dpi(rgb.len_of(Axis(1)));
    let use_dice = ir_rib.is_some() && env.dice_on();

    // OEM scene balance of the confirmed frames against the whole roll.
    let mut shifts = None;
    if slide.is_none() {
        if oem.is_some() && env.sba_on() {
            emit(progress, "OEM scene balance", 0.0);
        }
        let roll: Option<Vec<ArrayView3<u16>>> = req.roll_centres.as_ref().map(|rc| rc.iter().map(|&c| crop_at(c, req.frame_w)).collect());
        let crops: Vec<ArrayView3<u16>> = (0..n).map(crop).collect();
        shifts = sba_shifts(env, &crops, oem.as_ref(), roll.as_deref(), req.sba_dpi.as_deref(), req.scene_dmin);
    }

    // Saturation / tone tables, built once per (saturation, tone).
    let tables: Mutex<HashMap<(i32, Tone), std::sync::Arc<Table>>> = Mutex::new(HashMap::new());
    let table_for = |base: &Table, profile: &Path, a: &Adjust| -> Result<std::sync::Arc<Table>, String> {
        let key = (a.saturation, a.tone);
        if let Some(t) = tables.lock().unwrap().get(&key) {
            return Ok(t.clone());
        }
        let t = std::sync::Arc::new(adjust::adjusted_table(base, profile, a.saturation, a.tone)?);
        tables.lock().unwrap().insert(key, t.clone());
        Ok(t)
    };

    let clean = |i: usize, part: ArrayView3<u16>| -> Result<Array3<u16>, String> {
        let Some(ir_rib) = &ir_rib else { return Ok(part.to_owned()) };
        let fw = width_of(i);
        let (a, b) = crop_rows(rows, req.centres[i], fw);
        let ir = ir_rib.slice(s![a..b, ..]);
        let (out, mask) = if use_dice {
            dice::with_thread_dice(&env.dice_dll, |d| dice::clean_frame(d, part, ir, true))?
        } else {
            ice::clean_frame(part, ir, &ir_noise, dpi)
        };
        let pct = 100.0 * mask.iter().filter(|&&m| m).count() as f64 / mask.len().max(1) as f64;
        eprintln!("ICE frame {} ({}): {pct:.2}% rebuilt", i + 1, if use_dice { "OEM" } else { "ours" });
        Ok(out)
    };

    let render = |i: usize| -> Result<FrameFiles, String> {
        let cleaned = clean(i, crop(i))?;
        let part = rot90(cleaned.view(), rot_k).as_standard_layout().into_owned();
        let adj = req.adjustments.as_ref().and_then(|a| a.get(i).copied().flatten());
        let (pos, rendered): (Picture16Owned, Array3<u8>) = if let Some(sp) = &slide {
            let p = match &adj {
                None => slide::render_slide(part.view(), sp, req.flare),
                Some(a) => {
                    let t = if a.saturation != 0 || a.tone != Tone::None { Some(table_for(&sp.romm, &env.profiles().join("romm.pf"), a)?) } else { None };
                    slide::render_slide_adjusted(part.view(), sp, req.flare, a, t.as_deref().unwrap_or(&sp.romm))
                }
            };
            let pos = Picture16Owned::Rgb(p);
            let r = output::to_u8(&pos.picture());
            (pos, r)
        } else if let Some(o) = &oem {
            let bal = match &shifts {
                Some(s) => Some(Balance { gain: [1.0; 3], offset: s[i] }),
                None => req.rpd_balance.map(|b| Balance { gain: b[0], offset: b[1] }),
            };
            let t = match &adj {
                Some(a) if a.saturation != 0 || a.tone != Tone::None => Some(table_for(&o.table, &env.rpd_profile(), a)?),
                _ => None,
            };
            let out = colour::render_oem(part.view(), &o.negmat, &o.lut, t.as_deref().unwrap_or(&o.table), bal.as_ref(), adj.as_ref(), grey);
            let keep_colour = adj.is_some_and(|a| a.tone != Tone::None);
            let pos = if grey && !keep_colour { Picture16Owned::Grey(colour::to_grey(out.view())) } else { Picture16Owned::Rgb(out) };
            let r = output::to_u8(&pos.picture());
            (pos, r)
        } else {
            let p = fallback::invert_c41(part.view(), req.base, &lut);
            if grey {
                let pos = Picture16Owned::Grey(colour::to_grey(p.view()));
                let r = output::to_u8(&pos.picture());
                (pos, r)
            } else {
                let r = match &rpd_jpeg {
                    Some(t) => fallback::render_jpeg_default(p.view(), t),
                    None => output::to_u8(&Picture16::Rgb(p.view())),
                };
                (Picture16Owned::Rgb(p), r)
            }
        };

        let k = i + 1;
        let files = FrameFiles {
            index: k,
            raw_tiff: env.work_dir.join(format!("frame_{k:02}_raw.tif")),
            tiff: env.work_dir.join(format!("frame_{k:02}.tif")),
            jpg: env.work_dir.join(format!("frame_{k:02}.jpg")),
            thumb: env.work_dir.join(format!("thumb_{k:02}.jpg")),
        };
        let dx = req.dx_film.as_ref();
        // raw negative (uninverted, 16-bit), for inverting in another tool
        output::write_tiff16(&files.raw_tiff, &Picture16::Rgb(part.view()), dx)?;
        output::write_tiff16(&files.tiff, &pos.picture(), dx)?;
        output::write_jpeg(&files.jpg, rendered.view(), 92, dx)?;
        output::write_jpeg(&files.thumb, output::thumbnail(rendered.view()).view(), 85, None)?;
        Ok(files)
    };

    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let workers = n.min(cpus).min(env.export_workers(ir_rib.is_some())).max(1);
    let what = if ir_rib.is_some() { " (removing dust)" } else { "" };
    emit(progress, &format!("Exporting {n} frames{what}"), 0.0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(workers).build().map_err(|e| e.to_string())?;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Result<FrameFiles, String>> = pool.install(|| {
        use rayon::prelude::*;
        (0..n)
            .into_par_iter()
            .map(|i| {
                let r = render(i);
                let d = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                emit(progress, &format!("Exported frame {d}/{n}{what}"), d as f64 / n.max(1) as f64);
                r
            })
            .collect()
    });
    let frames = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    emit(progress, "Done", 1.0);
    Ok(frames)
}

/// `make_contact_sheet` into the work folder; the path, or None with no frames.
pub fn contact_sheet(env: &Env, frames: &[FrameFiles]) -> Result<Option<PathBuf>, String> {
    let items: Vec<(usize, &Path)> = frames.iter().map(|f| (f.index, f.jpg.as_path())).collect();
    let path = env.work_dir.join("contact_sheet.jpg");
    Ok(output::write_contact_sheet(&path, &items)?.then_some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_at_once_by_memory() {
        let gb = |n: u64| n << 30;
        // (RAM, cores, without ICE, with ICE)
        for (ram, cpus, plain, ice) in [(4, 8, 4, 2), (8, 8, 8, 5), (16, 10, 10, 10), (48, 14, 14, 14), (8, 4, 4, 4), (2, 8, 2, 2)] {
            assert_eq!((workers_for(gb(ram), false, cpus), workers_for(gb(ram), true, cpus)), (plain, ice), "{ram} GB, {cpus} cores");
        }
    }

    #[test]
    fn crop_rows_like_python() {
        assert_eq!(crop_rows(10000, 5000, 3000), (3500, 6500));
        assert_eq!(crop_rows(10000, 100, 3000), (0, 3000));
        assert_eq!(crop_rows(10000, 9900, 3000), (7000, 10000));
        assert_eq!(crop_rows(2000, 1000, 3000), (0, 2000));
        assert_eq!(crop_rows(10000, 5001, 3001), (3501, 6502));
    }

    #[test]
    fn rot90_like_numpy() {
        let a = Array3::from_shape_fn((2, 3, 1), |(y, x, _)| 10 * y + x);
        let r1 = rot90(a.view(), 1);
        assert_eq!(r1.dim(), (3, 2, 1));
        assert_eq!(r1[(0, 0, 0)], a[(0, 2, 0)]);
        let r3 = rot90(a.view(), 3);
        assert_eq!(r3[(0, 0, 0)], a[(1, 0, 0)]);
        let r2 = rot90(a.view(), 2);
        assert_eq!(r2[(0, 0, 0)], a[(1, 2, 0)]);
    }

    #[test]
    fn adjust_json_like_clean_adjust() {
        let v: serde_json::Value = serde_json::json!({"red": 12.5, "blue": "-30", "saturation": 9, "tone": "blue"});
        let a = adjust_from_json(&v).unwrap();
        assert_eq!((a.red, a.blue, a.saturation, a.tone), (12, -30, 5, Tone::None));
        assert_eq!(adjust_from_json(&serde_json::json!({"red": 0.4})), None);
        assert_eq!(adjust_from_json(&serde_json::Value::Null), None);
    }
}
