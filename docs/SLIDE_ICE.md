# Dust removal on slide film (E-6): investigation notes

Status: **shipped, one open issue.** Slides get dust removal since
2026-11-21 (`prescan` in `rust/crates/pakon-roll/src/lib.rs` keeps the IR
plane for them; the Adjust page offers it, on by default). On roll 000012
below it still over-cleans the blown highlights; turn dust removal off for
such a roll. These notes record what was measured so that issue can be
picked up without redoing it.

## Why slides were excluded at first

On 2026-11-02, in the Python pipeline, both the OEM Digital ICE (DICE, run
under emulation) and our native port smoothed the grain over most of each
slide frame. The IR plane was rewritten in the Rust port a week later, so
that result had to be re-checked.

## What the OEM software does

From `re/out/tlb_all.c` (TLB.dll, the F-135 / F-135+ engine):

- **The DICE call is the same for every film type.** `DMLDICEProcess` gets
  14-bit R, G, B, IR planes, black 300, before the density LUT; the only
  slide difference is the dynamic struct's negative flag (`+0x14`), set to
  `film type != 2`, i.e. off for slides. Our export now passes it the same
  way (it used to pass `true` for every film).
- **Red-to-IR cross-talk correction, base F-135 only.** The per-line
  fixed-pattern correction (`FUN_100246d0`, MMX) ends each line with
  `IR = IR_corrected − R_corrected × (0x10000 / IrCrossTalkFactor) >> 16`,
  i.e. `IR −= R / factor`, using the same pixel's corrected red. Every film
  type, before DICE.
  - `IrCrossTalkFactor` is a registry setting read by `FUN_10010cc0`
    (settings + 0x40), clamped to 1–20. Its default is **10** (the settings
    constructor, `param_1[0x10] = 10`); no installer file sets it. A factor
    of 1 makes `0x10000 / 1` overflow to 0: no correction.
  - Model detection (`FUN_1000afd0`) probes the controllers with `04 03
    <addr>`: when 0x44 or 0x46 answers (F-135+) it sets the factor to **0**
    (no correction); on 0x24 / 0x26 (base F-135) the factor stays 10.
- **`FN_bCalibrateFixedPatternIrLag`** (`FUN_1001fb80`) is the other
  direction: per-pixel R, G, B tables of the IR LED's leak into the visible
  channels, subtracted per line. We already cover that leak: our dark table
  is captured with the IR LED on.

We do not port the red-to-IR correction yet.

## Measurements (2026-11-17)

Five E-6 rolls, four on the base F-135 (serial 3054) and one on the F-135+
(serial 138), all at Base 16 + IR. Dust removal forced on for slides, DICE with the negative flag
off; "rebuilt" is the share of each frame's pixels DICE changed.

| Roll | Scanned | Rebuilt per frame | Notes |
|---|---|---|---|
| 000011 | 2026-10-23 | 0.11–0.29% | dust; native port 0.09–0.22% |
| 000012 | 2026-10-23, same session and settings as 000011 | **9–21%** | dense roll with blown highlights; native port 8–14% |
| slide-test/slide.raw | 2026-10-14 | 0.50–0.85% | dust |
| slide-test/strip2.raw | 2026-10-14 | 0.51–0.98% | dust |
| 000025 (**F-135+**, serial 138) | 2026-11-21 | 0.46–0.77% | dust, hairs and a scratch; native port 0.32–0.53%; grain unchanged at 1:1 |

On 000011 the negative flag made no difference (identical numbers with it on
and off); it was not compared on the other rolls.

On 000012 the rebuilt pixels are almost all in large blown-out areas, where
the slide is nearly clear film (frame 2: 89% of pixels brighter than
140/255 rebuilt, under 0.1% of pixels darker than 80/255). There DICE
smooths the grain away and lifts the level by about 5/255 (up to 25); it
also removes the real dust specks it should.

The cause is in the IR plane: on 000012 it is not flat across the picture
but **falls about 7% as the slide gets brighter** (median 43.5k where the
picture is dark, 40.5k in the brightest areas, at 0.5% pixel noise), so a
whole blown area reads as one low-IR region, which both DICE and the native
port treat as a defect. On 000011 and slide-test/slide.raw (strip2 was not
measured) the IR **rises** with brightness instead, by 6–7% (47.2k → 50.7k
and 58.4k → 62.2k as red goes from under 2k to 35–50k): roughly what the
OEM's `IR −= R / 10` removes, which overcorrects them at the bright end in
our units (50.7k → 46.6k, 62.2k → 58.3k). On 000012 the OEM correction
makes the fall worse (43.5k → 37.9k). 000012's fall is not a
fixed red-to-IR leak (000011, scanned minutes earlier with identical
calibration, goes the other way) and is not explained yet.

## Tried and dropped

**Keeping only the rebuilt pixels near a dip in the IR** (IR 6% under the
lowest mean of the four 25 × 25 boxes around the pixel, grown by 8 px): cut
000012 to 0.12–0.38% and kept the real specks, but inside blown areas the
pixels it kept carried DICE's brighter level and showed as light squares.
Worse than either extreme.

**A per-roll linear fit of IR against R, G and B, subtracted before dust
removal**: not finished. The per-channel coefficients disagree between
000011 and 000012 (same session), so a single linear model does not
describe the effect.

## To resume

1. Scan the same E-6 strip on both models (a base F-135 and an F-135+),
   Base 16 + IR. Same picture through two scanners: tells whether 000012's
   IR fall comes from the film or the scanner. (F-135+ slide data now
   exists: 000025, clean; the OEM applies no IR correction on that model.)
2. Measure each roll with `examples/ice_exp.rs` (see its header): the
   rebuilt share per frame, and the IR plane's median against picture
   brightness.
3. Decide whether to port the base-F-135 `IR −= R / IrCrossTalkFactor`
   correction. It applies to every film type on that model, so it changes
   colour negatives too, which work well today: ship it only with
   before/after numbers on negative rolls.
4. Slides are enabled (2026-11-21) with the negative flag off. If a fix
   for 000012 lands, re-measure all five rolls with `examples/ice_exp.rs`.
