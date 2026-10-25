# Project status

_Reset 2026-09-28. The previous snapshot (sessions of May–June 2026) is in git
history; everything still true from it has been folded into `docs/`._

## Goal

A **non-USB-replay client** for the Kodak/Pakon F-135 and F-135+: every command
the scanner receives is built by our code from known parameters (per-unit
EEPROM, a calibration we run, the chosen resolution), not replayed from a
captured `.pakscan`/`.pakfw` file. The captures stay in the repo as **test
fixtures**: generated command streams are checked byte-for-byte against them.

## Where things stand

Both models scan end to end on hardware by replaying captures:

- **F-135**: serial 3054 (ours), Linux and macOS, scripts in `resources/`.
- **F-135+**: serial 16402 (Ali Bosworth, PRs #2 and #3, August 2026), macOS,
  scripts in `resources/f135plus/` converted from OEM driver-level captures
  with `tools/fx35_datalogger.py`. Base 16 and Base 8 verified live; Base 4 is
  a poor replay candidate; Base 4 + IR decodes but has not been replayed.
  Coverage matrix in `docs/F135_PLUS_CAPTURES.md`.

| Step | How it works now | Replay-free? |
|---|---|---|
| Firmware load (cold `0f05:f235` → warm `0f05:f135`) | from the OEM Intel HEX files (`--load-firmware-hex`), or replay of `resources/f135.pakfw` | yes |
| Open handshake + model detection | synthesized in C; probes report F-135 / F-135+ | yes |
| EEPROM read (`0xA4`/`0xA9`) | `tools/pakon_eeprom.py backup` (all four copies, validated on serial 3054); decoded in C by `pakon_eeprom` | yes |
| Setup / configure | built in code (`pakon_setup`), both models; run on the F-135 | yes (F-135) |
| Light calibration | built in code (`--light-cal`), F-135 | yes (F-135) |
| Scan | `--scan-code --base 16\|8\|4` (F-135, IR on): nothing replayed, whole strip; Base 16 2026-10-09, Base 8/4 2026-10-20 (C-41 and B&W, CLI and web app); replay modes `--scan`/`--scan-sm` remain | yes (F-135) |
| Teardown | built in code; stops the motor on the F-135 (2026-10-09); the replay modes still replay the captured tail | yes (F-135) |
| Film advance / eject | F-135: `--advance-code`, `--eject` (DX sensors), in code; F-135+: timed `--advance` (captured speed) | yes (F-135) |
| Decode (`tools/pakon_image.py`) | independent; row layout auto-detected per raw | yes |
| Web UI (`web/`) | F-135 with an EEPROM backup: HEX firmware, `--scan-code` (feed prompt), `--eject`; else / F-135+: replayed scripts | yes (F-135) |

Known limits of replay: a scan fits the capture's length (the F-135+ script
is a 4-frame strip); open-loop replay skips the OEM's film-exit wait, so the
strip can stay in the transport (`--advance` pushes it out on the F-135+;
on the F-135 use `resources/advance.pakscan --steps N`); modes without a
capture (e.g. F-135+ Base 16 + IR) have no script.

## The projects we are working from

Laid out side by side in `~/Desktop/Code/pakon/` (see `../repos.md`):

- **pakon-macos** (this repo): our driver, decoder and web app. AGPL-3.0.
- **pakon-reference**: Ali Bosworth's public, implementation-agnostic
  reference for the F-X35 family (CC BY 4.0). Cites this project for the
  reply direction, per-resolution strides and motor stop behaviour. Source of
  the per-unit EEPROM layout, the safety rules and the DX subsystem.
- **libpakon-main**: Stefan Dierauf's independent C++ driver + "Pakon Studio".
  A driven (trace-transcribed) client for the F-135 line. **No licence file:
  read for reference only, never copy code.** Notes in
  `docs/LIBPAKON_COMPARISON.md`.
- **pakon-scanning-software**: the original Windows XP OEM install (git-ignored
  here). Its `TLB.dll` is the F-135/F-135+ engine; decompiled 2026-09-28 with
  Ghidra. Findings in `docs/TLB_FINDINGS.md`. Our earlier reverse engineering
  used `TLA.dll`, which is the **F-235** engine; facts from it are being
  re-checked against `TLB.dll`.

## What the two models' captures tell us (2026-09-28)

- **One command set for both models.** With the controller addresses mapped
  (`0x40`/`0x44` → `0x20`/`0x24`), the F-135+ Base 16 scan and our F-135 scan
  use the same 55 commands. Only our capture has one extra, the probe of the
  Plus bootloader address `0x46`. There is no separate Plus protocol: one
  address-parameterised sequence serves both.
