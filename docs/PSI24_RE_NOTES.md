# PSI 2.4 C-41 render: reverse-engineering findings

What the real Pakon PSI 2.4 produces for a C-41 negative, how that was decoded from `PakonIMAu.dll` (PSI 2.4,
`oem/psi24/`), and how closely the Rust port reproduces it. This is the current state only; rejected
experiments appear as short "tried, worse" bullets.

Code: `rust/crates/pakon-image/src/{colour,psi24,ebp}.rs`, tables in `rust/crates/pakon-image/assets/`,
tools in `rust/crates/pakon-image/examples/`. KCDFS (SCP LUT and shifts) is `rust/crates/pakon-oem/src/kcdfs.rs`.

Conventions: "exact path" is the bit-exact 8-bit render (`psi24::render`); "HQ" is the float render
(`psi24::render_hq`, `render_hq_f32`). "MAE" is the mean absolute error in 8-bit sRGB levels against the real DLL's
output on the same input, averaged over R, G, B. DLL addresses are in `PakonIMAu.dll` (image base `0x10000000`)
unless stated; they come from the Ghidra decompile.

---

## 1. Pipeline

```
raw 16-bit (3 planes)
  -> RPD 12-bit            c41_lut + 3x10 EEPROM negative matrix            (colour::rpd_adjusted)
  -> SCP LUT + KCDFS shift out = clamp(clamp(lut[rpd]) + shift, 0, 4095)    (per channel)
  -> Kodak CMM             rpd.pf combined with srgb.pf, 65^3 sampled table, tetrahedral  (psi24::pre_ebp)
  -> 8-bit sRGB
  -> EBP (8-bit)           Gain & Offset -> Gamma -> Noise Filter -> Black Print          (ebp.rs)
  -> output
```

- **raw -> RPD** is bit-accurate: `colour::c41_lut()` plus the unit's 3x10 EEPROM `negative_matrix` reproduces the
  DLL's RPD dump to 0.14 codes (mean absolute, of 4095). PSI 2.4 uses the same `c41_lut` and per-unit matrix as PSI 3.
- **SCP LUT and shifts** come from KCDFS (section 5). The LUT is a 3 x 4096 `u16` table per roll; the shift is one
  signed value per channel and frame. Scaling the shift by a factor has a sharp minimum at 1.0, so the shift enters
  exactly as `clip(clip(LUT) + shift)`. In `colour::rpd_adjusted` the LUT is interpolated linearly between entries.
- **Kodak CMM** (`kodakcms.dll`, `SpCombineXforms(rpd.pf, srgb.pf)`): the whole RPD -> sRGB conversion is one
  transform (both profiles are `mft2` 16-bit LUTs). It is stored as a sampled 65^3 table (section 2).
- **EBP** (EnhancedBlackPrinting, `ImaEBP*Op` classes) runs on 8-bit planar sRGB, using the `uchar` template
  instances. The chain order is Gain and Offset, Gamma, Noise Filter, Black Print, with every attribute at its
  constructor default (only `padValue` is set). The 12-bit instances (`0x10183e40`, `0x10180dc0`) are never called on
  the C-41 path. Without EBP a render has about 1.3x too much contrast and a shadow toe.
- Scene-balance dispatch: for C-41 (`film = 1`) `PIColorSceneBalanceColNegPlanar` (`0x10005860`) takes the EBP
  branch; the baked SCP LUT + shift step is `FUN_10001fe0`; contrast is `1 + p[9]/1000` (identity at default sliders);
  the roll-level calls (`PIKcdfsStartNewRoll` `0x1005ced4` ... `PIKcdfsGetShift_B` `0x1005ce70` string addresses in
  `TLB.dll`) are driven from the roll balance runner at `TLB.dll 0x10034550..0x1003497f`.
- `TLXClientDemo` passes all picture colour settings (Red, Green, Blue, Brightness, Contrast, Sharpness) as 0 and
  `Config/ColorCorrection/defaults.ini` has no per-film overrides, so factory defaults mean no adjustments.

## 2. The Kodak CMM table

