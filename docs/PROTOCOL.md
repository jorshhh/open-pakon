# Pakon F-135 / F-135+ wire protocol

How a host drives the Kodak/Pakon F-135 over USB: identities and firmware,
frames, the controllers, the open, calibration, the scan, film transport and
teardown. Register encodings are in [REGISTERS.md](REGISTERS.md), the
per-unit EEPROM in [EEPROM.md](EEPROM.md), the image data and colour path in
[IMAGING.md](IMAGING.md), model differences in
[SCANNER_FAMILY.md](SCANNER_FAMILY.md).

Confidence: **[C]** confirmed (on hardware, in captures, or read from the
OEM engine's code); **[INF]** inferred. Sources, named where it matters:

- USB captures of the OEM software and hardware runs of this project's code on
  a base F-135, **serial 3054** (captures in `resources/`);
- OEM driver-level captures and live runs on an F-135+, **serial 16402**
  (Ali Bosworth; scripts in `resources/f135plus/`);
- `TLB.dll` 3.1.0.28, the OEM F-135/F-135+ scan engine (function names are
  the engine's own, from its error-ID table; addresses are into that build).
  `TLA.dll` is the F-235 engine and is not a source for this scanner;
- [pakon-reference](https://github.com/alibosworth/pakon-reference) (CC BY
  4.0) for family-wide facts: EEPROM layout, DX, safety incidents.

Names: **LOW** is the lower controller (LEDs, lamp temperatures, DX, FIFOs;
called PICL elsewhere), **SCN** the scanner controller (CCD FPGA register
banks and the film motor; called PICM elsewhere). The addresses below are the
F-135's; the F-135+ uses the same protocol at other addresses (see "Model
differences").

## USB identities and firmware

| State | VID:PID | Meaning |
|---|---|---|
| cold | `0f05:f235`, bcdDevice/revision `aa07` | FX2 bootstrap from the boot EEPROM (I2C `0x51`); no scanner protocol |
| warm | `0f05:f135` "Pakon F135-USB Film Scanner" | operational, after the host loads firmware |

The scanner is a Cypress EZ-USB FX2 bridge in front of the two controllers.
The F-135 and F-135+ are identical at the USB level (same IDs, same FX2
image, same endpoints); only the controller probes tell them apart [C, both
models]. Firmware is RAM-only: a power cycle returns the unit to cold [C].

**Personality.** The cold device carries an 8-byte C0 boot record, read
during the load: `c0 05 0f 35 f2 07 aa 04` on serial 3054 [C] (signature
`c0`, VID `0f05`, PID `f235`, revision `aa07`, a config byte). The OEM
driver picks the FX2 image by `<PID>_<revision>`; `F235_AA07` selects
`Pakon7.hex`, used by both F-135 and F-135+ [C]. Other personalities are in
[SCANNER_FAMILY.md](SCANNER_FAMILY.md).

**Load sequence** [C: 1104 transfers, byte-identical to the OEM load
captured in `resources/f135.pakfw`; run from cold on serial 3054]. All are
vendor control transfers on EP0, one transfer per Intel HEX record (≤ 16
bytes), in file order:

1. Hold the 8051 in reset: CPUCS = 1, written with `0xA0` at both `0x7F92`
   (EZ-USB) and `0xE600` (FX2), twice.
2. The stage-1 loader (`firmware/PknLdr.hex`, a record table extracted from
   the OEM `F235Ldr.sys`) to internal RAM with `0xA0`. All its records are
   below `0x2000`.
3. Run it: CPUCS = 0 (both addresses).
4. Post-bootstrap handshake: vendor OUT `0xA4`, `wValue 0x00A1`, no data.
5. Personality read: vendor IN `0xA9`, `wValue 0`, `wIndex 0`, 8 bytes.
6. The main image (`firmware/Pakon7.hex`): records at `≥ 0x2000` to external
   RAM with `0xA3` (served by the stage-1 loader); then CPUCS = 1; then the
   records below `0x2000` to internal RAM with `0xA0`.
7. CPUCS = 1, then CPUCS = 0: the image runs and the device re-enumerates
   as `0f05:f135` (within a few seconds).

The last transfers may fail at the USB level because the device
re-enumerates under them; that is normal. Load only when the cold ID is
`0f05:f235` and the personality read in step 5 is `F235_AA07`
(`c0 05 0f 35 f2 07 aa ..`): another project loaded F-235 firmware onto an
F-135 from an unstable personality read, which lit a fault LED (recoverable
by power cycle). This implementation refuses any other personality.

## Endpoints and control requests

Warm device: interface 0, alternate setting 0, three bulk endpoints [C]:

| EP | Dir | Role |
|---|---|---|
| `0x01` | OUT | command frames |
| `0x81` | IN | replies |
| `0x86` | IN | image stream; the OEM reads 20480-byte transfers (40 × 512-byte packets) |

EP0 vendor requests (the OEM driver passes only `0xA0` and `0xA2`–`0xAC`):

| Request | Use |
|---|---|
| `0xA0` | FX2 internal RAM write / CPUCS (firmware load) |
| `0xA3` | external RAM write (firmware load, via the stage-1 loader) |
| `0xA4` | I2C EEPROM select, `wValue = ((0x50 + n) << 1) \| dir`, `wIndex 0x1234`; `0x00A1` in the firmware load |
| `0xA9` | EEPROM read (`wIndex 0x1234`), or the 8-byte personality (`wIndex 0`, stage-1 loader) |
| `0xA2` | **EEPROM write. Never send it.** |

The EEPROM read is in [EEPROM.md](EEPROM.md#reading-it): per chunk, select
`0xA4 wValue 0x00A5` (per-unit EEPROM `0x52`), then `0xA9 wValue = offset`,
`wLength ≤ 32`. `wValue 0x00A3` selects the boot EEPROM at `0x51` [C on
serial 16402, pakon-reference].

## Frames

On EP `0x01`: `[type][count][data × count]`, on-wire length `2 + count`, no
checksum, no padding (34 data bytes at most). `data[0]` is the bus address
[C].

| Type | Name | Host frame |
|---|---|---|
| `0x01` | READ | `01 03 <a> <n> <reg>`: read `n` bytes of `reg` |
| `0x02` | WRITE | `02 <n+3> <a> <n> <reg> <data × n>` |
| `0x03` | STATUS | `03 01 <a>`: one-byte status poll |
| `0x04` | COMMAND | `04 03 <a> 00 <cmd>` |
| `0x07` | reply | reply to COMMAND / WRITE; never sent by the host |

**Type `0x00` wedges the FX2 until a power cycle (a USB reset does not clear
it). Never send it.** An invalid payload is harmless (status 2); an invalid
type is not.

### Replies

On EP `0x81`. A reply echoes the address in `data[0]` and carries a status
or flags byte in `data[1]` [C]:

| Request | Reply |
|---|---|
| COMMAND, WRITE | `07 02 <a> <status>` |
| STATUS | `03 02 <a> <flags>` |
| READ | `01 <n+2> <a> <flags> <data × n>` |

Status values (`07` replies; OEM driver enumeration, via pakon-reference):

| Status | Meaning | OEM handling |
|---|---|---|
| `0x00` | success | OK |
| `0x01` | not acknowledged (device absent or command rejected) | error (absent, for a probe) |
| `0x02` | invalid packet | error |
| `0x03` | bad checksum | retry |
| `0x04`, `0x05` | USB-layer error | error |
| `0x06` | USB-layer error | retry |
| `0x07` | host-algorithm error | error |
| `0x08` | success (seen in some sequences) | OK |
| `0x09` | bus error | retry |

A reply too short to carry a status byte has no status; never read it as
success.

Flags byte of READ and STATUS replies [C, `PPB_CheckReply` in `TLB.dll`]:

| Bit | Meaning |
|---|---|
| `0x01` | busy: retry |
| `0x02` | (host `0x10` only) image FIFO overflow |
| `0x04` | error, except in a reply from the host |
| `0x08` | set in ordinary READ replies (`0x08`, or `0x88` with an event pending); meaning unknown |
| `0x10` | retry |
| `0x20` | error |
| `0x80` | event pending (see "Event service") |

READ and STATUS replies must echo the request type. Captures also show the
host's status reply carrying one more, constant byte `0xaa`; its meaning is
unknown.

**OEM reply rules** [C, `TLB.dll`]: up to 3 tries per transfer on a retry
status; transfer timeout 2 s. After every COMMAND or WRITE (except commands
`0x01`, `0x0D`, `0x25`) the OEM polls `03 01 <a>` until flags bit 0 clears,
up to 44 tries with a growing delay. This implementation sends the busy poll
after every write and command it generates.

The bridge must be opened (HostReset + HostSetMode, below) before any
controller traffic flows: frames sent without it time out [C, F-135+].

## Bus addresses

| Addr | Device |
|---|---|
| `0x10` | host: the FX2 bridge itself |
| `0x20` | LOW (F-135) |
| `0x24` | SCN (F-135) |
| `0x40` / `0x44` | LOW / SCN (F-135+) |
| `0x22` `0x26` `0x42` `0x46` | the controllers' bootloaders: **never address**, except the presence probe |
| `0x28` | focus steppers (only with the OEM focus option) |

Bus addresses are I2C addresses shifted left one bit. An address that is
not a known controller may be an I2C device, including the EEPROMs (`0xA2`
= boot EEPROM `0x51`, `0xA4` = per-unit EEPROM `0x52`). Send nothing to any
other address.

## Open handshake and model detection

```
04 03 10 00 85        HostReset          -> 07 02 10 00
02 04 10 01 8f 00     HostSetMode 0      -> 07 02 10 00
04 03 44 00 00        probe SCN, F-135+  -> 07 02 44 <00 present / 01 absent>
04 03 46 00 00        probe its bootloader
04 03 24 00 00        probe SCN, F-135
04 03 26 00 00        probe its bootloader
```

- The bridge answers HostReset and HostSetMode only on the first open after
  power-on or a firmware load; on later opens those two replies time out
  while the bridge works normally. Treat them as best effort.
- The OEM (`TLB.dll` `FUN_1000afd0`) probes `0x44`, `0x46`, `0x24`, `0x26`
  in that order: `0x44` or `0x46` answering means F-135+, else `0x24` or
  `0x26` means F-135. A controller that answers only at its bootloader
  address still counts as present to the OEM.
- An F-135 answers `0x24` present, `0x44` absent; an F-135+ the reverse.
- A bootloader answering means that controller is in its firmware-update
  state. This implementation then reports an error and does not drive the
  scanner. It probes `0x44`, `0x46`, `0x24` for `--open` and only `0x44` and
  `0x24` before a scan.
- Cross-check: the EEPROM type word is 1350 on an F-135 and 1351 on an
  F-135+ ([EEPROM.md](EEPROM.md)). A scan refuses an EEPROM archive whose
  type does not match the probed model.

## Controller init

Sent once after model detection (OEM order, as captured on both models;
this implementation's `setup::init` matches all five setup captures byte
for byte, leaving out event-service traffic and the status-LED word). Every
WRITE and COMMAND is followed by the busy poll.

| # | Frame(s) | What |
|---|---|---|
| 1 | SCN `0x97` = `01` | SCN controller init |
| 2 | LOW `0x03` = `01`, read 12 bytes of LOW `0x07` | module info (version words, board serial field) |
| 3 | SCN `0x03` = `01`, read 12 bytes of SCN `0x07` | module info |
| 4 | ResetFifos: host `02 04 10 01 84 02`, `04 03 LOW 00 8a` | |
| 5 | LOW `0x8F` `e8 ff 18 00`, `0x8C` `e0 ff 20 00`, `0x8B` `f0 00 20 03`, `0x8D` `a0 00 70 03` | lamp / motherboard temperature bands |
| 6 | LOW `0xD0` = `00`, `0xD1` = `01` | TEC setup: replay as-is |
| 7 | LOW `0x87` = `00 00` | unknown |
| 8 | LOW `0x80` = `01`; `0x82` = zero duties + period word; `0x80` = `00` | LEDs enabled, zero, off |
| 9 | LOW `0x89` = `00` | per-resolution flag |
| 10 | bank `0x82`: reg 6 = `0x0FFD`, reg 0 = `0x0060`, reg 11 = 0, reg 4 = `0x003E`, reg 5 = `0x080E`, regs 1–3 = 0, reg 10 = `0x0400` | CCD FPGA defaults (pixel window 62..2062, the `TLB.dll` default) |
| 11 | bank `0x84`: reg 0 = `0x0078`, reg 1 = `0x0080` | AFE config |
| 12 | `04 03 SCN 00 a2` | release the motor drive |
| 13 | bank `0x82` reg 9 = `0x0014`, then `0x0017` | front-panel LEDs: middle green |

The init period word is `0x0997` on the F-135 and `0x03D6` on the F-135+
[C, captured]. They equal `0xFFD × 0.6` and `0xFFD × 0.24` (the F-135+ clock
being 2.5× faster) [INF].

Module info on the F-135+ (serial 16402): LOW `88 0f 0a 05 00 00 "12345"`,
SCN `08 10 06 05 00 00 "12345"` (reply flags first), i.e. firmware NL05/0A
and NM05/06, with a factory placeholder serial [C]. The engine treats a LOW
controller that is not type `'D'` with firmware version below 200 as old
(see "Lamp gate"); `'D'` also marks the Plus elsewhere in the engine [INF:
the type character comes from this module info].

## Mode configure

Per resolution and IR mode, after init (`setup::configure`): ResetFifos;
LOW `0x91` = trigger + `01`; then, writing a register only when its value
changes (as the OEM does), in captured order: bank `0x82` reg 0 IR bit
(`0x100`), reg 6 integration, reg 0 dual-tap bit (`0x002`), reg 4 = 6 (3
dual-tap), reg 5 = EEPROM Offset + 2000 (1000 dual-tap), LOW `0x89`; then
ResetFifos. This is the **calibration window**: reg 4 starts on the
optically black pixels; the scan start later moves reg 4 to the Offset.

The per-mode values (integration, trigger, dual-tap, LOW `0x89`, line
width) for both models are in [REGISTERS.md](REGISTERS.md#scan-modes).

## Lamp gate

The OEM calibrates only on a stable lamp [C, `TLB.dll`]:

- `FN_bLampTemperatureStable` polls every 250 ms until the engine's
  "lamp stable" flag is up; after 300 s it fails with `EC_LampWarmUpFailure`,
  and it bails out on a hardware fault. Calibration waits on the same flag.
- The flag starts at −1 once per driver session and is set only by the event
  service: a LOW event whose status has any bit of `0x5B` is examined; status
  bit `0x02` moves the flag −1 → 0 and, in the same pass, 0 → 1 (stable).
  When the engine's timer runs out it also reads LOW `0x83` (lamp flags: bit
  `0x02` the same, `0x20` warning, `0x40` fault) and the temperatures `0x84`,
  `0x88`. A LOW controller that is not type `'D'` with firmware version below
  200 skips all this and sets the flag on its first event.
- In `resources/calib_prelude.pakscan` the one event before calibration is
  LOW status `0x02`, 38 host polls (~7.6 s) after `d1 01`.

Measured on serial 3054: the LOW `0x02` event arrives on the first poll when
the lamp is already at its setpoint; right after a calibration (lamp at
40.5–40.6 °C) the scanner held the event back ~14 s until the lamp read
40.3 °C, with the `0x83` flags at `0x00` throughout, so the event is what
ends the wait. The lamp settles in seconds; 300 s is only the OEM ceiling.
Lamp setpoint read back as 40.0 °C [C].

This implementation (`cmd::wait_lamp`): event service every 200 ms, LOW
`0x83` and `0x88` once a second; stable on a LOW event status or flags with
bit `0x02`; a flags fault (`0x40`) or 300 s ends the run.

## Light calibration

Light calibration (LED currents and duties, A/D gains and offsets, per-column
tables) is **not** in the EEPROM: the OEM derives it on the scanner and
keeps it in the Windows registry (`…\Pakon\TLB\Scan\DpiBase{4,8,16}_35`:
`Gain_R/G/B`, `Offset_R/G/B`, `Current_R/G/B/Ir`, `DutyCycle_*` (film),
`DutyCycleOpenGate_*`, `FullLightCorrections`, `SpliceDarkness`,
`DetectWhite_G`, `DetectFilm_G`). Per-column tables are kept in memory only.
A client measures it itself.

### Conditions

Read from `TLB.dll` [C] unless marked.

- **Static: open gate, motor stopped.** Nothing under `CalibrateBefore` /
  `FindCorrections` drives the film transport (only the optional focus
  steppers at `0x28`). The LEDs are switched on explicitly with LOW `0x80`,
  `0x81`, `0x82`; nothing refreshes them and the motor does not gate them.
  Confirmed on serial 3054: with non-zero `0x81`/`0x82` the open gate reads
  full scale, motor stopped.
- After a stable lamp (above); refuses positive film (film type 2) and
  non-35 mm formats, so slide film calibrates under another type and applies
  its own densities at scan.
- **When** (`FN_bBeforeScan`): one record per film format, film type (2
  counted as 1) and IR flag, with two timestamps: last calibration and last
  full one. Recalibrate when there is no record, the last calibration is
  older than `DarkPointCorrectIntervalMinutes` (default 60), or it is forced.
  A full calibration (LED current search) when the last full one is older
  than 72000 s (20 h); otherwise from the saved currents. A scan flag
  `0x10000` adds 900 s to "now" (meaning unknown).

### The calibration line

With acquire on (bank `0x82` reg 0 bit 0) and the motor stopped, EP `0x86`
streams lines continuously. Measured on serial 3054 [C]:

- A line is `blocks × (reg5 − reg4)` u16 samples: the RGB block (pixels
  interleaved R, G, B), then, with IR on, the IR block (`blocks` = 4, or 3
  with IR off). Base 16 + IR on serial 3054 (window 6..2043): 8148 samples;
  IR off: 6111.
- At Base 8, LOW `0x89` = 1 narrows the line: 1527 px measured out of a
  2036 px window (about ¾, matching the 1500 px output). Base 4 (dual-tap,
  window 3..1021): the line is the window, 1018 px.
- The line marker (bit 0 of the first word of each line, set on no other
  sample) is present in calibration reads too, lit or dark, IR on or off.
  (An earlier IR-on probe reported no marker; the later IR-off work found it
  in every mode.) Where a read starts within a line varies from read to
  read, so each read is aligned on the marker. Aligning instead on the
  black-to-lit step at the start of each block assumes 20 black pixels and
  lands early on a unit whose IR block has fewer (serial 1101).
- The first ~20–22 pixels of each block are optically black; the level
  ramps to the open-gate level by pixel ~37 (Base 16).
- Scan column `j` is calibration pixel `Offset − 6 + j` (the scan window
  starts at the EEPROM Offset, the calibration window at 6) [C: the empty
  gate profile matches column for column].
- Re-arm with ResetFifos (host `02 04 10 01 84 02`, LOW `8a`) after changing
  the light or the A/D settings, then read; skip a few lines.
- After an A/D offset write, wait ~300 ms before measuring the dark level;
  without it the reading varies by about ±50 counts (one offset code ≈ 54
  counts).
- The IR LED drives only the IR block; the visible LEDs leave it unchanged.
  The IR LED adds 13–17 counts to the RGB dark level.
- With acquire on, keep reading `0x86` and serving events. A host-side stall
  of ~0.33 s with acquire on and nobody reading (Base 16) was followed by a
  LOW event with status `0x42` and every channel reading black from the
  next re-arm, although the LEDs were still commanded on; removing the stall
  fixed it (3 of 3 runs) [C for the observation, INF for the cause].

### Phases

`CalibrateLEDs` (`TLB.dll` `0x10020dc0`), read from code [C]:

1. Window: the first 6 pixels are optically black (3 dual-tap); the active
   columns start at `Offset − 6`.
2. Lamp off (LOW `0x80` = 0).
3. Dark offsets: A/D gain code 13 (g ≈ 1.2), offsets starting at 10/10/10;
   up to 8 iterations of 32 lines; target black-pixel mean 300 ± 32; step
   `off += round((mean − 300) × −0.0133929)`.
4. Fixed-pattern dark: 128 lines averaged per column; 128 summed for the
   black and active means.
5. LED current search (full calibration only): current from 1, +1 per
   32-line pass until the column peak exceeds the cap (R, G 64000; B 65500;
   IR 40000) or the current reaches its ceiling; then duty = cap × (n−1)/n.
6. Duty refine: up to 32 iterations, `duty × 63968 / peak` (IR 39968),
   converged when the peak is in [63936, 64000] (IR [39936, 40000]); two
   converged passes 200 ms apart. If still short at full duty: current + 1;
   at the current ceiling, A/D gain code + 1 (max `0x3E`); else
   `EC_InsufficientLight`.
7. Fixed-pattern bright (formulas below).
8. IR mode only: the IR LED alone; visible dark tables become
   `v[c] − (min − 300)`.
9. Save to the registry.

Calibration duty caps follow the film base: `cap = (period − 2) / 10^D`, and
at scan `duty = open-gate duty × 10^D` (clamped to period − 2), with the
film-base densities D (R / G / B / IR, `TLB.dll` `FUN_10020230`):

| Film | D |
|---|---|
| colour negative (type 1) | 0.144 / 0.40 / 0.715 / 0 |
| B&W C-41 (type 8) | 0.10 / 0.25 / 0.25 / 0.08 |
| other: positive at scan, silver B&W (types 2, 4) | 0 / 0.03 / 0 / 0.08 |

This implementation uses 0.12 / 0.12 / 0.12 / 0.08 for slide instead of the
"other" row (more light at scan, ~⅓ stop more shadow range).

### Per-column tables

`TLB.dll` `0x1001f550` [C]. From 128-line sums of the dark (D) and bright (B) passes, per column `c`:

- `den = (B[c] − D[c]) − (Bmask − Dmask)`, where the mask terms are the
  optically black pixels' drift between the two passes;
  `gain[c] = min(125 × 2³² / den, 0x3FFFF)` (IR: `0x7530000000`), a 16.16
  fixed-point gain that takes the open gate to 64000 (IR 60000);
- smear = `65536 × (Bmask − Dmask) / (Bactive − Dactive)`, kept only within
  1..699 (0 on serial 3054);
- applied per line as `(raw − dark[c]) × gain[c] / 2¹⁸`, so the open gate
  lands at 16000 in the 14-bit domain. Details in [IMAGING.md](IMAGING.md).

**Cleaning check** (`FN_bCalibrateAfter` `0x1001dd80`, when enabled): on the
open-gate table, skip 20 columns at each edge, slide a window and flag
`CLEANING_REQUIRED` when a window and its neighbour differ by more than
`(1000 + setting) / 1000` on any of R, G, B (a narrow dip = dust on the
gate). Defaults are registry values, not in the install.

### This implementation

`pakon-scan` `calib` and `pakon-proto` `lightcal` run the OEM phases, with
these differences, all verified on serial 3054 (F-135, Base 16 / 8 / 4, IR
on and off):

- IR on: the dark steps run with the IR LED alone (current 1, half duty, LOW
  `0x80` = `0x02`), so the RGB dark tables include the IR LED's 13–17
  counts; IR off: every LED off, as the OEM.
- The line width is the register window at Base 16 and 4, and measured at
  Base 8 (from the marker spacing, or the variance of the mean line), inside
  the first dark read so the FIFO never stalls.
- A channel counts as settled when its peak is in the window or the next
  refine step moves its duty by at most one tick (at B's duty of ~273, one
  tick is ~234 counts, wider than the 64-count window).
- Reuse rules as the OEM (60 min / 20 h), one saved record per film type,
  resolution base and IR state.

Reference results, serial 3054, Base 16 + IR (for sanity checks only; per
unit):

| | This implementation | OEM |
|---|---|---|
| A/D offsets R/G/B | −38 / −31 / −31 (−39 on some runs) | −38 / −31 / −31 |
| A/D gain codes | 13 / 13 / 13 | 13 / 13 / 13 |
| currents R/G/B/IR | 2 / 2 / 3 / 2 | 2 / 3 / 3 / 2 |
| duties R/G/B/IR (period `0x742`) | 900–904 / 504–505 / 273–274 / 1357–1382 | 896 / 708 / 278 / 1353 |
| duration | 12–13 s | |

(G differs because the OEM's current stopped at 3.) Base 4 + IR: currents
2 / 3 / 3 / 2, period 929, duties about half of Base 16's. Base 8 + IR:
currents 2 / 2 / 4 / 2.

## Event service

The OEM runs a dedicated thread (`Thread_PpbInterrupt`, `TLB.dll`
`0x1002eb70`) for the whole session [C]:

1. Poll `03 01 10` every **200 ms**, or every **1 ms at high priority while
   scanning**. Nothing to do unless flags bit `0x80` is set (bit `0x02` =
   image FIFO overflow).
2. For LOW, then SCN: read `01 03 <a> 01 02` (reply `01 03 <a> <flags>
   <status>`). If the reply flags have `0x80`, acknowledge with **`02 05 <a>
   02 06 00 <status>`** (writing the status back clears those bits).
3. LOW status & `0xA4` → read the sensor/DX block `01 03 LOW 1e 90`.
4. LOW status & `0x5B` → lamp: read `0x83` (flags, when the engine's timer
   runs out), `0x84`, `0x88` (temperatures) and update the lamp-stable flag
   and the warning/fault state.
5. Update the front-panel LEDs (bank `0x82` reg 9) from the result.

In the captures every ack is from LOW; `0x90` follows 93 of 95 acks whatever
the status, and `0x84`/`0x88` follow every `0x02` and `0x40` ack. This
implementation (`setup::event_followup`) acks, then reads `0x90` after every
LOW ack and `0x84`, `0x88` when status & `0x5B`; it reproduces 91 of the 95
acks of the seven captures (the rest depend on reply data).

Without the event service a driven scan died about 38% into a strip; with
it run after every image read, whole strips scan [C, serial 3054].

**Hardware status word.** The engine folds events into a 32-bit status
whose bits are the TLX `HARDWARE_CB` enum [C, `tlx.dll` type library]:
`HOST_FAULT 0x1`, `DX_FAULT 0x2`, `LAMP_FAULT 0x4`, `CCD_FAULT 0x8`,
`MOTOR_FAULT 0x10`, `HARDWARE_ERROR 0x40`, `LAMP_WARNING 0x100`,
`LAMP_ERROR 0x200`, `TEMP_WARNING 0x400`, `TEMP_ERROR 0x800`,
`LAMP_BURN_OUT_ERROR 0x1000`, `LAMP_FAN_WARNING 0x2000`,
`LAMP_FAN_ERROR 0x4000`, `POWER_WARNING 0x40000`, `POWER_ERROR 0x80000`,
`MOTOR_STEPPER_CCD 0x200000`, `MOTOR_STEPPER_LENS 0x400000`,
`MOTOR_FILTER_WHEEL 0x800000`, `MOTOR_FILM_GUIDE 0x1000000`,
`FILM_IN_GUIDES_ERROR 0x2000000`, `BLOWER_WARNING 0x4000000`,
`CLEANING_REQUIRED 0x8000000`, `FILM_EMULSION_DOWN 0x10000000`,
`FILM_TAIL_FIRST 0x20000000`, `FILM_SENSE_ENTRY 0x40000000`,
`FILM_SENSE_EXIT 0x80000000`. The film bits come from the DX block (below).
While the engine is not ready for film, film at a sensor (with an option
setting on) aborts the scan with `EC_FilmInGuides`.

## DX sensor block (LOW `0x90`)

`01 03 LOW 1e 90` returns 30 bytes after the reply header `01 20 <LOW> 08`
(layout from pakon-reference `dx-barcode.md`, checked against every reply in
the three scan captures) [C]:

| Bytes | Field |
|---|---|
| 0–1 | position counter, big-endian (LOW `0x91` resets it) |
| 2 | entry count (≤ 5) |
| 3.. | entries, 5 bytes each: byte 0 low nibble = type, high nibble = flags (first entry only); byte 1 = flag; bytes 2–4 = data |

Entry types: 0 none; 3 decoded barcode; 5 position of the next code; 6
marks; 7 film start; 8 sensor edge pairs. Types 5, 7, 8 carry a big-endian
16-bit position in bytes 2–3.

A type-3 entry, bytes 2–4:

| Byte | Bits |
|---|---|
| 2 | 7–2 `frame_raw[5:0]`; 1 must be 0; 0 parity |
| 3 | 7–6 `product[1:0]`; 5 invalid; 4–1 generation (DX part 2); 0 `frame_raw[6]` |
| 4 | 4–0 `product[6:2]` (DX part 1) |

Parity is even over the 19 bits of bytes 2, 3 and 4[4:0]. `frame_raw` counts
half frames (frame × 2, + 1 for the "A" code); it is unsigned: a 36-exposure
roll reaches 73 (36A). 124–127 are leader labels (`X`, `XA`, `00`, `00A`).

**First-entry flags** (high nibble; `TLB.dll` `FN_bDrvGetHardwareStatusDx`
`0x10009790`) [C]: `0x20` film at the entry sensor (`FILM_SENSE_ENTRY`),
`0x10` film at the exit sensor (`FILM_SENSE_EXIT`), `0x40` emulsion side down
(`FILM_EMULSION_DOWN`), `0x80` tail first (`FILM_TAIL_FIRST`). The sensor
flags are thresholded by the firmware (per-unit DX pots). On full rolls the
nibble runs `0` → `2` → `3` … `3` → `1` → `0` as the film passes; a strip fed
the other way round also shows `0xb` / `0x9` (tail first). Emulsion down has
not been seen.

**Serial 3054 decodes the barcode** (F-135+ serial 16402 never produced a
type-3 entry) [C]:

Captures of serial 3054 (the `.pcapng` files are at git tag `python-final`):

| Capture | Sensor reads | Codes | Film (part 1 / part 2) | Frames |
|---|---|---|---|---|
| `36frames.pcapng` | 38 | 69 | 97 / 10 | 0A..36A |
| `pakon_fullroll.pcapng` | 25 | 45 | 96 / 12 | 0A..24A |
| `pakon_scan.pcapng` | 9 | 5 | 82 / 3 (KODAK GOLD 400) | 6 down to 4 |

All 119 codes pass parity, none has the invalid bit. 97/10 and 96/12 are not
in the OEM's 2006 product-code table. A few half frames are missing per roll,
which the engine fills by interpolation.

- Inside a block, type-5 and type-3 entries alternate, one pair per half
  frame, the code after its position. The first entry of a block may be
  either; a code that leads a block takes its position from the type 5 that
  ended the block before.
- Byte 1 of a type 3 (and of the type 5 beside it) is `0x01` for every code
  on both full rolls and `0x02` on the strip whose numbers count down. It
  does not mark the "A" code. Which of the two staggered sensors read the
  code is [INF], not established.
- Each scan starts with two type-7 entries (first flag varies: `0x01`,
  `0x49`, `0x79`; second `0x02`, the film start) and ends with type-8
  entries (flag `0x01` then `0x02`; the strip only `0x02`).

**How the engine uses the entries** (`TLB.dll`, partly traced): one DX object
per strip; type 7 anchors the DX position to the scan line where the pixel
test saw film; 5 gives the position of the next code; 3 the code; 6 marks;
8 pairs edges `1000·k/u ≤ gap ≤ 2500·k/u` into frame marks at the midpoint
`+ 600·k/u` (1200 tail first), with `k` = 23700 and `u` a DX unit that was
not traced. Odd ("A"-slot) codes are shifted by `26975·k/u`. Missing half
frames are filled at the measured pitch.

## Image stream

EP `0x86` carries little-endian u16 samples, one line after another; no
per-line commands [C]:

- A scan line is the RGB block (pixels interleaved R, G, B) followed, with
  IR on, by the IR block (one sample per pixel).
- Bit 0 of the first word of each line is set (the line marker); it is
  clear on every other sample. A line whose first word lacks it is
  `EC_DRV_LostSync` to the OEM (`FifoOverflow` if the host flags have
  `0x02`).
- Samples per line: 4 × width with IR, 3 × width without; width 2000 / 1500
  / 1000 at Base 16 / 8 / 4 (so 8000 at Base 16 + IR, 6000 at Base 16 or at
  Base 8 + IR, 4000 at Base 4 + IR, 3000 at Base 4).

Decoding, trilinear registration and colour are in [IMAGING.md](IMAGING.md).

## Scan sequence

The OEM runs `FN_bBeforeScan` (`0x1002dbd0`) → `FN_iScanStrips`
(`0x10029b80`) → `FN_bAfterScan` (`0x1002a900`) [C]:

1. Clear pending events (wake the event thread).
2. ResetFifos: `02 04 10 01 84 02`, `04 03 LOW 00 8a`.
3. Calibrate if due (above), motor stopped.
4. FPGA settings (`FN_bDrvPutCcdFpgaSettings`): bank `0x82` reg 0 (IR bit),
   reg 4 = EEPROM Offset, reg 5 = Offset + height, reg 6 = integration;
   bank `0x84` gains and offsets.
5. LEDs on: LOW `0x80`, `0x81`, `0x82` (scan duties).
6. Motor, no wait: `02 05 SCN 02 a5 <speed16 LE>`, `04 03 SCN 00 a0`.
7. Acquire: bank `0x82` reg 0 = control | 1 (`0x160` → `0x161` on the F-135
   with IR, `0x60` → `0x61` on the F-135+), then `02 06 LOW 03 91 <v16 LE>
   01`.
8. One overlapped read over the whole ring buffer: the driver keeps reading
   `0x86`. **No per-line commands.** The event service runs throughout.
9. End of film is decided on the host (below). Stop acquiring: bank `0x82`
   reg 0 = control without bit 0.
10. Teardown (below).

The frames this implementation sends after calibration (`setup::scan_start`,
each write followed by its busy poll), matching `resources/scan.pakscan`:

```
02 06 SCN 03 82 00 <reg0 & ~1>      acquire off
02 04 10 01 84 02 ; 04 03 LOW 00 8a ResetFifos
02 06 SCN 03 82 04 <Offset>         scan pixel start
02 04 LOW 01 80 03                  LEDs on (01 with IR off)
02 0f LOW 0c 82 <duties, period>    scan duties
02 05 SCN 02 a5 <speed>             motor speed
04 03 SCN 00 a0                     motor forward
02 06 SCN 03 82 00 <reg0 | 1>       acquire on
02 06 LOW 03 91 <trigger> 01        scan-line trigger
```

The film is fed once the motor runs (the OEM shows the right front-panel LED
solid green). Motor speed = EEPROM `MotorSpeed(_Ir) × MotorAdjust(_Ir) /
1000`, clamped (400–9500 F-135, 1000–32766 F-135+); see
[EEPROM.md](EEPROM.md#what-the-values-are-used-for).

**Never re-arm (ResetFifos / LOW `0x8A`) into a live scan stream**: it
wedges the bus. On an empty read window, poll the host instead.

**Strip mode** (several cut strips in one scan; TLX `ScanPictures` with
`STRIP_MODE_FULL_ROLL` 0 or `MULTI_STRIPS_4/5/6_FRAMES` 4/5/6) [C,
`TLB.dll`]:

- Strip buffers are allocated up front: strip length = N × pitch + pitch/4
  lines, count = roll length / strip length (≤ 15). Nothing differs on the
  scanner side.
- One session: one overlapped read for every strip, the motor runs
  throughout, one teardown at the end.
- Per strip, the reader re-measures the open-gate level from its first 3
  lines and rebases the film thresholds; at the strip's end it waits for the
  next. Before every strip after the first it resets its ring pointers (host
  side) and **resends LOW `0x91`** (same frame as at the start; the engine
  calls this write `FN_bDrvDxStart`).
- `NoFilmTimeOut` (seconds): before the first strip, an error
  (`EC_NoFilmTimeOut`); after at least one strip, the normal end of the
  session. A strip longer than its buffer is `EC_ProcessedRingTailOverflow`.
- Engine state (scanner object `+0x70`): 1 film in, 3 strip ended, 2
  waiting for the next strip; it drives the film LED.

## End of film

The scanner sends no end-of-roll signal [C]. The OEM decides on the host:

- The line level (green [INF]) is compared with the calibrated
  `DetectFilm_G` and `DetectWhite_G`, rebased from the first 3 lines: film
  starts when the level drops below `DetectFilm` and ends when it rises back
  to `DetectWhite`.
- `NoFilmTimeOut` (seconds) and `ScanPacketReadyTimeOut` (ms) bound the
  waits.

This implementation (`pakon-scan` `scan`): per 20480-byte chunk, "film" when
≥ 10% of samples exceed 8000; "open gate" when ≥ 45% exceed 55000 (its own
calibration takes the open gate to full scale; only saturated R/G/B reach
it, IR never does) or, for replayed OEM scans (open gate ~48900), ≥ 60%
exceed 40000. The roll ends after 24 open-gate chunks following film, or 2
empty ~5 s read windows after film; no film within ~2 min aborts.

## Teardown

`FN_bAfterScan`, the same in all seven captures (three F-135, four F-135+)
[C]:

```
02 06 SCN 03 82 00 <reg0 & ~1>      acquire off (FN_iScanStrips, before this)
02 04 LOW 01 80 00                  LEDs off
02 04 10 01 84 02 ; 04 03 LOW 00 8a ResetFifos
04 03 LOW 00 92                     end acquisition / close the DX window
(wait 20 ms)
04 03 SCN 00 a2                     release the drive
```

**Stopping the motor:** `a2` stops the motor only while the acquire bit is
clear. Clear bank `0x82` reg 0 bit 0 first, then `a2` [C: captures,
`TLB.dll`, and serial 3054]. A truncated scan without teardown leaves the
device in an unknown state: power-cycle. This implementation sends the
teardown on every way out of a scan, a panic included.

## Film transport

The OEM moves film with the motor commands alone: speed (`0xA5`), start
`0xA0` (or `0xA1` reverse), stop `0xA2` [C]. With acquire off, `a2` alone
stops it.

- F-135 advance (the captured advance of serial 3054): speed `0x251C`
  (9500, the top of the F-135 range), `a0`, wait, `a2`. The OEM's DX-pot
  service procedure uses the same 9500 on the F-135 and `0x7FFE` on the
  F-135+.
- F-135+ advance (serial 16402 capture): speed `0x647E` (25726, that unit's
  Base 4 `MotorSpeed`), engage, acquire on (reg 0 `0x63`), then acquire off
  → `a2`.

**Film sensing.** `01 03 LOW 04 93` returns the four DX detector levels:
bytes 0–1 the entry sensor, 2–3 the exit sensor [C, serial 3054]. Empty:
190 / 216 / 224 / 202. Under film at least one byte of the pair stays below
170 (the DX code makes it fluctuate). The sensors are 1.4 s apart at the
advance speed. When the tail leaves the drive rollers the strip stops under
the exit sensor (reading flat) and has to be pulled out by hand. Thresholds
are per unit (the DX pots are). The OEM's film-track test
(`FN_bFilmTrackTest`) counts film present when each reading is below its
empty baseline minus a margin (default not found); pakon-reference reports
it polling `0x93` about every 3 ms on an F-135+.

This implementation's eject (F-135, `pakon-scan` `transport`): motor at 9500,
event service and `0x93` every 100 ms; out when the exit sensor saw film and
both are clear for 5 polls; stalled when the entry is clear and the exit
covered and flat (±4) for 15 polls; no film after 150 polls. The motor stop
is sent on every way out.

## Timings

| What | Value | Source |
|---|---|---|
| transfer timeout | 2 s | `TLB.dll` |
| busy poll | ≤ 44 tries, growing delay | `TLB.dll` |
| host event poll | 200 ms idle, 1 ms while scanning | `TLB.dll` |
| lamp-stable poll | 250 ms, ≤ 300 s | `TLB.dll` |
| A/D offset settle | ~300 ms | measured, serial 3054 |
| duty-refine passes | 2 converged, 200 ms apart | `TLB.dll` |
| recalibration | 60 min (from saved currents), 20 h (full) | `TLB.dll` |
| teardown | 20 ms between LOW `0x92` and SCN `0xA2` | captures, `TLB.dll` |
| lamp event after TEC setup | ~7.6 s (`calib_prelude`), seconds on a warm unit | captures, serial 3054 |
| re-enumeration after firmware load | a few seconds | serial 3054 |

## Model differences

The F-135+ uses the same commands in the same order: with addresses mapped
(`0x40`/`0x44` → `0x20`/`0x24`), the F-135+ Base 16 scan and the F-135 scan
use the same 55 commands [C]. What differs: the addresses, the LED current
ceilings, the init LED period word, the motor speed range, a 2.5× faster
pixel clock (so other integration times and triggers), and the EEPROM type
word. Details in [SCANNER_FAMILY.md](SCANNER_FAMILY.md) and the per-mode
table in [REGISTERS.md](REGISTERS.md#scan-modes).

## Safety rules

These protect hardware. Every new command path follows them (from
pakon-reference `per-unit-data-and-safety.md`, the OEM engine, and incidents
on real units):

1. **Never send type byte `0`.** It wedges the FX2 bridge until a power
   cycle.
2. **Never address the bootloaders `0x22`/`0x26`/`0x42`/`0x46`**, except the
   OEM presence probe `04 03 <addr> 00 00`. A type-4 command `0x0C`–`0x0F`
   to a bootloader erases a 64-byte flash row; a real unit lost motor
   controller firmware this way and needed reflashing.
3. **Never write to unknown bus addresses.** They can be the I2C EEPROMs; a
   write sweep erased a real unit's boot personality (it then enumerated as
   a bare FX2, `04b4:8613`).
4. **Never issue vendor request `0xA2`.** It writes the per-unit EEPROM,
   whose data exists nowhere else.
5. **Replay the OEM TEC values** (LOW `0xD0 00`, `0xD1 01`) in the OEM's
   sequence; never probe or sweep them.
6. **Respect the LED current ceilings** ([REGISTERS.md](REGISTERS.md#low--leds));
   duties ≤ period − 2.
7. **No invented commands.** Every frame comes from a capture, the OEM
   engine or the reference, and is checked against a capture in a unit test.
   A teardown built on assumptions once left a unit dark and jammed film.
8. **Controller addresses come from the probes**, never from constants.
9. **Per-unit values come from the unit's own EEPROM**, never from another
   unit (not from serial 3054, not from test fixtures).
10. **Load firmware only on the cold `0f05:f235` with the `F235_AA07`
    personality.**

Operational rules that follow from the protocol:

- Clear the acquire bit before `a2`, and always send the teardown after a
  scan; after a truncated scan, power-cycle.
- Never re-arm the FIFOs (LOW `0x8A`) into a live scan stream.
- Keep the event service running while acquiring.
- FPGA writes accept only banks `0x82` and `0x84`.
- Keep motor speeds inside the board's range; never run F-135+ scripts
  (their speeds) on a base F-135.
- Keep A/D gain codes ≤ `0x3E` (`0x3F` zeroes the readout).
- Do not write the DX pots (LOW `0x94`/`0x96`, an OEM service procedure).
- Read the EEPROM once per power cycle (see [EEPROM.md](EEPROM.md)).

How this implementation enforces them: a frame of type 0 cannot be built and
is refused before sending; a sequence is checked as a whole (no empty frame,
no bootloader address) before any of it is sent; the USB layer refuses
vendor request `0xA2` whatever asks for it; LED, duty, A/D and motor values
outside the limits are refused; the firmware load refuses any personality
but `F235_AA07`.

## Where it is implemented

| Part | Code |
|---|---|
| USB transport, firmware load | `rust/crates/pakon-usb` (`device`, `fw`, `hex`) |
| frames, statuses | `rust/crates/pakon-proto/src/frame.rs` |
| init, configure, scan start, teardown, events | `rust/crates/pakon-proto/src/setup.rs` |
| calibration maths, cache | `rust/crates/pakon-proto/src/lightcal.rs`, `calcache.rs` |
| DX block | `rust/crates/pakon-proto/src/dx.rs` |
| EEPROM decode | `rust/crates/pakon-proto/src/eeprom.rs` |
| reply rules, event service, lamp gate | `rust/crates/pakon-scan/src/cmd.rs` |
| open, session | `rust/crates/pakon-scan/src/session.rs` |
| calibration, scan, transport | `rust/crates/pakon-scan/src/calib.rs`, `scan.rs`, `transport.rs` |
| EEPROM read | `rust/crates/pakon-scan/src/eeprom_read.rs` |

`rust/crates/pakon-proto/tests/setup.rs` checks the generated frames against
the captures in `resources/`.

## Open questions

- **F-135+ driven scan.** Clearing the acquire bit then `a2` stops the motor
  on the F-135; the F-135+ captures show the same order, but it has not been
  run from code there. Calibration and the code-built scan have not run on
  an F-135+.
- **F-135+ Base 16 + IR and Base 8 + IR**: integration `0x9C2` / `0x850` and
  triggers 49 / 77 are predicted from `TLB.dll`'s table, never captured or
  run. Row stride at Base 16 + IR on the F-135+ (expected 8000 samples).
- **LED period on the F-135+** for modes other than the captured ones (the
  ×0.6 rule assumes the F-135 clock).
- **Reply flags bit `0x08`** and the host status reply's trailing `0xaa`.
- **Event status bits** beyond the two groups the engine tests (`0xA4` → DX,
  `0x5B` → lamp); what LOW status `0x42` during a stalled calibration means.
  (An independent driver, libpakon, treats LOW `0x04`/`0x40` as film end,
  `0x20` as scan progress, `0x02` as service; not checked against
  `TLB.dll`.)
- **Unnamed registers**: LOW `0x87` (written `00 00` at init), the exact
  effect of LOW `0x89` (narrows the line to ~¾ at Base 8), the layout of the
  12-byte module info at `0x07`, bank `0x82` regs 1–3 and 10–11, bank
  `0x84` regs 0–1.
- **TEC**: what `0xD0`/`0xD1` set. pakon-reference lists them as F-135+ only
  (its CCD is cooled), but the OEM engine and the captures send them on the
  base F-135 too.
- **Advance speed**: how the OEM picks 9500 (F-135) and 25726 (F-135+) for
  film advance.
- **DX**: which staggered sensor reads a code (type-3 byte 1); the DX
  position unit (`TLB.dll` DPI `+0x9c`, set outside the traced code); type-6
  entries; the emulsion-down flag (never seen); the first type-7 flag.
- **OEM defaults not found in the install**: `WaitForLamp_*` settle times,
  the temperature threshold defaults, `NoFilmTimeOut`, the film-track
  margin, the cleaning-check window; the meaning of scan flag `0x10000`.
- **Lamp setpoint**: this init does not send LOW `0x8E`, yet the unit holds
  40.0 °C; where its default comes from.
- **IR/visible lag**: the OEM calibrates an "IR lag"; on serial 3054 the IR
  and visible planes measured 0 rows / 0 columns apart.

## Provenance

Facts come from this project's captures and hardware runs, from reverse
engineering of the OEM Windows software (`TLB.dll`, `PakonIMAu.dll`,
`tlx.dll`) for interoperability, and from the public pakon-reference. The
OEM binaries and decompiler output are third-party copyrighted material and
are not in this repository; only the interface facts needed for
interoperability are recorded.