- **The per-mode values are known.** On the F-135+ (serial 16402):

  | Mode | `0x91` value | Motor speed `0xA5` | = EEPROM field |
  |---|---|---|---|
  | Base 16 | `0x003c` | 5900 | Base 16 MotorSpeed |
  | Base 8 | `0x0075` | 11434 | Base 8 MotorSpeed |
  | Base 4 | `0x0107` | 25726 | Base 4 MotorSpeed |
  | Base 4 + IR | `0x00c5` | 19278 | Base 4 MotorSpeed_Ir |

  Motor speed is exactly the EEPROM value (adjust factor 1000), confirming the
  `TLB.dll` formula on real captures. The `0x91` values match the
  pakon-reference table. Our F-135 captures use `0x0010` and speed 1557;
  1557 is close to another base F-135's Base 16 IR speed (1530) and our rows
  are the 8000-sample IR layout, so they are most likely Base 16 + IR.
- **The OEM waits on events; replay can't.** The 18-minute Base 4 capture
  replays badly because the OEM waits on lamp temperature (up to 300 s) and
  calibration. A driven client waits on the event service instead.
- **Reply bytes are recoverable.** `PAKON_DEBUG=4` against real hardware logs
  every reply, which the Windows captures lack; use it to check each generated
  sequence.

## Next steps

Each step works on its own; the capture it replaces becomes a unit-test
fixture (generated bytes must match it).

1. ~~**Read our EEPROM**~~ — done 2026-10-09: all four copies valid and
   equal; serial 3054, type 1350. Base 16 `MotorSpeed_Ir` = **1557**, so our
   F-135 captures are Base 16 + IR and motor speed from the EEPROM is proven
   on both models. Archive in `backups/eeprom/F135-3054-cycle1` (git-ignored);
   the C decoder matches the Python one on it. Original plan: **Read our EEPROM** (`python3 tools/pakon_eeprom.py backup
   backups/eeprom/F135-3054-cycle1`, see `docs/EEPROM_BACKUP.md`) and check
   that 1557 is our Base 16 `MotorSpeed_Ir`. About 5 minutes on hardware. If it
   is, motor speed from the EEPROM is proven on both models. This is also the
   first hardware test of that tool's read path; `pakon_replay --read-params`
   remains the fallback but reads only the primary copies.
2. ~~**EEPROM decoder in C**~~ — done 2026-10-09: `src/pakon_eeprom.c`
   (`pakon_eeprom_decode`): CRC check, backup fallback, type, serial,
   Offset, MotorSpeed(_Ir), adjust words, both matrices. `test_eeprom_c`
   covers the two in-repo fixtures and the corruption cases; a one-off
   cross-check against `tools/pakon_eeprom.py` matched field for field on
   all four dumps in pakon-reference. Not yet wired into
   `pakon_replay --read-params`.
3. **Setup in code, both models** — done 2026-10-09. Ran on serial 3054
   with `pakon_replay --setup --eeprom-dir backups/eeprom/F135-3054-cycle1`:
   all 77 frames accepted (every write status 0, every busy poll clear on
   the first try; LOW polls carry flag `0x80`, event pending, since no event
   service runs yet). Not yet run on an F-135+. `src/pakon_setup.c` builds the frames from the SCN `0x97` init
   to the first acquire: `pakon_setup_init` (addresses + the model's LED
   period word) and `pakon_setup_configure` (per-mode trigger, IR, dual-tap,
   integration, pixel window, LOW `0x89`; writes only changed registers, as
   the OEM does). `test_setup` matches all five captures byte for byte (F-135
   Base 16 + IR; F-135+ Base 16/8/4/4 + IR), leaving out event-service
   traffic and the status-LED word `0x0313`, whose timing varies. Findings:
   the pixel window during calibration is reg 4 = 6 (3 dual-tap), reg 5 =
   EEPROM Offset + 2000 (1000 dual-tap), on both units; the F-135 and F-135+
   init differ only in the LED period word (`0x0997` / `0x03D6`).
   `pakon_cmd_run_seq` sends a sequence with the OEM reply rules and the busy
   poll (≤ 44 tries); `pakon_replay --setup` runs it (Base 16 only).
   Model detection (probes before `0x97`) was already in C.
4. **Teardown in code** — done 2026-10-09: `pakon_setup_teardown` writes
   bank `0x82` reg 0 back to the configured value (acquire clear), LEDs
   off, ResetFifos, `92`, 20 ms, `a2`. `test_setup` matches the teardown of
   all seven captures (three F-135, four F-135+); only the status-LED words
   differ between them and are left out. `pakon_replay --setup --teardown`
   ran on serial 3054: all 11 frames accepted. In `--scan-code` it stopped
   the running motor at the end of the scan (2026-10-09).
5. **Event service during the scan** — done 2026-10-09:
   `pakon_cmd_service_events` (host poll; LOW then SCN event read; ack
   `02 05 <addr> 02 06 00 <status>`; then `0x90`, plus `0x84`/`0x88` when
   status & `0x5B`). `test_setup` reproduces 91 of the 95 acks in the
   captures (the rest depend on reply data). `--scan-sm` now runs it after
   every image read, and **ran a whole strip on serial 3054**: 5 frames,
   film detected and end-of-roll white reached, 4 events acknowledged, no
   FIFO overflow, 286 MB raw (17920 rows). Before, it died ~38% in. Setup
   and teardown in that run were still the replayed ones.
