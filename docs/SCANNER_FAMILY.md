# The Pakon F-X35 scanner family

The four models this project may meet, how to tell them apart, what is
specific to each, and how to add support for one that does not work yet.

Markers: **[C]** confirmed (on hardware, in captures, or read from the OEM
binaries and driver source), **[DOC]** stated in OEM readmes, INF files or the
public pakon-reference, not verified here, **[INF]** inferred.

Sources: this project's runs on an F-135 (serial 3054) and an F-135+ (serial
16402); the OEM driver-level captures of serial 16402 (`resources/f135plus/`);
the OEM driver package and firmware readmes (`ReadmeF135/235/335.txt`,
`F235usb2.inf`, `FirmwareLoader/`); the open-source FX35 driver source; the
OEM engines `TLB.dll` (F-135/F-135+) and `TLA.dll` (F-235); pakon-reference;
libpakon's device catalog.

Names: the two F-135-line controllers are called LOW and SCN in
[REGISTERS.md](REGISTERS.md). Older material and the OEM call them PICL (=
LOW) and PICM (= SCN); the "+" suffix (PICL+, PICM+) marks the Plus boards.

## At a glance

| | F-135 | F-135+ ("Plus", "Hybrid") | F-235 | F-335 |
|---|---|---|---|---|
| USB cold (bootstrap) | `0f05:f235` [C] | `0f05:f235` [C] | `0f05:f235` [DOC] | `0f05:f235` [DOC] |
| USB warm (operational) | `0f05:f135` [C] | `0f05:f135` [C] | `0f05:35f2` [DOC] | `0f05:f335` [DOC] |
| Personality → FX2 image | `F235_AA07` → `Pakon7.hex` [C] | `Pakon7.hex`, same image [C]; `AA07` [INF] | `F235_AA05` → `Pakon5.hex` [DOC] | `F235_AA08` → `Pakon8.hex` [DOC] |
| OEM engine DLL | `TLB.dll` [C] | `TLB.dll` (same build) [C] | `TLA.dll` [DOC] | `TLC.dll` [DOC] |
| Controller bus addresses | LOW `0x20`, SCN `0x24` [C] | LOW `0x40`, SCN `0x44` [C] | unknown | unknown |
| Controller boards (firmware prefix) | PICL `PL` + PICM `PM`, PIC16 | PICL+ `NL` + PICM+ `NM`, PIC18 [C] | `MC` motor, `DX`, `LP` lamp, `CD` CCD, `AP` APS [DOC] | `MD` motor, `LQ` LED, `DY` DX, `CE` CCD, `AP` APS [DOC] |
| Illumination | LED: R, G, B, IR [C] | LED: R, G, B, IR [C] | lamp (`LP` board) [DOC] | LED (`LQ` board) [DOC] |
| CCD cooler (TEC) | not listed by the OEM; init sends the TEC writes anyway (see below) | yes [DOC]; TEC writes in every init [C] | unknown | unknown |
| DX barcode reading | in LOW (PICL) [C] | in LOW (PICL+) [DOC] | dedicated `DX` board [DOC] | dedicated `DY` board [DOC] |
| APS (IX240) film | no | no | yes (`AP` board) [DOC] | yes (`AP` board) [DOC] |
| EEPROM scanner type word | 1350 [C] | 1351 [C] | unknown | unknown |
| Units run by this project | serial 3054 | serial 16402 | none | none |
| State here | everything built in code: firmware, setup, light calibration, scan at Base 16/8/4, advance/eject; Rust port run on 3054 | firmware load, model detection and replayed scans at Base 16 and Base 8 on 16402 with the C tools; the Rust port has not been run on a Plus | untested | untested |

## Identity and firmware selection

