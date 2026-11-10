# Rust port — plan

Move the C driver (`src/`, `tools/pakon_replay.c`, `tools/pakon_probe.c`) and
the Python image pipeline and backend (`web/`, `tools/pakon_image.py`,
`oem_dice.py`, `oem_sba.py`, `pakon_ice.py`) to Rust. The UI stays the
existing page, `web/static/index.html`: a Rust web server replaces FastAPI
behind the same `/api/*` endpoints, and a Rust webview window replaces
pywebview. No Flutter, no rewrite of the front end.

The current C + Python app keeps shipping until each Rust stage reaches
parity. Nothing is removed before its replacement is verified.

## Target architecture

```
web/static/index.html   unchanged front end (HTML/JS, fetch + SSE)
      │  HTTP on localhost (or the LAN), same /api/* contract as web/app.py
pakon-app           window: wry/tao webview (WKWebView / WebView2 /
      │             WebKitGTK) on the server's port; closing it quits
pakon-web           axum server: /api/* routes and SSE streams (today:
      │             web/app.py, decode.py, orders.py, telemetry.py,
      │             update.py, eeprom.py, dx.py)
      │
pakon-scan          orchestration (today: pakon_replay.c), event service,
      │             teardown guaranteed on drop
pakon-roll          prescan, autoframe, export of a roll (today:
      │             web/decode.py, web/dx.py's file metadata)
pakon-oem           the OEM DLLs under Unicorn: Digital ICE, Ansel SBA
      │             (today: oem_dice.py, oem_sba.py)
pakon-image         decode, OEM colour, ICE, framing, output files
      │             (today: pakon_image.py, pakon_ice.py, pakon_render.c)
pakon-proto         frames, commands, setup, light cal, EEPROM, DX, calcache
      │             pure logic, no I/O (today: pakon_proto/cmd/setup/
      │             lightcal/eeprom/dx/calcache/hex .c)
pakon-usb           transport only: nusb, enumeration, firmware load,
                    bulk/control I/O, no packet knowledge (today: pakon_usb.c,
                    pakon_fw.c)
pakon-cli           `pakon probe`, `pakon replay`, `pakon scan`: hardware
                    debugging and parity checks against the C tools
```

Layering rules carry over from the C driver: `pakon-usb` knows nothing about
packets, `pakon-proto` knows nothing about USB. `pakon-log` behaviour
(`PAKON_DEBUG=0..4`, every packet logged) moves to `tracing`.

## Rules (unchanged from the project)

- Safety rules in `.claude/skills/pakon-scanner` and pakon-reference
  `per-unit-data-and-safety.md` apply in full. Encode them in types where we
  can: frame type `0` cannot be constructed; sequences refuse frames with no
  address or to the bootloader addresses `0x22/0x26/0x42/0x46` before
  sending anything (`Seq::check_safe`). The one exception is the OEM presence
  probe `04 03 <addr> 00 00`, which the transport layer sends on its own path
  for model detection; type-4 commands `0x0C`–`0x0F` there erase flash and
  must never be expressible. Addresses come from probe results, not
  constants; per-unit values come from the EEPROM.
- No invented commands. Port behaviour exactly, including teardown order.
  Every generated frame stream is checked against a capture or the C output.
- libpakon-main has no licence: read, never copy. Same for this port.
- Hardware steps need the human at the scanner (base F-135, serial 3054).
  F-135+ needs a run on a Plus unit (serial 16402 was the reference).

## Stages

Each stage ends with something releasable and has a parity gate.

### Stage 1 — `pakon-proto` (pure logic) — **done**

Ported, each with its C tests as `cargo test`:

1. framing (`pakon_proto`) → `frame`
2. Intel HEX (`pakon_hex`) → `pakon-usb::hex` (transport side, as in C)
3. capture scripts (`.pakscan`) → `script` (new: the C tests each parsed
   them inline)
4. EEPROM (`pakon_eeprom`) → `eeprom`
5. setup streams (`pakon_setup`) → `setup`
6. light calibration (`pakon_lightcal`) → `lightcal`; calcache
   (`pakon_calcache`) → `calcache`
7. DX (`pakon_dx`) → `dx`

`pakon_cmd` is not pure (it sends sequences over USB with the OEM reply
rules), so it moves to stage 3 with the transport.

