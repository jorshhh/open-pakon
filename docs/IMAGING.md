# Imaging pipeline

How the scanner's raw `0x86` image stream becomes exported files: decode,
flat field, framing, the OEM colour path, balance, adjustments, dust
removal and output. The wire protocol and calibration sequence are in
[PROTOCOL.md](PROTOCOL.md) and [REGISTERS.md](REGISTERS.md), the per-unit
EEPROM data (NegMatrix, PosMatrix, offsets) in [EEPROM.md](EEPROM.md), the
models in [SCANNER_FAMILY.md](SCANNER_FAMILY.md), the code layout in
[TECHNICAL.md](TECHNICAL.md), and open work in [ROADMAP.md](ROADMAP.md).

Markers: **[C]** confirmed (read from the OEM code or data, or measured on
hardware); **[INF]** inferred. OEM addresses refer to `TLB.dll` 3.1.0.28
(the F-135 / F-135+ engine), `PakonIMAu.dll` (the colour engine, built on
Kodak's Ansel library) and `DMLDICELib.dll` (Applied Science Fiction
Digital ICE). Hardware measurements are on F-135 serial 3054 unless noted;
F-135+ stream facts come from serial 16402 (captures) and serial 138
(code-built scans in all six modes).

## Overview

| Step | What happens | Code |
|---|---|---|
| Raw stream | 16-bit LE samples, one scan line per row, R,G,B triplets then an optional IR block | `pakon-image/src/decode.rs` (`detect_row_layout`, `marker_column`) |
| Ribbon | marker-align rows, deinterleave, register the trilinear lines on red | `decode.rs` (`measure_leads`, `validated_leads`, `register`), `ribbon.rs` (`build`) |
| Flat field | `black + (raw − dark) × gain` per column, from the calibration line | `flat.rs`; tables written by `pakon-scan/src/calib.rs` |
| IR plane | the IR block, flat-fielded against the open-gate IR | `ice.rs` (`ir_plane`) |
| Framing | the OEM picture grid (`FN_iFramePictures`) | `framing.rs` (`frame_grid`) |
| Dust removal | OEM Digital ICE under Unicorn, else the native port | `pakon-oem/src/dice.rs`, `pakon-image/src/ice.rs` |
| Colour | density LUT → per-unit NegMatrix → 12-bit RPD → `rpd.pf` → sRGB | `colour.rs` (`render_oem`); slides `slide.rs` (`render_slide`) |
| Balance | Ansel SBA + FOS from `PakonIMAu.dll` under Unicorn, else a roll balance | `pakon-oem/src/sba.rs`, `balance.rs` (`roll_balance`) |
| Adjustments | the OEM `PIColorAdjustPlanar` arithmetic, saturation / B&W tone profiles | `colour.rs`, `adjust.rs` |
| Fallback | no EEPROM archive: Dmin positive, `rpd.pf` look for the JPEG | `fallback.rs` |
| Output | 16-bit TIFFs, JPEG, thumbnails, preview strip, contact sheet, DX metadata | `output.rs`, `film.rs` |
| Orchestration | prescan, autoframe, export of a roll | `pakon-roll/src/lib.rs` |

All `pakon-image` paths are under `rust/crates/`. The app runs the pipeline
in two stages, as a minilab does: a **prescan** (ribbon, film base, frames,
preview strip) and, after the operator confirms the frame boxes, an
**export** (crop, dust removal, balance, render, write).

## Raw image stream

### Row layout [C]

The `0x86` stream is unsigned 16-bit little-endian samples. Each scan line
is `width` per-pixel interleaved R, G, B triplets, followed by a block of
`width` IR samples when IR is on (one IR sample per pixel, not
interleaved):

```
[R0 G0 B0 R1 G1 B1 … R(w−1) G(w−1) B(w−1)] [IR0 IR1 … IR(w−1)]   (IR block only with IR on)
```

So `row_samples = 3 × width`, or `4 × width` with IR. The width is fixed per
resolution ("Base"): the OEM's per-base table gives 2000 / 1500 / 1000
output pixels at Base 16 / 8 / 4 across a 23.7 mm image height, and the
film advances one pixel pitch per line (square pixels, ~11.85 µm at
Base 16).

| Resolution | IR | Samples/row | Width (px) | Seen on |
|---|---|---|---|---|
| Base 16 | on | 8000 | 2000 | F-135 (the usual colour mode); F-135+ serial 138 |
| Base 16 | off | 6000 | 2000 | F-135+ captures and serial 138; F-135 with IR off |
| Base 8 | on | 6000 | 1500 | F-135+ serial 138; F-135 from the rule |
| Base 8 | off | 4500 | 1500 | F-135+ captures and serial 138; F-135 with IR off |
| Base 4 | on | 4000 | 1000 | F-135+ captures and serial 138 |
| Base 4 | off | 3000 | 1000 | F-135+ captures and serial 138; F-135 with IR off |

The stride alone is ambiguous: 6000 is both Base 16 without IR and Base 8
with IR. The scan therefore records `row_samples` and `ir_lane` in
`<raw>.flat.json`, and the decoder takes the layout from there
(`flat::flat_row_layout`). Without it, `detect_row_layout` autocorrelates a
central window of the file (up to 2 M samples) at each known stride and
takes the smallest stride within 0.02 of the best score (multiples of the
true stride score as high), provided the best reaches 0.6. Its table assumes
6000 means no IR. `PAKON_LINEWIDTH` (with `PAKON_IR_LANE=0/1`) forces a
layout.

The IR block is the scanner's defect channel: infrared passes through the
dyes but is blocked by dust and scratches. Silver B&W blocks IR, so it is
scanned with IR off and has no IR block.

### Line marker and row origin [C]

The scanner sets bit 0 of the first RGB word of every line; no other sample
has it set. The OEM uses it as a sync check (`EC_DRV_LostSync` when a line's
first word has bit 0 clear). It is present in calibration reads too, lit or
dark, IR on or off.

A raw file starts wherever the capture started, so the triplet phase is
unknown. `marker_column` finds the word position whose bit 0 is set in at
least 60% of sampled rows (three blocks of 1000 rows: start, middle, end),
and every row is rotated so that word becomes column 0 (`sample[(3 px + c +
marker) mod row_samples]`). After this the order is R, G, B. Rows are rotated
within the file row rather than re-sliced, so the last `marker` samples of a
row come from the previous scan line; on code-built scans the marker sits at
column 1.

The marker-aligned R, G, B order replaces several older decoder models
(B,R,G order, per-zone channel order, an IR band mid-line), which were
artefacts of a floating row phase.

## Decode to the RGB ribbon

### Trilinear registration [C]

The CCD is trilinear: the green and blue lines sit 8 and 16 lines from red
at Base 16. A feature on raw row `y` in red appears on row `y + 8` in green
and `y + 16` in blue, so the output row `i` takes R from raw row `i`, G
from `i + 8` and B from `i + 16`. The spacing scales with the width:

| Resolution | G lead | B lead | Measured |
|---|---|---|---|
| Base 16 | −8 | −16 | F-135, IR on and off |
| Base 8 | −6 | −12 | F-135, IR off |
| Base 4 | −4 | −8 | F-135, IR off |