The Windows harness `psi24-cmm` loads the real `kodakcms.dll`, repeats the logged call sequence
(`SpProfileLoadProfileW` for `rpd.pf` and `srgb.pf`, `SpXformGet` intents 1 and 2, `SpCombineXforms`) and calls
`SpEvaluate` on its own grids. As a cross-check, all 3440 `cmm_in -> s0` pairs captured from real renders reproduce
exactly (0 mismatches).

`SpEvaluate` output types: 1 = u8 (what the render uses), 4 = u16 in 12-bit range, 5 = u16 full range; 0, 2, 3, 7, 8
are rejected, 6 is not interpretable. Type 5 is not `u8 * 257`, and `round(u16 / 257) == u8` for only 92 % of colours.

| table | file | use |
|---|---|---|
| type 1, u8 | `assets/cmm65_u8.u8` | exact path and the 8-bit decisions of the HQ path |
| type 5, u16 | `assets/cmm65.u16` | float planes of the HQ path |

Both are 65^3 grids, R-major, inputs `round(i * 4095 / 64)`, i.e. the 12-bit `cmm_in` of the previous step, output
sRGB. `assets/make_psi24_tables.py` documents how they were produced. Type 5 sits +0.5..+0.9 u8 levels above type 1 in
the shadows, which is why the exact path must use the u8 table.

Lookup is tetrahedral (`Table::lookup_tetra`: six tetrahedra per cell along the main diagonal, four corners),
coordinate `x / 4095`.
- Tetrahedral against trilinear: roll A exact MAE 0.909 -> 0.771, roll B 0.979 -> 0.876; probe suite flat. The gain
  comes from the transform's geometry (a smooth neutral diagonal, no clipped off-diagonal corners near the gamut kink),
  not from the DLL being known to interpolate tetrahedrally; its own interpolation is not known.
- Tried, worse: warping the input to the exact node positions `round(i * 4095 / 64)`: roll A 0.926 -> 0.941, roll B
  0.975 -> 1.000, suite 0.62 -> 0.71.
- Tried, worse: the earlier LittleCMS `rpd -> romm -> srgb` pair plus fitted per-channel 1D corrections: on the 3440
  exact pairs MAE 1.75 / 1.18 / 1.08 against 0.19 / 0.27 / 0.31 for the sampled table. Removed.

Properties of the transform: it clips hard (neutral reaches 255 near input 2600, saturated channels clip to 0) and
is not monotonic near black on the neutral axis (the u8 table dips for R near `x = 0.098`).

## 3. EBP stages (8-bit, `ebp.rs`)

Plain `uchar` attribute defaults: black 0.10 %, white 0.15 %, shadow/highlight 0.06 with thresholds 5 / 250, Gamma
mean limits 39.06 / 48.82 (%), citras 0.5; Black Print luma 0.5, chroma 0.25, luma max 50, chroma max 20, ratio 0.4.
Planes between stages hold `level << 4` in the port; each stage rounds back to 8 bits like the DLL.

### 3.1 Gain and Offset (`ImaEBPGainAndOffsetOp`, 8-bit instance; LUT builder `0x10181bd0`)

1. Histogram per channel of the image blurred with `[1 4 6 4 1]` horizontally then vertically, each pass `>> 4`
   (truncating); bit-exact against dumped histograms.
2. `nb = trunc(n * 0.0010 + 0.5)`, `nw = trunc(n * 0.0015 + 0.5)`. `lo` is the first bin where the cumulative count
   reaches `nb`, `hi` the last bin from the top reaching `nw`. A channel is empty only when `hi <= lo`.
3. White: `avg = sum(hi) / 3` (integer). If `hi_R <= avg`: `hi_c = (hi_c + avg) / 2`. Otherwise (the `bVar2` branch):
   `hi_c = ((hi_c + hi_R) / 2 + 255) / 2`. Black: if `(lo_R + lo_B) >> 1 < lo_G` then `lo_G = (lo_R + lo_B) >> 1`.
