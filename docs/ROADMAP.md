# Roadmap

What works today, what is still open, and what is known to be wrong. Open
work is also tracked in [GitHub issues](https://github.com/jorshhh/open-pakon/issues).
How to build and run the code is in [TECHNICAL.md](TECHNICAL.md).

## Goal

A client for the Kodak/Pakon F-135 and F-135+ that never replays USB
captures: every command the scanner receives is built in code from known
parameters (the unit's EEPROM, a calibration we run, the chosen resolution
and film). The captures stay in the repo as test fixtures: generated command
streams are checked byte for byte against them.

The F-135 is there. The F-135+ still replays captured scripts.

## What works today

"Hardware" below means run on a real scanner. Hardware runs so far were on
one F-135 (serial 3054) and one F-135+ (serial 16402, a contributor's unit).
Some steps were verified on hardware with the earlier C driver and not yet
rerun with the Rust code; the Rust code builds the same command streams
(checked byte for byte), but the tables say which is which.

### F-135

| Step | How | Hardware |
|---|---|---|
| Firmware load | From the OEM Intel HEX files; captured `.pakfw` as fallback | Rust, CLI (macOS). Not yet from the app window. |
| Open handshake, model detection | In code | Rust (macOS) |
| EEPROM read and archive | In code, OEM read-only requests, automatic in the app | Rust, app (macOS) |
| Setup, lamp wait, light calibration, flat field | In code | Rust (macOS) |
| Scan, Base 16, IR on, colour negative | In code, stops on its own at end of roll, DX read | Rust, CLI and app (macOS): 4-frame and 36-frame rolls |
| Scan, Base 8 and Base 4 | In code | C driver (macOS); no Rust run recorded |
| Scan with IR off (silver B&W default) | In code, all three bases | C driver; no Rust run recorded |
| Film types: colour negative, silver B&W, B&W C-41, slide (E-6) | Per-film LED duties at scan; decode per film | C driver, a strip of each; no Rust run recorded except colour negative |
| Teardown, motor stop | In code, guaranteed on drop | Rust (macOS) |
| Film advance and eject | In code; eject reads the DX film sensors | Rust, CLI (macOS). Not yet from the app window. |
| Decode, framing, OEM colour, OEM scene balance, Digital ICE, export | `pakon-image`, `pakon-oem`, `pakon-roll` | Rust, app (macOS), bit-identical to the Python pipeline on real rolls |

Without an EEPROM archive the F-135 falls back to replaying the captured
Base 16 scan.

### F-135+

| Step | How | Hardware |
|---|---|---|
| Firmware load | Same FX2 image as the F-135 | C driver (macOS); Rust not run on a Plus |
| Open handshake, model detection | In code | C driver; Rust not run on a Plus |
| Setup | Built in code for both models and matches the Plus captures byte for byte (Base 16, 8, 4, 4 + IR) | Not run on a Plus |
| Scan | Replay of captured scripts (`resources/f135plus/`) | C driver: Base 16 and Base 8 live. Base 4 replays badly (the capture waits on lamp and calibration events). Base 4 + IR decodes but was never replayed. Rust not run on a Plus. |
| Light calibration, flat field | Not in code | — |
| Film advance | Timed run at the unit's captured motor speed (`--advance`) | C driver |
| Eject by film sensors | Not in code | — |
| Decode | Row layout detected per raw | Works on the Plus raws |

The app on a Plus scans Base 16 colour negative only (the replayed script).
A replayed scan is as long as its capture (a 4-frame strip), and the strip
can stop short of the exit: the OEM polls the film out, a replay cannot.

### Platforms

| Platform | Package | On a scanner |
|---|---|---|
| macOS 13+ (Apple silicon; universal build for Intel) | `Pakon.app` + dmg, ad hoc signed | Yes, F-135, Rust app and CLI |
| Windows 10 / 11 (64-bit) | `Pakon-Setup.exe`, installs WinUSB; unsigned | No. Built and smoke-tested in a container only. |
| Linux (Debian/Ubuntu, amd64 and arm64) | `.deb` with a udev rule | No, not with the Rust code. Built and smoke-tested in a container only. (Replay scans ran on Linux with the earlier C driver.) |

## Open work

### F-135+ built in code

1. A first run of the Rust code on a Plus, replay path: firmware, open,
   Base 16 scan, advance.
2. Setup in code on the Plus (already matches its captures).
3. Light calibration and flat field (issue #15).
4. Code-built scan (issue #14), all bases. The first scan no replay could
   do: Base 16 + IR, never captured; the `TLB.dll` table predicts
   integration `0x9c2` and `0x91` = 49 (`0x31`) on serial 16402.
5. Film advance and sensor-driven eject (issue #16).

### Platforms and releases

- A scan from the Windows and the Linux package on real hardware.
- Eject and firmware load from the macOS app window (done from the CLI).
- Developer ID signing and notarization of the Mac app (issue #25):
  `build.sh` supports both, but no certificate or notary profile is set up,
  so builds are ad hoc signed and other Macs refuse them. The Windows
  installer is unsigned (SmartScreen warns once).
- Release CI: on each `v*` tag, build the macOS, Windows and Linux packages
  and publish them as release assets linked from the download page.

### Image

- The remaining OEM scene-balance stages (issue #22), starting with SCPLut,
  and a scan made with the OEM TLX software to compare against.
- Slides get no scene balance: the OEM runs Ansel in a positive mode, not
  modelled. Slide colour still needs a check against a lightbox.
- Per-picture adjustments (issue #55): the TLX arithmetic is implemented
  (contrast, R/G/B and brightness offsets, unsharp mask, saturation, B&W
  tone; [IMAGING.md](IMAGING.md)); the app exposes only Sharpen. Per-frame
  sliders need a preview with one frame's scene balance (the roll stage runs
  once, then each frame).
- DX frame numbers are decoded but not used: placing codes on frames is
  unresolved.

### Tools

- The `pakon_replay` modes not ported from the C tools (`--read-params`,
  `--params-out`, `--calib-probe`, `--film-sense`, `--trace-status`, advance
  scripts), if they are needed again. They remain at tag `python-final`.

### Ideas

- A dust check in the light calibration (see Known issues), and a way to
  force a new calibration from the app.

## Open questions to settle on hardware

Brief; the detail is in [PROTOCOL.md](PROTOCOL.md) and
[REGISTERS.md](REGISTERS.md).

- Does clearing the acquire bit then `a2` stop the motor on the F-135+, as
  it does on the F-135?
- F-135+ Base 16 + IR: row stride (expected 8000 samples) and the predicted
  integration and `0x91` values.
- The film-advance speed is the captured one (9500); it is not derived from
  the EEPROM. The DX film-sensor thresholds are this unit's (the sensor pots
  are per unit).
- The first ~40–60 columns of every line carry amplified IR noise with a
  heavy dark tail; its cause is unknown (a dark scan would tell). ICE widens
  its margin there to compensate.
- Do overexposed (near-clear) slide frames trip end-of-roll detection?

## Known issues

### Licence: Unicorn (parked)

Unicorn, which runs the OEM DLLs, is GPL-2.0 and is linked into the
AGPL-3.0 binary; the two licences do not allow that combination. Relicensing
the app as GPL-2.0 is no way out: tao, sync_wrapper, ring, zopfli and
ab_glyph are Apache-2.0 only, which is incompatible with GPL-2.0. Options:

- run the OEM DLLs in a separate GPL-2.0 helper process;
- ask Unicorn's maintainers for a linking exception;
- a build without the OEM path: no OEM Digital ICE or scene balance; our
  own ICE port and the OEM colour path without scene balance remain.

### Dust in the film gate during calibration

The light calibration measures the open gate, and the flat field divides
every scan by it. Dust in the gate during calibration casts a shadow in the
calibration line (seen: a 14–18 % dip) that the scan does not have, so every
frame gets a coloured line along the scan direction. Fix: clean the gate and
calibrate again. The app reuses a calibration for up to an hour and has no
button to force a new one; from the CLI, `pakon_replay --scan-code
--recalibrate` does. A dust check in the calibration (a narrow dip in the
open-gate profile) is a possible feature.

### Digital ICE is about 10 % slower per frame

In the app, OEM Digital ICE takes about 10 % longer per frame than it did in
the Python app, although the same code measured faster in isolation (1.9 s
against 2.35 s a frame). No cause has been found yet. How many frames export at
once is in [TECHNICAL.md](TECHNICAL.md) (export worker rule).

### Other

- Unsigned builds: see Platforms and releases.
- The OEM product-code table for DX is from 2006; newer films have no entry,
  and then no film name or ISO is written into the exports.
- B&W C-41 film's IR is grainier than colour negative's; ICE replaces some
  grain with grain (no visible change at 1:1). A wider margin for that film
  is a candidate if it ever shows.