`measure_leads` measures the leads on the central 30% of the rows: the
vertical log gradients `ln(v + 50)` of every third column, correlated over
±40 lines. `validated_leads` accepts the measurement only if `−13s ≤ g ≤ −3s`
and `−22s ≤ b ≤ −9s` (`s = width / 2000`), else uses the fixed spacing above
(`trilinear_leads`, rounded half to even). On the F-135+ captures the
measured leads came back implausible and the fixed spacing rendered clean;
whether the Plus firmware already line-aligns is open.

`register` deinterleaves and co-registers in one pass and crops to the rows
every channel covers. Red is the reference, so ribbon row `y` is raw row
`y`; the IR plane uses the same rows (IR-to-visible offset measured 0 rows,
0 columns).

### The ribbon

`ribbon::build` produces the whole roll as one `(rows, width, 3)` 16-bit
image: layout, marker, leads, registration, flat field, and the IR plane
when the raw has an IR lane and the flat field has an IR table. The prescan
caches the ribbon and the IR plane as memory-mapped `.npy` files, so the
export crops frames without decoding again.

## Flat field

### The OEM correction [C]

`TLB.dll` calibrates on the open gate with the motor stopped (calibration
details in [PROTOCOL.md](PROTOCOL.md)). After the LED duties converge it
averages 128 dark lines and 128 bright (open-gate) lines per column
(`FUN_1001f550`, inputs are 128-line sums):

```
den[c]  = (B[c] − D[c]) − (Bmask − Dmask)
gain[c] = min(125 · 2³² / den[c], 0x3FFFF)          16.16 fixed point
        = 64000 · 65536 / (mean bright − mean dark − black drift)
```

`Bmask − Dmask` is the drift of the optically black pixels between the two
passes. The open gate thus normalises to 64000 per column (IR: numerator
`0x7530000000`, 60000). Per line (`FUN_100246d0`, MMX):
`out = (raw − dark[c] − smear) · gain[c] / 2¹⁸`, which puts the open gate at
16000 in the 14-bit domain. The smear term is `65536 · (Bmask − Dmask) /
(Bactive − Dactive)`, kept only when it falls in 1..699, times a running line
level; it is a column-uniform offset. The `125 · 2³²` constant is also what
an independent clean-room F-135 driver uses.

### Ours [C]

`flat.rs` applies the same correction in the raw 16-bit domain, keeping the
~300 black the A/D offset loop targets (`RAW_BLACK = 300`):

```
gain[c] = min(64000 / ((bright[c] − dark[c]) − drift), 0x3FFFF / 65536) × slide_scale[c]
out     = clip(300 + (raw − dark[c]) × gain[c], 0, 65535), rounded
drift   = mean(bright[0:black_px]) − mean(dark[0:black_px])
```

The open gate lands at 64300; `(v − 300) / 4` then gives the OEM's 16000.
The smear term is left out (it measured 0 on serial 3054).

- **Tables.** `pakon_replay --light-cal` writes `flatfield.json` (or
  `flatfield-base8.json` / `flatfield-base4.json`) into the EEPROM archive;
  `--scan-code` writes `<raw>.flat.json` next to the raw
  (`pakon-scan/src/calib.rs`, `flat_json`). Fields: `line_px`, `px0`,
  `black_px` (20 at Base 16, scaled with the width), per-channel `dark` and
  `bright` tables (R, G, B, IR), `film`, `duties`, `scan_duties`,
  `row_samples`, `ir_lane`. The decoder prefers the raw's own file, else
  an archive file that covers the scan width (`find_flat_field`).
  `PAKON_FLAT=0` turns it off.
- **Column mapping.** Scan column `j` is calibration pixel `Offset − 6 + j`
  (`px0`): the calibration window starts at pixel 6 (`reg4 = 6`), the scan
  window at the EEPROM Offset. Confirmed on hardware: the empty gate shows
  the calibration profile column for column.
- **Without it** the illumination falls to ~25% at scan column 0 on serial
  3054 (0.23 at column 0, 0.53 at 10, 0.78 at 40), the same on every roll,
  which shows as a bright band along one long edge. With it, clear film
  reads 0.92–1.03 across the line.
- **Dark tables with IR on** are taken with the IR LED lit (calibration
  aligns its reads on the IR block); the IR LED adds 13–17 counts to the RGB
  dark table. With IR off the dark tables are true dark.
- **Slides.** A slide scan runs its LEDs brighter than the calibration (see
  the slide path), so `load_flat_field` scales each channel's gain by
  calibration duty / scan duty (`duties` / `scan_duties`) when `film` is
  `slide`. Other films keep the OEM behaviour: the scan duty is the
  calibrated duty × 10^D of the film-base density, and the gain is not
  compensated, which neutralises the film base optically (below).

### Dust in the gate during calibration [C]

The gain is `64000 / (bright − dark)` per column, so anything that shades
the bright line but is not there during the scan becomes a permanent
column error. On serial 3054, dust in the gate during the open-gate
calibration cast a **14–18% shadow** in a few columns of the bright line
that the scan itself did not have. Those columns got a correspondingly
higher gain, which drew a **coloured line through every frame** of the roll,
at the same position across the film. It is not a decode error: the Rust
and the Python decoders give identical results from the same raw.

Interpolating the bright line over the dusty columns (from the clean
columns on either side) before computing the gain was verified to remove
the line. The pipeline does not yet detect or repair this on its own; a
calibration with a clean gate gives clean tables.

The OEM watches for exactly this: when its settings enable it,
`FN_bCalibrateAfter` → `FUN_1000f5b0` scans the open-gate per-column table,
skips 20 columns at each edge, slides a window (setting `+0x1618` columns
from offset `+0x161c`) and raises `CLEANING_REQUIRED` when a window and its
neighbour differ by more than `(1000 + setting +0x1620) / 1000` on any of
R, G, B. The defaults are registry values, not in the install.

## Framing

### The OEM picture grid [C]

`frame_grid` (`framing.rs`) ports `TLB.dll` `FN_iFramePictures`
(`0x100079c0`) and its helpers. Geometry is fixed per resolution
(`FUN_10011a60`):

| Resolution | Block (lines) | Picture (lines) | Pitch (lines) | Width (px) |
|---|---|---|---|---|
| Base 16 | 8 | 3000 | 3206 | 2000 |
| Base 8 | 6 | 2250 | 2405 | 1500 |
| Base 4 | 4 | 1500 | 1603 | 1000 |

Every picture is 375 blocks and the pitch ~400 blocks at every base. A width
with no OEM entry takes the nearest resolution's geometry.

1. **Profile** (`FUN_10006870`): one value per block, the 8-bit mean
   (R+G+B)/3 of the block's first line, inverted (`255 − v`) for every film
   type except positive (type 2). The gap (clear base on a negative, black
   film on a slide) is low, the picture high.