6. **First scan no replay could do:** compose F-135+ Base 16 + IR (never
   captured) from the per-mode values, on Ali Bosworth's unit.
7. **Calibration in code** — light calibration done 2026-10-09 (F-135):
   `pakon_replay --light-cal --eeprom-dir DIR` runs `FN_bCalibrateLEDs` on the
   open gate with the motor stopped: dark offsets, LED current search, duty
   refine (`src/pakon_lightcal.c`, `test_lightcal`). On serial 3054 it
   converged to offsets −38/−31/−31 (OEM: same), currents R2 G2 B3 IR2 (OEM
   G3), duties R900 G505 B273 IR1357 (OEM R896 G708 B278 IR1353; G differs
   because its current stopped at 2). Findings: during calibration a line is
   4 × (reg5 − reg4) samples (8148 here), RGB block then IR block; no marker
   bit, and the read's start within a line varies, so each read is aligned on
   its black pixels (`pakon_lc_find_origin`), which needs light, so the dark
   loop runs with the IR LED alone (TLB.dll has all LEDs off). The A/D offset
   needs ~300 ms to settle after a write. Convergence: a channel counts as
   settled when its peak is in the window or the next step moves its duty by
   ≤ 1 tick (at B's ~273 one tick is ~234 counts, wider than the 64-count
   window). **Used in a scan:** `pakon_replay --scan-code` (2026-10-09) ran
   setup, this calibration, the scan start (scan window, C-41 scan duties,
   EEPROM motor speed 1557) and the teardown, all built in code, on a whole
   strip: 0 transfer errors, 6 events acknowledged, motor stopped by itself;
   the image matches the replayed scan, and the scan-window line marker is
   back (column 1, every row). **Per-column fixed-pattern tables**
   (2026-10-10, verified on serial 3054): after the duties converge,
   `light_cal_run` averages 128+ dark lines (IR LED alone, for alignment) and
   128+ open-gate lines per pixel; `--light-cal` writes them to
   `DIR/flatfield.json` (or `--flat-out`), `--scan-code` to `OUT.flat.json`.
   The decoder and the web app apply the OEM's `(raw − dark) · gain` per column
   (`apply_flat_field`). Without it the illumination falls to ~25% at scan
   column 0 on serial 3054 (the bottom edge after the export's rotation), the
   same on every roll, which shows as a bright band. On hardware: the empty
   gate shows the same profile column for column (0.23 at column 0, 0.53 at
   10, 0.78 at 40), which confirms scan column j = calibration pixel
   Offset − 6 + j; the IR LED adds 13–17 counts to the RGB dark table; clear
   film then reads 0.92–1.03 across the line. Smear came out 0 (the OEM keeps
   only 1..699). Left: F-135+. The old `pakon_calib` (`--calibrate`, built from the F-235 engine
   `TLA.dll`) is superseded.
8. **Firmware from Intel HEX** — done 2026-10-09: on serial 3054 from cold,
   personality `c0 05 0f 35 f2 07 aa 04`, 1104 transfers with 0 errors, warm
   `0f05:f135`, then `--setup --teardown` all accepted. Command:
   `pakon_probe --load-firmware-hex firmware/PknLdr.hex firmware/Pakon7.hex`.
   The stage-1 loader is a record table inside the OEM `F235Ldr.sys`
   (`tools/extract_fx2_loader.py`), not `PknInit.hex`; the main image is
   `Pakon7.hex`. `pakon_fw_build` reproduces all 1104 transfers of
   `f135.pakfw` (`test_fw`). Refuses to load unless the personality is
   F235_AA07. OEM files stay local (`firmware/README.md`).
9. **Film handling** — done for the F-135, 2026-10-09:
   `pakon_replay --advance-code SECONDS` (timed move at the captured advance
   speed `0x251C` = 9500, matching `advance.pakscan`) and `--eject` (runs
   until the DX sensors say the strip is out or stalled at the exit). From
   a `--film-sense` run: `0x93` bytes 0–1 are the entry DX sensor, 2–3 the
   exit one; empty 190/216/224/202 on serial 3054, film keeps one byte of a
   pair under 170. Once the tail leaves the drive rollers the strip stalls
   under the exit sensor and has to be pulled by hand; `--eject` detects that
   and stops. Not derived: the advance speed (it is not the EEPROM Base 4
   speed on this unit); thresholds are this unit's (DX pots are per unit).
   F-135+ not done.

## Scan options (later): resolution and film type

The OEM TLX software offers Base 4 / 8 / 16 and colour negative / B&W /
slide. Both change the scan itself, not just the decode.

**Resolution (Base 4 / 8 / 16).** Per base: pixel window and motor speed
(EEPROM), integration time (`TLB.dll` table; F-135: Base 4/8/16 =
`0x913`/`0xD9D`/`0xFF0`, with IR `0x60D`/`0x913`/`0xC1A`), LED period
(integration × 0.6), dual-tap at Base 4, and the `0x91` trigger.
**`--scan-code --base 4|8|16`** (F-135, IR on; `pakon_setup_mode`; branch
`feat/scan-base`). It had a calibration regression on hardware, written up
under "Hardware test" below, fixed 2026-10-20; all three bases scan. The
per-base table in `TLB.dll`
(FUN_10011a60) is shared by both models, and FN_bSetF135 overrides only the
integration times, so the F-135 also gets Base 8 = 1500 px out of a 2000 px
window with LOW `0x89` = 1, and Base 4 = dual-tap, 1000 px. Light
calibration measures the line width from the open-gate data
(`pakon_lc_find_line_px`) instead of assuming the window, and scales the
black pixels and scan window with it; the scan's `.flat.json` records the
row layout for the decoder (F-135 Base 8 + IR rows are 6000 samples, like
F-135+ Base 16). The `0x91` value is inferred (2026-10-09), not yet tested:

- Rule: `0x91` = K × line pitch × (0.01, or 0.005 with IR) ÷ integration
  time; line pitch relative to Base 16 is 1 / 4/3 / 2 for Base 16 / 8 / 4
  (3000 / 2250 / 1500 lines per frame). One K fits all four F-135+ captures
  exactly (Base 16, 8, 4, 4 + IR on serial 16402).
- Cross-check: in every known mode on both models `0x91` ≈ EEPROM motor
  speed ÷ ~98 (`TLB.dll`: default motor speed = 100 × `0x91`).
- F-135 predictions (K from our Base 16 + IR = 16, `TLB.dll` integration):

  | Mode | Formula | EEPROM speed ÷ 98 (serial 3054) | Value |
  |---|---|---|---|
  | Base 16 | 24 | 24.0–24.3 | 24 |
  | Base 8 | 38 | 37.4–37.9 | 37 or 38 |
  | Base 8 + IR | 28 | 28.0–28.4 | 28 |
  | Base 4 | 85 | 84.1–85.3 | 85 (84 possible) |
  | Base 4 + IR | 64 | 63.1–64.0 | 64 |

- To do on hardware: scan one strip at Base 8 and Base 4 and check the
  frame proportions (3:2) — a wrong `0x91` stretches the image ~3%. Check the
  `calibration line:` output (expected ~1528 px at Base 8, the window at
  Base 4) and that the decoder applies the scan's flat field. Base 8 vs 37/38
  only matters with IR off, which `--scan-code` does not use on the F-135.
  Low risk: `0x91` only sets the line rate.

**Hardware test, 2026-10-13 (serial 3054, branch `feat/scan-base`).**

- Base 4 setup and calibration work: window 3..1021, measured calibration
  line 1018 px (= the window, so the F-135 does output the dual-tap
  1000 px line), dark offsets -38/-31/-31, currents R2 G3 B3 IR2, duties
  converged (period 929).
- Base 4 scan: from the motor start the whole stream read black (~300
  counts, IR too; max 416), before and through the film. End of roll waits
  for open-gate white, so the loop never stopped; killed and torn down with
  `--setup --teardown` (motor stopped, clean). Probably the same fault as
  below, so the Base 4 trigger (64) is still untested.
- **Regression:** `--light-cal` at Base 16 fails with the branch build, 4 of
  4 runs, at the dark-table step ("no light to align lines on", table read
  2), and passes with the `main` build, 4 of 4 (cold and warm, run in
  between). In each failing run LOW raises event status `0x42` (0x40 is in
  the lamp group `0x5B`; never seen in the passing runs), and from the next
  FIFO re-arm every channel reads black although the IR LED is still
  commanded on. Temperatures with the event are normal (lamp 40.06 °C at
  the 40.0 °C setpoint, motherboard 28.8 °C).
- Full packet traces (`PAKON_DEBUG=4`) of both builds send the same
  commands apart from the calibration values. The first branch version
  added one A/D write and one open-gate read before the dark loop; moving
  the line-width measurement into the first dark read (no extra traffic)
  did not fix it. Left: timing — the branch spends ~0.33 s (Debug build) in
  `pakon_lc_find_line_px` with acquire on and nobody reading the FIFO or
  servicing events.
- Done in code since (2026-10-20, rebased on `main`): the line width is
  measured only at Base 8 (`lc_geometry`; Base 16/4 take the window), so at
  Base 16 the calibration again sends main's traffic with no host-side
  stall. The calibration cache is per film and base
  (`calibration-<film>.bin` at Base 16, `calibration-<film>-baseN.bin` at
  8/4); Base 8 reuses the saved, measured line width. On hardware
  (2026-10-20, Base 8 B&W): the first scan calibrated and saved, the
  second reused it (motor start 2 s after launch instead of ~16 s), same
  values, clean scan, flat field with the right geometry (1527 / 27 / 15).
- Rerun 2026-10-20: Base 16 `--light-cal` passes 3/3 (12–13 s each without
  `PAKON_DEBUG`; offsets -38/-31/-31, currents R2 G2 B3 IR2, duties
  904/504/274/1382, ~136 table lines). So timing was the cause: Base 8 must
  measure the line width without stalling the FIFO / event service.
- Base 4, 2026-10-20: `--light-cal` passes 2/2 (line 1018 px = window,
  duties half of Base 16's at period 929). `--scan-code --base 4` scanned a
  7-frame C-41 strip with real image data (trigger 64): frame pitch 1590
  rows vs the OEM 3:2 pitch 1603 (-0.8%, within frame-spacing spread), so
  64 looks right. End of roll did not trip: the Base 4 open-gate IR lane
  sits at ~37.8k, under `SM_WHITE_THRESH` (40000), so open gate is only
  75.7% "white" against the 84% `SM_WHITE_FRAC_CODE_PCT`; killed and torn
  down with `--setup --teardown --base 4` (clean). Film on that strip never
  exceeded 40000, but a Base 16 roll (2026-10-19) reached 79.6%, so the
  rule needs a better signal, not a lower fraction.
- Base 4 B&W (`--film bw`), 2026-10-20: 6 frames, clean, and end of roll
  tripped on its own (B&W IR scan duty 820 vs C-41 687 lifts the open-gate
  IR lane over 40000), which supports the IR-level diagnosis above.
  Decoder: channel registration now scales the trilinear leads with the
  line width (Base 4 = -4/-8, measured) and correlates edges, not levels.
- Base 8, 2026-10-20: `--light-cal --base 8` passes 2/2 (16 s) with the
  line-width measurement in the first dark read: 1527 px of a 2036 px
  window (as predicted), no `0x42` event; currents R2 G2 B4 IR2. A C-41
  `--scan-code --base 8` scanned 7 frames with 0 errors and stopped on its
  own at end of roll; DX read; leads -6/-12 as predicted; frame pitch
  2388.6 rows vs the OEM 2405 (-0.7%), so trigger 28 looks right.
- End of roll for `--scan-code` now counts samples above 55000 (only
  saturated R/G/B; IR never gets there) with a 45% cutoff
  (`SM_WHITE_THRESH_CODE`), instead of 40000 / 84%, which sat on the IR
  level. Replayed offline over 16 saved `--scan-code` raws (Base 4/8/16,
  C-41, B&W, slide): stops 25-28 chunks after every strip, including the
  four Base 4 C-41 scans that never stopped, and never on a strip. On
  hardware, 2026-10-20: a Base 4 C-41 scan from the web app (order 000010,
  100 MB) stopped on its own and processed. Also: the web Scan page has a
  Stop button (SIGINT =
  end of roll in the image loop, then the normal teardown).
- Also seen: with stdout redirected to a file the progress lines are
  block-buffered (the "feed the film" line is flushed, the rest lags). (The
  `--scan-code` image loop now catches Ctrl-C.)

**IR off (F-135; branch `feat/bw-ir-off`, 2026-10-20).** `--ir on|off`;
`--scan-code --film bw` defaults to off (silver blocks IR, so the dust
channel is empty), so B&W from the web app scans without IR. Integration
`0xFF0` / `0xD9D` / `0x913` (`TLB.dll`), triggers 24 / 38 / 85 (same rule as
above), the EEPROM's IR-off motor speed and adjust (serial 3054: 2365 /
3689 / 8302 vs 1557 / 2767 / 6224 with IR, 1.52x / 1.33x / 1.33x faster).
- With IR off (CCD reg 0 bit 8 clear) a line is 3 x line_px samples, no IR
  block (6111 at Base 16; probe, open gate).
- The line marker (bit 0 of the first RGB word, set on no other sample) is
  present in calibration reads too, lit or dark, IR on or off. IR-off
  calibration aligns on it, runs the dark steps with every LED off (as the
  OEM does; the dark tables are true dark tables now), takes the Base 8
  line width from the marker spacing (1527 px, as measured with light), and
  calibrates R/G/B only (IR LED current 0; IR-off ceiling R 6 G 8 B 8 IR 0).
  IR on is unchanged.
- Hardware: `--light-cal --ir off` passes at Base 16 / 8 / 4 (10-12 s); a
  Base 16 + IR run afterwards matches earlier ones. A Base 8 B&W half-frame
  strip scanned with IR off: 4500-sample rows, 0 errors, stopped on its own,
  8 half frames, leads -6/-12, frame pitch -1.1% of the OEM's (IR-on B&W
  Base 8: -1.2%), so trigger 38 looks right. Base 16 B&W (cactus strip)
  with IR off: 6000-sample rows, 0 errors, stopped on its own, 6 frames,
  leads -8/-16; the strip spans 18987 rows vs 18999 for the same strip with
  IR on (order 000002), pitch -1.22% vs -1.07%, so trigger 24 is right and
  the 1.52x transport speed buys the same rows per frame. Base 4 B&W
  (same strip) with IR off: 3000-sample rows, 0 errors, stopped on its
  own, 6 frames, leads -4/-8; 9489 rows vs 9481 with IR on, pitch -1.19%
  vs -1.29%, so trigger 85 is right. All three IR-off modes checked.

