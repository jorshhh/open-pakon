//! Pakon image pipeline (`docs/IMAGING.md`): the raw 0x86 stream to
//! the registered, flat-fielded RGB ribbon; then (as they are ported) OEM
//! colour, framing, dust removal and export. Ports `tools/pakon_image.py`,
//! `web/decode.py`, `tools/pakon_ice.py`, `tools/oem_*.py` and
//! `src/pakon_render.c`, checked against the Python output.

// Index loops mirror the numpy / C code they port, line for line.
#![allow(clippy::needless_range_loop)]

pub mod adjust;
pub mod balance;
pub mod colour;
pub mod decode;
pub mod fallback;
pub mod film;
pub mod flat;
pub mod framing;
pub mod output;
pub mod ice;
pub mod ribbon;
pub mod slide;
pub mod stats;

/// Dark level the light calibration targets (`pakon_image._RAW_BLACK`).
pub const RAW_BLACK: f32 = 300.0;

/// rpd.pf → sRGB on 52³ nodes (LittleCMS, 8-bit; `rust/tools/tables.py`, git tag python-final).
pub const RPD_TABLE_U8: &[u8] = include_bytes!("../assets/rpd.srgb52.u8");
/// romm.pf → sRGB, the slide output table.
pub const ROMM_TABLE_U8: &[u8] = include_bytes!("../assets/romm.srgb52.u8");