2. **Threshold** (`FUN_10005d20`, mode 0): a 256-bin histogram of the
   profile. Walk up from bin 0 to the first peak (stop once a bin falls
   below 3/5 of a peak holding more than n/150 samples). `lo` = walk down
   while bin ≥ peak/20; `hi` = walk up (≤ 50 bins) while bin ≥ peak/10. If
   `(peak − lo) × 4 < hi − peak`, the threshold is the first bin above the
   peak below peak/2; else `hi + (hi − peak)/2`. Ones = profile > threshold.
3. **Sweep** (`FN_iFramePicturesSub`, `0x100072c0`): move the threshold +2,
   then −2, while the count of runs with 95% < length < 115% of a picture
   does not drop; keep the best.
4. **Placement** (`FN_iFramingFindPictures`, `0x10006e70`; slot = start × 2 /
   pitch, first placed wins):
   - `LookForNicePictures`: a run of 95–115% of a picture gives a start at
     `run start + (run length − picture) / 3`; a run of 95–115% of
     picture + pitch (a lost gap) gives two pictures, at `run start + (len −
     pitch − picture) / 3` and `run start + pitch`.
   - `LookInBetweenEnds`: between two found pictures, `n − 1` evenly spaced
     ones, `n = span / pitch` (minus 1 if the remainder is under pitch/4).
   - `LookAtEnd` / `LookAtBeginning`: step one pitch outward;
     `FN_iFramingEstimatePlacement` slides a picture-long window over a
     `(pitch − picture)`-wide range and keeps the offset with the most ones
     (ties: nearest the centre); stop when no film is left (fewer ones than
     half a picture).
   - Nothing found: `FramingBlindlyPlacePictures`, a pitch grid from
     `(pitch − picture) / 2`.
5. **Crop**: rows `start … start + picture − 1`, columns `0 … width − 1`. The
   crop is anchored at the detected picture start and spans the full sensor
   width; it is never centred between gaps.

Framing runs on the whole ribbon; nothing is trimmed first.

### Formats

The Format scales picture length and pitch (`framing::Format`):

| Format | Picture / pitch at Base 16 | Basis |
|---|---|---|
| full | 3000 / 3206 | OEM table |
| half | 1500 / 1603 | half of full (TLX has no half-frame preset); lands on all 10 confirmed frames of a half-frame B&W roll |
| pano (XPan) | 5530 / 5736 | ours; scaled to other bases by the same fractions |

### Measured [C]

- On five real rolls (full frame, half frame, slide) the whole-ribbon
  framing found every frame the operator confirmed, within ~10–30 rows.
- A 5-frame Base 16 colour-negative strip: 5 "nice" pictures, starts
  3176–3296 rows apart, every crop between the gaps.
- Measured frame pitch on real film runs 1.07–1.29% shorter than the OEM
  pitch at Base 16, 8 and 4 (F-135, IR on and off).

### In the app

Framing runs when the operator presses Auto-frame (`/api/autoframe`, in the
selected format; `pakon-roll` `autoframe`); a raw opens with its saved
boxes, or none. The prescan still frames the roll in its last auto-framed
format, for the scene balance. Crops at export are centred on each box
centre: rows `[r1 − fw, r1)` with `r1 = min(rows, max(0, centre − fw/2) +
fw)`, each box with its own format's length.

### Not modelled

- DX-checked framing: with DX enabled, `FUN_10006310` accepts a picture only
  when one DX frame mark falls in its first quarter and the next in its last
  quarter.
- Strip mode: the OEM frames each cut strip separately and numbers pictures
  through the roll; ours frames the whole ribbon.

## Colour: the OEM F-135 path

### Order of operations in `TLB.dll` [C]

`FN_bLoadImageFromBuffer` (`0x10026c90`) on the linear 16-bit planar picture:

1. Digital ICE on R, G, B, IR when the picture has IR (see below).
2. Per-plane **density LUT** (`FUN_1000dfc0`).
3. **3×10 matrix in density space** (`FN_bReadMatrix_3x10`, `0x1000d880`):
   NegMatrix for colour negative and both B&W types, PosMatrix for
   positive, then `PIColorCorrectColRevPlanar` for positive only.
4. Scale / rotate.
5. Per-picture adjustments (`FN_bApplyColorAdjustments` →
   `PIColorAdjustPlanar`).
6. The 12-bit RPD / sRGB profiles and the Ansel scene balance (roll balance
   over up to 40 pictures).

`PIColorCorrectColNegPlanarScan/…Save` are resolved with `GetProcAddress`
and never called.

### Density LUT [C]

```
LUT[i] = 3500 · log10(16383 / i),   i = 1 … 16383;   LUT[0] = 0x3FFF
```

16384 entries, 3500 codes per density decade. It is the tonal flip of the
negative. The OEM ships the same curve as `_ClientColNegLut.txt`, which fits
this formula with zero error over all 16384 entries. `c41_lut` in
`colour.rs` builds it in f64 and stores f32. The engine builds an identical
second copy for positives (`FUN_1000dfc0(2)`, LUT `+0x44` vs `+0x40`), so the
two differ only if client override files are present.

Input: `x = clamp((raw16 − 300) / 4, 1, 16383)`, truncated to the table
index.

### NegMatrix and PosMatrix [C]

Per unit, from the EEPROM (NegMatrix at section A `0x026`, PosMatrix at
`0x09e`; [EEPROM.md](EEPROM.md)). On densities R, G, B:

```
out = m0·R + m1·G + m2·B + m3·R² + m4·G² + m5·B² + m6·RG + m7·BR + m8·GB + m9
```

`+0.5`, clamped to 0…4095: **12-bit RPD**. The cross-term order is RG, BR,
GB.

- Serial 3054's NegMatrix is near-diagonal ≈ 0.28 (≈ 1000/3500, so ~1000 RPD
  codes per density decade) with offsets 167 / 452 / 671.
- The PosMatrix is 0.25·I on every unit seen (three units):
  875 codes per decade, no inversion. `pos_matrix_default` uses it when no
  EEPROM archive is available.
- Client files `ClientColNegLut.txt` + `ClientColNegMat_3x10.txt` override
  the LUT and NegMatrix only if both exist. The shipped
  `_ClientColNegLut.txt` / `_ClientColNegMat.txt` (leading underscore) are
  never loaded.

### Film base is neutralised optically

The NegMatrix constants are positive, so no film base is subtracted in the
matrix [C]. Instead the light calibration caps the calibration duty at
`1 / 10^D` and runs the scan at `open-gate duty × 10^D` of a nominal
film-base density (`FUN_10020230`) [C], which largely cancels the base
before the data reaches the LUT [INF].

| Film | R | G | B | IR | Source |
|---|---|---|---|---|---|
| Colour negative | 0.144 | 0.40 | 0.715 | 0 | `TLB.dll` |
| B&W C-41 | 0.10 | 0.25 | 0.25 | 0.08 | `TLB.dll` |
| Other (silver B&W; positive in the OEM) | 0 | 0.03 | 0 | 0.08 | `TLB.dll` |
| Slide (ours) | 0.12 | 0.12 | 0.12 | 0.08 | ours, see the slide path |

These live in `pakon-proto/src/lightcal.rs` (`DENSITY_*`).

### `rpd.pf` and the sRGB table [C]