**Film type.** Scan side: the scan LED duties are the calibrated duties ×
10^density, with `TLB.dll`'s film-base densities (R/G/B/IR): colour negative
0.144/0.40/0.715/0, B&W C-41 0.10/0.25/0.25/0.08, other (slide, silver B&W)
0/0.03/0/0.08. IR/ICE must be off for silver B&W. Decode side: colour negative
uses the EEPROM NegMatrix, slide the PosMatrix (no inversion), B&W the
NegMatrix then grey.

- **Silver B&W — done 2026-10-13, untested on film:** `pakon_replay
  --scan-code --film bw` calibrates and scans with the "other" densities
  (`pakon_lc_density_other`) and records `"film": "bw"` in `RAW.flat.json`. The
  scan stays in the Base 16 + IR mode (the IR lane is scanned and ignored;
  ICE is not implemented), so no untested IR-off timing is needed.
  Decode: `render_oem(..., grey=True)` averages the three RPD channels, then
  `rpd.pf`, then Rec. 709 luma; `pakon_image.py --bw`; web: a film type on
  Scan and Process, kept per raw in `RAW.film`. Needs a strip of silver film
  to check the duties and the end-of-roll detection (clear B&W base should
  read ~60–70% of the open gate, under the white threshold).
- **Slide (E-6) — done 2026-10-14:** `pakon_replay
  --scan-code --film slide` scanned at first with the "other" densities, as for silver
  B&W (now 0.12 on R/G/B, see below) (`TLB.dll` refuses film type 2 in calibration itself, so it calibrates
  separately and applies the type at scan; ours does the same). The IR lane
  is scanned and ignored. Decode (`render_slide`, `pakon_image.py --slide`,
  web film type "Slide (E-6)"): the same density LUT → the unit's PosMatrix
  (0.25·I on all three units known; the factory value is used without an
  EEPROM archive) → `ColRevLut1.pf`, a lut16 device link "Pakon CMY to
  ROMM12", evaluated from the tag → `romm.pf` → sRGB. No scene balance (the
  OEM runs Ansel in a positive mode, not modelled). Needs a strip
  of slide film: check the colour against a lightbox, and that
  overexposed (near-clear) frames don't trip end-of-roll white.
  First strip scanned 2026-10-14 (serial 3054): whole strip, 0 errors,
  plausible colour, warm overall. Shadows are the weak point (noise floor
  ~D 3.3, blacks lifted by stray light to D 2.6–3); the decode now computes
  density exactly and removes half the stray light measured between frames
  (`--slide-flare`, docs/IMAGING.md). More light at scan (2026-10-15,
  untested on hardware): slides now use film-base densities 0.12/0.12/0.12/0.08
  instead of the OEM's "other" row, so calibration raises the LED current and
  the scan runs ~1.32x the open-gate light (~1/3 stop more shadow range);
  `RAW.flat.json` records `scan_duties` and the decoder scales the flat gain by
  calibration duty / scan duty for slides. Rescanned the same strip
  2026-10-15: currents 2/1/1/2 (red stepped up), scan duties
  1185/1229/1036/1641; the decoded densities match the first scan (median
  1.28/1.45/1.68 vs 1.28/1.42/1.67), so the compensation is right; the noise
  floor in the black gaps fell 13/22/23% (R/G/B, 24.6/18.7/17.5 -> 21.3/14.5/13.5
  counts); raw clipping only on the leader/tail edges (first/last 300 rows),
  none in pictures. Visually a subtle gain. The gap level also fell after
  compensation (89 -> 73 counts in G), so part of the "stray light" is additive
  (not scaled by the LEDs), likely a residual black-level error.
  Second strip 2026-10-15: clean scan, but the detail-based frame grid
  missed the gaps (pitch 2732) and the stray light measured at its cuts
  crushed the shadows; slides now frame from the black gaps
  (pitch ~3180 on all three raws; now the OEM framing, `tlx_frame_grid`)
  and measure stray light
  on the gap rows, capped at D 2. Still to do: slide orientation
  (`--rotate 90` comes out wrong).