- **Every model cold-enumerates as `0f05:f235`**, the bootstrap identity of an
  FX2 with a C0-type boot EEPROM (VID, PID, revision, one extra byte; no
  firmware). The host must load firmware every power-on; it lives in RAM
  only. [C for F-135 and F-135+; DOC for F-235/F-335 via the shared driver
  package and libpakon's catalog]
- **The personality** is the 8-byte boot record read with vendor request
  `0xA9`: `c0 <VID LE> <PID LE> <revision LE> <extra>`. Serial 3054 returns
  `c0 05 0f 35 f2 07 aa 04`, i.e. `0f05:f235` revision `0xAA07`. The OEM
  loader picks the FX2 image by registry key `<PID>_<revision>`: `F235_AA05`
  → `Pakon5.hex`, `F235_AA07` → `Pakon7.hex`, `F235_AA08` → `Pakon8.hex`.
  The model digit of the revision is 05 = F-235, 07 = F-135 line, 08 = F-335.
  [C from `F235usb2.inf` and the FX35 loader source]
- **Loading is two-stage**: a stage-1 loader (a record table inside the OEM
  `F235Ldr.sys`, not `PknInit.hex`), then the main image, then the device
  re-enumerates warm. [C on serial 3054] Details in
  [PROTOCOL.md](PROTOCOL.md).
- **The F-135 and F-135+ are identical at the USB level**: same cold and warm
  IDs, same FX2 image, same endpoints (`0x01` command OUT, `0x81` reply IN,
  `0x86` image IN), same 20480-byte image chunks, same `0xA4`/`0xA9`
  parameter-table read (`wValue 0x00A5`, `wIndex 0x1234`, 32-byte chunks).
  USB descriptors cannot tell them apart. [C on both]
- **Why the Plus has no personality of its own**: the personality only
  selects the FX2 image, and the FX2 is a plain USB-to-bus bridge, identical
  on both. Everything Plus-specific sits behind it on the controller boards.
  The OEM `FirmwareLoader/Personalities/` folder has no Plus image and the
  INF has no Plus key. [DOC] The raw personality bytes have not been read off
  an F-135+: what is confirmed is that the F-135 firmware sequence
  (`resources/f135.pakfw`, which leaves exactly `Pakon7.hex` in RAM,
  10355/10355 bytes) brings serial 16402 up warm as `0f05:f135`. [C]
- **Host software**: one COM façade (`tlx.dll`) picks an engine by which
  kernel device the installed FX35 driver build publishes: `\\.\Pakon135` →
  `TLB.dll` (F-135 and F-135+, one build that knows both address sets),
  `\\.\PakonX35` → `TLC.dll` (F-335), `\\.\Loopback` → `TLA.dll` (F-235).
  The engine letters follow build order, not model numbers. All engines
  carry the full model enum `F135`, `F135_PLUS`, `F235`, `F235C`, `F335`,
  `F335C`. [DOC, pakon-reference] A fact sourced to `TLA.dll` is about the
  F-235 and does not automatically hold on the F-135 line.

## Telling models apart: the presence probes

After the bridge open (HostReset `0x85`, HostSetMode `0x8F`), the OEM probes
candidate controller addresses with `04 03 <addr> 00 00`. The reply is
`07 02 <addr> <status>`: status 0 = the controller acked (present), 1 = not
acked (absent). [C on both F-135-line models]

| Address | Meaning |
|---|---|
| `0x10` | HOST: the FX2 bridge, every model |
| `0x20` / `0x22` | F-135 LOW (PICL) / its bootloader |
| `0x24` / `0x26` | F-135 SCN (PICM) / its bootloader |
| `0x40` / `0x42` | F-135+ LOW (PICL+) / its bootloader |
| `0x44` / `0x46` | F-135+ SCN (PICM+) / its bootloader |

- `TLB.dll` probes `0x44`, `0x46`, `0x24`, `0x26` in that order; the first
  set that answers picks the model, and a controller answering only at its
  bootloader address still counts as present. [C, decompile] In the
  captures the Plus session stops after `0x44`; the F-135 session goes on to
  `0x46` and `0x24`. [C]
- Live replies: on serial 16402, `0x44` → `07 02 44 00` (present) and
  `0x24` → `07 02 24 01` (absent); on an F-135 it is exactly the inverse. [C]
- Cross-check: the EEPROM scanner type word at section A `0x0C` is 1350 on
  the F-135 and 1351 on the F-135+ ([EEPROM.md](EEPROM.md)). [C]
- Bootloader addresses are probed only with the presence probe above; other
  commands there can erase PIC flash (safety rules in [PROTOCOL.md](PROTOCOL.md)).
- Whether the F-235 and F-335 controllers answer at any of these addresses
  is unknown. Their engines name different bus participants (below), so no
  assumption is made.

## Where the code handles model differences

- `rust/crates/pakon-proto/src/frame.rs`: `Address` (`PicL`/`PicM` 0x20/0x24,
  `PicLPlus`/`PicMPlus` 0x40/0x44, and the bootloader addresses).
- `rust/crates/pakon-proto/src/setup.rs`: `Model` (`F135`, `F135Plus`);
  `SetupState::model()` (from the LOW address); `scan_mode` (per-model,
  per-base trigger and integration); `LED_PERIOD_F135` / `LED_PERIOD_F135_PLUS`;
  `led_ceiling` (`LED_CEILING_F135` / `LED_CEILING_F135_PLUS`); the motor
  speed clamp per model inside `motor_speed`.
- `rust/crates/pakon-proto/src/eeprom.rs`: `TYPE_F135` / `TYPE_F135_PLUS`.
- `rust/crates/pakon-scan/src/session.rs`: `open_check` (handshake, probes of
  0x44, 0x46, 0x24, reports the model, refuses a PIC in its bootloader) and
  `setup_session` (probes, picks the address set and LED period word, checks
  the EEPROM type matches).
- `rust/crates/pakon-scan/src/transport.rs`: `timed_advance_plus` (the Plus
  timed advance, refused on an F-135); `transport` (advance/eject, F-135 only).
- `rust/crates/pakon-scan/src/scan.rs`: `scan_code` (F-135 only);
  `rust/crates/pakon-scan/src/calib.rs`: light calibration uses the F-135
  ceilings.
- `rust/crates/pakon-scan/src/replay.rs`: replays `.pakscan` scripts with
  either address set; its safety test checks every script in `resources/`
  and `resources/f135plus/`.
- `rust/crates/pakon-usb/src/fw.rs`: `personality_is_f135` (the HEX loader
  refuses anything but `F235_AA07`).
- `rust/crates/pakon-image/src/decode.rs`: the known row layouts, Plus ones
  included; `PAKON_LINEWIDTH` (+ `PAKON_IR_LANE`) forces one.
- `rust/crates/pakon-web/src/scanner.rs`: the app detects the model and picks
  the code-built scan (F-135 with an EEPROM backup) or the replayed script
  (F-135+: `resources/f135plus/base16.pakscan`).

## The F-135+

### Hardware and firmware

- Controller boards PICL+ and PICM+ on PIC18 (the F-135's are PIC16). The
  latest OEM images are `NL050A.HEX` (PICL+) and `NM0506.HEX` (PICM+), the
  "Production+" set in `ReadmeF135.txt`. [DOC] The NL050A image disassembles
  as PIC18 with ADC, PWM, SPI to the FX2 and four timers. [C]
- Module info (`PPB_MINFO`, 12 bytes) read live on serial 16402: PICL+
  `88 0f 0a 05 00 00 "12345"`, PICM+ `08 10 06 05 00 00 "12345"`, i.e. NL05/0A
  and NM05/06 with the factory placeholder serial "12345". [C]
- The OEM PICM firmware readme lists "Add F135 Plus/Hybrid support" as a PM
  revision note, so late F-135 motor firmware knows the Plus variant. [DOC]
- A thermoelectric cooler (TEC) on the CCD [DOC], set up in every init with
  LOW `0xD0 00`, `0xD1 01` [C]. Replay these values; never sweep them.
- DX reading is integrated in PICL+. [DOC] Serial 16402 has never produced a
  decoded barcode entry (type 3) in its `0x90` replies; serial 3054 produces
  one per half frame ([PROTOCOL.md](PROTOCOL.md)). [C]
- Marketed at roughly twice the F-135's throughput. [INF from OEM material;
  not measured] Its CCD timing clock is 2.5× the F-135's (below). [C]

### What differs on the wire

The command set is the same. With the addresses mapped (`0x40`/`0x44` →
`0x20`/`0x24`), the F-135+ Base 16 scan and the F-135 scan use the same 55
commands; the F-135 capture has one more, the probe of `0x46`. [C] So one
address-parameterised sequence serves both. What differs:

| | F-135 | F-135+ | Source |
|---|---|---|---|
| LOW / SCN address | `0x20` / `0x24` | `0x40` / `0x44` | [C] captures |
| LOW `0x82` period word in init (duties zero) | `0x0997` | `0x03D6` | [C] captures; derivation unknown |
| CCD timing clock | 833333.3 Hz | 2.5× that | [C] `TLB.dll` trigger formula fits all Plus captures with 2.5×, the ratio of the two period words |
| Integration (bank `0x82` reg 6), per mode | `TLB.dll` F-135 table | `TLB.dll` base table | [C] see below |
| `0x91` trigger, per mode | see [REGISTERS.md](REGISTERS.md) | see below | [C] |
| Motor speed clamp (`0xA5`) | 400–9500 | 1000–32766 | [C] `TLB.dll` |
| LED current ceilings R/G/B/IR, IR off | 6 / 8 / 8 / 0 | 4 / 20 / 20 / 0 | [C] `TLB.dll` |
| LED current ceilings R/G/B/IR, IR on | 8 / 8 / 8 / 8 | 8 / 24 / 24 / 8 | [C] `TLB.dll` |

Everything else in init matches byte for byte across models, including the
temperature threshold writes (`0x8F E8 FF 18 00`, `0x8C`, `0x8B`, `0x8D`) and
the TEC writes. The code-built setup, configure and teardown reproduce all
four Plus captures byte for byte (`rust/crates/pakon-proto/tests/setup.rs`),
leaving out event-service traffic and status-LED words, whose timing varies.
[C] Teardown is the same on both: acquire bit off, LEDs off, ResetFifos,
`0x92`, 20 ms, `0xA2`. [C, all seven captures]

### Motor speeds and limits

- Speed written to SCN `0xA5` = EEPROM `MotorSpeed` (or `MotorSpeed_Ir`) ×
  adjust ÷ 1000, adjust clamped 900–1100, result clamped to the model's range
  (Plus 1000–32766). [C, `TLB.dll`] On serial 16402 every captured scan speed
  equals the EEPROM value exactly (adjust 1000). [C]
- On both models the EEPROM speed is about 98–100 × the `0x91` trigger
  (`TLB.dll` default speed = 100 × `0x91`). [C]
- The Plus speeds exceed the F-135 maximum (9500), so Plus scripts and the
  Plus advance must not run on an F-135; `timed_advance_plus` refuses one.
- The Plus timed advance (`pakon_replay --advance`, the app's advance on a
  Plus) replays the film-feed section of the Base 4 capture: SCN `0xA5` =
  `0x647E` (25726, serial 16402's Base 4 scan speed), `0xA0`, bank `0x82`
  reg 0 = `0x0063` (acquire on, dual-tap as at Base 4), a status keep-alive
  every second for a fixed time, then reg 0 = `0x0060` (acquire off) before
  `0xA2`, as the OEM does, then the status LEDs. It is time-based: no
  film-exit sensing exists for the Plus.
- `TLB.dll` carries a Plus-specific speed `0x7FFE` (vs `0x251C` on the F-135),
  selected by the model flag in its DX service code. It never appears in the
  captures and is not used here. [C, decompile]

### Per-mode parameters

Serial 16402 EEPROM: Offset 60 / 58 / 30 at Base 16 / 8 / 4. Integration and
trigger are per model, not per unit; pixel window and motor speed are per unit.

| Mode | `0x91` trigger | Integration (reg 6) | Motor speed (16402) | Pixel window (reg 4..5) | LOW `0x89` | Dual-tap | Width | Row stride | Status |
|---|---|---|---|---|---|---|---|---|---|
| Base 16 | `0x003C` (60) | `0x0FFD` | 5900 (`0x170C`) | 6..Offset+2000 | 0 | no | 2000 | 6000 | captured, replayed live |
| Base 8 | `0x0075` (117) | `0x0AFD` | 11434 (`0x2CAA`) | 6..Offset+2000 | 1 | no | 1500 | 4500 | captured, replayed live |
| Base 4 | `0x0107` (263) | `0x0753` | 25726 (`0x647E`) | 3..Offset+1000 | 0 | yes | 1000 | 3000 | captured |
| Base 4 + IR | `0x00C5` (197) | `0x04E2` | 19278 (`0x4B4E`) | 3..Offset+1000 | 0 | yes | 1000 | 4000 | captured |
| Base 16 + IR | `0x0031` (49) | `0x09C2` | EEPROM `MotorSpeed_Ir` | 6..Offset+2000 | 0 | no | 2000 | 8000 [INF] | predicted, never run |
| Base 8 + IR | `0x004D` (77) | `0x0850` | EEPROM `MotorSpeed_Ir` | 6..Offset+2000 | 1 | no | 1500 | 6000 [INF] | predicted, never run |

- The trigger frame is `02 06 40 03 91 <v16 LE> 01`. Its value follows
  `round(23700 × clk × (0.01, or 0.005 with IR) ÷ (integration × width))`,
  23700 µm being the image height; with the Plus clock (2.5 × 833333.3) it
  gives every captured value. [C] The two IR rows come from the same
  `TLB.dll` table and formula, never captured. [INF]
- The mode table (Base 16 height 2000; Base 8 height 2000 narrowed to 1500 px
  with LOW `0x89` = 1; Base 4 dual-tap, height 1000) is one `TLB.dll` table
  for both models; on the F-135 only the integration times are overridden.
  [C, decompile]
- IR is enabled by bank `0x82` reg 0 bit `0x100` and the LED enable LOW
  `0x80` (bit 0 visible, bit 1 IR): the Base 4 + IR capture is the only one
  writing `0x80` = `0x02`/`0x03`. [C]
- Fixed in every mode: the whole cold-start init (37 commands, byte-identical
  across sessions) and the 60-pair `0xA4`/`0xA9` parameter-table read. [C]
- Per session, not per mode: the converged calibration values (CCD gain and
  offset, LED duties) and small motor differences at scan start. [C]

### Image stream

Same transport as the F-135: 16-bit little-endian samples in 20480-byte bulk
chunks from `0x86`, rows of per-pixel interleaved R, G, B, with an IR block
after the RGB block **only when IR is on**. So the stride is 3 × width, or
4 × width with IR, and must come from the mode, not a constant. A 6000-sample
stride is ambiguous (F-135+ Base 16, or F-135 Base 8 + IR). Byte layout,
marker bit and layout detection: [IMAGING.md](IMAGING.md). [C on all four
captured modes]

Frame geometry is the OEM's per resolution, shared with the F-135: picture
length 3000 / 2250 / 1500 rows and frame pitch 3206 / 2405 / 1603 rows at
Base 16 / 8 / 4.

### Replies seen on the Plus

The OEM captures have no reply bytes (below); these come from the live runs
on serial 16402 with full reply logging. [C]

- READ replies: `01 <count> <addr> 88 <data…>`.
- LOW status poll `03 01 40` → `03 02 40 80`: `0x80` = host event pending, as
  on the F-135.
- HostReset (`0x85`) and HostSetMode (`0x8F`) reply only on the first open
  after power-on or firmware load; later opens time out while the bridge keeps
  working. The OEM fires them without depending on replies.
- Raw commands time out unless the bridge open and module-info reads have run
  first.
- The 30-byte `0x90` sensor read returns idle values with no film in the
  transport.

### Scripts in `resources/f135plus/`

Converted from four OEM sessions on serial 16402 (TLXCD, the same 4-frame
strip each time). Each has the full command stream (init, calibration, scan,
teardown) and the image reads, but no reply bytes, so replay sends commands
and reads replies without byte-verifying them. A scan can only be as long as
the captured strip.

| Script | Mode | Converted | Captured stream decodes | Live replay on serial 16402 |
|---|---|---|---|---|
| `base16.pakscan` | Base 16 | yes | yes, 2000×3000 frames | **yes**, full scan: 1168 commands, 10270 image reads, all full 20480-byte chunks |
| `base8.pakscan` | Base 8 | yes | yes, 1500 wide | **yes**, full scan |
| `base4.pakscan` | Base 4 | yes | yes, 1000 wide | poor candidate: an 18-minute session with long OEM waits (lamp temperature up to 300 s, calibration); open-loop replay races through them, the motor starts early and image reads time out |
| `base4_ir.pakscan` | Base 4 + IR | yes | yes, IR block splits off cleanly (near-uniform IR transparency with dust specks) | not run |
| `init_base16.pakscan` (removed; at git tag `python-final`) | init only, cut from the Base 16 session before any film or motor activity beyond the standard park | yes | n/a | **yes**, 67/67 commands and 60 control transfers acked |
| `advance.pakscan` (removed; at git tag `python-final`) | the motor-start prelude of the Base 4 film feed | yes | n/a | its values are what `timed_advance_plus` sends |

Live runs used the C tools on macOS; the Rust replay (`pakon-scan`
`replay.rs`) runs the same scripts but has not been run on a Plus. Modes
never captured (Base 16 + IR, Base 8 + IR) have no script; they need another
capture or a code-built Plus scan.

### Not known yet on the F-135+

- Any code-built scan, light calibration or advance: the setup frames match
  the captures, but `--scan-code`, `--light-cal`, `--advance-code` and
  `--eject` are F-135 only, and none has been sent to a Plus.
- Base 16 + IR and Base 8 + IR (trigger, stride 8000 / 6000) on hardware.
- Whether clearing the acquire bit then `0xA2` stops the motor in the
  code-built teardown, as it does on the F-135.
- The DX detector levels (`0x93`) and film-present thresholds; the `0x90`
  payload with film; whether the unit decodes barcodes at all.
- Line registration: measuring the R/G/B line leads on the captured streams
  gave implausible values, while the F-135's fixed offsets (G −8, B −16
  lines) render them clean. The Plus firmware may already line-align. [INF]
- The raw personality bytes (expected `F235_AA07`).
- Whether the base F-135 has a TEC at all: the OEM readmes list it only for
  the Plus, yet the F-135 init sends the same `0xD0`/`0xD1` writes and they
  are acked.

## F-235 (untested)

[DOC, from `ReadmeF235.txt`, the OEM package and pakon-reference, unless noted]

- Multi-board: `MC` motor control (PIC16 or PIC18 depending on PCB revision),
  a dedicated `DX` board with its own microcontroller, `LP` lamp board
  (revisions with and without a fan tachometer), `CD` CCD board
  (`CD010F.hex`, "the one and only"), `AP` APS board for IX240 film. A filter
  wheel; the only model the readmes call lamp-lit.
- Engine `TLA.dll` (the original F235 SDK engine). Its bus participants are
  named `AD_HOST`, `AD_APS_READ`, `AD_DX_READ`, `AD_CCD1`, `AD_LAMP`,
  `AD_MOTOR` (plus bootloader twins); their numeric addresses are not
  documented. It also carries APS cartridge and MOF handling.
- `TLA.dll`'s frame detection has a perforation-based stage, which only makes
  sense for a model that images the full film width including the sprocket
  holes. [INF from decompile] Its colour path is a density LUT plus a 3×4
  matrix (`_ClientColNegMat.txt`); the F-135 line uses a per-unit 3×10
  NegMatrix instead ([IMAGING.md](IMAGING.md)). [C, decompile]
- Firmware `Pakon5.hex` via `F235_AA05`, warm `0f05:35f2`. The image stream,
  bus addressing and scan protocol are unverified.

## F-335 (untested)

[DOC, from `ReadmeF335.txt` and pakon-reference, unless noted]

- Multi-board: `MD` motor board (a revision note: "raised limit on motor
  speed"), `LQ` LED illumination board, `DY` DX board, `CE` CCD board, `AP`
  APS board.
- Engine `TLC.dll`, the twin of `TLA.dll` with `AD_LED_LAMP` in place of
  `AD_LAMP`.
- Firmware `Pakon8.hex` via `F235_AA08`, warm `0f05:f335`. Everything at the
  protocol level is unverified.

## Supporting another model

**What carries over** [INF]: the FX2 bridge, the cold identity, the
personality-driven firmware selection, the endpoints and very likely the
frame format `[type][count][addr][len][cmd][data…]` and the HOST (`0x10`)
commands. **What does not**: the bus addresses of the boards, the command
dialect of each board, the image geometry. The F-135+ was supported by
capturing the OEM software driving the real scanner, converting the capture
to a script, replaying it, then moving the per-mode values into code. The
same path applies to an F-235 or F-335; a capture from one is the single most
useful contribution.

Safety while exploring: never send type byte 0, never write to an address
that is not in a capture, never send anything but the presence probe to a
bootloader address, never issue vendor request `0xA2` (writes the per-unit
EEPROM). See [PROTOCOL.md](PROTOCOL.md).

### Capturing

The OEM software runs on Windows, typically in a VM with the scanner passed
through. Get one *successful* scan in the VM first.

- **USB passthrough**: the scanner re-enumerates during the firmware load
  (cold `f235` → warm). A VirtualBox USB filter on `0f05:f235` alone keeps it
  attached to the VM through that; a vendor-only filter (`0f05`, any product)
  also works. [C]
- **Linux host, usbmon** (commands): `sudo modprobe usbmon`; `lsusb | grep
  0f05` for the bus and device number (it changes on replug); `sudo dumpcap -i
  usbmon<bus> -w scan.pcapng` while scanning in the VM; filter to the device
  during analysis. Captures enumeration, firmware load and the command channel.
- **Host usbmon does not capture image payloads.** With VirtualBox usbfs
  passthrough the large bulk-IN transfers on `0x86` show their length but
  zero captured bytes (`len_cap=0`): the guest buffers are filled where the
  host cannot snapshot them. [C] For image data, **capture inside the Windows
  VM** with Wireshark + USBPcap; it sees full payloads. Use this too when the
  host cannot see the device at all.
- **usbmon text node** (no Wireshark): `cat
  /sys/kernel/debug/usb/usbmon/<bus>u > handshake.txt` truncates each URB to
  ~32 bytes; good only for the small command exchange.
- **Driver-level logging**: the F-135+ captures came from a modified FX35
  kernel driver that logs every `IOCTL_PAKON_SEND_AND_RECEIVE_PACKET` and
  every EP6 image read, with a shared performance-counter clock. The driver
  writes the IOCTL input buffer verbatim to the bulk command pipe, so the
  logged payload *is* the wire frame. [C] Limits: it starts after the driver
  has loaded (no enumeration or firmware load), and reply buffers come back
  empty, because with `METHOD_NEITHER` the user buffer cannot be read safely
  in the completion path. Replies can be recovered by replaying the converted
  script on the hardware with `PAKON_DEBUG=4`, which hexdumps every transfer.
- **Per capture, record**: model and serial number, resolution, IR on/off,
  film type, and wall-clock marks (app opened, scan pressed, first image,
  scan finished) for finding phase boundaries. One capture per setting.

### From capture to support

The analysis tools are at git tag `python-final`: `tools/analyze_capture.py`
(pcapng or usbmon text → URB timeline, decoded control setup packets and
Pakon frame headers, a summary of the transfer types seen) and
`tools/fx35_datalogger.py` (driver logs → decoded command timeline, a
`.pakscan` script with image reads interleaved, or the raw image stream).

1. Identify the model's personality and warm PID from the enumeration, and
   which bus addresses its probes and commands use.
2. Convert the session to a `.pakscan` and run the replay safety check
   (`rust/crates/pakon-scan/src/replay.rs`) over it.
3. Replay on the hardware with `PAKON_DEBUG=4` to get the replies; prefer a
   short single-scan session, since replay cannot wait on events the way the
   OEM does.
4. Find the row layout of the image stream (stride, channel order, IR block)
   and force it with `PAKON_LINEWIDTH` / `PAKON_IR_LANE` until the decoder
   knows it ([IMAGING.md](IMAGING.md)).
5. Move the addresses and per-mode values into `pakon-proto` (`Address`,
   `Model`, `scan_mode`) with the capture as a byte-for-byte test fixture.