**Gate (met):** init, configure and teardown match all seven captures byte
for byte; event follow-ups reproduce 91 of the 95 captured acks, as the C
does; scan start, LEDs, AFE, panel and advance match the captured frames;
the EEPROM fixtures of serials 2233 and 16402 decode to the same values; the
calibration cache reads a file written by the C driver and writes back the
same bytes; DX log lines match the C formatter.

### Stage 2 — `pakon-image`, `pakon-oem`, `pakon-roll` — **done**

Port the numpy pipeline to `ndarray` + `rayon`, repeating numpy's float
arithmetic (f32 where numpy stays f32, f64 where it promotes, numpy's
percentile / median / pairwise-sum rounding) so the output is bit-identical.
(The PyO3 bridge is dropped: the Rust server replaces the Python one in
stage 4, so the Python app never needs to call Rust.)

Parity tests (`pakon-image/tests/golden.rs`, `pakon-oem/tests/{dice,sba}.rs`,
`pakon-roll/tests/roll.rs`) compare against outputs the Python pipeline
writes (`rust/tools/golden*.py`) from a real roll; they run with
`PAKON_GOLDEN=DIR` (and `PAKON_GOLDEN_RAW` / `PAKON_GOLDEN_FULL` /
`PAKON_GOLDEN_ROLL` for the whole roll) and skip otherwise, since raws are
gigabytes. Order: `golden.py RAW DIR --rows 20000:32000`, then
`golden_colour.py`, `golden_ice.py`, `golden_dice.py`, `golden_adjust.py`,
`golden_fallback.py`, `golden_roll.py DIR` on DIR; `golden_sba.py FULL DIR`
and `golden_roll.py ROLLDIR RAW` for the whole roll.

The rpd.pf and romm.pf sRGB tables come from LittleCMS in Python
(`rust/tools/tables.py`) and are embedded as data, so Rust needs no
LittleCMS.

Parts:

1. decode + ribbon: row layout, marker, leads, registration, flat field —
   **done, bit-exact** (whole 1.9 GB roll: 2.0 s vs Python 13.9 s)
2. OEM colour: density LUT, NegMatrix, adjustments, unsharp, rpd.pf table,
   grey — **done, bit-exact** (38 ms per Base 16 frame)
3. Dmin, roll balance — **done, bit-exact**
4. slide: exact density, ColRevLut1 link, romm.pf, gaps, flare — **done,
   bit-exact**
5. TLX framing, all formats, both polarities — **done, exact**
6. saturation / tone tables (`adjusted_table`), slide adjustments —
   **done, bit-exact**. Slide saturation/tone works in Rust (romm.pf is
   evaluated through its matrix/TRC, within 0.003 of the LittleCMS table);
   in Python `adjusted_table` raises on romm.pf (no lut16 tag), a latent bug
   since the UI sends no saturation
7. IR plane + our ICE (`pakon_ice.py`) — **done, bit-exact** (0.7 s/frame)
8. OEM Digital ICE and SBA under Unicorn (`oem_dice.py`, `oem_sba.py`) —
   **done, bit-exact** (`pakon-oem`: own PE loader, `unicorn-engine`; DICE
   1.9 s/frame vs 2.35 s, SBA roll stage 0.3 s vs 1.1 s; x87 helpers run in
   the stub code itself, no nested emulation)
9. fallbacks without an EEPROM archive (`invert_c41`, `render_jpeg`) —
   **done**: `invert_c41` bit-exact; `render_jpeg` within tolerance (mean
   0.43 codes) because Python runs rpd.pf through LittleCMS's 8-bit
   transform and Rust through the 52³ table
