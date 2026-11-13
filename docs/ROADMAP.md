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

The F-135 is there. The F-135+ scans in code too, at every base with IR
on and off; only its film advance and eject are still timed runs at a
captured speed.

## What works today

"Hardware" below means run on a real scanner. Hardware runs so far were on
one F-135 (serial 3054) and two F-135+: serial 16402 (a contributor's unit,
C driver, replays) and serial 138 (a demo unit, Rust, code-built scans,
2026-11-13).
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
| Firmware load | Same FX2 image as the F-135, from the OEM HEX files | Rust, CLI (macOS), serial 138 |
| Open handshake, model detection | In code | Rust (macOS), serial 138 |
| EEPROM read and archive | In code, automatic in the app | Rust, app (macOS), serial 138 |
| Setup | Built in code; matches the Plus captures byte for byte (Base 16, 8, 4, 4 + IR) | Rust (macOS), serial 138 |
| Light calibration, flat field | In code, with the Plus LED ceilings and LED period | Rust (macOS), serial 138, all six modes |
| Scan, Base 16 / 8 / 4, IR on and off, colour negative | In code, stops on its own at end of strip, DX read | Rust, CLI (macOS), serial 138: all six modes on a 4-frame strip; app: Base 16 + IR |
| Scan, replayed (`resources/f135plus/`) | Capture replay, kept as a fallback without an EEPROM archive | C driver on 16402 (Base 16, 8); Rust on 138 (Base 16) |
| Film types other than colour negative | Per-film LED duties, as on the F-135 | Not run on a Plus |
| Film advance | Timed run at the captured motor speed (`--advance`) | C driver on 16402; Rust on 138 |
| Eject by film sensors | Not in code (`--eject`, `--advance-code` are F-135 only) | — |
| Decode | Row layout detected per raw: 6000 / 4500 / 3000 samples IR off, 8000 / 6000 / 4000 IR on | Works on the Plus raws, Digital ICE included |

The code-built scan stops by itself at the end of the strip and its
teardown stops the motor (serial 138).

### Platforms

| Platform | Package | On a scanner |
|---|---|---|
| macOS 13+ (Apple silicon; universal build for Intel) | `Pakon.app` + dmg, ad hoc signed | Yes, F-135 (serial 3054) and F-135+ (serial 138), Rust app and CLI |
| Windows 10 / 11 (64-bit) | `Pakon-Setup.exe`, installs WinUSB; unsigned | No. The app installs and opens on Windows 11 (a VM, no scanner); the service processes and exports with Digital ICE under Wine. |
| Linux (Debian/Ubuntu, amd64 and arm64) | `.deb` with a udev rule | No, not with the Rust code. The app installs and opens on Linux Mint 22 (no scanner); the package processes and exports with Digital ICE in a clean Ubuntu 22.04. (Replay scans ran on Linux with the earlier C driver.) |

## Open work

### F-135+ built in code

Done on serial 138 (2026-11-13): setup, light calibration and flat field
(#15), and the code-built scan at every base with IR on and off (#14),
from the CLI and the app. Left:

1. Film advance and sensor-driven eject (issue #16): the Plus still runs a
   timed advance at a captured speed.
2. Silver B&W, B&W C-41 and slide on a Plus (the per-film duties are the
   F-135's rules, not checked there).
3. The code-built scan on a second Plus (serial 16402 or another).

### Platforms and releases

- The USB path on Windows and Linux, on a scanner (nothing below has run
  there with the Rust code):
  - the WinUSB driver install on a clean Windows (`pakon_driver.exe`), and
    the udev rule giving the logged-in user access on Linux;
  - firmware load on the cold device (`0f05:f235`), then the device coming
    back as `0f05:f135` (on Windows, nusb claims interface 0 of the cold
    device; in a VM both IDs must be passed through);
  - the EEPROM read and its archive when a warm scanner connects;
  - open, setup, light calibration and a full scan with end-of-roll stop,
    at Base 16, 8 and 4, IR on and off;
  - Stop during a scan, Advance film (eject), and closing the window while
    scanning (teardown: motor stopped, lamp off);
  - unplugging and replugging, and a scanner switched off mid-session.
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