`rpd.pf` ("RPD Rendering Profile", a Kodak KCMS ICC profile cascading
`RPD_dls_3` and `yellow5`) is an **input** profile, scanner class, RGB → Lab,
whose RGB is the 12-bit RPD the NegMatrix produces. It renders equal R, G, B
RPD as neutral. Feeding it an sRGB-encoded positive (as the old decoder did)
skews and oversaturates the colour.

`render_oem` (`colour.rs`) does, per pixel:

```
raw16 → x = clamp((raw16 − 300)/4, 1, 16383) → LUT → NegMatrix (all ten terms) → RPD 0…4095
      → [contrast] → [balance: RPD·gain + offset] → [colour offsets] → [unsharp mask]
      → [B&W: mean of R, G, B] → rpd.pf table at RPD/4095 → 16-bit sRGB
```

each step clamped to 0…4095. The `rpd.pf` → sRGB transform is a 52³ table on
exact 8-bit nodes (0, 5, 10, … 255), computed once with LittleCMS (8-bit
transform, perceptual intent, LittleCMS's built-in sRGB as output) and
embedded in `pakon-image` (`assets/rpd.srgb52.u8`). It is applied
trilinearly, so the output keeps 16-bit precision even though each node is
8-bit. No auto-levels follow.

### Film types [C]

| App film | TLB.dll type | Scan | Decode |
|---|---|---|---|
| `c41` colour negative | 1 | IR on, colour-negative densities | LUT → NegMatrix → `rpd.pf` |
| `bw` silver B&W | 4 | IR off, "other" densities | as `c41`, then grey |
| `bwc41` chromogenic B&W | 8 | IR on, B&W C-41 densities | as `bw` |
| `slide` (E-6) | 2 | IR on (ignored), slide densities | LUT → PosMatrix → `ColRevLut1.pf` → `romm.pf` |

`FUN_1000d880` treats types 1, 4 and 8 alike; type 2 is the positive path.

**B&W** (`bw`, `bwc41`): `render_oem(…, grey = true)` averages the three RPD
channels before the table, renders the neutral through `rpd.pf`, and keeps
the Rec. 709 luma (`0.2126 R + 0.7152 G + 0.0722 B`, `to_grey`) as a
one-channel image. A B&W tone (below) keeps the colour result. Where the OEM
converts to grey is not traced.

### Slide path [C]

`TLB.dll` film type 2: the same density LUT, then the PosMatrix (engine
object `+0xC8`), then `PakonIMAu!PIColorCorrectColRevPlanar(planes, w, h, 0,
0)`, which applies `ColRevLut1.pf` in place: a Kodak device link "Pakon CMY
to ROMM12" (ICC `mft2`/lut16, A2B0, 3→3, 511-entry input curves, 31³ grid,
identity output curves), giving 12-bit ROMM RGB. `romm.pf` (TLB.dll's
`CRInputProfile`) is then the input profile instead of `rpd.pf`. `PIBegin`
also loads `ColRevLutS6.lut` (4096 × 12-bit) into a global that nothing in
`PakonIMAu.dll` reads. Calibration itself refuses film type 2
(`FindCorrections`, error `0x1b`/`0xf`), so the OEM calibrates under another
type and applies the positive densities at scan.

`render_slide` (`slide.rs`) evaluates the lut16 straight from the tag
(input curves, trilinear grid, output curves; `slide::Link`), then
`romm.pf` → sRGB through an embedded 52³ table (`assets/romm.srgb52.u8`),
the same way as `rpd.pf`. A neutral density ramp stays neutral: D 0 →
white, D 1 → sRGB 102, D 3 → 9.

Four departures from the OEM, all for the shadows. Slides are the scanner's
weak point: the noise floor on serial 3054 is ~20–27 counts, so the usable
range is about D 3.3, while slides reach D 3.5–4.

- **Exact density.** `colour::rpd(…, exact = true)` computes `3500 ·
  log10(16383 / x)` in floating point (x clamped to 0.25…16383) instead of
  the 14-bit table; at D 3 one 14-bit step is ~0.026 D, which bands lifted
  shadows. Negatives keep the table.
- **More light at scan.** Slides get a film-base density of 0.12 on R, G, B
  (the OEM uses the "other" row), so the calibration duty is capped at
  1/1.32 of the period, the current search goes one step higher, and the
  scan runs ~1.32× the open-gate light (~1/3 stop more shadow range). Slide
  film never transmits more than about D 0.15, so nothing on the film clips.
  The decoder scales the flat gain by calibration duty / scan duty, so the
  film comes out at its true transmission. Measured on a rescan of the same
  strip: decoded median densities 1.28 / 1.45 / 1.68 vs 1.28 / 1.42 / 1.67
  before, the noise floor in the black gaps 13 / 22 / 23% lower (R / G / B,
  24.6 / 18.7 / 17.5 → 21.3 / 14.5 / 13.5 counts), raw clipping only on the
  first and last ~300 rows of leader and tail.
- **Stray light.** The black film between frames should read about D 3.5
  (~20 counts) but reads D 2.6–3 (R 172, G 86, B 71 counts above black on
  the first slide roll), lifted by scattered light, red-heavy. Per roll,
  `find_slide_gaps` finds the gap rows (rows whose 95th-percentile
  luminance, central ¾ of the width, stays under 3× the 1st percentile of
  all rows, in runs of 80+ rows); `measure_slide_flare` takes each channel's
  median there, caps it at D 2 (640 counts), and subtracts `SLIDE_FLARE`
  (0.5) × (that − the D 3.5 level, `64000 · 10^−3.5` ≈ 20 counts) from the
  raw before the density step. 0.5–0.75 looked right on that roll (deeper,
  neutral shadows, detail kept); 1.0 crushed the shade.
- **No scene balance.** The OEM runs the Ansel balance for positives too, in
  another mode (4, "CP-Balance"); it is not modelled.

Framing uses the uninverted profile for slides, so the black gaps are the
low level, as `TLB.dll` does for type 2.

### The shared ColNeg files

`_ClientColNegMat.txt` is a 3×3 + offset dye-crosstalk / orange-mask matrix
(diagonal 1.119 / 1.101 / 1.083, small off-diagonal terms, offsets −82.6 /
−586.9 / −707.8). It belongs to the F-235 engine (`TLA.dll`); `TLB.dll`
never calls that path. In our scale its offsets over-subtract G and B (a red
cast), so even the fallback leaves it off.

## Balance

### Ansel scene balance (SBA) under emulation [C]

The export runs the OEM's own Ansel balance from `oem/PakonIMAu.dll` in
Unicorn (32-bit x86) and adds each frame's shift to its RPD before
`rpd.pf` (`pakon-oem/src/sba.rs`). The balance core is plain C, so it runs
as-is with the structures the OEM C++ wrapper (`AnsSbaCapabilityImpl`) would
build; every import goes through a cdecl stub (calloc/free, setjmp, the x87
maths helpers). `pe.rs` maps the DLL at its preferred base, `emu.rs` binds
the stubs and calls functions.