10. output: 16-bit TIFF with DX metadata, JPEG, thumbnails, preview strip,
    contact sheet — **done** (own TIFF writer, `jpeg-encoder` with EXIF + XMP
    segments, `image` for the sheet; tifffile and Pillow read them back with
    the same tags; JPEG bytes differ from libjpeg's, pixels round-trip)
11. prescan / autoframe / export orchestration (`web/decode.py`) — **done,
    bit-exact** (`pakon-roll`): prescan meta, ribbon and IR caches and every
    exported TIFF match Python on the slice and on the whole 36-frame roll
    (DICE, SBA with FOS and the scene film base, adjustments, B&W). Whole
    roll: prescan 8.7 s, export 32 s; Python ~61 s for the same

**Gate:** pixel-equivalent (exact, or within a documented tolerance for
float paths) to the Python output on real rolls and on `test/fixtures`.

### Stage 3 — `pakon-usb` + `pakon-scan` + `pakon-cli` — **verified on F-135 serial 3054; F-135+, web app and Windows/Linux pending**

Ported:
- `pakon-usb::fw`: the FX2 load sequence from HEX equals all 1104 transfers
  of `resources/f135.pakfw`.
- `pakon-usb::device`: `nusb` transport (list/find, open + claim, bulk
  send/recv with IN lengths rounded to whole packets, control, firmware
  load from HEX or `.pakfw`, wait for re-enumeration); vendor request 0xA2
  refused in the transport.
- `pakon-scan::cmd` (`pakon_cmd.c`): reply rules, busy polls, retries, PIC
  probe, event service with DX blocks, lamp wait; tested against a
  simulated scanner (`tests/sim.rs`), including "an unsafe sequence sends
  nothing".

- `pakon-scan::{session, calib, scan, transport, replay}` (`pakon_replay.c`):
  open check and model detection, EEPROM archive, code-built setup, lamp
  gate, light calibration with the calcache reuse rules, `<raw>.flat.json`,
  scan start, the image loop with end-of-roll detection and DX recording,
  teardown and motor stop in drop guards (they run on errors, stops and
  panics), the F-135 eject / advance, the F-135+ timed advance, and the
  `.pakscan` replay scan with autostop. Captured scripts are checked
  against the safety rules before anything is sent (every script in
  `resources/` passes). Loop, guards and rules tested on the simulated
  scanner.
- `pakon-cli`: `pakon_replay` and `pakon_probe` binaries with the C tools'
  flags and output lines for the modes the web app uses (`--open`, `--scan`
  [`--autostop` `--drain` `--advance`], `--scan-code`, `--light-cal`,
  `--setup`, `--eject`, `--advance-code`, `--advance`; `--list`,
  `--load-firmware`, `--load-firmware-hex`). Not ported yet: `--read-params`,
  `--calib-probe`, `--film-sense`, advance scripts (`--steps`). The Python
  web app runs them unchanged with `PAKON_BUILD=rust/target/release`.

Hardware session on F-135 serial 3054 (2026-11-09), all passed: firmware
load, `pakon_replay --open`, `--setup`, `--light-cal`, and a full
`--scan-code` run with autostop, DX and teardown. The scan decodes
identically in Rust and Python.

Still to do:
- the web app on the Rust binaries (`PAKON_BUILD=rust/target/release`)
- one F-135+ run on a Plus unit (serial 16402 was the reference)
- Windows and Linux
- the CLI modes not ported yet (above)

Transport is `nusb` (no libusb). Teardown (LEDs off → ResetFifos → `92` →
20 ms → `a2`) is guaranteed on drop and on panic. Ship `pakon-cli` alongside
the C tools until it matches on real scans on every platform, then retire
the C.

**Gate:** dry-run command streams match the C tools byte for byte; real scans
on serial 3054 (**met**); one F-135+ run on a Plus unit (open).

Windows keeps `packaging/windows/pakon_driver.c` (WinUSB setup); `nusb` needs
WinUSB just as libusb does.

### Stage 4 — Rust server + webview — **in progress: macOS app built, hardware run pending**

`pakon-web` (axum + tokio): every route in `web/app.py` with the same request
and response shapes, including the server-sent event streams for firmware,
scan, eject, process, autoframe, export and save, so `index.html` runs
unchanged. Scans call `pakon-scan` in process instead of spawning
`pakon_replay`; image jobs call `pakon-image` on a blocking pool. Telemetry,
update check, orders, file browsing and reveal-in-Finder port alongside.
`pakon-app` (wry + tao) opens the page in a native window, as
`packaging/macos/pakon_app.py` does today; the server still serves the LAN
when asked, so a browser on another machine keeps working. Retire FastAPI,
uvicorn, pywebview and PyInstaller: one native binary per platform.

**Gate:** `index.html` works end to end against the Rust server (an API
contract test replays each endpoint against both servers and compares);
signed builds on macOS (arm64 + Intel), Windows, Linux.

Done so far:
- `pakon-web`: every route of `web/app.py` with the same shapes, the event
  streams included (`orders`, `eeprom`, `update`, `telemetry` port
  `web/*.py`). Scans, eject and the firmware load call `pakon-scan` /
  `pakon-usb` in process, one at a time behind a device lock, and show the
  same `pakon_replay` lines the Python app picked out; the Stop button and
  shutdown set the scan's stop flag, so the teardown always runs. The EEPROM
  is read with the OEM read requests in `pakon-scan::eeprom_read` (no Python
  `pakon_eeprom.py`) and archived in the same layout. `pakon_web` serves it
  without a window (`--host`, `--port`), as uvicorn did.
- API contract: a script runs the Python and the Rust servers side by side
  on the same requests and on a whole roll (open, process, autoframe, export
  with sharpening, save in every format): 51 of 51 checks equal, exported
  TIFFs bit-identical. `tests/api.rs` keeps the routes covered in `cargo
  test` without the Python server.
- JPEG: `jpeg-encoder` downsampled chroma by keeping one sample of each 2x2
  block; it now gets libjpeg's box-filtered chroma (`output::Rgb420`), and
  the exported JPEGs match Pillow's error against the TIFF to 0.001 code,
  sizes within 0.1 % (before: 4–8 % more error, 8 % larger).
- `pakon-app` (tao + wry + muda): the window, one app per user (a second
  launch brings it to the front), the Mac menu (Quit, Copy/Paste), links
  open in the browser, Info.plist / release.env settings, logs to
  `~/Library/Logs/Pakon`. Closing the window, Cmd-Q or a system quit ends a
  running scan and waits for the teardown before exiting.
- `packaging/macos/build-rust.sh`: Pakon.app from the one binary plus the
  files it reads; only system libraries (no libusb, Unicorn linked in),
  hardened runtime with the JIT entitlement; 36 MB app / 14 MB dmg (the
  PyInstaller app: 101 MB / 46 MB). Tested ad hoc signed on arm64: the
  live EEPROM read, process and export with the OEM scene balance under
  Unicorn, quit.

- Speed against the Python app, same requests, M4 Pro: process 3.3-6.2x
  faster, export without ICE 1.9x, a whole 33-frame roll (process, export
  with sharpening, save) 2.6x. The contact sheet decodes and shrinks its
  frames in parallel (2.08 s -> 0.29 s for 33, pixel-identical).
- Memory: the Rust app holds 2.3 GB before export where Python holds
  8.7 GB (36-frame roll). Per frame with Digital ICE, 6 at once: 0.48 GB
  full frame, 0.68 GB panorama (upper bounds), so the ICE budget is
  0.75 GB (Python: 1.5 GB). Frames at once follow the cores while each fits
  in half the RAM (`pakon-roll` `workers_for`), not a fixed 6 / 4 / 2: 36
  ICE frames in 11.1 s instead of 29.7 s with 6 at once, identical TIFFs.
- First scans from the app on serial 3054 (a 4-frame and a 36-frame roll):
  scan, process, export with ICE. A coloured line in every frame came from
  dust in the gate during the open-gate calibration (a 14-18 % shadow in
  the bright line that the scan did not have), not from the port: the
  Python decode of the same raw is identical.

Still to do: eject and firmware load from the window on serial 3054
(checked from the command line, not yet from the window); Developer ID
signing and notarization; Windows and Linux packages
(`packaging/windows`, `packaging/linux` still ship the Python app); then
retire FastAPI, uvicorn, pywebview and PyInstaller.

## Status

| Stage | Item | State |
|---|---|---|
| 1 | workspace scaffold | done |
| 1 | framing | done |
| 1 | Intel HEX | done |
| 1 | capture scripts | done |
| 1 | EEPROM | done |
| 1 | setup | done |
| 1 | light calibration, calcache | done |
| 1 | DX | done |
| 2 | image pipeline, OEM DLLs, roll orchestration | done (bit-exact but `render_jpeg`, see part 9) |
| 3 | USB, commands, scan orchestration, CLI | done on F-135 serial 3054 (2026-11-09): firmware load, open, setup, light cal, full `--scan-code` with autostop, DX and teardown; the scan decodes identically in Rust and Python. Pending: web app on the Rust binaries, an F-135+ run, Windows/Linux |
| 4 | server + webview | macOS app built, matching the Python server on every route and a whole roll, scanning on serial 3054; pending: signed release, Windows/Linux, Digital ICE ~10 % slower than Python per frame |

## Build

```sh
cd rust && cargo test
cargo run --release -p pakon-app          # the app, from a checkout
cargo run --release -p pakon-web          # the service alone, port 8000
packaging/macos/build-rust.sh 0.9.0       # Pakon.app + dmg in dist/macos-rust
```
