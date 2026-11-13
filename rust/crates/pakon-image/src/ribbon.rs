//! The ribbon: the whole roll as one registered, flat-fielded (rows, width, 3)
//! 16-bit image (port of `web/decode.py` `_build_ribbon`, visible part).

use std::path::{Path, PathBuf};

use memmap2::Mmap;
use ndarray::{Array2, Array3, ArrayView2};

use crate::decode::{self, Layout, Leads};
use crate::flat::{self, FlatJson};
use crate::ice;

/// A raw scan file, memory-mapped as little-endian u16 samples.
pub struct Raw {
    map: Mmap,
}

impl Raw {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Raw> {
        let f = std::fs::File::open(path)?;
        // SAFETY: the raw is only read; a file truncated underneath us would
        // fault, as numpy's memmap would.
        let map = unsafe { Mmap::map(&f)? };
        Ok(Raw { map })
    }

    /// The samples. Raws are written by our own tools on little-endian hosts;
    /// the map is page-aligned, so the cast is aligned.
    pub fn samples(&self) -> &[u16] {
        let (pre, mid, _) = unsafe { self.map.align_to::<u16>() };
        assert!(pre.is_empty(), "mmap is page aligned");
        const { assert!(cfg!(target_endian = "little"), "raws are little-endian") };
        mid
    }
}

/// What the ribbon was built with, for logs and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonInfo {
    pub layout: Layout,
    pub marker: Option<usize>,
    pub leads: Leads,
    pub leads_measured: bool,
    pub flat: Option<PathBuf>,
}

pub struct Ribbon {
    pub rgb: Array3<u16>,
    /// The IR plane, row for row with `rgb`, flat-fielded, for dust removal;
    /// only for raws with an IR lane and a flat field with an IR table.
    pub ir: Option<Array2<u16>>,
    /// The flat field used (its IR table feeds `ice::ir_noise_factor`).
    pub flat: Option<FlatJson>,
    pub info: RibbonInfo,
}

/// Options matching the web app's environment overrides.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// PAKON_LINEWIDTH (+ PAKON_IR_LANE): force the row layout.
    pub layout: Option<Layout>,
    /// The EEPROM archive (for the unit's flatfield*.json).
    pub eeprom_dir: Option<PathBuf>,
    /// PAKON_FLAT=0.
    pub no_flat: bool,
}

/// Marker-align, deinterleave, register and flat-field a raw. `progress`
/// gets (step, fraction) as `_build_ribbon` emits them.
pub fn build(raw_path: &Path, opt: &Options, progress: &mut dyn FnMut(&str, f64)) -> Result<Ribbon, String> {
    progress("Loading", 0.0);
    let raw = Raw::open(raw_path).map_err(|e| format!("{}: {e}", raw_path.display()))?;
    let samples = raw.samples();

    progress("Detecting row layout", 0.05);
    let layout = opt
        .layout
        .or_else(|| flat::flat_row_layout(raw_path).map(|(row_samples, ir_lane)| Layout { row_samples, ir_lane }))
        .or_else(|| decode::detect_row_layout(samples))
        .ok_or("could not detect the row layout of this raw (no known stride matched); set PAKON_LINEWIDTH (+ PAKON_IR_LANE=0/1) to force")?;
    let width = layout.width();
    progress(
        &format!("Layout: {} samples/row, {width} px ({})", layout.row_samples, if layout.ir_lane { "RGB+IR" } else { "RGB" }),
        0.08,
    );

    let lw = layout.row_samples;
    let lines = samples.len() / lw;
    let img = ArrayView2::from_shape((lines, lw), &samples[..lines * lw]).map_err(|e| e.to_string())?;

    progress("Aligning rows", 0.10);
    let marker = decode::marker_column(img, 0.6, 3000);
    let m = marker.unwrap_or(0);

    progress("Registering channels", 0.30);
    let (leads, leads_measured) = decode::validated_leads(decode::measure_leads(img, width, m), width);
    let mut rgb = decode::register(img, width, m, leads);

    let flat_path = if opt.no_flat { None } else { flat::find_flat_field(raw_path, opt.eeprom_dir.as_deref(), Some(width)) };
    let mut applied = None;
    let mut flat_json = None;
    if let Some(p) = &flat_path {
        match FlatJson::read(p).and_then(|f| flat::load_flat_field(&f, width).map(|ff| (f, ff))) {
            Ok((f, ff)) => {
                flat::apply_flat_field(&mut rgb, &ff);
                progress(&format!("Flat field: {}", p.file_name().unwrap_or_default().to_string_lossy()), 0.55);
                applied = Some(p.clone());
                flat_json = Some(f);
            }
            Err(e) => eprintln!("flat field not applied: {e}"),
        }
    }

    // IR plane for dust removal: registration keeps red as the row
    // reference, so ribbon row y is raw row y.
    let mut ir = None;
    if let (true, Some(f)) = (layout.ir_lane, &flat_json) {
        match ice::ir_plane(img, m, f, rgb.len_of(ndarray::Axis(0)), 0.9) {
            Ok(p) => ir = Some(ice::ir_to_u16(&p)),
            Err(e) => eprintln!("no IR plane for dust removal: {e}"),
        }
    }
    Ok(Ribbon { rgb, ir, flat: flat_json, info: RibbonInfo { layout, marker, leads, leads_measured, flat: applied } })
}
