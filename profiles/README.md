# Colour profiles & correction data

Kodak/Pakon colour-correction data extracted from the OEM F-X35 COM SERVER
install (`Config/ColorCorrection/`), used by the image pipeline
(`rust/crates/pakon-image`) to reproduce the OEM's image processing. © Eastman Kodak Company.

## Used today

- **`rpd.pf`** — "RPD Rendering Profile" (a `RPD_dls_3` + `yellow5` ICC cascade).
  The OEM "Use Color Correction (12-bit RPD)" stage: the input profile of
  the 12-bit RPD image. Sampled with LittleCMS into a 52³ table that is
  embedded in `pakon-image` (`assets/rpd.srgb52.u8`); the app also reads the
  file for the saturation and B&W tone adjustments.
- **`ColRevLut1.pf`** — the slide path's device link, "Pakon CMY to ROMM12"
  (ICC lut16): PosMatrix output → 12-bit ROMM. `render_slide`
  (`pakon-image/src/slide.rs`).
- **`romm.pf`** — ROMM RGB input profile; the slide path's `CRInputProfile`
  (embedded as a table too, `assets/romm.srgb52.u8`).
- **`satplus03..15.pf` / `satminus03..15.pf`**: saturation-variant rendering
  profiles; the per-picture saturation adjustment picks one
  (`pakon-image/src/adjust.rs`).
- **`cold_bw.pf`, `warm_bw_ld0_1_4-5.pf`, `sepia_ld0_9_22.pf`**: the B&W
  tones (cold, warm, sepia) of the B&W adjustment
  (`pakon-image/src/colour.rs`).

## Not kept

The OEM install has more files in `Config/ColorCorrection/` that nothing
reads: `srgb.pf`, `unity.pf`, `ColRevLutS6.lut`, `Defaults.ini`, and
`_ClientColNegLut.txt` / `_ClientColNegMat.txt`, whose values are compiled
into `pakon-image` (`c41_lut`, the matrix constants). `docs/IMAGING.md`
describes them; they are at git tag `python-final`.

Re-extract from an OEM install (`Config/ColorCorrection/`) if needed.