- **B&W C-41 (chromogenic: XP2, BW400CN) — done 2026-10-25, a test roll
  scanned and processed well in the app (web film type "B&W C41") the same
  day:** `pakon_replay --scan-code --film bwc41` calibrates and scans with
  the OEM's B&W C-41 densities 0.10/0.25/0.25/0.08 (`pakon_lc_density_bwc41`,
  TLB.dll film type 8). The image is dye, so IR stays on and ICE works.
  Decode as silver B&W (NegMatrix, then grey); web film type "B&W C41"
  (`GREY_FILMS` in `web/decode.py`). Its calibration has its own cache file
  (`calibration-bwc41-*.bin`).

## Feature work (independent of the replay-free steps)

- **Ansel scene balance (SBA + FOS)** — done 2026-10-09: the web export runs
  the OEM's own code from `oem/PakonIMAu.dll` under emulation
  (`tools/oem_sba.py`, `docs/IMAGING.md`). Left: SCPLut, DX-code DPI
  selection, the CLI (`pakon_image.py`), a Python port verified against the
  emulator, and a TLX reference scan to check against.
- **Web UI: show that RAW/TIFF exports are being generated.** Exporting raw
  negatives or TIFFs takes a while with no feedback: each button just
  navigates to `/api/export?fmt=…`, and the server builds the whole zip
  (deflate-compressing every 16-bit TIFF) before the download starts, so
  the page looks idle. Fix: disable the button and show "Preparing N
  TIFFs…" while it runs, ideally with per-frame progress (build the zip as
  an SSE job like Process/Export, then hand the browser the download), and
  consider `ZIP_STORED` for the TIFFs, since deflating large 16-bit images
  is most of the wait.