Why the OEM code runs rather than a reimplementation: the balance logic is
code, not data, so a port would have to be reverse-engineered function by
function; emulating only the plain-C core needs neither a 32-bit Windows
runtime nor the DLL's undocumented C++ interface, and a future port can be
checked bit for bit against it.

| Step | Function | Inputs |
|---|---|---|
| decode the SBA tables | `SbaDecodePcode` `0x102884b0` | `oem/ansel/sba/Pcode/pcode-dls_1.7` |
| hue weighting table | `Makesfs` `0x102ac820` | `Sfs/sfsTable35`; out = object `+0x2c` |
| per frame: 24×36 sample grid | `createAlgData` `0x1028ceb0` | planar 16-bit RPD, `minDmin` aim |
| per frame: pass 1 | `Sba()` `0x1028b8d0`, mode 1 | DPI block (`FUN_10214e30`), command switches |
| roll: FOS | `SbaCalcFosResults` `0x1028f570` | first `maxFramesToFos` frames, reference `fpo` |
| per frame: pass 2 | `Sba()` mode 2, then `Preference()` `0x1028c780` | frame Dmin lowered to the roll's `fosDmin` |

The result is the shift at object `+0x3a38` (`getShifts`): R, G, B in RPD
codes. The shift both balances colour and sets each frame's density.

**FOS** (film-order analysis): the roll's film base is each channel's lowest
frame Dmin, and the near-neutral samples of all frames give the film's grey
axis (a closed-form 3×3 eigen solve in the opponent axes N = (R+G+B)/√3,
GM = (2G−R−B)/√6, ILL = (B−R)/√2). With fewer than `mff` (4) frames each
frame is balanced alone (`Sba()` mode 0, then `Preference`), as the OEM
does. The OEM's FOS (`FUN_1023ff80`) loads the scan history only when
`useHistory` is set; every shipped `.dpi` has it at 0. With `mff` frames or
more it runs `SbaCalcFosResults` and then updates the history
(`FUN_1023fd20`), which nothing reads back.
`dataPathItems/fos/fos-history-srcType-1.dpi` exists but is unused.

**What the OEM feeds Ansel** (`FN_bKcdfsCorrections`, `0x10034a60`, once per
strip): `PIAnselStartNewRoll(strip, mode)`, `PIAnselAddScene` per picture
(≤ 40), `PIAnselEndRoll`, `PIAnselAnalyzeRoll`. Each strip is its own Ansel
roll. The AddScene struct (`PakonIMAu` `FUN_10002290`) carries sourceType 1
(35 mm negative) and metric 1 (PD12) for modes 1–3, 3 (35 mm chrome) for
mode 4, 5 (camera) for mode 0; for negatives also `dmin` (the strip's film
base in RPD), `productCode` and `genCode` (the strip's DX code, −1 = none).
`PakonIMAu` `FUN_10001e70` maps the mode to a processing path: 0
"DC-Premium", 1/2 "CN-Enhanced", 3 "CN-Lockbeam", 4 "CP-Balance".

Ours, input by input:

- **Parameters from the DX code.** `sba.map` selects the `.dpi` on
  sourceType + productCode + genCode: 78/13 and 79/15 (chromogenic B&W),
  96/any and 43/any have their own (in `oem/ansel/sba/SbaDPI/`), everything
  else `sba-CN-default.dpi`. `sba::dpi_for` reads the map the same way (first
  matching row, `any` matches anything, no DX code matches only `any`),
  with the DX code from `<raw>.dx.json`. The command switches are the
  constructor's no-metadata defaults.
- **Scene `dmin`.** The OEM wrapper (`FUN_10218110`) sets the frame's film
  base to the lower of the scene `dmin` and the frame's own minimum per
  channel; a scene value at or below the `.dpi`'s `mindmin` counts as none.
  The OEM measures it with `FN_bFindTmax` (`0x10005ae0`, colour negative and
  B&W only): near the strip start (from 1 inch in [INF]; window length not
  traced), central ¾ of the width, per channel a 16384-bin histogram, Tmax =
  the level the brightest 0.1% of samples exceed; then through the LUT and
  NegMatrix. `sba::film_base` takes the same statistic (99.9th percentile,
  central ¾) on the clear film between the film's arrival (the first row
  whose green level falls below 90% of the open gate) and the first frame,
  1 mm in from each end, at least 2 mm long; `film_base_rpd` converts it.
  The OEM window starting 1 inch in would be inside frame 1 on a roll, so
  ours uses the leader. A fogged leader (a roll loaded in daylight) measures
  dense and changes nothing; on a clear leader most frames move 1–5 RPD, a
  few up to ~35. The prescan keeps the value with the roll.
- **Analysis image** (`analysis_image`): the frame rotated to landscape,
  area-averaged in raw space to 192 × 128 (centred), then 12-bit RPD. The
  OEM's analysis size is not traced.
- **The roll.** The export balances the confirmed frames against the
  prescan's frames (the whole roll), as the OEM balances against the roll,
  not against the pictures picked. The preview strip gets the roll's mean
  shift.

Not modelled: the SCP stage (configured on, applied before SBA), each strip
as its own Ansel roll, and the positive mode. Not yet compared against OEM
output for the same film. By eye (serial 3054): a roll whose film base was
off-nominal (red ~0.15 D low), which the roll balance below left strongly
blue, comes out neutral with natural skin and whites; a warm roll keeps its
colour and gains per-frame exposure.

`PAKON_OEM_SBA=0` turns the SBA off, `PAKON_SBA_DMIN=0` leaves the scene
`dmin` out. If the DLL or its data is missing, or the emulation fails, the
roll balance is used.

### Roll balance (fallback) [C]

`balance::roll_balance`: a roll-level two-point balance in RPD, measured once
per roll on the prescan's downsampled ribbon and applied to every frame as
`RPD · gain + offset`:

```
base    = RPD of the film base (Dmin, below)
med[c]  = median RPD of picture pixels (darkest 20% by luminance and any channel ≥ 4000 left out; ≥ 100 pixels)
target  = med[c] + s · (mean(med) − med[c]),   s = OEM_BALANCE = 0.5
gain[c] = (target − base[c]) / max(med[c] − base[c], 100)
offset  = base[c] · (1 − gain[c])
```

The film base stays put and the roll's picture median moves half way to
neutral, a per-channel affine like Ansel's SCP stage. Without any balance
the output leans blue (the NegMatrix offsets put blue ~500 RPD above red). A
per-frame neutral balance turns a blue sky white, a plain per-channel offset
leaves blue shadows, and making the film base neutral as well pushed one
roll blue and another cream, so `rpd.pf` appears to expect an unbalanced
base [INF]. Chosen by eye on two rolls (serial 3054).

**Dmin** (`balance::dmin`, `measure_dmin`): per channel, the median over 12
row bands of 1500 rows of each band's 99.5th percentile of the samples below
63000 (a whole-roll percentile is dominated by blown highlights), at least 1.

### The OEM Ansel cascade (context)

`PakonIMAu.dll` is built on Kodak's Ansel library (classes
`CiColorCorrectionAnsel`, `AnsLut`, `AnsImaBuilder`). It is data-driven: ~48
stages of ASCII LUT and parameter files (333 files in the install, under
`anselinstalldir/dataPathItems/<stage>/` and `Config/ColorCorrection/`):

