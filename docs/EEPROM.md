# The per-unit EEPROM (I2C `0x52`)

Every F-135 and F-135+ carries a small serial EEPROM with the data that
makes it that unit: its model and serial number, the start of the imaged
region on the CCD for each resolution, its motor speeds, and its colour
matrix. The data exists nowhere else and cannot be regenerated. This page
gives the layout, how to read and decode it, what the values are used for,
the archive the app writes, and the values seen on real units.

Confidence: **[C]** confirmed (on hardware, or read from the OEM engine
`TLB.dll` 3.1.0.28); **[INF]** inferred. The layout and read procedure are
from Ali Bosworth's
[pakon-reference](https://github.com/alibosworth/pakon-reference/blob/76d9cd0a38f523398c3a19392070539ee80b75cf/docs/calibration.md)
(CC BY 4.0), confirmed here on serial 3054.

## What it holds, and what it does not

| Store | Where | Contents | Replaceable |
|---|---|---|---|
| per-unit EEPROM | I2C `0x52` on the motherboard, read by the host through the FX2 | serial, model type, per-base Offset and motor speeds, motor-adjust words, NegMatrix, PosMatrix; each section stored twice with CRC-32 | **no**: factory-measured |
| boot personality | first 8 bytes of the I2C EEPROM at `0x51`, read by the FX2 at power-up | the C0 boot record `c0 05 0f 35 f2 07 aa 04` (cold ID `0f05:f235`, revision `aa07`) | yes: known per model |
| light calibration | the host (the OEM keeps it in the Windows registry, `…\Pakon\TLB\Scan\DpiBase{4,8,16}_35`) | LED currents and duties, A/D gains and offsets, film-detection levels | yes: re-measured ([PROTOCOL.md](PROTOCOL.md#light-calibration)) |
| controller firmware | flash in LOW and SCN | Kodak PIC firmware | reflashable, with care |
| FX2 firmware | RAM | `Pakon7.hex` | loaded at every power-up |

The service manual's "the EEPROM is the source of truth, the registry a
cache" is true only of what the EEPROM holds; for the light calibration the
registry is the only copy.

On an F-135+ main board (PCB #125430 REV C) a photograph shows two 8-pin
`24LC64` parts (8192 bytes each), very likely the chips at `0x51` and `0x52`
[INF, pakon-reference]. The calibration sections end at `0xA24`; what the
rest of the chip holds is unknown. The base F-135's board (PCB #125039A) has
not been examined.

## Reading it

Two vendor control requests per chunk, both with `wIndex = 0x1234`, and
nothing else [C]:

| Step | `bmRequestType` | `bRequest` | `wValue` | `wLength` |
|---|---|---|---|---|
| select | `0x40` (vendor OUT, no data) | `0xA4` | `0x00A5` | 0 |
| read | `0xC0` (vendor IN) | `0xA9` | byte offset into the EEPROM | ≤ 32 |

- The select is repeated before every read. The OEM reads the 8-byte header
  first, then 32 bytes at a time at increasing offsets (`0x008`, `0x028`,
  …).
- The select's `wValue` is `((0x50 + n) << 1) | direction`: `0x00A5` reads
  `0x52`; `0x00A3` reads the boot EEPROM at `0x51` [C on serial 16402]. The
  write direction (`0x00A4` select, then `0xA2`) is from the disassembly
  only; the OEM's scanning path never issues it, only its Calibration
  Wizard. **Never send `0xA2`.**
- A read with the wrong `wValue` or `wIndex` addresses no EEPROM and
  returns bus-idle `0xFF` that looks like data.
- `0xA9` with `wIndex = 0` is a different operation: the 8-byte personality
  read of the firmware load ([PROTOCOL.md](PROTOCOL.md#usb-identities-and-firmware)).
- The scanner must be warm (`0f05:f135`). The read sends no command frames:
  no motor, LED, TEC or bootloader traffic.
- Read once per power cycle. One project reported its unit returning good
  data only on the first read after power-on and degraded data (still
  reporting success) later; serial 16402 and serial 3054 read consistently.
  Confirm a read by comparing across power cycles, not by re-reading within
  one.

## Layout

Two sections, each stored twice. Every copy starts with an 8-byte header
`{u32 length; u32 crc32}` (little-endian) followed by the payload; the
length counts the header [C].

| Section | Primary | Backup | Length |
|---|---|---|---|
| A | `0x000` | `0x400` | 398 bytes (8 header + 390) |
| B | `0x800` | `0xA00` | 36 bytes (8 header + 28) |

Section A, absolute offsets, little-endian:

| Offset | Type | Field |
|---|---|---|
| `0x000` | u32 | length (398) |
| `0x004` | u32 | CRC-32 of `0x008`–`0x18D` |
| `0x008` | u32 | hardware version (`ScannerVersionHw`); 400 on every unit read |
| `0x00C` | u32 | scanner type (`ScannerType`): **1350 = F-135, 1351 = F-135+** |
| `0x010` | u32 | serial number (`ScannerSerialNumber`) |
| `0x014` / `0x016` / `0x018` | u16 × 3 | Base 4: Offset, MotorSpeed, MotorSpeed_Ir |
| `0x01A` / `0x01C` / `0x01E` | u16 × 3 | Base 8: the same three |
| `0x020` / `0x022` / `0x024` | u16 × 3 | Base 16: the same three |
| `0x026`–`0x09D` | f32 × 30 | NegMatrix0–29: 3 rows × 10 |
| `0x09E`–`0x115` | f32 × 30 | PosMatrix0–29: 3 rows × 10 |
| `0x116`–`0x18D` | 120 bytes | unused (zero on every unit read) |

The OEM engine mirrors `0x008`, `0x00C`, `0x010` into the registry as
`ScannerVersionHw`, `ScannerType`, `ScannerSerialNumber` (on serial 16402:
400, 1351, 16402) [C].

Section B, absolute offsets (relative offset in the copy in brackets):

| Offset | Type | Field |
|---|---|---|
| `0x800` | u32 | length (36) |
| `0x804` | u32 | CRC-32 of `0x808`–`0x823` |
| `0x808` (+8) | u16 × 4 | Base 4: MotorAdjust, MotorAdjustDrag, MotorAdjust_Ir, MotorAdjustDrag_Ir |
| `0x810` (+16) | u16 × 4 | Base 8: the same four |
| `0x818` (+24) | u16 × 4 | Base 16: the same four |
| `0x820` (+32) | u32 | unknown (0 on serial 3054) |

The names and order of the adjust words are the engine's
(`FN_bReadEEPromToRegistry` `0x10016a90`, `FUN_10016860`, `FUN_10016610`)
[C].

## Decoding

**Validity of a copy:** exactly the expected length, a length field equal to
it, and a CRC-32 (zlib/PKZIP, reflected polynomial `0xEDB88320`) over the
payload only (bytes 8 to the end) equal to the stored one.

**Which copy the OEM uses** [C, pakon-reference: the engine's
EEPROM-to-registry routine and serial 16402's registry]: at start-up it reads
the primary of each section; if the length or CRC is bad it reads the backup
and uses that. The primary is never repaired. Only if both copies of a
section fail does it keep what it read and raise the warning
`INITIALIZEW_EEPROM_BLANK` / `INITIALIZEW_EEPROM_CHECKSUM_BAD`. A unit can
carry a bad primary for years without anyone noticing; read both copies.

**This decoder** (`rust/crates/pakon-proto/src/eeprom.rs`): per section, a
valid primary, else a valid backup. If both are valid and differ, it keeps
both, warns, and uses the primary. A section with no valid copy is not
decoded. The result counts as decodable only when both sections decode, the
type is 1350 or 1351, and every matrix coefficient is finite. Values are
reported as stored: not clamped, not applied to hardware, never replaced by
another unit's.

**Matrices:** each row is 10 coefficients applied to density
`(R, G, B)`: `m0·R + m1·G + m2·B + m3·R² + m4·G² + m5·B² + m6·RG + m7·BR +
m8·GB + m9` [C, `TLB.dll` `FN_bReadMatrix_3x10`]. pakon-reference gives the
cross terms as RG, GB, BR; the engine's order is RG, BR, GB, and the colour
path follows the engine. On every unit read the quadratic and cross columns
are ≈ 0, so NegMatrix acts as a 3×4 affine; PosMatrix is the plain 0.25
diagonal.

## What the values are used for

| Field | Use |
|---|---|
| type | the model; cross-checked against the presence probes before a scan (an archive of the other model is refused) |
| serial | archive name; key of the saved light calibration |
| hardware version | not used |
| Offset (per base) | CCD pixel-window start: bank `0x82` reg 4 at scan, reg 5 = Offset + height (2000, or 1000 at Base 4); the calibration window is 6..Offset + 2000. The engine requires Offset ≥ 6 (≥ 3 dual-tap) and Offset + height ≤ `0x848` [C, `FN_bDrvPutCcdFpgaSettings` `0x1002c340`] |
| MotorSpeed / MotorSpeed_Ir (per base) | SCN `0xA5` motor speed for scans with IR off / on |
| adjust words (per base) | scale the motor speed: `speed = MotorSpeed(_Ir) × adjust / 1000`, adjust clamped 900–1100, speed clamped 400–9500 (F-135) or 1000–32766 (F-135+) [C, `FN_bBeforeScan` → `FN_bDriveMotorAdvanceFilm`] |
| NegMatrix | colour transform for colour negative and both B&W types, on density data after the log LUT ([IMAGING.md](IMAGING.md)) |
| PosMatrix | the same for positive (slide) film |
| section B unknown u32 | not used |

**Motor speed is proven on both models** [C]: the captured scan speeds equal
the EEPROM value × 1000 / 1000: serial 3054 Base 16 + IR scans at 1557
(`MotorSpeed_Ir`), serial 16402 at 5900 / 11434 / 25726 (Base 16 / 8 / 4
`MotorSpeed`) and 19278 (Base 4 `MotorSpeed_Ir`). The drag words (1008 on
every unit) would give other values, so scans use MotorAdjust /
MotorAdjust_Ir; when the engine uses the drag words is not known.

The engine reads NegMatrix and PosMatrix into its colour object unless a
client matrix file is installed (`ClientColNegLut.txt` and
`ClientColNegMat_3x10.txt`, both required).

## Values on real units

Sanity references only: every field below is per unit or per model; never
use one unit's values on another. Serial 3054 is read and decoded here; the
other five are from pakon-reference's
[dump comparison](https://github.com/alibosworth/pakon-reference/tree/76d9cd0a38f523398c3a19392070539ee80b75cf/docs/resources/eeprom).

Offset / MotorSpeed / MotorSpeed_Ir per base:

| Unit | Type | Base 4 | Base 8 | Base 16 |
|---|---|---|---|---|
| 3054 (F-135) | 1350 | 21 / 8302 / 6224 | 42 / 3689 / 2767 | 43 / 2365 / 1557 |
| 2233 (F-135) | 1350 | 27 / 8162 / 6119 | 54 / 3627 / 2720 | 54 / 2325 / 1530 |
| 5963 (F-135+) | 1351 | 29 / 25512 / 19117 | 58 / 11338 / 7494 | 59 / 5850 / 4796 |
| 16275 (F-135+) | 1351 | 27 / 25802 / 19335 | 54 / 11467 / 7580 | 55 / 5917 / 4850 |
| 16402 (F-135+) | 1351 | 30 / 25726 / 19278 | 58 / 11434 / 7557 | 60 / 5900 / 4836 |
| 17157 (F-135+) | 1351 | 34 / 25498 / 19107 | 66 / 11332 / 7490 | 68 / 5847 / 4793 |
| 138 (F-135+) | 1351 | 32 / 25381 / 19019 | 62 / 11280 / 7456 | 64 / 5820 / 4771 |

NegMatrix diagonal and constants (the part that varies):

| Unit | Diagonal R / G / B | Constants R / G / B |
|---|---|---|
| 3054 (F-135) | 0.28037 / 0.28020 / 0.26599 | 167.1 / 452.5 / 671.3 |
| 2233 (F-135) | 0.27680 / 0.27967 / 0.26158 | 163.4 / 441.4 / 651.0 |
| 5963 (F-135+) | 0.28383 / 0.32009 / 0.33758 | 145.6 / 386.7 / 602.9 |
| 16275 (F-135+) | 0.28920 / 0.27583 / 0.27824 | 159.6 / 444.8 / 635.5 |
| 16402 (F-135+) | 0.29299 / 0.28521 / 0.32000 | 165.8 / 429.8 / 638.2 |
| 17157 (F-135+) | 0.28860 / 0.31628 / 0.32606 | 162.1 / 401.3 / 607.7 |
| 138 (F-135+) | 0.28399 / 0.28346 / 0.28035 | 157.6 / 411.5 / 620.3 |

Chip health and section B:

| Unit | Copies | Section B (CRC) |
|---|---|---|
| 3054 | all four valid, primaries = backups | `0x873e6ed3` |
| 2233 | all four valid | `0x2a582d50` (Base 4 MotorAdjust_Ir at `0x80C` = 1008) |
| 5963 | all four valid | `0x873e6ed3` |
| 138 | all four valid, primaries = backups; hardware version 400 | `0x873e6ed3` |
| 16275 | section A primary bad: length byte `0x5A` instead of `0x8E`, and a 17-byte block at `0x100`–`0x110` (a 256-byte page boundary); backup good | `0x873e6ed3` |
| 16402 | section A primary bad: byte `0x0A5` reads `0x48` instead of `0x00` (high byte of PosMatrix1, 0.0 → 131072.0); stable over four reads and two power cycles; backup good | `0x873e6ed3` |
| 17157 | all four valid | `0x873e6ed3` |

Serial 3054 in full: hardware version 400; adjust words 1000 / 1008 / 1000
/ 1008 (MotorAdjust / Drag / _Ir / Drag_Ir) at every base; section B
unknown u32 = 0; PosMatrix 0.25 diagonal.

What the six units show:

- **Per unit:** serial, the three Offsets, the six motor speeds, NegMatrix.
  Offset varies more than expected: 21–34 at Base 4 and 43–68 at Base 16
  (serial 3054 is the lowest seen). Base 4's Offset is about half of Base
  8's on every unit (dual-tap pixels) [INF].
- **Per model:** the type word; motor speeds (about 3.1× higher on the Plus
  at Base 4 and 8, a mechanical difference); the ratio of IR to normal motor
  speed, 0.750 / 0.750 / 0.658 (Base 4 / 8 / 16) on both F-135s and
  0.749 / 0.661 / 0.820 on all four Plus units. The two F-135s'
  speeds differ by the same 1.7% at every base.
- **Constant:** hardware version (400); PosMatrix (0.25 diagonal, bit-
  identical on every unit, both models); the unused tail of section A.
- **Section B** is byte-identical on five of six units, F-135 serial 3054
  included; only F-135 serial 2233 differs, in one word. So section B is not
  a per-model default, as the reference suggested from serial 2233 alone; it
  looks like a factory default that one unit departs from [INF].
- **Two of six units carry a damaged section A primary** with a good backup;
  the OEM software had been reading the backups without saying so.

The captured pixel windows match the EEPROM Offsets: serial 3054 Base 16
scans with reg 4 = 43 (`0x2B`), serial 16402 with 60 / 58 / 30 at Base 16 /
8 / 4 [C].

## The archive the app writes

The app (and `pakon-web` on its own) reads the EEPROM itself
(`rust/crates/pakon-scan/src/eeprom_read.rs`) and archives it
(`rust/crates/pakon-web/src/eeprom.rs`). It never writes or repairs the
EEPROM.

**When:** the first time it sees a warm scanner on a USB connection (bus and
address): at launch, after **Load firmware**, and before a scan or eject.
Exactly one warm scanner must be connected. Once per connection; a power
cycle or firmware load re-enumerates and so allows a new read.
`PAKON_EEPROM_DIR=DIR` uses that archive and turns the automatic read off.

**The read:** the four copies at their fixed extents (`0x000`/398,
`0x400`/398, `0x800`/36, `0xA00`/36): the 8-byte header first, then chunks
of up to 32 bytes, a select before every read. The extents are fixed, so a
corrupt length field cannot widen a read. No retries; on a failed select or
read, or a short or oversized transfer, no further transfers are made and
the read is reported as failed.

**Where:** `<data folder>/eeprom/<MODEL>-<serial>-<YYYYMMDD-HHMMSS>/` (local
time), with `<MODEL>` = `F135` or `F135PLUS`, for example
`F135-3054-YYYYMMDD-HHMMSS`.

| | Data folder |
|---|---|
| macOS app | `~/Library/Application Support/Pakon` |
| Windows | `%LOCALAPPDATA%\Pakon` |
| Linux package | `$XDG_DATA_HOME/pakon` (`~/.local/share/pakon`) |
| from a checkout | `backups/eeprom/` in the repository (git-ignored), unless `PAKON_DATA` names a data folder |

**Keep-first policy:** a new archive is written only when none exists for
that serial; otherwise the fresh read is discarded and the existing archive
is used. A read that is incomplete or does not decode is discarded and shown
as an error; nothing is archived from it. A new archive is assembled in a
temporary folder and renamed into place.

**Files:**

| File | EEPROM offset | Bytes, header included |
|---|---|---|
| `eeprom_0x52_sectionA_primary.bin` | `0x000` | 398 |
| `eeprom_0x52_sectionA_backup.bin` | `0x400` | 398 |
| `eeprom_0x52_sectionB_primary.bin` | `0x800` | 36 |
| `eeprom_0x52_sectionB_backup.bin` | `0xA00` | 36 |
| `report.json` | | what was read and decoded |
| `SHA256SUMS` | | SHA-256 of the four section files and `report.json`, sorted by name, `sha256sum` format |

Each section file holds the bytes exactly as read, a corrupt copy included.
This is a backup of the four known sections, **not of the whole chip, and
not of the boot EEPROM**.

**`report.json`** (schema 1, the layout of the earlier Python tool):

| Key | Contents |
|---|---|
| `schema_version` | 1 |
| `sections` | per copy: `offset`, `file`, `sha256`, `missing`, `bytes_read`, `expected_length`, `declared_length`, `stored_crc32`, `computed_crc32` (hex), `valid`, and `error` when invalid |
| `selected` | `A`, `B`: the copy decoded (`sectionA_primary` …), or null |
| `copies_equal` | `A`, `B`: primary and backup byte-identical (null if either is missing or the wrong length) |
| `decoded` | `hardware_version`, `scanner_type`, `model`, `serial`, `resolution_bases` (`4`/`8`/`16`: `offset`, `motor_speed`, `motor_speed_ir`), `negative_matrix`, `positive_matrix` (3 rows of 10; JSON null stands for a non-finite value), `motor_adjustments` (`4`/`8`/`16`: `normal`, `drag`, `ir`, `drag_ir`), `section_b_unknown_u32` |
| `warnings` | e.g. a bad primary with the backup selected, valid copies that differ, an unknown type, non-finite coefficients |
| `decodable` | both sections decoded, known type, finite matrices |
| `all_copies_valid` | all four copies valid |
| `created_utc` | time of the read |
| `usb` | `vid`, `pid`, `bus`, `address`, `bcd_device` |
| `acquisition_complete`, `acquisition_error` | whether the read finished, and why not |
| `scope` | "Four documented sections of EEPROM 0x52, not the entire chip" |
| `tool` | the writer |

Read `warnings` once: a bad primary with a good backup is reported
explicitly and still yields valid decoded data. Keep a copy of the archive
away from the computer: this data exists nowhere else.

**Comparing reads across power cycles:** move the existing archive out of
the `eeprom` folder, power-cycle the scanner, load its firmware again, and
let the app write a new one. Compare the four `.bin` files, not
`report.json` (its time and USB address change). A difference needs
investigation, not an EEPROM repair.

**Tests:** `cargo test` (in `rust/`) decodes the published dumps of serial
2233 (F-135, all valid) and 16402 (F-135+, bad section A primary) in
`test/fixtures/eeprom/` (from pakon-reference, CC BY 4.0; see the README
there), and runs the read against a simulated EEPROM (fixed extents, a
select before every read, nothing after a failed transfer). The live read
has run on serial 3054 (macOS).

## Open questions

- What the hardware version word would mean if it ever differed from 400.
- What section B's trailing u32 is, and why serial 2233's Base 4
  MotorAdjust_Ir is 1008 when five other units have 1000.
- When the engine uses the MotorAdjustDrag words.
- What, if anything, lies past `0xA24` on the `0x52` chip, and the rest of
  the `0x51` chip; whether the base F-135's board carries the same parts.
- Whether reads can really degrade within one power cycle (reported on one
  unit, never seen on serial 3054 or 16402).