- **DX barcode (frame numbers, film identity).** Serial 3054's sensor
  decodes: every `0x90` reply in our three scan captures parses, with one
  parity-clean code per half frame (0A..36A on the 36-exposure roll) and the
  film's product/generation code (`docs/PROTOCOL.md`, DX sensor block).
  `pakon_dx` decodes the block, the event service logs it (`dx:` lines at
  `PAKON_DEBUG=2`), and a scan prints a summary and writes `<raw>.dx.json`.
  The web app looks the code up in the OEM product-code table, shows the
  film beside the raw, and writes its name (EXIF ImageDescription, XMP
  description and keyword) and ISO (EXIF/XMP) into every exported JPEG and
  TIFF; a code the 2006 table does not know (both our full rolls) writes
  nothing. Frame numbers are not used (placing codes on frames is the open
  question in dx-barcode.md). Not needed for colour: the OEM
  install ships only default film/colour LUTs; the DX-keyed data it does
  ship is 11 noise-reduction parameter files (inferred from the file names,
  not traced in code).
- **Release CI: installable builds on every release.** Each time a release is
  cut (a `v*` tag / GitHub release), a CI workflow builds executables for
  **macOS**, **Windows** and **Linux (`.deb`)** and publishes them to a
  download page (GitHub Release assets, linked from a GitHub Pages page).
  Points to settle:
  1. What ships: the C tools (`pakon_probe`, `pakon_replay`) need libusb per
     platform (Homebrew on macOS, vcpkg/MSYS2 on Windows, `libusb-1.0-0`
     as a `.deb` dependency); the web app needs Python + its requirements, so
     either a bundled app (e.g. PyInstaller) or an installer that sets up a
     venv.
  2. Windows needs a USB driver binding for libusb (WinUSB via Zadig or an
     `.inf`), and the C code has POSIX bits (`nanosleep`, `usleep`) to port.
  3. The OEM firmware (`firmware/PknLdr.hex`, `firmware/Pakon7.hex`) ships
     in the repo since October 2026, so the app bundles it; users no longer
     supply their own.
  4. macOS signing/notarization, or document the Gatekeeper workaround.