| # | Stage | Role |
|---|---|---|
| 1 | `filmLut/` | film density → scene; ships as identity |
| 2 | `SCPLut/` | Scan Color Processing: a per-channel affine `out = i·slope − offset` (`AnsSCPLut`, `FUN_10287eb0`), a balance step, not the inversion |
| 3 | `dsba/`, `sba/` | Digital Scene Balance: per-scene colour and density balance |
| 4 | `flesh/` | flesh-tone correction |
| 5 | `toneHelper/`, `contrast/`, `lighting/` | tone rendering per path (`CN-Enhanced`, `CN-Premium`, `DC-Premium`, `CP-Balance`) |
| 6 | `fugc/`, `deRender/`, `reRender/` | gamut, RIMM/ROMM colour-space management |
| 7 | `Config/ColorCorrection/*.pf` | output colour space and effects |

The OEM export exposes three toggles (PSI "Other Options"): **Color
Correction (12-bit RPD)** (`rpd.pf`), **Color Scene Balance (8-bit sRGB)**
(SBA/DSBA), and **Color Adjustments** (sliders, neutral by default). Ours
reproduces the first two and the adjustments without the rest of the
cascade.

## Per-picture adjustments [C]

`TLB.dll` `FN_bApplyColorAdjustments` (`0x10024010`) builds the argument
block for `PakonIMAu` `PIColorAdjustPlanar` (`0x10013bc0`). Six controls are
each the sum of an automatic and a user value (picture `+0x08…+0x1c` plus
`+0x20…+0x34`): red, green, blue, brightness, contrast, sharpness; then the
B&W effect (`+0x4c`), saturation (`+0x50`), the film mode and a per-picture
option. On 12-bit RPD:

| Control | Arithmetic | One TLX click |
|---|---|---|
| Contrast | table `(i − 1550) · (1000 + contrast/2) / 1000 + 1550`, clipped | 25 = 1.25% of slope |
| Red / green / blue + brightness | `(channel + brightness) · 4096 / 4000` added to that channel | 25 = 25.6 RPD (~0.025 D) |
| Sharpness | `ImaUnsharpMaskOperation`: `x + amount · (x − blur)`, separable 3×3 `[¼, ½, ¼]` blur, `amount = sharpness × 0.01`, after the tables | ~67 (3 clicks = 200) |
| Saturation (−5…5) | picks `satminus15.pf` … `satplus15.pf` (step × 3) | |
| B&W effect (1 warm, 2 cold, 3 sepia) | `warm_bw_ld0_1_4-5.pf`, `cold_bw.pf`, `sepia_ld0_9_22.pf` | |

The saturation and effect profiles are abstract Lab → Lab profiles chained
between the input profile and the output. `Defaults.ini` (sections by DX
product number, `BnW`, `POSITIVE`, `IMPORTED`; read by `FUN_10026210`) gives
each film's starting values, but the shipped file lists every product with
no values, so OEM output carries no per-film offsets; its comment gives the
click units above.

Ours (`colour.rs` `Adjust`, `adjust.rs`): the same arithmetic in the order
contrast → scene-balance shift → offsets → unsharp mask. The automatic
halves are taken as 0 (where `TLB.dll` fills them is not traced). Values are
clamped to ±1000 (saturation ±5). Saturation and tone are applied as a
correction to the `rpd.pf` table (`adjusted_table`): our own float
evaluation of the input profile → Lab → the profile chain → sRGB (D50,
Bradford), minus the same without the chain, added to the LittleCMS table.
An unadjusted table is therefore untouched; our evaluation and LittleCMS's
differ by a few levels, mostly outside sRGB. Slides get the same
arithmetic on their 12-bit ROMM values, after the `ColRevLut1` link and
before `romm.pf` (`render_slide_adjusted`), where the OEM runs
`PIColorAdjustPlanar` for film type 2.

**Sharpening.** The app exposes one control for the whole export
(`pakon-roll` `SHARPEN_LEVELS`): off / low / medium / high = 0 / 67 / 133 /
200, i.e. 1 / 2 / 3 TLX clicks (amount 0.67 / 1.33 / 2.0). At Base 16 the
3×3 mask mostly lifts the grain; high is strong.

## Fallback path without a NegMatrix [C]

Used when the scanner's EEPROM archive is missing or does not decode, or
`rpd.pf` is missing (`fallback.rs`):

- **`invert_c41`** (the positive, written to the TIFF): `x = clip(raw / Dmin ×
  16383)` per channel (Dmin from `balance::dmin`, which removes the orange
  mask and white-balances) → the density LUT → sRGB encode of `LUT / 16383`
  → ×4 to 16-bit. No crosstalk matrix and no grey world: the
  `_ClientColNegMat` offsets over-subtract G/B in this scale (red cast), and
  grey world strips intentional scene warmth (blue lean). Dmin
  normalisation alone matched OEM reference scans of two rolls.
- **`render_jpeg`** (the JPEG look): the 8-bit positive through the `rpd.pf`
  table, quantised to 8 bits, then per-channel auto-levels between the 0.5
  and 99.7 percentiles, a midtone lift `x^(1/1.5)`, and a soft shoulder
  above 0.85, `x' = k + (1 − k)(1 − e^−(x − k)/(1 − k))`, instead of clipping.
  This feeds `rpd.pf` an sRGB positive, which is not what the profile
  expects; it is a look, not the OEM path.
- B&W: `invert_c41` then Rec. 709 luma.

## Digital ICE (dust and scratch removal)

The scanner captures the IR channel with its own LED, current and exposure
(`WaitForLamp_Ir`, `Current_Ir`, `CcdExposure_Ir`). The OEM applies Digital
ICE through `DMLDICELib.dll` (loaded by `CN_CiDLLDigitalIce`; PSI "Use
Scratch Removal"; settings `IrChannelSavedInPlanarFile`,
`IrCrossTalkFactor`). Dust removal runs on the registered, flat-fielded raw
crop, before rotation and colour.

### The IR plane [C]

`ice::ir_plane`: the IR block of each marker-aligned row, minus the 300
black, times a per-column gain so the calibration's open-gate IR reads 0.9
× 65535; when the scan ran another IR duty than the calibration (slides,
B&W: ~1.2×) the duty ratio is taken out. It is stored with the ribbon as
16-bit. Raws without an IR block (silver B&W) cannot be cleaned.

### OEM Digital ICE under emulation [C]

`DMLDICELib.dll` is Applied Science Fiction's "Digital ICE Library (DML
version)" 1.0.0.0. It links the MSVC runtime statically and imports only
KERNEL32. Exports: `DICEVersion`, `DMLDICEBegin(ulong, DICEInfoStaticTag)`,
`DMLDICEProcess(void*, DICEInfoDynamicTag)`, `DMLDICEDefectCount`,
`DMLDICEEnd`. `TLB.dll` (`FUN_10012ca0`) calls `Begin(0x65, static {0x20, 2,
1, 0xe, 2, 0, 0, 0})`, and in `FN_bLoadImageFromBuffer`, when the picture
has IR, before the density LUT: `Process(handle, dynamic {0x30, planes,
width, height, 300, negative = (film type ≠ 2), 0, 0, 0, width − 1, height −
1, 0})`. Errors −1…−9 map to `EC_DICE_*`.

