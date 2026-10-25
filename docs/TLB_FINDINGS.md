# TLB.dll findings — the F-135 / F-135+ engine

_Source: Ghidra 12.1.4 decompile of `TLB.dll` 3.1.0.28 (sha256 `5866ec56…`), the
135-line engine (earlier RE in this repo used `TLA.dll`, which is the F-235's).
Function names were recovered from the engine's own error-ID table (`FN_b…`).
Facts only; no Kodak code is reproduced. Frames marked X match
`resources/scan*.pakscan`. Date: 2026-09-28._

**Addresses.** SCN = `0x24` (base) / `0x44` (Plus): FPGA register banks + motor.
LOW = `0x20` / `0x40`: LEDs, temperatures, DX, FIFOs. Model detection probes
`0x44`, `0x46`, `0x24`, `0x26` in that order; a controller that answers only at
its bootloader address still counts as present.

## Corrections to our own docs

| Was | Is |
|---|---|
| bank 0x82 reg 6 = "Height" | integration / line period (≤ 0xFFD); reg 4 = pixel start (EEPROM Offset), reg 5 = start + height |
| bank 0x82 reg 9 = brightness candidate | status LEDs |
| LOW 0x8B/0x8C/0x8D/0x8F = exposure / LED geometry | temperature threshold bands (lamp/motherboard warn/fault), 1/16 °C |
| 0x8A = AcquireLine | ResetFifos (with host `02 04 10 01 84 02`) |
| SCN 0xA5 = motor calibration | motor speed (u16); 0xA0 forward, 0xA1 reverse, 0xA2 stop |
| illumination gated by the running motor | calibration is static, motor stopped; LEDs are turned on by LOW 0x80/0x81/0x82 |
| EEPROM region 2 = LED current/duty | motor-adjust words (3 bases × {Adjust, AdjustDrag, Adjust_Ir, AdjustDrag_Ir}) |
| NegMatrix unused / F-235 LUT+3×4 path | F-135 path = density LUT then per-unit 3×10 NegMatrix in density space |


## TLB.dll 3.1.0.28 — scan flow (X = matches pakon-macos resources/scan*.pakscan)
Addresses: FUN_1000afd0 probes 0x44, then 0x46 -> Plus; else 0x24/0x26 -> base.
SCN = 0x24/0x44 (FPGA banks + motor). LOW = 0x20/0x40 (LEDs, temps, DX, FIFOs).

### Frames
CMD `04 03 a 00 c`; WRITE `02 n+3 a n reg data`; READ `01 03 a n reg`; FPGA reg `02 06 SCN 03 bank reg v16`.
After each CMD/WRITE (except cmds 0x01/0x0D/0x25): poll `03 01 a` until bit0 clear, <= 44 tries.
Reply status 3/6/9 and flags 0x01/0x10 -> retry; 1/2/4/5/7 hard error; 0/8 OK.

### Prepare (FN_bBeforeScan 1002dbd0)
1. wake interrupt thread (clear pending events)
2. ResetFifos: `02 04 10 01 84 02`, `04 03 LOW 00 8a` X  (0x8a = ResetFifos, not AcquireLine)
3. recalibrate only if no record / older than DarkPointCorrectIntervalMinutes / forced.
   One record per (film format 0x36c/0x370, film type 0x374 with 2 -> 1, IR flag
   0x378) (FUN_1000f330, FUN_100109f0). Record +0x24 = time() of the last
   calibration, +0x30 = of the last full one (FindCorrections 10021590 stamps
   +0x24 always, +0x30 only when full). Interval = settings +0x38, default 0x3c =
   60 min (constructor FUN_10011fd0); PSI.exe reads it with GetProfileInt.
   Recalibrate when `interval*60 < now - rec+0x24`; scan flag 0x10000 adds 900 s
   to `now` (meaning unknown). Full (LED current search) when `now - rec+0x30 >
   72000` s = 20 h (FUN_10010250), else from the saved currents. Ours:
   `pakon_calcache.h`, `--scan-code` (`--recalibrate` forces full).
4. FN_bDrvPutCcdFpgaSettings: bank82 reg0 bit 0x100 = IR mode X (0x160/0x161); reg6 = integration period X (0x0c1a); reg4 = EEPROM Offset; reg5 = Offset+height
5. AtoD offsets bank84 regs5-7 sign-mag (bit 0x100 neg, ±255); gains bank84 regs2-4 (<= 63)
6. LampOn
7. motor start, no wait: `02 05 SCN 02 a5 speed16`, `04 03 SCN 00 a0` X (0xa1 reverse; 0xa5 = speed)

### Run (FN_iScanStrips 10029b80)
- one overlapped ReadFile over the whole ring; driver keeps reading 0x86
- start acquisition: FPGA reg0 = ctrl|1, then `02 06 LOW 03 91 v16 mode` X (`91 10 00 01`)
- NO per-line commands. Later strips resend only 0x91. Stop acquisition = clear reg0 bit0.

### Interrupt-service thread (Thread_PpbInterrupt 1002eb70) — the missing "housekeeping"
- polls `03 01 10` every 1 ms at high priority while scanning (200 ms idle)
- reply bit 0x80 -> for LOW and SCN: `01 03 a 01 02`; if flags & 0x80, ack `02 05 a 02 06 00 <status>` X (02052002060020)
- LOW status & 0xa4 -> `01 03 LOW 1e 90` (30-byte sensor read) X
- LOW status & 0x5b -> temps regs 0x83/0x84/0x88 (x 0.0625 °C)
- status LEDs = bank82 reg9 X

### Light (FN_bDrvLampOn 1002c5f0)
- LOW 0x80 enable: bit0 visible, bit1 IR X
- LOW 0x81 5 bytes currents [B, IR, R, 00, G]
- LOW 0x82 12 bytes u16 [dB, dIR, dR, 0, dG, period], clk 833333.3
- ceilings R/G/B/IR: base 6/8/8/0 (IR on 8/8/8/8); Plus 4/20/20/0 (IR on 8/24/24/8)
- on from BeforeScan to AfterScan; nothing refreshes it; motor does not gate it.
  Likely (inferred) cause of dark calibration: 0x81/0x82 never sent non-zero.
- LOW 0x8b/0x8c/0x8d/0x8f = temperature thresholds (LampTemp*, 1/16 °C), NOT exposure.
- `d0 00` / `d1 01` (TEC) also sent on the base F-135 X

### 0x91 value and motor speed (FN_bSetF135 10010760)
- v91 = round(A*clk*0.01/(E*B)); IR: 0.005 and E_ir
- E = 0x913/0xd9d/0xff0; E_ir = 0x60d/0x913/0xc1a
- default MotorSpeed = 100*v91 (matches F-135+ EEPROM)
- run speed = MotorSpeed(_Ir) * MotorAdjust(/_Ir/Drag/Drag_Ir)/1000, adjust clamp 900-1100 (EEPROM section B: 3 bases x 4)
- speed clamps: base 400-9500, Plus 1000-32766
- RESOLVED 2026-10-23 [C]: A = DPI+0x60 = 0x5c94 = 23700 at every base (FUN_10011a60; µm, the 23.7 mm image height), B = DPI+0x68 = output width (1000 / 1500 / 2000 px), so a line advances one pixel pitch (square pixels). F-135 (clk 833333.3): Base 16/8/4 = 24.2 / 37.8 / 85.0, with IR 15.9 / 28.3 / 63.8, i.e. the triggers in src/pakon_setup.c, including the captured 0x0010 (Base 16 + IR). F-135+: the captured 0x3c / 0x75 / 0x107 are 2.5× the formula with its integration times (DPI+0xe0: 0x753 / 0xafd / 0xffd at Base 4/8/16), the ratio of the LED period words (0x0997 / 0x03D6), so its clock is 2.5× faster. The same table's third set (+0x1b0 etc.: 0x4e2 / 0x850 / 0x9c2) gives the captured Plus Base 4 + IR 0xc5 (197.5) and predicts Plus Base 8 + IR = 77 (0x4d), **Base 16 + IR = 49 (0x31)**, never captured; to verify on serial 16402.
- Per-base table FUN_10011a60 (DpiBase4/8/16_35), built for both models; FN_bSetF135 (FUN_10011510 -> FUN_10010760 per base) overrides only the integration times, so the rest applies to the F-135 too [C]: dpi+0x64 height (1000 / 2000 / 2000 at Base 4 / 8 / 16), +0x68 output width (1000 / 1500 / 2000), +0x8c dual-tap (Base 4), +0x90 LOW 0x89 flag (Base 8), +0x6c lines per frame (1500 / 2250 / 3000)

### End of film: decided on the host from pixels; no scanner signal
- each line's first word LSB must be 1, else EC_DRV_LostSync (or FifoOverflow if host status 0x02)
- line level (green? inferred) vs DetectWhite_G / DetectFilm_G, rebased from first 3 lines
  film starts: level < DetectFilm; film ends: level >= DetectWhite
- NoFilmTimeOut = seconds; ScanPacketReadyTimeOut = ms

### Status LEDs, lamp gate, DX film sense, user adjustments [C] (2026-10-23)
- FN_bDrvSetLed 0x1000c1f0: bank 0x82 reg 9 (bit map in docs/REGISTERS.md). Code = address group | colour: group 0x00 = middle LED, 0x40 = right LED; colour 0 off, 1 red, 2 green, 3 orange, 4 blinking red, 8 blinking green, 0xC blinking orange. Writes only on change.
- FN_bStateMachineLedFilm 0x1000c420, right (film) LED from the scanner state +0x70: 0 -> off, 1 -> green (film in / scanning), 2 -> blinking green (waiting for the next strip: PSI "Blinking green - insert film"), 3 -> blinking orange (strip ended). From state 3 it does not switch to 0 or 2 while hardware-status bits 0xC0000000 (to 0) or 0x80000000 (to 2) are set.
- FN_bLampTemperatureStable 0x1002cf10: polls every 250 ms until the "lamp stable" flag (+0x298, set from the event service) is up; ≤ 300 s, then EC_LampWarmUpFailure (0x8c); bails out on a hardware fault (status & 0x40548c0). Calibration (FindCorrections 10021590) waits on the same flag. FN_bDrvInitLampTemperatures 0x1002d190 writes LOW 0x8f / 0x8c / 0x8b / 0x8d (bands), 0x8e (setpoint, optional), `d0 00`, `d1 01`.
- Lamp gate, what sets the flag [C] (2026-10-25): scanner +0x298 is the LOW bank object's +0xd0 (the bank sits at scanner +0x1c8). Its constructor (FUN_1000ae30) sets it to -1 once per driver session; nothing resets it. The interrupt thread's status read (FUN_1000bdd0) calls FUN_1000b890 for a LOW event whose status has any of 0x5b. There, status bit 0x02 moves the flag -1 -> 0 and the same pass 0 -> 1 (stable). When its timer (+0xac) runs out it also reads the lamp flags LOW `0x83`: bit 0x02 the same, 0x40 -> hardware status 0x80 (fault), 0x20 -> 0x40, then the temperatures 0x84 / 0x88. A LOW controller that is not type 'D' with firmware version < 200 skips all this and sets the flag on its first event. In calib_prelude.pakscan the one event before calibration is LOW status 0x02, 38 host polls (~7.6 s) after `d1 01`, then a second pass reads 0x83, 0x84, 0x88. Ours: `pakon_cmd_wait_lamp`.
- DX levels: FN_bDrvDxGetHardware 0x1000a830 (the four 0x93 bytes). FN_bFilmTrackTest 0x1002b620: film is present when each of the four readings is below its empty baseline minus a margin (settings +0x1628, default not found). FN_bAdjustDxPots 0x1002ace0 (service procedure): steps the DX pots until each reading drops below 79% of its start (83% when the model char +0x2f8 == 'D', the Plus: that flag also picks advance speed 0x7ffe vs 0x251c); writes hardware, not for us.
- User adjustments, FN_bApplyColorAdjustments 0x10024010 -> PakonIMAu PIColorAdjustPlanar (table +0x38): per picture, the user's R/G/B (and other) offsets are added to the automatic ones before the call; mode 1 for colour neg / B&W types, 0 for 0x1000000 / 0x2000000, else 4; picture +0x54 (1/2/4) -> 1/2/3. TLX COM: iBrightness, iContrast, iRed, iGreen, iBlue, iSaturation, iRedEye (GetPictureSaturation, GetPictureRedEyeSettings). Field meanings not traced.

### Hardware status word = TLX HARDWARE_CB bits [C] (2026-10-23)
- Values from the tlx.dll type library (enum HARDWARE_CB_000): HOST_FAULT 0x1, DX_FAULT 0x2, LAMP_FAULT 0x4, CCD_FAULT 0x8, MOTOR_FAULT 0x10, HARDWARE_ERROR 0x40, LAMP_WARNING 0x100, LAMP_ERROR 0x200, TEMP_WARNING 0x400, TEMP_ERROR 0x800, LAMP_BURN_OUT_ERROR 0x1000, LAMP_FAN_WARNING 0x2000, LAMP_FAN_ERROR 0x4000, POWER_WARNING 0x40000, POWER_ERROR 0x80000, MOTOR_STEPPER_CCD 0x200000, MOTOR_STEPPER_LENS 0x400000, MOTOR_FILTER_WHEEL 0x800000, MOTOR_FILM_GUIDE 0x1000000, FILM_IN_GUIDES_ERROR 0x2000000, BLOWER_WARNING 0x4000000, CLEANING_REQUIRED 0x8000000, FILM_EMULSION_DOWN 0x10000000, FILM_TAIL_FIRST 0x20000000, FILM_SENSE_ENTRY 0x40000000, FILM_SENSE_EXIT 0x80000000.
- FN_bDrvGetHardwareStatusDx 0x10009790 reads the 0x90 block; high nibble of the first entry: 0x20 -> SENSE_ENTRY, 0x10 -> SENSE_EXIT, and via FUN_10014320 0x40 -> EMULSION_DOWN, 0x80 -> TAIL_FIRST (also latches a flag on the strip's DX object). While the engine is not ready for film (internal state 3/4), film at a sensor with option settings +0x1614 set and scan flag 0x10 clear -> EC_FilmInGuides (0x81), scan aborted. Decoded on our captures: docs/PROTOCOL.md "DX sensor block".
- CLEANING_REQUIRED from calibration: FN_bCalibrateAfter 0x1001dd80 -> FUN_100101e0 -> FUN_1000f5b0 on the open-gate per-column table, when settings +0x15f0 enables it: skip 20 columns at each edge, slide a window (+0x1618 columns, ≥ 1, from offset +0x161c) and flag when a window and its neighbour differ by more than (1000 + setting +0x1620)/1000 on any of R/G/B (a narrow dip = dust on the gate). Defaults are registry values, not in the install.
- STRIP_MODE values: FULL_ROLL 0, MULTI_STRIPS_4/5/6_FRAMES 4/5/6 (= frames per strip).

### DX timeline per strip: placing frame numbers (2026-10-23, partly traced)
- DX object per strip (strip +0x78), FUN_10013800(k = DPI+0x60 = 23700, u = DPI+0x9c, d = DPI+0xa0, …): derived +0x58 = 26975·k/u (offset added to odd, "A"-slot codes, FUN_10014130), +0x2c = 5800·k/(u·d), +0x30 = 25900·k/(u·d); +0x28 = (film format == 4) halves every position. DPI+0x9c is copied from DPI+0x78 (0 in FUN_10011a60, set later; not traced), so the DX unit is not known yet.
- Entry dispatch FUN_100148a0 by type: 7 (film start) -> FUN_10013900 anchors the DX position (+0x38) to the **scan line where the pixel test saw film** (scanner +0xa8 -> +0x34); 5 -> position of the next code; 3 -> the code; 6 -> marks / d (+0x9d4 list); 8 -> pairs 1000·k/u ≤ gap ≤ 2500·k/u: midpoint + 600·k/u (1200 when tail first, flag from bit 0x80) -> frame marks (+0x840 list).
- FUN_10014a10: per-strip cleanup, drops codes closer than a minimum, fills missing half frames at the half-frame pitch (+0xb64) and extends at both ends; FUN_100150e0 measures the actual pitch from the codes; FUN_10015520 relates the DX timeline to scan lines with fixed distances (16000, 19200, 2250, 3000, 2000, 7500, 6000 ·k/u) and logs to `Logs\PakonDxLog.txt`.
- Framing check FUN_10006310 (with DX enabled): a picture [start, length] is accepted when one frame mark falls in its first quarter and the next in its last quarter.

### Digital ICE: DMLDICELib.dll [C] (2026-10-23)
- Applied Science Fiction "Digital ICE Library (DML version)" 1.0.0.0, imports only KERNEL32 (threads, heap, critical sections). Exports `DICEVersion`, `DMLDICEBegin(ulong, DICEInfoStaticTag)`, `DMLDICEProcess(void*, DICEInfoDynamicTag)`, `DMLDICEDefectCount(void*, DICEInfoDynamicTag, ulong*, ulong*)`, `DMLDICEEnd(void*)`. Loaded by FUN_10012ca0: Begin(0x65, static {0x20, 2, 1, 0xe, 2, 0, 0, 0}).
- Called in FN_bLoadImageFromBuffer 0x10026c90 when the picture has IR and the library loaded, on the linear 16-bit planar R, G, B, IR image **before** the density LUT: Process(handle, dynamic {0x30, planes, width, height, 300, negative = film type != 2, 0, 0, 0, width-1, height-1, 0}). Errors -1..-9 -> EC_DICE_* (FUN_10012e10).

### Per-film default adjustments [C] (2026-10-23)
- FUN_10026210 reads `Config\ColorCorrection\Defaults.ini`, section = DX product number, `BnW`, `POSITIVE` or `IMPORTED`: Red, Green, Blue, Brightness, Contrast, Sharpness (GetPrivateProfileInt, default 0) as the picture's starting adjustments. The shipped file lists every product with no values, so OEM output carries no per-film offsets. Its comment gives the UI units: one slider click = 25 for colour, brightness and contrast (4 clicks red = -100, 20 clicks brightness = -500, 4 clicks contrast = 100); sharpness 3 clicks = 200.

### Strip mode: several cut strips in one scan [C] (2026-10-23)
- COM (tlx.dll typelib): `ScanPictures(iStripMode, iScanControl, bstrRollId)`, one call per roll. Enum STRIP_MODE_000: `STRIP_MODE_FULL_ROLL`, `STRIP_MODE_MULTI_STRIPS_4_FRAMES`, `_5_FRAMES`, `_6_FRAMES`. PSI.exe settings `StripMode`, `StripModeFramesPerStrip`; prompt "Insert and remove film when prompted by the film LED. Press Stop when finished." (LED "Blinking green - insert film").
- Strip buffers (FN_bBeforeScan 1002dbd0): frames per strip N = scan params (+0x114)+0x14; 0 = full roll -> 1 buffer holding every line. Else strip length = N × pitch + pitch/4 lines, strip count (+0x384) = roll length / strip length, capped at 15; each strip buffer (CiBufferStrip, array at +0x304) gets total lines / count. Allocated up front, nothing differs on the scanner side.
- One session (FN_iScanStrips 10029b80): one overlapped ReadFile for every strip; motor runs throughout; teardown (FN_bAfterScan) once at the end.
- Per strip (reader thread 1002f550): strip index (+0x10) vs count (+0xc). Each strip re-measures the open-gate level from its first 3 lines and rebases DetectFilm / DetectWhite on it; film = level < DetectFilm, strip end = level ≥ DetectWhite. Then index + 1; if index < count the thread loops back to wait for film. Before every strip after the first it resets its ring pointers (FUN_10032f30(…, 0), host side) and **resends LOW `0x91`** (FUN_1000a7b0, same frame as at scan start).
- NoFilmTimeOut (settings +0x4b8, seconds; default not found): before the first strip -> EC_NoFilmTimeOut (error); after ≥ 1 strip -> normal end of the session (flag 1).
- Strip longer than its buffer -> EC_ProcessedRingTailOverflow (FUN_1002f450), PSI "film strip is longer than the scanner is configured to capture in one scan".
- Thread state on the scanner object +0x70: 1 film in, 3 strip ended, 2 waiting for the next strip. Probably the film LED (status LEDs = bank 0x82 reg 9); not traced.
- After the session (scan thread 10035040): walk the CiBufferStrip list, find pictures in each strip separately (FUN_10034a60, FN_iFramePictures), number them through the roll (strip index + picture index). Whether N limits the pictures found per strip: not traced.

### Teardown (FN_bAfterScan 1002a900) X
1. `02 04 LOW 01 80 00`  2. ResetFifos  3. `04 03 LOW 00 92`, wait 20 ms  4. `04 03 SCN 00 a2`
No motor-speed write before 0xa2, but FN_iScanStrips has already cleared FPGA bank 0x82 reg 0 bit 0 (acquire). The F-135+ captures show the same order (0x61 → 0x60, then a2). The August "write the idle speed register 0" fix was this FPGA write, so the finding and the OEM agree.

## TLB.dll 3.1.0.28 — calibration (read from code unless [INF])
- STATIC: open gate, motor stopped. No film-drive cmd anywhere under CalibrateBefore/FindCorrections (1001daa0/10021590); only optional focus steppers (flag 0x40, addr 0x28). Light switched on explicitly by FN_bDrvLampOn. "light only while motor runs" is wrong for TLB. pakon-macos likely never wrote 0x81/0x82 [INF].
- Preconditions: wait <= 300 s for lamp temp stable (1002cf10) else EC_LampWarmUpFailure; refuses positive film (colour 2) and non-35mm.
- Full LED search only if flag bit0 or > 72000 s (20 h) since FullLightCorrections (FUN_10010250); else reuse stored currents/duties.
### Phases (CalibrateLEDs 10020dc0)
1. window: first 6 px optically black (3 dual-tap); active cols start at dpi.offset - 6
2. lamp off (LOW 0x80 = 0)
3. dark offset: gain code 13 (g=1.2), offsets start 10/10/10, <= 8 iters x 32 lines, black-pixel mean target 300 ±32, off += round((mean-300) * -0.0133929)
4. fixed-pattern dark: 128 lines averaged -> per-column dark; 128 summed -> black & active means
5. LED current search (full only): current from 1, +1 per 32-line pass until column peak > cap (R,G 64000; B 65500; IR 40000) or ceiling; then duty = (n-1)/n
6. duty refine: <= 32 iters, duty *= 63968/peak (IR 39968), converged when peak in [63936,64000] (IR [39936,40000]); 2 converged passes 200 ms apart; if short: current+1, at ceiling A/D gain code+1 (max 0x3e); else EC_InsufficientLight
7. fixed-pattern bright (formula below)
8. IR mode only: IR LED alone; visible dark tables -> v[c] - (min - 300)
9. save to registry
### Per-column (1001f550; inputs are 128-line sums)
den = (B[c]-D[c]) - (Bmask-Dmask); gain[c] = min(125*2^32/den, 0x3ffff) (IR 0x7530000000)
"prefix terms" = drift of optically black pixels between bright and dark passes.
Per line: 64000*65536/(b-d-Δblack) -> 16.16 fixed point, open gate -> 64000 (IR 60000).
Smear = 65536*(Bmask-Dmask)/(Bactive-Dactive), kept only if 1..699.
Applied per line in FUN_100246d0 (MMX): out = pmulhuw(sat(raw - dark[c] - smear term), gain[c] >> 2), i.e. (raw - dark) * gain / 2^18, so the open gate lands at 16000 in the 14-bit domain. Dark tables are u16 per column (struct +0x20/0x24/0x28), gains u32 16.16 (+0x30/0x34/0x38, IR +0x3c), smear (+0x40/0x44/0x48) times a running line level is a column-uniform offset. Implemented 2026-10-10 (dark + gain, not the smear term): `pakon_replay` writes the tables as JSON, `pakon_image.py` applies them.
### Film-base densities (FUN_10020230): calib duty capped at 1/10^D, scan duty = open-gate duty * 10^D
colour neg R0.144 G0.40 B0.715 IR0; B&W C-41 0.10/0.25/0.25/0.08; other (positive at scan, B&W) 0/0.03/0/0.08
### Encodings
- LampOn: LOW 0x80 bit0 vis bit1 IR; 0x81 [B,IR,R,0,G]; 0x82 u16 [dB,dIR,dR,0,dG,period]; period = IntegrationTime*0.6 (clk 833333.3 Hz); duty ticks <= period-2; settle waits WaitForLamp_*
- ceilings R/G/B/IR: base 6/8/8/0 (IR on 8/8/8/8); Plus 4/20/20/0 (IR on 8/24/24/8)
- A/D gain bank84 regs2-4: code = round((1-1/g)*75.6), g in [1,6]; 13 = the "13/13/13"
- A/D offset bank84 regs5-7 sign-mag (bit8 neg, ±255)
- bank82: reg6 integration time (<= 0xffd); reg4 pixel start; reg5 start+height (max 0x848 single, 0x424 dual tap); reg0 bits 0x001 acquire, 0x002 dual-tap, 0x100 IR, 0x060 init (-> 0x160/0x161); reg9 status LEDs
- temps (1002d190): LOW 0x8F lamp warn, 0x8C lamp fault, 0x8B motherboard warn, 0x8D motherboard fault; 0x8E lamp setpoint (LampTempWorking 37..48 °C); then LOW 0xD0=0, 0xD1=1 (TEC). 0.0625 °C/count. Readback LOW 0x84 setpoint, 0x88 lamp+mobo temps.
### Registry …\TLB\Scan\DpiBase{4,8,16}_35
Gain_R/G/B, Offset_R/G/B, Current_R/G/B/Ir, DutyCycle_* (film), DutyCycleOpenGate_*, FullLightCorrections, SpliceDarkness, DetectWhite_G, DetectFilm_G. Per-column tables in memory only.
### Open
16-bit multiplier in scan MMX kernel (FUN_100246d0) from 16.16 table; SmearC ramp (IR); dpi+0x90 flag (LOW 0x89); defaults of WaitForLamp_* and temp thresholds.

## TLB.dll 3.1.0.28 (sha256 5866ec56…) — EEPROM use and 135-line colour path
[C] = read from code/disassembly, [I] = inferred

### EEPROM -> engine (FN_bReadEEPromToRegistry 0x10016a90, FUN_10016860, FUN_10016610) [C]
- Per base (4/8/16): Offset, MotorSpeed, MotorSpeed_Ir from section A 0x014/0x01a/0x020.
- Section B per base, 4 words each (0x808/0x810/0x818): MotorAdjust, MotorAdjustDrag, MotorAdjust_Ir, MotorAdjustDrag_Ir; clamped 900..1100.
  -> the byte where the base F-135 differs (0x80C) is base-4 MotorAdjust_Ir (1008 vs 1000).
- NegMatrix (A 0x026) / PosMatrix (A 0x09e) -> colour object, unless a client matrix file is loaded.

### Offset = CCD pixel-window start (FN_bDrvPutCcdFpgaSettings 0x1002c340, scanner PIC) [C]
- FPGA bank 0x82: reg4 = Offset, reg5 = Offset + height, reg6 = line period (<= 0xFFD).
- reg6 is NOT height (pakon-macos REGISTERS.md is wrong): 0x0C1A = base-16 IR line period.
- Checks: Offset >= 6 (>= 3 half mode), Offset + height <= 0x848. Default: height 2000, offset 62, period 0xFFD.

### Motor speed (FN_bBeforeScan 0x1002dbd0 -> FN_bDriveMotorAdvanceFilm 0x1000b6d0) [C]
- speed = MotorSpeed[_Ir] * MotorAdjust[Drag/_Ir] / 1000 -> scanner-PIC reg 0xA5; then cmd 0xA0 forward / 0xA1 reverse; timed moves end with 0xA2.
- Reference names "SetMotorCalibration"(0xA5) and "StopFilmDrive"(0xA1) are wrong.
- Clamps: Plus 1000..32766, base 400..9500.
- 0x91 trigger value = round(A*833333.3*0.01/(P*B)) (0.005 with IR), P = 0x913/0xD9D/0xFF0 per base (FN_bSetF135 0x10010760); A, B unresolved.

### Colour negative path [C]
- PIColorCorrectColNegPlanarScan/…Save: GetProcAddress'd, never called.
- Real path: FN_bLoadImageFromBuffer 0x10026c90 (flag 0x10):
  1. per-plane LUT on planar 16-bit: LUT[i] = 3500*log10(16383/i), 16384 entries, LUT[0]=0x3FFF (FUN_1000dfc0).
  2. 3x10 on density (FN_bReadMatrix_3x10 0x1000d880): NegMatrix for colour neg and both B&W; PosMatrix for positive (then PIColorCorrectColRevPlanar).
     out = m0R+m1G+m2B+m3R²+m4G²+m5B²+m6RG+m7BR+m8GB+m9, +0.5, clamp 0..4095 -> 12-bit.
     Term order is RG, BR, GB (reference says RG, GB, BR).
  3. scale/rotate -> 12-bit RPD/sRGB profiles + Ansel scene balance.
- NegMatrix constants positive -> no film-base subtraction here; base likely neutralised by light calibration [I].
- Roll balance FN_bKcdfsCorrections 0x10034a60, once per strip (2026-10-23) [C]: the 3 roll values are the strip's FN_bFindTmax output (strip +0x6cac, below), through the same LUT+matrix; then PIAnselStartNewRoll(strip, mode), PIAnselAddScene per picture (≤ 40), PIAnselEndRoll, PIAnselAnalyzeRoll. Each strip is its own Ansel roll.
  - mode: colour neg / B&W types (1, 4, 8, 0xC) -> 3 if settings +0x4ac == 1, else 1 or 2 (param_5); positive (2) -> 4. PakonIMAu FUN_10001e70 maps it to the processing path: 0 "DC-Premium", 1/2 "CN-Enhanced", 3 "CN-Lockbeam", 4 "CP-Balance" (paths per item in `anselinstalldir/dataPathItems/*/*.map`).
  - AddScene struct (PakonIMAu FUN_10002290): [0x12] mode -> sourceType 1 (35 mm negative) + metric 1 (PD12) for modes 1–3, 3 (35 mm chrome) for 4, 5 (camera) for 0; for negatives also `dmin` = [0x15..0x17] (the Tmax triple in RPD), `productCode` = [0x18], `genCode` = [0x19] (the strip's DX, -1 = none). sba.map selects the SBA .dpi on sourceType + productCode + genCode (docs/IMAGING.md).
- FN_bFindTmax 0x10005ae0 [C]: colour neg / B&W types only. Near the strip start (from row 25400 × (vtbl +0x10) / (+0xc6c), probably 1 inch in µm, for 6 × (+0xc70) / (vtbl +0x24) rows; inputs not traced), central ¾ of the width, per channel: 16384-bin histogram, Tmax = the level that the brightest 0.1% of samples exceed (the clear film base). Stored at strip +0x6cac (R, G, B).
- ClientColNegLut.txt + ClientColNegMat_3x10.txt override only if both exist; shipped _ClientColNegLut.txt (leading underscore) is never loaded.

### Positive (slide) path [C] (2026-10-14)
- FUN_1000d880: film type 2 -> matrix at +0xC8 (PosMatrix0..29), same 3x10 polynomial, +0.5, clamp 0..4095; then PakonIMAu PIColorCorrectColRevPlanar (DAT_10075544+0x4c) with (planes, w, h, 0, 0). Types 1/4/8 -> +0x50 (NegMatrix), no further call.
- Before it, FUN_10022a60 applies LUT +0x44 for type 2, else +0x40; FUN_1000dfc0 fills both with K*log10(16383/i) (K = +0x20, 3500), so they are identical unless ClientColRevLut.txt / ClientColRevMat_3x10.txt override.
- PIBegin(…, romm.pf, rpd.pf, srgb.pf, ColRevLut1.pf, ColRevLutS6.lut): ColRevLut1.pf path -> PakonIMAu DAT_106b2f08, opened by PIColorCorrectColRevPlanar (FUN_102f7480(path, 2)) as a device link; ColRevLutS6.lut -> FUN_1001e0b0 -> 4096 u16 at DAT_106b3708, which has no other reference in PakonIMAu.dll (dead).
- ColRevLut1.pf: KCMS 'link' class, "Pakon CMY to ROMM12", A2B0 mft2 3->3, grid 31, 511 input entries, 2 output entries (identity).
- FindCorrections (FUN_10021590) refuses film type 2 (error 0x1b/0xf), so calibration runs under another type; the scan uses the "other" densities (FUN_10020230).
- The Ansel balance call (DAT_10075544+100, flag 0x20) runs for every type; its mode word is 1 when the film type is odd (only colour neg, type 1) and 4 otherwise (positive 2, silver B&W 4, B&W C-41 8).

### Picture framing (FN_iFramePictures 0x100079c0) [C] (2026-10-16)
- Geometry is fixed per resolution (FUN_10011a60, the per-base object; FN_bSetF135 sets the rest):

  | Base | block (lines) | picture (lines) | pitch (lines) | width (px) |
  |------|---------------|-----------------|---------------|------------|
  | 16   | 8 | 3000 (+0x6c) | 3206 (+0x70) | 2000 (+0x68) |
  | 8    | 6 | 2250 | 2405 | 1500 |
  | 4    | 4 | 1500 | 1603 | 1000 |

  Every picture is 375 blocks; pitch is ~400 blocks at every base.
- Output rect (default branch): rows = block*start + 0 .. +picture-1, cols 0 .. width-1 (full sensor width). The crop is anchored at the detected picture start, never centred between gaps.
- Profile (FUN_10006870): one value per block = 8-bit (R+G+B)/3 of the low-res line, inverted (255-v) for every film type except type 2 (positive). The gap (clear base) is low and the picture is high.
- Threshold (FUN_10005d20, mode 0): 256-bin histogram; walk up from bin 0 to the first peak (stop once a bin is < 3/5 of a peak holding > n/150 samples). lo = walk down while bin >= peak/20; hi = walk up (<= 50 bins) while bin >= peak/10. If (peak-lo)*4 < hi-peak, threshold = first bin above the peak < peak/2; else hi + (hi-peak)/2. ones = profile > threshold.
- Sweep (FN_iFramePicturesSub 0x100072c0): move the threshold +2 then -2 while the count of runs with 95% < length < 115% of picture does not drop; keep the best. Logs "Framing Ones".
- Placement (FN_iFramingFindPictures 0x10006e70), slots = start*2/pitch:
  1. LookForNicePictures (FUN_10006930): a run of 95–115% of picture -> start = run start + (run length - picture)/3. A run of 95–115% of picture + pitch (missing gap) -> two pictures: run start + (len - pitch - picture)/3 and run start + pitch.
  2. LookInBetweenEnds (FUN_100063d0): between two found pictures, fill n-1 evenly spaced pictures, n = span/pitch (minus 1 if the remainder is < pitch/4).
  3. LookAtEnd / LookAtBeginning (FUN_10006ae0 / FUN_10006ca0): step one pitch outward; FN_iFramingEstimatePlacement (FUN_100064e0) slides a picture-long window across a (pitch - picture)-wide range and keeps the offset with the most ones (ties: nearest the centre). Stops when FUN_10006630 finds no film.
  4. Nothing found -> FramingBlindlyPlacePictures (FUN_10006720): a pitch grid from (pitch - picture)/2.
- With DX enabled (+0xca4) each picture must also agree with the DX frame-number positions (FUN_10006310).
- Check on a 5-frame Base 16 colour-negative strip: 5 "nice" pictures, starts 3176–3296 apart, every crop between the gaps. The former find_frame_grid (since removed) locked a 3606 pitch, found 4 frames, and its crops straddle the gaps.