- **Digital ICE (IR dust/scratch removal)** in `tools/pakon_image.py` as
  `--ice`, then a checkbox in the web UI. About 1 day; testable offline on our
  F-135 captures, whose 8000-sample rows already carry the IR block (today it
  is discarded).
  **Prototype, 2026-10-15 (`tools/pakon_ice.py`, not wired in yet):** a numpy
  port of nkscan's `dust.rs` (MIT/Apache; the openICE pipeline, GPL-3.0,
  AGPL-compatible), Coolscan LS-5000 constants. Pakon side: the IR plane
  (flat-fielded against the open gate, black 300: our IR dark table is taken
  with the IR LED on), IR/visible offset measured 0 rows / 0 columns, gate
  margin ~3.5% (`PAKON_METERING` 0.985; Nikon's ~2% also flagged noise and
  edges), confidence spread over rows too. Tried on a slide strip and a C-41
  roll (serial 3054): hairs and a scratch removed, 0.1-0.5% of pixels
  rebuilt, ~2.4 s per frame; the red -> IR dye leak is 0.014 on the slides,
  0.040 on C-41 (independently 0.041, corr 0.98).
  **2026-10-20:** in the web app (Frames: "Remove dust (ICE)", applied at
  export; off for raws without an IR lane, i.e. B&W since IR off). Works at
  every base (IR block = last quarter of the row; dpi from the width); the
  IR plane takes out the slide / B&W scan-vs-calibration IR duty (1.2x).
  The first ~40-60 columns of every line have amplified, heavy-tailed IR
  noise (the gate shades the line start: open gate 0.26x at column 0, noise
  2.6-3.8x, dark tail 3-6x), which passed the gate as specks: 15-36% of the
  rebuilt pixels. The gate margin is now widened per column by
  max(light fall-off from the calibration, the frame's own dark tail); the
  edge share fell to 7-22%, the middle unchanged, real hairs and scratches
  still removed (C-41 Base 4 / 8, slide Base 16; ~0.4 / 1.1 / 2.4 s a
  frame). Cause of the dark tail unknown (a dark scan would tell).
  **2026-10-25:** `pakon_image.py --ice` (the web export and the CLI share
  `pakon_ice.clean_frame`); `test/test_ice.py` (synthetic frames: a clean
  frame untouched, a speck and a hair rebuilt to within 5%, black kept, the
  IR plane's gain and duty ratio, the line-start noise and tail factors, a
  scratch along the scan not raising its own margin). B&W C-41 (XP2, Base 16,
  36 frames, `bwc41`): 0.31-0.80% of each frame rebuilt, 1-4% of it in the
  first 3% of columns, ~20% in the darkest quarter of the picture (no image
  ghost); hairs, specks and lint removed cleanly. Most hits are 3-20 px
  specks spread evenly over the frame, which at 1:1 are grain replaced by
  grain (no visible change): the IR of this film is grainier than colour
  negative's, a candidate for a wider margin if it ever shows. The plan it
  followed:
  1. Split the IR block per row; align it to the visible image (the OEM
     calibrates an "IR lag", so it may be offset; measure it like the
     trilinear R/G/B leads).
  2. Remove dye crosstalk from the IR (the OEM's `IrCrossTalkFactor`: cyan
     dye absorbs a little IR, so image detail leaks into the IR channel).
  3. Defect mask: coarse (~64 px) local background of the IR, flag pixels
     clearly darker than it, dilate ~6 px.
  4. Fill masked pixels from the smallest surrounding box with enough clean
     pixels (boxes 5 / 17 / 49 px), per channel.
  5. Grey the option out for silver B&W film (IR is blocked by silver; B&W
     C-41 is fine).

  Prior art: [opticfilm135i-linux](https://github.com/cgillinger/opticfilm135i-linux)
  (`of135i/image.py`, same method; GPL-2.0 with no "or later" found, so
  **read only, do not copy** into this AGPL project) and
  [darkoom-infrafill](https://github.com/Inrixia/darkoom-infrafill) (MIT,
  mask only).

## Open questions to settle on hardware

- Stopping: clearing the acquire bit then `a2` stops the motor on the F-135
  (2026-10-09, `--scan-code`). Still to confirm on the F-135+.
- ~~Do the LEDs light with the motor stopped?~~ Yes (2026-10-09): with
  non-zero `0x81`/`0x82` the open gate reads full scale, motor stopped.
- ~~Why our F-135 captures use `0x91` = `0x0010`; the two unresolved inputs
  of the `0x91` formula.~~ Resolved 2026-10-23 (`docs/TLB_FINDINGS.md`, "0x91
  value"): 23700 µm image height and the output width; `0x0010` is Base 16 +
  IR. The same table predicts F-135+ Base 16 + IR (step 6): integration
  `0x9c2`, `0x91` = 49 (`0x31`); to verify on serial 16402.
- Row stride at the highest resolution with IR on for the F-135+ (expected 8000).

## Safety rules (from pakon-reference, apply to every new command path)

- Never send type byte `0` (wedges the bridge until power cycle).
- Never send to bootloader addresses `0x22`/`0x26`/`0x42`/`0x46` except the
  OEM presence probe `04 03 <addr> 00 00` (used by `--open`); type-4 commands
  `0x0C`–`0x0F` there erase flash.
- Never write unknown bus addresses (can erase the boot EEPROM).
- Never issue vendor request `0xA2` (writes the per-unit EEPROM).
- Replay the OEM TEC values (`0xD0 00`, `0xD1 01`); do not sweep them.
- Respect the LED current ceilings (`docs/REGISTERS.md`).

## Handy commands

```sh
cmake -S . -B build && cmake --build build && ctest --test-dir build --output-on-failure
./build/pakon_probe --load-firmware resources/f135.pakfw   # cold -> warm
./build/pakon_replay --open                                 # handshake to Idle
./build/pakon_replay --read-params --params-out eeprom.bin  # EEPROM dump (primaries)
mkdir -p backups/eeprom                                     # git-ignored archive root
python3 tools/pakon_eeprom.py backup backups/eeprom/NAME    # all four copies + decode
python3 tools/pakon_eeprom.py decode backups/eeprom/NAME    # offline decode
./build/pakon_replay --scan resources/scan.pakscan --image scan.raw
./build/pakon_replay resources/advance.pakscan --steps N
python3 tools/pakon_image.py scan.raw --invert-c41 --rotate 90

# F-135+ (scripts from serial 16402)
./build/pakon_replay --scan resources/f135plus/base16.pakscan --image scan.raw --advance
python3 tools/pakon_image.py scan.raw --linewidth 6000 --no-ir-lane --invert-c41 --jpeg
uvicorn web.app:app --host 0.0.0.0 --port 8000
```
