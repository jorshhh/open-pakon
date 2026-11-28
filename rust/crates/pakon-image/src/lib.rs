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
pub mod ebp;
pub mod fallback;
pub mod film;
pub mod flat;
pub mod framing;
pub mod output;
pub mod psi24;
pub mod ice;
pub mod ribbon;
pub mod slide;
pub mod stats;

/// Dark level the light calibration targets (`pakon_image._RAW_BLACK`).
pub const RAW_BLACK: f32 = 300.0;

/// rpd.pf → sRGB on 52³ nodes (LittleCMS, 8-bit; `rust/tools/tables.py`, git tag python-final).
pub const RPD_TABLE_U8: &[u8] = include_bytes!("../assets/rpd.srgb52.u8");
/// PSI 2.4 rpd_24.pf (`PdenX1000_kpt1-gs1_3.pf`) → sRGB on 52³ nodes.
pub const RPD24_TABLE_U8: &[u8] = include_bytes!("../assets/rpd_24.srgb52.u8");
/// romm.pf → sRGB, the slide output table.
pub const ROMM_TABLE_U8: &[u8] = include_bytes!("../assets/romm.srgb52.u8");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour::Table;

    #[test]
    fn rpd24_table_loads_and_matches_neutrals_differs_on_blue() {
        let t3 = Table::from_u8(52, RPD_TABLE_U8).expect("rpd.srgb52.u8");
        let t24 = Table::from_u8(52, RPD24_TABLE_U8).expect("rpd_24.srgb52.u8");
        let n3 = t3.lookup([0.45, 0.45, 0.45]);
        let n24 = t24.lookup([0.45, 0.45, 0.45]);
        for c in 0..3 {
            assert!((n3[c] - n24[c]).abs() < 0.01, "neutral mismatch ch {c}: {n3:?} vs {n24:?}");
        }
        let b3 = t3.lookup([0.35, 0.35, 0.60]);
        let b24 = t24.lookup([0.35, 0.35, 0.60]);
        let diff = (b24[0] - b3[0]).abs() + (b24[1] - b3[1]).abs() + (b24[2] - b3[2]).abs();
        assert!(diff > 0.05, "expected noticeable blue shift between PSI 2.4 and 3.x: {b24:?} vs {b3:?}");
    }
}