4. Ranges `r = hi - lo`; if the narrowest range is below 128, ranges are widened to `(r * 128 + min/2) / min` (cap 255),
   so the stretch is capped near 2x. Stage-1 gain `s = 255 / r`, offset `lo`. Stage-1 LUT `round((v - lo) * s)`.
5. Luma histogram of the stage-1 image (unblurred), bin `(3 + 2 * (R + G + B)) / 6`. `lo2` is found with a centred
   three-bin window `h[b-1] + h[b] + h[b+1]` reaching `trunc(n * 0.06 * 0.03 + 0.5)` from `b = 3` up; `hi2` the same
   from `0xfc` down (count halved in the `bVar2` case).
6. If `(lo2 <= 5 && hi2 >= 250) || hi2 < lo2 + 63.75`, only stage 1 applies. Otherwise the knee: target `tl = lo2 * 2/3`
   when `lo2 > 5` (else `lo2`), `th = (hi2 * 3 + 255) >> 2` in the `bVar2` case else `(hi2 + 255) / 2`; slope
   `k1 = (th - tl) / (hi2 - lo2)`, `k2 = (255 - th) / (255 - hi2)`; below `lo2` the curve scales by `tl / lo2`. The
   knee is evaluated on the 0..255 scale and rounded once.
7. `p = k1 * 3 * 255 / sum(r)` is the effective gain that Gamma reads (the Gamma builder's `+0x10` double).

The DLL rounds (`+0.50000000001`) at all three `ftol` sites (`0x101824e0`, `0x1018267e`, `0x10182807`).

### 3.2 Gamma (`ImaEBPGammaOp`; build `0x10181245..0x10181405`)

Per pixel: `L = (R + G + B + 1) / 3`; `c' = lutL[L] + trunc_toward_zero((c - L) * gainL[L] / 4096)`, with
`lutL[i] = floor(255 * (i / 255)^g + 0.5)` and `gainL[i] = floor(((slope - 1) * 0.5 + 1) / p * 4096 + 0.5)`,
`slope = (i / (255 p))^(g-1) * p^g * g`.

The exponent is a histogram statistic, not spatial:
1. Luma `l8`, then `b5` = `[1 4 6 4 1]` blur of `l8 << 7` (Q7, `>> 4` per pass). Bins are `(b5 + 64) >> 7`.
2. Drop the two outermost non-empty bins at each end; `mean` and `median` come from the rest. If the remaining span is
   63.75 bins or less, `g = 1`.
3. `g2` (dark): `ln((3 med + t_hi) * mean / (3 med + mean) / 255) / ln(mean / 255)` with `t_hi = ceil(48.82 * 2.56) = 125`;
   used alone when `mean <= t_lo = ceil(39.06 * 2.56) = 100`, absent (`g = g1`) when `mean >= t_hi`.
4. `g1` (bright): `b35` = `[1 2 1]` blur of `b5` (`>> 2` per pass), `diff = b35 - b5`,
   `sigma = sqrt(sum(diff^2) / (n - 1))`, `thr = trunc(0.75 sigma + 0.5)`; edge pixels `|diff| > thr`; `e` = mean of
   `(b5 + 64) >> 7` over edge pixels / 255; `g1 = ln((0.4882 + e) / 2) / ln(e)`.
5. In between, `g = ((mean - t_lo) g1 + (t_hi - mean) g2) / (t_hi - t_lo)`. Clamp to `[0.7, 2.0]`.

Gamma struct after build: `+0x50` = g1, `+0x58` = g2, `+0x60` = exponent used, `+0x68` = trimmed mean. The preview
helper `psi24::GAMMA_CAP = 2.0` (the DLL's own clamp) caps the estimate for low-resolution previews (the edge statistic depends on
resolution); the verification tools use an uncapped estimate.

### 3.3 Noise Filter (`ImaEBPNoiseFilterOp<uchar>`, exec `0x1016c830`, vtable `0x10284a04`)

Attributes `+0xc4 = 20.0`, `+0xc8 = 2.0`, `+0xcc = 10.0`. Image-adaptive:
1. Noise estimate `nv` (float at struct `+0xe4`): `sum((d*d + 32) >> 6) / (n*64 - 64)` over the interior, with
   `d = 8c - sum of the 8 neighbours` (3x3 Laplacian) of `R - B`.
2. `Y = (R + G + B) / 3`. If `nv > 0.25`, `Y` is blurred with `[1 1 1; 1 8 1; 1 1 1] >> 4` (truncating, edges replicated).
3. Detail `hp3 = Y - lp3(Y) + 128`, `hp5 = Y - lp5(Y) + 128` (byte). `lp5` is `[1 4 6 4 1] >> 4` per axis. `lp3` is
   `[1 3 1] / 5` horizontally, and vertically the DLL (`FUN_10170b90`) overwrites its row buffer before reloading it,
   so rows `(y-1, y, y+1)` weigh `[1, 2, 2] / 5` (a 0.2 px shift). The code does exactly this. Every pass truncates.
4. `Y' = Y + trunc(core(hp3) * A + 0.5) + trunc(core(hp5) * B + 0.5)`, soft coring `core(d) = d - X` for `d >= X`,
   `d + X` for `d < -X`, else 0, with `X = min(trunc(20 nv + 0.5), 10)`, `A = min(0.4 + 2 nv, 1.5)`, `B = min(0.2 + 2 nv, 1)`.
5. `C1 = (2G - R - B) / 4 + 128`, `C2 = (B - R) / 2 + 128` (C division) from the unblurred RGB, blurred with the 3x3
   kernel of step 2, then smoothed with `m = trunc(10 nv + 0.5)`: `m < 3` `[1 3 1] / 5`; `m = 3` binomial 3 `>> 2`;
   `m = 4..5` binomial 5 `>> 4`; `m >= 6` binomial 9 `>> 8` (horizontal then vertical, truncated, edges replicated).
6. `R = Y' - (4 c1 + 3)/6 - c2`, `G = Y' + (8 c1 + 3)/6`, `B = Y' - (4 c1 + 3)/6 + c2`, clamped (`c1, c2 = chroma - 128`,
   C division).

Real values seen: nv 0.27 to 0.60 on roll 1 (2250 x 1500) and 0.29 to 4.0 at 3000 x 2000. Not modelled: the DLL's first
column or row of the 5-tap filter weighs 12:3:1 instead of 11:4:1 (no measurable effect).

### 3.4 Black Print (`ImaEBPBlackPrintOp<uchar>`, exec near `0x101663ec` / `0x1016643a`)

`M` is the mean over all samples (the DLL sums it over all tiles on the x87 stack; it runs in strips but `M` is global,
confirmed at 3000 x 2000). `LT = clamp(round(0.5 M), 1, 50)`, `CT = clamp(round(0.25 M), 1, 20)`; truncating instead of
rounding gave only 82 % exact on one frame.
- `l` = luma of the image blurred with `[1 1 1; 1 8 1; 1 1 1] / 16` (rounded: `+8` before `>> 4`), `wgt = lut1[l]`,
  `lut1[v] = clamp(floor((LT - v) * 4096 / (LT * 0.4) + 0.5), 0, 4096)`; skip pixels with `wgt = 0`.
- `e = sum_c |l - raw_c|`, `q = lut2[e]` with the same form using `CT` (384 entries).
- `i13 = ((4096 - q) * wgt + 0x800) >> 12`, `s2 = (((q * l + 0x800) >> 12) * wgt + 0x800) >> 12`,
  `out_c = ((raw_c * (4096 - wgt) + 0x800) >> 12) + s2 + ((blur_c * i13 + 0x800) >> 12)`.

## 4. How each stage was verified

A Windows host runs the real `PakonIMAu.dll` offline with hooks, writing whole-image dumps before and after each EBP
stage (`s0` = CMM output, `s1`..`s4` after each stage, `look` = final) plus the stage structs. `psi24_stage` runs our
stage on the real previous stage and compares with the real next stage, which isolates each stage from upstream error.

| stage | check | result |
|---|---|---|
| Gain & Offset | `psi24_stage go`, real s0 -> real s1 | 100.0 % exact on all 13 frames with data (roll 1 frames 1-4, roll A 11/12/15/17/21, roll B 17/21/23/26) |
| Gamma | our exponent against the DLL's `+0x60` | g2 and the blend exact on all frames; `g1` equals the DLL's to 3 digits on all 8 bright frames of both rolls (0.957, 0.910, 1.041, 1.224, 0.944, 1.291, 1.069, 0.971) |
| Noise Filter | `psi24_stage nf`, real s2 -> real s3 | 99.9 to 100 % exact on the Base 16 frames, including nv 3.5; `noise_level` is within 0.06 % of the DLL's nv at 3000 x 2000 (ratio 1.0002..1.0006) |
| Black Print | `psi24_stage bp` | 100.00 % bit-exact on 8 Base 16 frames (and on roll 1 frames 1, 3, 4) |
| whole EBP | `psi24_stage all` from the real s0 | 0.06 to 0.11 MAE on the roll B frames, 90.6 to 95.3 % bit-exact |
| pre-EBP CMM | 3440 exact `cmm_in -> s0` pairs | 0 mismatches in the harness; our table lookup MAE 0.19 / 0.27 / 0.31 |

Known quirks that were settled and are in the code: the Gamma chroma term truncates toward zero, the G&O `lo2` window
is centred (a trailing window was a port bug), the 8-bit knee has one rounding with top 255 (the 12-bit knee rounds
twice and tops out at 4095), the Noise Filter vertical `lp3` is `[1 2 2] / 5`.

## 5. KCDFS: SCP LUT and shifts

`PIKcdfsStartNewRoll(lut, dmin_r, dmin_g, dmin_b)` (dmin from `0x1000d7b0`), `PIKcdfsAddThumbNail` (250 x 375
portrait thumbnails, the OEM's lo-res rect; an area-weighted box average of the frame, up to 40 frames), `PIKcdfsChainLoop(do_scp = 1, pref_flag)` and
`PIKcdfsGetShift_{R,G,B}` (`0x10001e50/80/b0`, `i16` at `[frame + 0x39c6/c8/ca]`). `kcdfs.rs` runs this C core under
Unicorn (entry points `CiColorCorrectionKcdfs` stages `bStep1` `0x10001940`, `bStep2` `0x10001c20` ->
`SbaCalcFosResults` `0x1022bb20`, `Sba` `0x102286c0`, `SbaBuildSCPLuts` `0x1022b250` or `Preference` `0x1022ad90`).

Behaviour measured on the real DLL (n thumbnails in one roll):
- The LUT and every frame's shift depend on the whole roll composition, and so do the downstream `nv` and the Gamma
  pick (one frame: nv 0.79 in one roll, 3.17 in another). Always use the LUT and shifts of the roll the frame belongs to.
- Every frame in a roll gets its own shift (up to 40 frames per batch). An earlier "D(n)" limit (distinct shifts only for the
  first D(n) positions) was a harness bug: the Windows test script wrote decimal frame indices and the host parsed them as hex,
  so `CiList::get` returned the last frame for indices >= n-1. Not a DLL property.
- Thumbnails: `createAlgData` wants a PORTRAIT image (row stride = `G_GEOMETRY[0]` = short edge) and rotates it itself; `Kcdfs::step1`
  serialises our upright landscape thumbnail rotated 90 deg CCW. The OEM lo-res rect is 250 x 375 (`sba::KCDFS_H/W`). The old
  real-DLL golden `psi24_ref_a` was captured with the stride bug and has been removed; `tests/kcdfs.rs` pins the synthetic roll's shifts instead.
- dmin matters once thumbnails are unscrambled (stale dmin: MAE 9.6 on the OEM roll; the roll's own base: 1.3).

## 6. High-bit-depth path

`psi24::render_hq` returns `u16` (`v * 257`); `render_hq_f32` returns sRGB on a 0..255 float scale. The exact path is
unchanged and remains the validation reference.

- **Decisions are exact.** The 8-bit chain runs once on a rounded copy (`ebp::hq_params`, u8 CMM table) and records
  Gain & Offset (`off`, `s`, knee `l2/h2/tl/th/k1/k2`, `p`), the Gamma exponent, the Noise Filter `nv` (measured after
  Gamma) and the Black Print mean (after the Noise Filter). `ebp::hq_apply` then applies the same stages continuously on
  float planes with nothing quantised between them. The float planes enter from the type-5 table (tetrahedral).
- **Analytic loss model** (`HqOptions::loss_model`, default on). A pure float chain sits about +0.5 (R) and +2.3 (G, B)
  levels above the exact chain because the DLL's integer chain floors and truncates. The model subtracts what the C
  code loses; it has no fitted constants:
  - Gamma chroma term pulled toward 0 by `0.5 * clamp(ch, -1, 1)` (the DLL truncates toward zero);
  - Noise Filter chroma `c1`, `c2` pulled toward 0 by `0.375 * clamp(c1, -1, 1)` and `0.25 * clamp(c2, -1, 1)` before
    the blur (the `/4` and `/2` truncate);
  - after the blurs, with texture weight `wt = min(nv / 0.1, 1)` (flat patches lose nothing): luma loses
    `wt * (1/3 + 15/32 [nv > 0.25])`, chroma loses `wt * (15/32 + 2 k)` with `k = 0.4 / 3/8 / 15/32 / 255/512` for
    `m <= 2 / 3 / 4-5 / >= 6`;
  - the rebuild's `(4u+3)/6`, `(8u+3)/6` add +1 for `u < 0`, applied as `clamp(-u, 0, 1)`, not weighted by `wt`;
  - Gain & Offset and the end of the chain carry no loss term (the stage-1 floor and the knee rounding found no net bias).
  `loss_model: false` gives the pure float chain.
- Tried, worse: fitted per-stage constants (`HQ_LOSS`) tuned on 12 images: roll A 1.292, roll B 1.351, against 1.009 and
  1.069 for the analytic model; weighting the pre-blur truncation terms by `wt` (suite HQ 1.07 / 1.21 against 0.94 / 0.97).
- Unit test `psi24::tests::hq_stays_close_to_the_8bit_path_and_has_no_banding` checks HQ against the exact path and that
  a smooth row has many more distinct 16-bit codes. `golden_hashes_of_synthetic_fields` pins the exact and HQ output
  of three synthetic fields bit for bit; a deliberate change of the maths must update the hashes.

## 7. Current accuracy (MAE against the real DLL, auto Gamma)

| | probe suite M3 / M4 | roll A (8 frames) | roll B (8 frames) |
|---|---|---|---|
| exact path | 0.60 / 0.71 | 0.771 | 0.876 |
| HQ | 0.99 / 0.99 | 0.887 | 0.856 |

Per frame, exact path: roll A frames 3/7/10/11/12/15/17/21 = 0.68 0.79 0.51 0.83 0.54 0.83 1.30 0.69; roll B frames
17/21/23/26/27/28/29/30 = 1.35 1.34 0.98 0.68 0.60 0.81 0.59 0.66.

Rolls A and B are two held-out scans of the same pictures in different KCDFS rolls (Base 16, 3000 x 2000, one 37-frame
roll and a 25-frame roll with its own LUT and shifts). The probe suite is 21 synthetic probes (flat patches, ramps,
sweeps; M3 = shifts off, M4 = shifts and EBP on). Roll 1 (four frames, 2250 x 1500) was the development set.

## 8. Known residuals

- **R = 0 gamut cliff.** In bright saturated blue sky the Kodak transform clips R to 0; a 64-unit table cell straddles
  that kink. Roll B frames 17 and 21 have an R MAE of about 2.6 to 2.8 (reported with tetrahedral lookup) against 0.2 in G and B, and the cliff amplifies one-unit RPD
  grain in the R channel about 10x (which is why `nv` there is 3.2 to 3.5 and flips between rolls). Everything downstream
  of the CMM is exact on those frames, so this is the pre-EBP table. A finer grid on [0, 3000] (61^3 or 121^3, u8 and
  type 5, needs another Windows run) would remove it.
- **`nv <= 0.25` branch never exercised.** The luma pre-blur off and `m <= 2` chroma smoothing branches are decoded from
  the disassembly but no real frame reached them (lowest nv 0.265 on roll 1 and 0.294 on rolls A/B).
- **No Base 8 (2250 x 1500) capture of the Base 16 film.** The kernels matched at 3000 x 2000 without change; resolution dependence of
  anything beyond the Gamma edge statistic is untested.
- The CMM table's own interpolation inside the DLL is unknown; nine points in one `rpd.pf` cell (corners
  `{1600, 1700}^3` and the centre) would settle it on a future Windows run.
- Roll 1 pre-EBP images carry a +0.4 to +0.6 bias on three of four frames (shadows +0.7 to +1.0); a constant -2 on the
  12-bit `cmm_in` removes it. Not applied, not reproduced since the u8 table; its origin is unknown.
- Tried, worse: coordinate descent directly on end-to-end MAE (overfits: 0.65 / 0.61 in sample, 1.44 / 1.88 held out).

## 9. Capture harnesses (Windows, offline, real DLLs; not in this repo)

- `psi24-cmm`: standalone 32-bit harness (`cmm_harness.exe`) calling `kodakcms.dll` `SpEvaluate` on grids `C65`, `C33`,
  off-node `C17off` and 1-D ramps; produced the CMM tables and the cross-check.
- `psi24-ebp-luts` (probes): synthetic probe images (`P1_ramp`, `P2_ramp_Gonly`, `P3a_*` ramps, `P3b_*`/`P3c_*` patches,
  `P3e_original`), each rendered with shifts off (M3) and on (M4) through the real DLL, with the stage dumps. Source of the
  21-probe suite.
- `psi24-film2` (roll A) and `psi24-film2b` (roll B): two rolls scanned at Base 16, every frame 3000 x 2000, with stage
  dumps (`s0`..`s3`, `look`), the roll's `scp_lut.u16`, the KCDFS shifts, and the EBP structs; plus the KCDFS limit
  experiments (section 5).
- A hook host (`oem_host_hooks.cpp`) around `PakonIMAu.dll` records stage buffers and structs; unhooked renders are
  byte-identical.

## 10. Running the tools

All are cargo examples of `pakon-image`: `cargo build --release -p pakon-image --examples`, run from `rust/`.

`psi24_probe <lut.u16> <in.raw> <ref.rgb> <shifts r,g,b | 0> [gamma | auto]`: render a probe or frame from its planar
12-bit `u16` RPD input, with the roll's LUT and the shifts, and print MAE, bias and sample pixels against the real
output. `ref.rgb` is planar 8-bit.
- `PSI24_HQ=1` renders with the HQ path (output quantised to 8 bit for the comparison).
- `PSI24_W`, `PSI24_H` frame size (default 2250 x 1500; 3000 x 2000 for Base 16).
- `PROBE_OUT=<file>` writes our render as planar 8-bit RGB.

`psi24_stage <in.u8> <go | gamma | nf | bp | all> <ref.u8> [out.u8]`: run one EBP stage on a real stage dump and compare
with the next real stage (planar 8-bit; prints MAE, bias, percent exact and stage parameters). `PSI24_W`, `PSI24_H` as above.
- `GFIX=<g>` fixed Gamma exponent for `gamma` (default: the estimator), `P=<p>` the G&O gain `p` it reads (default 1).

`psi24_ref <1.look dir> [auto | uncapped | gamma]`: the four roll-1 frames (`psi0N.raw`, `scp_lut.u16`, `look0N.rgb`)
with their shifts.
- `SHIFTS="r,g,b;r,g,b;..."` replaces the real shifts (for example open-pakon's own KCDFS ones);
  `DSHIFT=r,g,b` adds a constant to every shift; `PSI24_HQ=1` as above.
- The gated integration test `tests/psi24_ref.rs` runs the same frames against `PAKON_PSI24_REF=<1.look dir>`.

Tests: `cargo test --release -p pakon-image`.