`pakon-oem/src/dice.rs` runs it in Unicorn the same way: the planes go in as
14-bit R, G, B, IR back to back, `(raw − 300) / 4`; only the pixels DICE
changes are taken back into the 16-bit frame (`v × 4 + 300`). Every import is
a stdcall stub, the runtime's `DllMain` runs first, `fs:[0]` points at a
thread block, and the DLL's worker threads run on the spot when
`WaitForSingleObject` waits for them. One emulator per export thread
(`with_thread_dice`). `PAKON_OEM_DICE=0` (or a missing DLL;
`PAKON_DICE_DLL` points elsewhere) falls back to the native port.

Measured against the native port on the same frames: with dust added to a
real frame, DICE left 766 of 8586 defect pixels (native 1084); on a clean
roll DICE changed 0.00–0.01% of pixels where the native port changed
0.13–0.25%, softening foliage.

**Slides get no dust removal**: on slide film both DICE and the native port
smoothed the grain over most of the frame, and DICE's negative flag
(dynamic `+0x14`) changed nothing.

### Native port [C]

`pakon-image/src/ice.rs` ports nkscan's `dust.rs` (MIT/Apache-2.0; the
openICE description of Digital ICE as tuned for the Nikon Coolscan LS-5000)
and keeps its constants. The Pakon-specific parts:

- the IR plane above; resolution `dpi = width / (24 mm / 25.4)`: 2117 /
  1587 / 1058 at Base 16 / 8 / 4;
- gate metering `PAKON_METERING = 0.985` (a ~3.5% margin; Nikon's ~2% also
  flagged noise and edges), with the confidence spread over rows too;
- a per-column margin: the first ~40–60 columns of every line have
  amplified, heavy-tailed IR noise (the gate shades the line start: open
  gate 0.26× at column 0, noise 2.6–3.8×, dark tail 3–6×). The margin is
  widened per column by the larger of the calibration's light fall-off
  (`ir_noise_factor`: 0.9 / (open gate / its 90th percentile), ≥ 1) and the
  frame's own dark tail (`ir_tail_factor`: the 0.5th percentile of each
  column's deviation from a 5-row median, relative to the middle of the
  line, median over 15 columns, 1…4). This cut the edge share of rebuilt
  pixels from 15–36% to 7–22% with the middle unchanged;
- rebuilt pixels only rise (the result is the max of the rebuild and the
  original), in a log density domain `D(v) = 65535/16 · log2(v + 1)`.

Measured: the red → IR dye leak is 0.014 on slides and 0.040 on C-41; hairs
and scratches are removed with 0.1–0.5% of pixels rebuilt on colour
negative; on B&W C-41 (XP2, Base 16, 36 frames) 0.31–0.80% per frame, 1–4%
of it in the first 3% of columns, mostly 3–20 px specks that at 1:1 are
grain replaced by grain. The cause of the line-start dark tail is unknown.

## Output

### Per frame [C]

`pakon-roll` `export_frames`, frames in parallel:

1. Crop the cached ribbon at the box (full sensor width).
2. Dust removal on the raw crop (OEM DICE, else native; not for slides).
3. Rotate by the requested multiple of 90° (the app sends 90°).
4. Render: slide path, OEM path (with SBA shifts or the roll balance, and
   the adjustments), or the fallback.
5. Write:

| File | Content |
|---|---|
| `frame_NN_raw.tif` | the registered, flat-fielded, dust-cleaned, rotated **negative**, 16-bit linear RGB (black ~300), for inverting in another tool |
| `frame_NN.tif` | the 16-bit positive: sRGB from the `rpd.pf` / `romm.pf` table (OEM and slide paths), or the `invert_c41` positive (fallback); one channel for B&W |
| `frame_NN.jpg` | 8-bit (`v >> 8`) of the TIFF, quality 92; in the fallback, the `render_jpeg` look |
| `thumb_NN.jpg` | every `step`-th pixel so both sides are near 600 px, quality 85 |

The raw scan itself stays as written by the scanner, with its
`<raw>.flat.json` and `<raw>.dx.json`.

### TIFF [C]

`output::write_tiff16`: uncompressed baseline TIFF, little-endian, 16 bits
per sample, one strip; RGB or grey. With a known DX film, the film name as
`ImageDescription` (270) and an XMP packet (700).

### JPEG [C]

`output::jpeg_bytes` uses the `jpeg-encoder` crate with libjpeg's colour
handling, so the result matches what Pillow/libjpeg produce:

- RGB → YCbCr with libjpeg's 16-bit fixed point (`jccolor.c`):
  `Y = (19595 R + 38470 G + 7471 B + 0x8000) >> 16`; Cb and Cr with
  `+ (128 << 16) + 0x7FFF` rounding.
