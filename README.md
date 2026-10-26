<p align="center">
  <img src="brand/logo.png" width="112" alt="Pixel-art drawing of the Pakon F-135 scanner">
</p>

<h1 align="center">Pakon</h1>

<p align="center">
  <b>Your Pakon, back in service.</b><br>
  Scan film on your Kodak Pakon F-135 or F-135+ from your Mac.<br>
  No old Windows PC needed.
</p>

<p align="center">
  <a href="https://github.com/jorshhh/pakon-macos/releases/latest/download/Pakon.dmg"><b>⬇ Download for Mac</b></a>
  &nbsp;·&nbsp; Apple silicon, macOS 13 or later
</p>

![Pakon's Frames page: a colour roll laid out as a strip, with a box around each picture](docs/images/frames.jpg)

## What is this?

The Kodak Pakon F-135 is a film scanner that photo labs used to scan 35 mm
rolls quickly. Lots of them are now in the hands of film photographers, but
the original software only runs on old versions of Windows.

**Pakon** is a free app that replaces it. Plug the scanner into your Mac, feed
a strip of film, and get finished pictures: JPEGs ready to share, 16-bit TIFFs
for editing, or the raw negatives. It uses Kodak's own colour processing, so
your scans look the way a Pakon scan should.

It's an independent project, not made or supported by Kodak.

## Features

- **Runs on a modern Mac.** One app: no Windows, no virtual machine, no
  drivers to install.
- **Kodak's colour.** The same colour inversion, scene balance and rendering
  as the original software, reproduced on your Mac.
- **Colour negative, black & white and slide film.** Slides are scanned as
  unmounted strips.
- **Check every frame before you save.** Pakon finds the pictures on the
  strip; you nudge, add or remove the boxes, including half and panoramic
  frames.
- **Saves the way a lab does.** Each roll gets its own numbered folder with
  JPEGs, TIFFs, raw negatives and a contact sheet. Open a roll again later
  and your boxes come back.
- **Knows your film.** The scanner reads the code printed on the film's edge;
  when Pakon recognises it, the film name and ISO go into every picture's
  details (for example "Kodak Gold 400").
- **Even edges.** Pakon calibrates the scanner's light before every scan, so
  the edges of each picture aren't darker than the middle.

![Pakon's Export page: save the roll as JPEGs, TIFFs, raw negatives or a contact sheet, above a grid of finished frames](docs/images/export.jpg)

## Which scanners work?

| Scanner | Works? |
|---|---|
| **Kodak Pakon F-135** | Yes, fully. This is the scanner Pakon is tested on. |
| **Kodak Pakon F-135+** | Yes. It scans with settings recorded from another F-135+, so there are fewer options for now. |
| F-235, F-335 | No. They're different machines inside. |

## Getting started

1. **Install.** Download `Pakon.dmg`, open it, and drag Pakon to
   Applications. It's signed and notarized, so it opens like any other app.
2. **Connect the scanner.** Plug it into your Mac with USB and switch it on.
3. **Open Pakon** and go to **Scan**. The first time after the scanner is
   switched on, press **Load firmware** (the scanner forgets it when it's
   powered off). The first time Pakon sees your scanner, it saves a copy of
   the scanner's factory settings in `~/Library/Application Support/Pakon`.
   Keep a backup of that folder somewhere safe: it exists nowhere else.
4. **Scan.** Pick the film type and press **Scan**. Pakon calibrates for
   about a minute: **keep the film out** until it says *"Motor running — feed
   the film now"*, then feed the strip. The scan stops by itself after the
   film comes through.
5. **Check the frames.** The roll opens in **Frames**. Pick the **Format**
   (full frame, half frame or panoramic) and press **Auto-frame** to put a box
   on every frame, or click to place a box yourself and press **Enter** to add
   the next one. Use the arrow keys to nudge.
6. **Save.** To sharpen the frames, pick **Low**, **Medium** or **High**
   next to **Sharpen** (the stronger settings also sharpen the film grain).
   Press **Export**, then choose what to save: JPEGs, TIFFs, raw
   negatives or a contact sheet. Everything lands in your work folder
   (`~/pakon` unless you pick another), one folder per roll.

If a strip stays inside the scanner, press **Advance film** on the Scan page.

## Good to know

- To update, download the new version, quit Pakon, and drag it into
  Applications to replace the old one. Your scans and settings stay where
  they are.
- For B&W film developed in colour chemistry (Ilford XP2, Kodak BW400CN),
  pick **B&W C41** as the film type. Dust removal works on it, as it does
  on colour negative.
- If the scanner stops responding, switch it off and on, then load the
  firmware again.
- If something goes wrong, the log is at `~/Library/Logs/Pakon/pakon.log`.
  Please [open an issue](https://github.com/jorshhh/pakon-macos/issues) with
  it.
- **Linux, or a scanner shared on your network:** run Pakon from source. See
  the [technical guide](docs/TECHNICAL.md). A Windows build is planned.

## Coming next

More resolutions and everything the F-135+ still lacks. Follow along in the
[issues](https://github.com/jorshhh/pakon-macos/issues).

## For developers

Building from source, the command-line tools, the web service, the driver's
design and the scanner's protocol are all in the
**[technical guide](docs/TECHNICAL.md)**, with deeper notes in [`docs/`](docs/).

## Privacy

The first time it opens, the Mac app asks whether it may send anonymous
**crash reports** and **usage statistics**. Nothing is sent unless you say
yes, each has its own switch, and you can change your mind any time on the
**Settings** page. They go to the developer's own server running
[Rybbit](https://rybbit.com) (open-source analytics), not to a third party.

- **Crash reports:** the error and where it happened in the code, and for a
  failed scan the last lines of the scanner log.
- **Usage statistics:** which pages you open, how long scans, processing and
  exports take, the film type, the number of frames, the export format and
  the scanner model.
- **Never sent:** your pictures, file or folder names, roll names, or your
  scanner's serial number or factory settings.

Pakon also asks GitHub whether there's a newer version. Copies you build
yourself never send anything; see the
[technical guide](docs/TECHNICAL.md) and `web/telemetry.py` for details.

## Credits

pakon stands on other people's work:

- **Ali Bosworth**:
  [pakon-reference](https://github.com/alibosworth/pakon-reference) (CC BY
  4.0), the public, implementation-agnostic reference for the F-X35 family.
  The DX barcode support is built on its account of the sensor block, the
  barcode layout and the OEM product-code table; its EEPROM layout and safety
  rules are used throughout. Ali also brought up and verified the F-135+ on
  serial 16402: its captures are the source of `resources/f135plus/` and of
  `docs/F135_PLUS_CAPTURES.md`. Several of the reference's hardware facts
  were measured with this project.
- **Mats Fagerberg** ([thetalkingdrum](https://github.com/thetalkingdrum)):
  the base F-135 EEPROM dump (serial 2233) used as a test fixture, via
  pakon-reference, and pointing us to nkscan for Digital ICE.
- **Digital ICE** ([#37](https://github.com/jorshhh/pakon-macos/pull/37)): a
  numpy port of the dust removal in
  [nkscan](https://github.com/activexray/nkscan) (`src/dust.rs`, MIT OR
  Apache-2.0, © the nkscan authors), which implements the pipeline documented
  by [openICE](https://github.com/a6o/openICE) (a6o, GPL-3.0).
- **Kai Kaufman** ([ktkaufman03/FX35](https://github.com/ktkaufman03/FX35)):
  the open-source FX35 Windows driver. Its package carries the OEM firmware
  as Intel HEX (`firmware/README.md`), and with logging added it recorded the
  F-135+ captures (`tools/fx35_datalogger.py`).
- **Stefan Dierauf**: libpakon, an independent C++ driver, used as a
  read-only reference (`docs/LIBPAKON_COMPARISON.md`).
- **Kodak / Pakon**: the original F-X35 software. Its F-135 engine `TLB.dll`
  was reverse-engineered for interoperability (`docs/TLB_FINDINGS.md`); the
  colour engine `PakonIMAu.dll`, its scene-balance data and the DX
  product-code table are used as-is (`oem/`), as are the colour profiles in
  `profiles/` (© Eastman Kodak Company).
- **Software**: [libusb](https://libusb.info),
  [FastAPI](https://fastapi.tiangolo.com),
  [uvicorn](https://www.uvicorn.org), [NumPy](https://numpy.org),
  [Pillow](https://python-pillow.org) (with littleCMS),
  [tifffile](https://github.com/cgohlke/tifffile),
  [PyUSB](https://github.com/pyusb/pyusb),
  [Unicorn](https://www.unicorn-engine.org) and
  [pefile](https://github.com/erocarrera/pefile) (to run the OEM scene
  balance), [pywebview](https://pywebview.flowrl.com) (the Mac app),
  [Ghidra](https://ghidra-sre.org) (reverse engineering) and
  [Rybbit](https://rybbit.com) (opt-in analytics).

## License

Free and open source under the GNU Affero General Public License, version 3
or later (`AGPL-3.0-or-later`). Full text in `LICENSE`; see `NOTICE` for
copyright and third-party terms.

AGPL was chosen because the web app is the product: §13 requires that anyone
who runs a modified pakon as a network service offer its source to the users
of that service, which plain GPL would not.

The scanner firmware under `firmware/` and the Kodak files under `oem/` and
`profiles/` are third-party files with their own terms and are **not**
covered by this license; see `firmware/README.md`, `oem/README.md` and
`profiles/README.md`.

## Disclaimer

Reverse-engineered and unofficial. Not affiliated with or endorsed by Kodak or
Pakon.
