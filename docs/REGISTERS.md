# Pakon F-135 / F-135+ register map

Every register and command a client writes or reads, with encodings and the
values seen. Frames, sequences and timing are in [PROTOCOL.md](PROTOCOL.md);
the per-unit EEPROM in [EEPROM.md](EEPROM.md).

Sources: `TLB.dll` 3.1.0.28 (the OEM F-135/F-135+ engine), cross-checked
against the captures in `resources/` (F-135 serial 3054, F-135+ serial
16402) and hardware runs. Confidence: **[C]** read from the OEM engine,
seen in captures or measured on hardware; **[INF]** inferred. Entries are
[C] unless marked otherwise.

## Controllers

| Role | F-135 | F-135+ | Carries |
|---|---|---|---|
| host | `0x10` | `0x10` | the FX2 bridge: open, FIFO reset, event flag |
| SCN ("scanner") | `0x24` | `0x44` | FPGA register banks `0x82`/`0x84` (CCD), film motor |
| LOW ("lower") | `0x20` | `0x40` | LEDs, lamp temperatures and TEC, DX sensors, FIFO reset, `0x91` scan trigger |
| bootloaders | `0x26` / `0x22` | `0x46` / `0x42` | **never address** (presence probe only) |

The addresses come from the presence probes (see
[PROTOCOL.md](PROTOCOL.md#open-handshake-and-model-detection)), never from
constants. pakon-reference and libpakon call LOW "PICL" and SCN "PICM";
"PICM = motor, PICL = light/CCD" is misleading, since the FPGA banks and the
motor share SCN.

## Frame forms

| Operation | Bytes |
|---|---|
| command | `04 03 <a> 00 <cmd>` |
| write | `02 <n+3> <a> <n> <reg> <data × n>` |
| read | `01 03 <a> <n> <reg>` → `01 <n+2> <a> <flags> <data × n>` |
| status poll | `03 01 <a>` → `03 02 <a> <flags>` |
| FPGA register write | `02 06 SCN 03 <bank> <reg> <v16 LE>` |
| FPGA register read-back | write `02 04 SCN 01 <bank+1> <reg>`, then read `01 03 SCN 02 07` |

After every command or write (except commands `0x01`, `0x0D`, `0x25`) the OEM
polls `03 01 <a>` until bit 0 (busy) clears, up to 44 tries [C]. FPGA writes
accept only banks `0x82` and `0x84` and can be verified by read-back [C].

## Host (`0x10`)

| Frame | Meaning |
|---|---|
| `04 03 10 00 85` | HostReset; replies only on the first open after power-on / firmware load |
| `02 04 10 01 8f 00` | HostSetMode 0; same reply behaviour |
| `02 04 10 01 84 02` | first half of ResetFifos (then LOW command `0x8A`) |
| `03 01 10` | event poll: flags `0x80` event pending, `0x02` image FIFO overflow |

## Registers on both controllers

| Reg | Access | Use |
|---|---|---|
| `0x02` | read 1 | pending-event status byte; the reply's flags byte has `0x80` when an event is pending |
| `0x06` | write 2 | acknowledge: `00 <status>` clears the bits that were set |
| `0x03` | write 1 | `01` at init, before the module-info read |
| `0x07` | read 12 | module info at init (version words, board serial field); read 2 = FPGA read-back on SCN |

`0x07` returns what the preceding write selected (`0x03` → module info,
`bank + 1` → an FPGA register) [INF]. F-135+ module info: LOW `88 0f 0a 05 00
00 "12345"` (NL05/0A), SCN `08 10 06 05 00 00 "12345"` (NM05/06), reply flags
first, placeholder serial [C, serial 16402].

## FPGA bank 0x84 — CCD analog front end

| Reg | Field | Encoding |
|---|---|---|
| 0 | AFE config | `0x0078` at init |
| 1 | AFE config | `0x0080` at init |
| 2 / 3 / 4 | A/D gain R / G / B | code = round((1 − 1/g) × 75.6), g ∈ [1, 6]; 13 (g ≈ 1.2) is the OEM starting value; max `0x3E` |
| 5 / 6 / 7 | A/D offset R / G / B | sign-magnitude: bit `0x100` = negative, magnitude ≤ 255 |

Gain `0x3F` zeroes the readout on serial 3054; stay at or below `0x3E`. One
offset code moves the dark level by about 54 counts; allow ~300 ms after an
offset write before measuring.

## FPGA bank 0x82 — CCD timing and control

| Reg | Field | Notes |
|---|---|---|
| 0 | control | bits: `0x001` acquire, `0x002` dual-tap, `0x100` IR, `0x060` set at init. Seen: `0x160`/`0x161` (F-135, IR on), `0x60`/`0x61` (F-135+, IR off), `0x63` (F-135+ advance capture). Clearing bit 0 before SCN `0xA2` is what stops the motor |
| 1 / 2 / 3 | CCD exposure R / G / B | written 0 at init (`FN_bDrvInitCcd`); not the brightness lever |
| 4 | pixel start | calibration: 6 (3 dual-tap), the optically black pixels; scan: the EEPROM **Offset** for the base. Must be ≥ 6 (≥ 3 dual-tap). Init default `0x003E` (62) |
| 5 | pixel end | EEPROM Offset + height (2000; 1000 dual-tap); ≤ `0x848` single-tap, `0x424` dual-tap. Init default `0x080E` |
| 6 | integration / line period | ≤ `0xFFD`; per mode (below). Init default `0x0FFD` |
| 9 | status LEDs | front-panel LED word (below) |
| 10 (`0xA`) | timing | `0x0400` at init |
| 11 (`0xB`) | timing | written 0 at init, with the integration setting |

A line during calibration is `blocks × (reg5 − reg4)` samples; during a scan
the output width per base (below). Scan column `j` is calibration pixel
`Offset − 6 + j`.

### Reg 9: front-panel status LEDs

Three LEDs: the left one is always green; the middle and right ones are
red/green bicolour. Bits, **active low** (a set bit turns that colour off):

| Bit | Function |
|---|---|
| `0x01` | right red |
| `0x02` | right green |
| `0x04` | middle red |
| `0x08` | middle green |
| `0x40` | blink right red |
| `0x80` | blink right green |
| `0x100` | blink middle red |
| `0x200` | blink middle green |

Off = both colour bits set; red + green = orange.

| Word | Meaning | Seen |
|---|---|---|
| `0x0014`, then `0x0017` | end of init; `0x0017` = middle green, right off (idle) | init captures |
| `0x00D4` | right LED blinking orange/off: calibrating | TLX, before the first scan |
| `0x0313` | middle LED orange/off blink | `scan.pakscan` calibration |
| `0x0015` | right LED solid green: motor running, feed the film | TLX |
| `0x0217`, `0x0295`, `0x0215` | while scanning | captures |
| `0x001F` | all off | captures |

The engine (`FN_bDrvSetLed`, `0x1000c1f0`) writes only on change, with a code
= LED group | colour: group `0x00` middle, `0x40` right; colour 0 off, 1 red,
2 green, 3 orange, 4 blinking red, 8 blinking green, `0xC` blinking orange.
The right ("film") LED follows the scan state (`FN_bStateMachineLedFilm`):
off when idle, green with film in / scanning, blinking green waiting for the
next strip ("insert film"), blinking orange when a strip has ended.

## Scan modes

Per resolution base and IR state. One per-base table in `TLB.dll`
(`FUN_10011a60`) serves both models; on the F-135, `FN_bSetF135` overrides
only the integration times [C].

| Base | Height (reg 5 − reg 4) | Output width | Lines per frame | Dual-tap | LOW `0x89` |
|---|---|---|---|---|---|
| 16 | 2000 | 2000 px | 3000 | no | 0 |
| 8 | 2000 | 1500 px | 2250 | no | 1 |
| 4 | 1000 | 1000 px | 1500 | yes | 0 |

Integration (bank `0x82` reg 6) and scan-line trigger (LOW `0x91`):

| Model | Base | IR | Integration | Trigger | Status |
|---|---|---|---|---|---|
| F-135 | 16 | on | `0x0C1A` | `0x0010` (16) | captured; run on serial 3054 |
| F-135 | 16 | off | `0x0FF0` | 24 | run on serial 3054 |
| F-135 | 8 | on | `0x0913` | 28 | run on serial 3054 |
| F-135 | 8 | off | `0x0D9D` | 38 | run on serial 3054 |
| F-135 | 4 | on | `0x060D` | 64 | run on serial 3054 |
| F-135 | 4 | off | `0x0913` | 85 | run on serial 3054 |
| F-135+ | 16 | off | `0x0FFD` | `0x003C` (60) | captured; replayed on serial 16402; run on serial 138 |
| F-135+ | 8 | off | `0x0AFD` | `0x0075` (117) | captured; replayed on serial 16402; run on serial 138 |
| F-135+ | 4 | off | `0x0753` | `0x0107` (263) | captured; run on serial 138 |
| F-135+ | 4 | on | `0x04E2` | `0x00C5` (197) | captured; run on serial 138 |
| F-135+ | 16 | on | `0x09C2` | `0x0031` (49) | from `TLB.dll`'s table, never captured; run on serial 138 (rows 8000 samples) |
| F-135+ | 8 | on | `0x0850` | `0x004D` (77) | from `TLB.dll`'s table, never captured; run on serial 138 (rows 6000 samples) |

"Run" means a whole strip scanned from code with a frame pitch within ~1.3%
of the OEM 3:2 pitch [C]; "replayed" means the captured OEM session was
replayed on that unit.

**Trigger formula** (`FN_bSetF135`, `0x10010760`) [C]: `round(23700 × clk ×
0.01 / (integration × output width))`, with 0.005 instead of 0.01 when IR is
on; 23700 µm is the image height (DPI `+0x60`), so a line advances one pixel
pitch (square pixels). `clk` = 833333.3 Hz on the F-135; the F-135+ triggers
are 2.5× the F-135 formula with their integration times, so its clock is
2.5× faster. Cross-check: in every known mode on both models the trigger ≈
EEPROM motor speed / ~98 (the engine's default motor speed is 100 ×
trigger).

**LED period** (LOW `0x82` last word) = integration × 0.6, truncated, on
the F-135 (`0x0C1A` → `0x0742`; Base 4 + IR → 929) [C]; integration × 0.24
(0.6 over the 2.5× clock), truncated, on the F-135+: `0xFFD` → 982, `0xAFD`
→ 675, `0x753` → 450, `0x4E2` → 300, the period word of each Plus capture
[C, captured], and run in all six modes on serial 138. The init period word
is `0x0997` (F-135) / `0x03D6` (F-135+).

## LOW — LEDs

| Cmd | Access | Meaning |
|---|---|---|
| `0x80` | write 1 | LED enable: bit 0 visible, bit 1 IR. `00` all off; `01` visible; `02` IR only; `03` all |
| `0x81` | write 5 | LED currents, order **`B, IR, R, 0, G`**, each ≤ the ceiling below |
| `0x82` | write 12 | u16 LE **`dutyB, dutyIR, dutyR, 0, dutyG, period`**; period = integration × 0.6 (pixel clock 833333.3 Hz on the F-135); each duty ≤ period − 2 |

LED current ceilings (the OEM clamps to these; never exceed):

| Board | IR off: R / G / B / IR | IR on: R / G / B / IR |
|---|---|---|
| F-135 (SCN `0x24`) | 6 / 8 / 8 / 0 | 8 / 8 / 8 / 8 |
| F-135+ (SCN `0x44`) | 4 / 20 / 20 / 0 | 8 / 24 / 24 / 8 |

The LEDs stay on from before the scan to after it; nothing refreshes them
and the motor does not gate them. The IR LED drives only the IR block.

## LOW — lamp temperatures and TEC

| Cmd | Access | Meaning |
|---|---|---|
| `0x8E` | write | lamp temperature setpoint (`LampTempWorking`, 37–48 °C); optional |
| `0x8F` | write 4 | lamp warning band |
| `0x8C` | write 4 | lamp fault band |
| `0x8B` | write 4 | motherboard warning band |
| `0x8D` | write 4 | motherboard fault band |
| `0xD0` | write 1 | TEC setup: `00`. Replay as-is, never sweep |
| `0xD1` | write 1 | TEC setup: `01`. Replay as-is, never sweep |
| `0x87` | write 2 | `00 00` at init; meaning unknown (pakon-reference's label: light power) |
| `0x83` | read 1 | lamp flags: `0x02` stable, `0x20` warning, `0x40` fault |
| `0x84` | read 2 | setpoint read-back (40.0 °C on serial 3054) |
| `0x88` | read 4 | lamp, then motherboard temperature |

Units: 0.0625 °C per count, signed 16-bit little-endian. Each band is two
words, low then high. The values written at init:

| Cmd | Bytes | Words | °C |
|---|---|---|---|
| `0x8F` lamp warning | `e8 ff 18 00` | −24 / +24 | −1.5 / +1.5 from the setpoint |
| `0x8C` lamp fault | `e0 ff 20 00` | −32 / +32 | −2.0 / +2.0 from the setpoint |
| `0x8B` board warning | `f0 00 20 03` | 240 / 800 | 15.0 / 50.0 [INF: absolute] |
| `0x8D` board fault | `a0 00 70 03` | 160 / 880 | 10.0 / 55.0 [INF: absolute] |

The lamp bands are offsets from the setpoint [C]; that the board bands are
absolute limits is inferred from their values. Measured on serial 3054: lamp
40.06 °C at the 40.0 °C setpoint, motherboard 28.8–29.1 °C.

## LOW — scan, FIFOs, DX

| Cmd | Access | Meaning |
|---|---|---|
| `0x89` | write 1 | per-resolution flag: 1 at Base 8 (narrows the line to the 1500 px output), else 0 |
| `0x8A` | command | **ResetFifos**, preceded by host `02 04 10 01 84 02`. Never during a live scan stream |
| `0x91` | write 3 | scan-line trigger: u16 LE value + mode byte `01`; also resets the DX position counter. Sent at configure, at scan start, and before every strip after the first in strip mode (`FN_bDrvDxStart`) |
| `0x92` | command | end acquisition; closes the DX decode window (teardown) |
| `0x90` | read 30 | DX sensor block: position, entry count, entries (layout in [PROTOCOL.md](PROTOCOL.md#dx-sensor-block-low-0x90)); read after each LOW event |
| `0x93` | read 4 | the four DX detector levels: bytes 0–1 entry sensor, 2–3 exit sensor. Serial 3054: empty 190 / 216 / 224 / 202, film < 170 |
| `0x94` | write | DX hardware setup (OEM service procedure only): do not send |
| `0x96` | write | DX pot values (OEM service procedure only): do not send |

## SCN — film motor

| Cmd | Access | Meaning |
|---|---|---|
| `0x97` | write 1 | controller init: `01` |
| `0xA5` | write 2 | motor speed, u16 LE |
| `0xA0` | command | start forward |
| `0xA1` | command | start reverse |
| `0xA2` | command | release the drive; stops the motor only with bank `0x82` reg 0 bit 0 clear |

Motor speed = `MotorSpeed(_Ir) × MotorAdjust(_Ir) / 1000` from the EEPROM
(adjust clamped 900–1100), clamped to 400–9500 (F-135) or 1000–32766
(F-135+). Values seen:

| Unit | Use | `0xA5` |
|---|---|---|
| F-135 serial 3054 | Base 16 + IR scan | `0x0615` (1557) |
| F-135 serial 3054 | advance | `0x251C` (9500) |
| F-135 serial 3054 | IR off, Base 16 / 8 / 4 | 2365 / 3689 / 8302 |
| F-135 serial 3054 | IR on, Base 8 / 4 | 2767 / 6224 |
| F-135+ serial 16402 | Base 16 / 8 / 4 | 5900 (`0x170C`) / 11434 / 25726 |
| F-135+ serial 16402 | Base 4 + IR | 19278 |
| F-135+ serial 16402 | advance | `0x647E` (25726) |

## Per-unit values that feed registers

| EEPROM field | Register |
|---|---|
| type word (1350 / 1351) | cross-check of the probed model |
| Offset per base | bank `0x82` reg 4 at scan; reg 5 = Offset + 2000 (1000 dual-tap) |
| MotorSpeed, MotorSpeed_Ir per base, × the adjust words | SCN `0xA5` |
| NegMatrix, PosMatrix | no register: the colour transform on the host ([IMAGING.md](IMAGING.md)) |

Field offsets and decoding: [EEPROM.md](EEPROM.md). Light calibration (LED
currents and duties, A/D gains and offsets, `DetectWhite_G`,
`DetectFilm_G`) is not in the EEPROM; the client measures it
([PROTOCOL.md](PROTOCOL.md#light-calibration)).

## Converged values on serial 3054 (sanity checks only)

From `resources/scan.pakscan` (OEM, Base 16 + IR). Per unit: never reuse on
another scanner.

| Register | Value |
|---|---|
| bank `0x84` gain R/G/B | 13 / 13 / 13 |
| bank `0x84` offset R/G/B | −38 / −31 / −31 |
| bank `0x84` reg 0 / 1 | `0x0078` / `0x0080` |
| bank `0x82` reg 0 | `0x0160` (`0x0161` acquiring) |
| bank `0x82` reg 4 / 5 | `0x002B` / `0x07FB` (Offset 43; window 43..2043) |
| bank `0x82` reg 6 | `0x0C1A` |
| bank `0x82` reg 10 | `0x0400` |
| LOW `0x81` currents R/G/B/IR | 2 / 3 / 3 / 2 |
| LOW `0x82` open-gate duties R/G/B/IR | 896 / 708 / 278 / 1353, period `0x0742` |

## Names in other sources

pakon-reference's command names are labels inferred from traces. Where they
differ from `TLB.dll`, the engine's meaning applies:

| Command | Reference / older label | Meaning (`TLB.dll`) |
|---|---|---|
| LOW `0x8A` | AcquireLine | ResetFifos |
| LOW `0x8B`/`0x8C`/`0x8D` | SetCcdExposure | temperature bands |
| LOW `0x8F` | SetLightConfig | lamp warning band |
| LOW `0x81` | SetCcdGainOffset | LED currents |
| LOW `0x82` | SetColorMatrix | LED duties and period |
| LOW `0x89` | EnableScan | per-resolution flag (Base 8) |
| SCN `0xA5` | SetMotorCalibration | motor speed |
| SCN `0xA1` | StopFilmDrive | start reverse |
| bank `0x82` reg 6 | height | integration / line period |
| bank `0x82` reg 9 | brightness | status LEDs |