- 4:2:0 chroma as libjpeg's `h2v2_downsample`: each 2×2 block's average
  with the alternating bias 1, 2, 1, 2 along the row, edges replicated.
  Keeping one sample per block instead (the crate's default) aliases film
  grain into colour noise; with the box filter the exported JPEGs match
  Pillow's error against the TIFF to 0.001 code and its file size within
  0.1% (the one-sample version: 4–8% more error, 8% larger).
- With a known DX film: an APP1 EXIF block (`ImageDescription`,
  `ISOSpeedRatings`) and an APP1 XMP packet (`dc:description`, `dc:subject`,
  `exif:ISOSpeedRatings`).

The bytes differ from libjpeg's; the decoded pixels round-trip.

### Preview, contact sheet, metadata

- **Preview strip** (prescan): the ribbon decimated by `round(width / 500)`,
  rendered with the roll balance (or the roll's mean SBA shift), rotated so
  the roll runs horizontally, JPEG quality 85.
- **Contact sheet** (`output::contact_sheet`): the frame JPEGs tiled 6
  across in 480 px cells at the first frame's aspect, Lanczos-3 resized, on
  a dark background with a "Frame NN" label under each; JPEG quality 90.
- **Film metadata** (`film.rs`): the DX product and generation read during
  the scan (`<raw>.dx.json`) are looked up in
  `oem/ansel/common/common-ProdCodeTable.dpi` (DX product/generation → ISO,
  with a product-line label per product, revised to 2006). The label is
  title-cased with a few spelling fixes (Illford → Ilford, Fujicolohr →
  Fujicolor, Kodakcolgold → Kodacolor Gold). An unknown code writes
  nothing.

## Colour profiles and tables

Kodak/Pakon data from the OEM install's `Config/ColorCorrection/`, kept in
`profiles/` (see `profiles/README.md`); the OEM DLLs and Ansel data in
`oem/` (see `oem/README.md`). All are used as shipped. The repo keeps only
the files the code reads; the others below are in the OEM install
(`Config/ColorCorrection/`) and at git tag `python-final`.

| File | What it is | Used for |
|---|---|---|
| `rpd.pf` | input profile, 12-bit RPD → Lab (lut16; `RPD_dls_3` + `yellow5`) | negatives and B&W; embedded as `assets/rpd.srgb52.u8`; read at run time for saturation and tone |
| `romm.pf` | ROMM RGB input profile, matrix/TRC (no lut16 tag) | slides (`CRInputProfile`); embedded as `assets/romm.srgb52.u8` |
| `ColRevLut1.pf` | device link "Pakon CMY to ROMM12", lut16, 31³ | slides, PosMatrix output → 12-bit ROMM |
| `srgb.pf` (not kept) | sRGB IEC61966-2.1 | the OEM output profile; ours uses LittleCMS's built-in sRGB |
| `satminus03…15.pf`, `satplus03…15.pf` | abstract Lab → Lab | saturation −5…5 |
| `warm_bw_ld0_1_4-5.pf`, `cold_bw.pf`, `sepia_ld0_9_22.pf` | abstract Lab → Lab | B&W tone |
| `unity.pf` (not kept) | Kodak Lab output space | unused |
| `ColRevLutS6.lut` (not kept) | 4096 × 12-bit curve | unused; the OEM loads it but never reads it |
| `_ClientColNegLut.txt` (not kept) | the density curve | provenance of `c41_lut` |
| `_ClientColNegMat.txt` (not kept) | F-235 3×3 + offset matrix | unused |
| `Defaults.ini` (not kept) | per-film slider defaults (all empty) | provenance of the click units |

**The embedded tables.** `pakon-image` links no colour-management library.
The `rpd.pf` and `romm.pf` → sRGB tables are LittleCMS's 8-bit transform
(perceptual intent, built-in sRGB output) sampled on 52³ nodes at 0, 5, …,
255, stored as bytes and divided by 255 at load; they were generated with
Pillow's ImageCms by `rust/tools/tables.py` (at git tag `python-final`) and
must be regenerated if the profiles change. Trilinear lookup weights are in
f64.

**LittleCMS 8-bit vs our float evaluation.** Where the pipeline evaluates a
profile itself (saturation and tone corrections, the `ColRevLut1` link), it
works in float from the ICC tags. `romm.pf` has no lut16 tag, so for slide
saturation and tone it is evaluated through its matrix/TRC, within 0.003 of
the LittleCMS table. The fallback JPEG look runs through the 52³ table where
the Python pipeline ran LittleCMS's 8-bit transform per pixel, so it differs
by a mean 0.43 codes.

## Verification

The Rust pipeline replaced a Python/numpy one (kept at git tag
`python-final`). It repeats numpy's float arithmetic (f32 where numpy stays
f32, f64 where it promotes, numpy's percentile, median and pairwise-sum
rounding; `stats.rs`) and is checked against outputs of the Python pipeline
on a real roll (`pakon-image/tests/golden.rs`, `pakon-oem/tests/dice.rs`,
`pakon-oem/tests/sba.rs`, `pakon-roll/tests/roll.rs`; they need
`PAKON_GOLDEN=DIR` and skip otherwise, since raws are gigabytes).

| Part | Result |
|---|---|
| decode, registration, flat field | bit-exact |
| OEM colour (LUT, NegMatrix, adjustments, unsharp, `rpd.pf` table, grey) | bit-exact |
| Dmin, roll balance | bit-exact |
| slide (exact density, `ColRevLut1`, `romm.pf`, gaps, flare) | bit-exact |
| framing, all formats, both polarities | exact |
| saturation / tone tables, slide adjustments | bit-exact |
| IR plane, native ICE | bit-exact |
| OEM DICE and SBA under Unicorn | bit-exact |
| fallback `invert_c41` | bit-exact |
| fallback `render_jpeg` | mean 0.43 codes (see above) |
| prescan and export of a whole 36-frame roll (DICE, SBA with FOS and scene film base, adjustments, B&W) | every TIFF bit-identical |

Measured speed (Apple M4 Pro): decode of a whole 1.9 GB roll 2.0 s; OEM
render 38 ms per Base 16 frame; native ICE 0.7 s and OEM DICE 1.9 s per
frame; SBA roll stage 0.3 s; a 36-frame roll prescans in 8.7 s and exports
in 32 s. Memory per frame being exported: ~0.35 GB without dust removal,
0.48 GB (full frame) to 0.68 GB (panorama) with DICE; frames run at once up
to the core count while each fits in half the RAM (`workers_for`, budget
0.5 / 0.75 GB; `PAKON_EXPORT_THREADS` overrides).

## Environment switches

| Variable | Effect |
|---|---|
| `PAKON_LINEWIDTH`, `PAKON_IR_LANE` | force the row layout |
| `PAKON_FLAT=0` | no flat field |
| `PAKON_EEPROM_DIR`, `PAKON_DATA` | where the EEPROM archive (NegMatrix, PosMatrix, flat fields) is found |
| `PAKON_OEM_SBA=0` | roll balance instead of the OEM scene balance |
| `PAKON_SBA_DMIN=0` | no scene film base for the SBA |
| `PAKON_OEM_DICE=0`, `PAKON_DICE_DLL` | native dust removal / another DICE DLL |
| `PAKON_EXPORT_THREADS` | frames exported at once |

## Open questions

- **Dust in the calibration gate**: detect and repair it automatically (the
  bright-line interpolation, or the OEM's `CLEANING_REQUIRED` test), or
  refuse the calibration.
- **SBA against OEM output**: no TLX reference scan of a strip we can
  rescan yet; SCPLut, the OEM's analysis image size, per-strip Ansel rolls,
  and the positive mode are not modelled.
- **Ansel mode for B&W**: one `TLB.dll` path gives the balance mode word as
  1 for odd film types and 4 otherwise (so 4 for silver B&W and B&W C-41),
  another maps colour negative and both B&W types to 1/2/3 and positive to
  4. Which call governs B&W is not resolved; ours treats B&W like colour
  negative.
- **Where the OEM goes grey** for B&W, and where it fills the automatic
  halves of the adjustments.
- **F-135+ trilinear leads**: measured leads were implausible on the Plus
  captures; whether its firmware already line-aligns.
- **Line-start IR dark tail**: cause unknown (a dark scan would tell).
- **`FN_bFindTmax` window**: its length, and the inputs to its start row.
- **The emulator's licence**: Unicorn is GPL-2.0 and is linked into the
  AGPL-3.0 app; options are a separate GPL-2.0 helper process, a linking
  exception, or a build without the OEM path.

## Provenance

The OEM facts come from reverse engineering the original Kodak/Pakon
Windows software for interoperability: Ghidra decompilation of `TLB.dll`
(the F-135 engine), `TLA.dll` / `TLC.dll` and `PakonIMAu.dll`, and
inspection of `Config/ColorCorrection/` and the Ansel data. No Kodak code is
reproduced. The OEM binaries the pipeline runs (`PakonIMAu.dll`,
`DMLDICELib.dll`) and their data are committed unmodified under `oem/`; the
ICC profiles and ColNeg data under `profiles/`. The profiles, the Ansel data
and `PakonIMAu.dll` are © Eastman Kodak Company; `DMLDICELib.dll` is Applied
Science Fiction's.
