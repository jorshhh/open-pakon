# pakon: technical guide

The developer side of pakon: how the Rust code is laid out, how to build,
test and run it, the command-line tools, the HTTP API, the switches, where
files go, packaging, and the implementation facts worth knowing before
changing anything. To scan, start with the [README](../README.md). What
works on which platform and model, and what is still open, is in
[ROADMAP.md](ROADMAP.md).

Everything is Rust, in the workspace under `rust/`. One native binary per
platform runs the scanner driver, the image pipeline and a web service, and
shows the page (`web/static/index.html`) in its own window. Any machine on
the local network can use the same service from a browser. The earlier
Python app and C tools are kept at the git tag `python-final`.

## Crates and layering

| Crate | What it does |
|---|---|
| `pakon-usb` | Transport only, on `nusb`: enumeration, FX2 firmware load (from Intel HEX or a captured `.pakfw`), bulk and control I/O. No packet knowledge. Refuses vendor request `0xA2`. |
| `pakon-proto` | Pure logic, no USB: frames, setup / configure / scan start / teardown streams, light-calibration steps, EEPROM decoder, DX decoder, calibration cache, capture scripts. |
| `pakon-scan` | The one crate that uses both: OEM reply rules and busy polls (`cmd`), the event service, opening and model detection (`session`), light calibration (`calib`), the code-built scan (`scan`), eject and advance (`transport`), EEPROM read (`eeprom_read`), replay of captured scripts (`replay`). Everything goes through the `Link` trait, so the rules are tested against a simulated scanner. |
| `pakon-image` | Raw stream to pictures: row layout, marker alignment and R/G/B registration (`decode`, `ribbon`), flat field (`flat`), OEM colour (`colour`), the no-EEPROM fallback (`fallback`), slide path (`slide`), film base and roll balance (`balance`), saturation / tone (`adjust`), TLX framing (`framing`), our IR dust removal (`ice`), DX film metadata (`film`), TIFF / JPEG / thumbnails / contact sheet (`output`), numpy-exact percentile and median (`stats`). |
| `pakon-oem` | The OEM DLLs run under Unicorn: Digital ICE (`oem/DMLDICELib.dll`, `dice`) and Kodak Ansel scene and roll balance (`oem/PakonIMAu.dll`, `sba`), with its own PE32 loader (`pe`) and import stubs (`emu`). |
| `pakon-roll` | A roll from raw to exported frames: prescan (ribbon cache, preview strip), auto-frame, export (crop, ICE, balance, render, write). Reads the image switches (`Env::from_env`). |
| `pakon-web` | The web service (axum + tokio): the `/api/*` routes and event streams the page uses, the order folders, EEPROM auto-archive, telemetry, update check. Binary `pakon_web`. |
| `pakon-app` | The window (tao + wry + muda): WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux, around the service on a free localhost port. Binary `pakon_app` (`Pakon` in the Mac bundle). |
| `pakon-cli` | `pakon_probe` and `pakon_replay`, for hardware debugging. |

```
pakon-app ──► pakon-web ──► pakon-roll ──► pakon-image
                  │              └───────► pakon-oem
                  └──► pakon-scan ──► pakon-proto
                            └───────► pakon-usb
pakon-cli ──► pakon-scan
```

Rules the layering enforces:

- `pakon-usb` knows nothing about packets; `pakon-proto` knows nothing about USB.
- The safety rules are checked before anything is sent: `Seq::check_safe`
  (`pakon-proto` `setup`) refuses type-0 frames and frames to the bootloader
  addresses, apart from the OEM presence probe that `pakon-scan` sends on its
  own path; every captured script is checked the same way before a replay;
  `pakon-usb` refuses vendor request `0xA2` whatever asks for it. The rules
  themselves are in [PROTOCOL.md](PROTOCOL.md).
- Teardown (LEDs off → ResetFifos → `92` → 20 ms → `a2`) runs in drop guards,
  so errors, stops and panics still stop the motor.
- In the service, scanner work (firmware, scan, eject) runs one job at a time
  behind a device lock, and image jobs one at a time on the blocking pool. A
  client that disconnects does not stop a job: a scan always runs to its
  teardown.

The page is unchanged HTML/JS (`fetch` plus server-sent events) served from
`web/static/`.

## Building and testing

### Prerequisites

Stable Rust from [rustup](https://rustup.rs), CMake and libclang: the
`unicorn-engine` crate builds Unicorn from source and links it in. No libusb.

| OS | Install |
|---|---|
| macOS | Xcode command-line tools (libclang), `brew install cmake` |
| Debian / Ubuntu | `sudo apt install build-essential cmake pkg-config libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev` |
| Windows | The package is cross-built from macOS or Linux with MinGW (`x86_64-pc-windows-gnu`), see `packaging/windows/README.md`. Building on Windows itself is not documented. |

### Build and test

```sh
cd rust
cargo test                        # every hardware-free test
cargo build --release             # all binaries into rust/target/release
```

The test profile builds at `opt-level = 2`, since the capture and image tests
crunch real data. What `cargo test` covers:

| Tests | What they check |
|---|---|
| `pakon-proto/tests/` (`setup`, `lightcal`, `eeprom`, `dx`, `calcache`) | Generated command streams against the captures (fixtures in `tests/fixtures`), the EEPROM decoder, DX decoding, the calibration cache format |
| `pakon-usb` unit tests | Intel HEX parsing; the firmware load built from HEX equals all 1104 transfers of `resources/f135.pakfw`; `0xA2` refused |
| `pakon-scan/tests/sim.rs` | Reply rules, busy polls, event service, scan loop, end of roll, teardown guards and "an unsafe sequence sends nothing", on a simulated scanner |
| `pakon-oem/tests/pe.rs` | The PE loader on the OEM DLLs |
| `pakon-web/tests/api.rs` | The HTTP routes |
| unit tests in each crate | EEPROM fixtures in `test/fixtures/eeprom`, DX in `test/fixtures/dx`, and the rest |

### Parity tests against the Python pipeline

`pakon-image/tests/golden.rs`, `pakon-oem/tests/{dice,sba}.rs` and
`pakon-roll/tests/roll.rs` compare against outputs the Python pipeline wrote
from a real roll. Raws are gigabytes, so the data is not in the repo and the
tests skip themselves unless pointed at it:

| Variable | Data |
|---|---|
| `PAKON_GOLDEN=DIR` | Outputs of the golden scripts for a slice of a roll |
| `PAKON_GOLDEN_RAW=FILE` | The raw itself |
| `PAKON_GOLDEN_FULL=DIR` | Outputs for the whole roll |
| `PAKON_GOLDEN_ROLL=DIR` | Whole-roll prescan / export outputs |

The scripts that write this data (`rust/tools/golden*.py`, and `tables.py`
for the embedded colour tables) are at tag `python-final`. Order: `golden.py
RAW DIR --rows 20000:32000`, then `golden_colour.py`, `golden_ice.py`,
`golden_dice.py`, `golden_adjust.py`, `golden_fallback.py`, `golden_roll.py
DIR` on DIR; `golden_sba.py FULL DIR` and `golden_roll.py ROLLDIR RAW` for the
whole roll.

## Running from a checkout

```sh
cd rust
cargo run --release -p pakon-app   # the app in its own window
cargo run --release -p pakon-web   # the service alone, port 8000 on all interfaces
cargo run --release -p pakon-web -- --host 127.0.0.1 --port 8000   # this machine only
```

Both find the page, `oem/`, `profiles/`, `firmware/` and `resources/` by
walking up from the executable or the working directory to the first folder
holding `web/static/index.html` (in a Mac bundle, `Contents/Resources`);
`PAKON_ROOT` names it instead. Open `http://<host>:8000` for the service
alone. Ctrl-C ends a running scan as at the end of the roll and waits for
the teardown before exiting. A build from a checkout never sends crash
reports or usage statistics and never checks for updates (no
`PAKON_VERSION`).

USB access: macOS needs nothing. On Linux, install
`packaging/linux/60-pakon.rules` into `/etc/udev/rules.d/` (the `.deb` does)
or run as root. On Windows the scanner needs WinUSB on both USB IDs
(`0F05:F235` and `0F05:F135`), which the installer's `pakon_driver.exe` sets
up (`packaging/windows/README.md`).

## Command-line tools

```sh
cd rust && cargo build --release -p pakon-cli
# rust/target/release/pakon_probe and rust/target/release/pakon_replay
```

They do one step at a time what the app does from its Scan page. Both take
`--timeout MS` (USB per-transfer timeout, default 1000) and honour
`PAKON_DEBUG=0..4`.

### `pakon_probe`

| Command | What it does |
|---|---|
| `--list` | Lists Pakon devices: `0f05:f235` is cold (needs firmware), `0f05:f135` is warm |
| `--load-firmware-hex STAGE1 MAIN` | Loads the FX2 firmware from the OEM Intel HEX files (`firmware/PknLdr.hex firmware/Pakon7.hex`, see `firmware/README.md`); refuses unless the scanner reports the F-135 / F-135+ personality |
| `--load-firmware PAKFW` | Replays a captured firmware load (`resources/f135.pakfw`; same FX2 image for both models) |

### `pakon_replay`

Code-built modes (F-135; each needs `--eeprom-dir DIR`, the unit's EEPROM
archive):

| Mode | What it does |
|---|---|
| `--scan-code [--base 4\|8\|16] [--ir on\|off] [--film c41\|bw\|bwc41\|slide] [--image OUT] [--recalibrate] [--max-mb N]` | Setup, lamp wait, light calibration (open gate: keep the film out), scan start, then prints `motor start (a0) sent -- feed the film now`. Stops on its own at end of roll; teardown stops the motor. Defaults: Base 16, IR on (off for `bw`), `c41`, `pakon_scan.raw`. `--recalibrate` forces a full calibration instead of the cached one. `--max-mb` (default 4096) is only a runaway guard: a 36-exposure roll at Base 16 with IR is up to about 2 GB. |
| `--light-cal [--base N] [--ir on\|off] [--flat-out FILE]` | Calibration only, open gate; writes the per-column tables to `DIR/flatfield.json` or `FILE` |
| `--setup [--base N] [--teardown]` | Setup only, optionally followed by the teardown |
| `--eject [--eject-seconds N]` | Runs the transport until the DX film sensors say the strip is out, or stalled at the exit (pull it out by hand); default limit 60 s |
| `--advance-code SECONDS` | Runs the transport for a fixed time (the app's Advance film) |

Replay and other modes:

| Mode | What it does |
|---|---|
| `--open` | Open handshake; prints the detected model |
| `--scan FILE [--image OUT] [--autostop] [--drain] [--advance]` | Replays a captured `.pakscan` (insert the film first). `--autostop` arms at motor start and stops at end-of-roll white, then replays the capture's teardown tail; `--drain` keeps reading image data after the script ends until the scanner is done; `--advance` pushes the strip out afterwards (F-135+). |
| `--advance [--advance-seconds N]` | F-135+ only: timed transport run at that unit's captured motor speed (above the base F-135's limit; on an F-135 use `--advance-code`). Default 15 s; a strip already at the exit needs 1–2 s. |

Ctrl-C during a scan stops it and runs the teardown before exiting; a second
Ctrl-C exits at once. With `PAKON_STOP_STDIN` set, a line starting with
`stop` on stdin does the same as the first Ctrl-C.

A code-built scan writes beside the raw: `OUT.flat.json` (the calibration's
per-column tables and row layout) and, when the scanner read the film's DX
code, `OUT.dx.json`. Open the raw in the app to frame and export it; there is
no command-line decoder.

F-135+ (replays the scripts from a reference unit):

```sh
R=rust/target/release
$R/pakon_probe --load-firmware resources/f135.pakfw
$R/pakon_replay --open                                    # "model detected: F-135+"
$R/pakon_replay --scan resources/f135plus/base16.pakscan --image scan.raw --advance
```

`resources/f135plus/` also has Base 8, Base 4 and `base4_ir.pakscan`. The
F-135 `.pakscan` files NAK on a Plus: its controllers answer at different
bus addresses ([SCANNER_FAMILY.md](SCANNER_FAMILY.md)).

### Not ported from the C tools

`--read-params`, `--params-out`, `--calib-probe`, `--film-sense`,
`--trace-status`, and the advance scripts (`advance.pakscan
--steps N`, `--limit`). The Rust `pakon_replay` names them and exits; the C
tools at tag `python-final` still have them. The EEPROM backup is no longer a
separate tool: the app and `pakon_web` read and archive it themselves
([EEPROM.md](EEPROM.md)).

## HTTP API

`pakon-web` serves the page and these routes (`rust/crates/pakon-web/src/lib.rs`).
Requests and responses are JSON; "stream" routes answer with server-sent
events (`data: <json>` chunks with a `type`: `progress`, `log`, `order`, `feed`, `done`, `error`). A
missing required field answers 422.

| Route | Method | What it does |
|---|---|---|
| `/api/status` | GET | Scanner state, the current raw and its size, frame count, processing state, work folder and next order, the EEPROM archive in use, whether the HEX firmware is present, a newer release |
| `/api/firmware` | POST, stream | Loads the firmware (HEX files, else `f135.pakfw`) |
| `/api/scan` | POST, stream | Scans into a new order folder; `film` (`c41`, `bw`, `bwc41`, `slide`), `base` (4, 8, 16) |
| `/api/scan/stop` | POST | Ends a running scan as at the end of the roll (teardown still runs) |
| `/api/eject` | POST, stream | Ejects the strip (F-135, code-built) |
| `/api/workdir` | POST | Sets the work folder |
| `/api/browse` | GET | Lists a folder on the host (the page's file browser) |
| `/api/mkdir` | POST | Makes a folder on the host |
| `/api/open` | POST | Makes a `.raw` on the host the current roll (in place, no copy) |
| `/api/process` | POST, stream | Prescan: builds and caches the ribbon, renders the preview strip |
| `/api/autoframe` | POST, stream | Places a crop on every picture with the OEM framing (`full`, `half`, `pano`) |
| `/api/preview` | GET | The preview strip (JPEG) |
| `/api/export_frames` | POST, stream | Exports the confirmed crops at full resolution |
| `/api/frames` | GET | The exported frames |
| `/api/frames/{n}/thumb` | GET | One frame's thumbnail |
| `/api/save` | POST, stream | Saves a format (raw negative TIFF, TIFF, JPEG, contact sheet) into the order folder |
| `/api/reveal` | POST | Shows a file in the system file manager |
| `/api/app/focus` | POST | Brings the window to the front (a second launch calls it) |
| `/api/update/open` | POST | Opens the newer release's page |
| `/api/telemetry/consent`, `/page`, `/error` | POST | The opt-in switches, page views, page errors |

Scans, eject and the firmware load call `pakon-scan` / `pakon-usb` in
process and stream the same progress lines `pakon_replay` prints. Any other
path is a file from `web/static/`.

## Environment variables

Read at run time by the app and `pakon_web` unless noted. An empty value
counts as unset.

| Variable | Effect |
|---|---|
| `PAKON_DEBUG=0..4` | Log verbosity (all binaries). 2 adds `dx:` lines; 4 hexdumps every packet and reply. |
| `PAKON_ROOT=DIR` | The folder holding `web/static`, `oem/`, `profiles/` |
| `PAKON_RESOURCES=DIR` / `PAKON_FIRMWARE=DIR` | Captured scripts / HEX firmware (default `<root>/resources`, `<root>/firmware`) |
| `PAKON_DATA=DIR` | Settings (`settings.json`) and EEPROM archives (`eeprom/`); also where the window app keeps its single-instance lock |
| `PAKON_LOGS=DIR` | The log folder (set by the app and the Linux launcher) |
| `PAKON_EEPROM_DIR=DIR` | Use this EEPROM archive and skip the automatic read |
| `PAKON_PAKSCAN=FILE` | Scan with this capture script instead of detecting the model |
| `PAKON_FLAT=0` | No flat field |
| `PAKON_LINEWIDTH=N` (+ `PAKON_IR_LANE=0/1`) | Force the row layout (samples per row, IR lane) instead of detecting it per raw |
| `PAKON_OEM_SBA=0` | Skip the OEM scene balance |
| `PAKON_SBA_DMIN=0` | Don't give the scene balance the strip's measured film base |
| `PAKON_OEM_DICE=0` | Use our ICE port instead of the OEM's Digital ICE |
| `PAKON_DICE_DLL=FILE` | Another `DMLDICELib.dll` |
| `PAKON_EXPORT_THREADS=N` | Frames exported at once (default: by cores and memory, see below) |
| `PAKON_TELEMETRY=0/1` | 0 turns crash reports and usage statistics off everywhere; 1 lets a checkout send them (still opt-in) |
| `PAKON_UPDATE_CHECK=0` | No check for a newer release |
| `PAKON_VERSION` | The app's version; set from `Info.plist` (Mac) or `release.env` (Windows, Linux) at startup. Enables the update check and telemetry. |
| `PAKON_RYBBIT_URL` / `_SITE_ID` / `_DOMAIN` | Where reports go; set the same way, only in release builds |
| `PAKON_STOP_STDIN` | `pakon_replay`: stop on a `stop` line on stdin |
| `PAKON_GOLDEN*` | Parity test data (above) |

Build-time variables (`PAKON_ARCH`, `PAKON_SIGN_IDENTITY`,
`PAKON_NOTARY_PROFILE`, `PAKON_DEB_ARCH`) are in the packaging READMEs.

## Where files go

| | macOS app | Windows | Linux (`.deb`) | From a checkout |
|---|---|---|---|---|
| Settings, EEPROM archives | `~/Library/Application Support/Pakon` | `%LOCALAPPDATA%\Pakon` | `~/.local/share/pakon` | `backups/` in the repo (git-ignored); the window app on Windows always uses `%LOCALAPPDATA%\Pakon` |
| Log (`pakon.log`) | `~/Library/Logs/Pakon` | `%LOCALAPPDATA%\Pakon\Logs` | `~/.local/state/pakon` | the terminal (Windows window app: as installed) |
| Scans (work folder, one folder per order) | `~/pakon` | `%USERPROFILE%\pakon` | `~/pakon` | same; set in the app |
| Temp (ribbon caches, exports, about 1 GB a roll; cleared at launch) | `/tmp/pakon_web` | `%TEMP%\pakon_web` | `/tmp/pakon_web` | same |

`PAKON_DATA` and `PAKON_LOGS` override the first two rows. The Linux launcher
(`packaging/linux/pakon`) honours `XDG_DATA_HOME` and `XDG_STATE_HOME`.

Inside the data folder:

- `eeprom/<model>-<serial>-<date>/`: the unit's EEPROM archive, all four
  copies with CRC results and SHA-256 hashes (`report.json`). The service
  reads it once per USB connection (at launch, after a firmware load, or
  before a scan or eject) with the OEM's read-only requests, archives a unit
  the first time it sees it, and afterwards picks the archive matching the
  connected serial. This data exists nowhere else; keep a copy off the
  machine ([EEPROM.md](EEPROM.md)).
- In each archive, `calibration-<film>[-base4|-base8][-noir].bin`: the saved
  light calibration. A scan reuses it within an hour, refreshes it after
  that, and runs a full calibration after 20 hours (`calcache::needed`);
  `flatfield.json` from `--light-cal` is used for raws without their own.

Beside each raw: `.flat.json` (calibration tables, row layout, scan duties),
`.dx.json` (film product code), `.film` (film type), `.format` (framing
format), `.crops.json` (confirmed crops, written at export).

## Packaging

Each package is the one `pakon-app` binary plus the files it reads. No
Python, no libusb.

| Platform | Command | Output | Details |
|---|---|---|---|
| macOS | `packaging/macos/build.sh VERSION` | `dist/macos/`: `Pakon.app` + dmg (Apple silicon) and a universal app + dmg | `packaging/macos/README.md` |
| Windows | `packaging/windows/build.sh VERSION` | `dist/windows/Pakon-Setup.exe`, cross-built on macOS or Linux (in Docker when the tools are missing) | `packaging/windows/README.md` |
| Linux | `packaging/linux/build-deb.sh VERSION` | `dist/linux/pakon_<arch>.deb`, natively on Debian/Ubuntu or in Docker; `PAKON_DEB_ARCH=amd64\|arm64` | header of `packaging/linux/build-deb.sh` |

Release builds read `packaging/release.env` (from
`packaging/release.env.example`) for the version and the report server.

## Implementation notes

### Parity with the Python pipeline

The image pipeline repeats numpy's float arithmetic (f32 where numpy stays
f32, f64 where it promotes, numpy's percentile, median and pairwise-sum
rounding), so its output is bit-identical to the Python pipeline it
replaced.

| Part | Parity |
|---|---|
| Decode and ribbon: row layout, marker, leads, registration, flat field | bit-exact |
| OEM colour: density LUT, NegMatrix, adjustments, unsharp mask, rpd.pf table, grey | bit-exact |
| Film base (Dmin), roll balance | bit-exact |
| Slide: density, ColRevLut1 link, romm.pf, gaps, flare | bit-exact |
| TLX framing, all formats, both polarities | exact |
| Saturation / tone tables, slide adjustments | bit-exact |
| IR plane and our ICE | bit-exact |
| OEM Digital ICE and scene balance under Unicorn | bit-exact |
| Prescan / auto-frame / export of a whole 36-frame roll (ICE, SBA with FOS and the film base, adjustments, B&W) | every exported TIFF bit-identical |
| HTTP API: Python and Rust servers on the same requests and a whole roll | 51 of 51 checks equal, TIFFs bit-identical |
| A real scan decoded by both | identical |

Known differences:

- **No-EEPROM fallback JPEG** (`fallback`): `invert_c41` is bit-exact, but
  the rendered JPEG is within a mean of 0.43 codes, because Python ran rpd.pf
  through LittleCMS's 8-bit transform and Rust uses a 52³ table.
- **Colour tables**: the rpd.pf and romm.pf sRGB tables were generated once
  with LittleCMS and are embedded (`pakon-image/assets/*.srgb52.u8`); there
  is no LittleCMS at run time.
- **Slide saturation / tone** works in Rust (romm.pf evaluated through its
  matrix and TRC, within 0.003 of the LittleCMS table); the Python version
  raised on romm.pf (no lut16 tag). The page sends no saturation, so it did
  not show.
- **JPEG bytes** differ from libjpeg's (see below); pixels round-trip.

Protocol side: init, configure and teardown match all seven captures byte for
byte; event follow-ups reproduce 91 of the 95 captured acks (the rest depend
on reply data); scan start, LEDs, AFE, panel and advance match the captured
frames; the EEPROM fixtures of two units decode to the same values; the
calibration cache reads and writes the same bytes as the C driver's.

### Performance

Pipeline parts, against the Python pipeline on the same data:

| Step | Rust | Python |
|---|---|---|
| Decode + ribbon, whole 1.9 GB roll | 2.0 s | 13.9 s |
| OEM colour, one Base 16 frame | 38 ms | |
| Our ICE, one frame | 0.7 s | |
| OEM Digital ICE, one frame (parity run) | 1.9 s | 2.35 s |
| OEM roll balance stage | 0.3 s | 1.1 s |
| Whole 36-frame roll: prescan + export | 8.7 s + 32 s | ~61 s |

The app against the Python app, same requests, Apple M4 Pro: process 3.3–6.2×
faster, export without ICE 1.9×, a whole 33-frame roll (process, export with
sharpening, save) 2.6×. The contact sheet decodes and shrinks its frames in
parallel: 0.29 s for 33. In the app, Digital ICE per frame measures about
10 % slower than the Python app's (see [ROADMAP.md](ROADMAP.md)).

The x87 helpers the OEM DLLs call run inside the stub code itself, with no
nested emulation. Digital ICE is one emulated thread, so one frame runs on
one core.

### Memory and the export worker rule

Before export the app holds 2.3 GB for a 36-frame roll (the Python app held
8.7 GB). Per frame being exported, measured on a 36-frame Base 16 roll with
sharpening, 6 at once: 0.35 GB without ICE; with Digital ICE 0.48 GB for a
full frame and 0.68 GB for a panorama crop (the largest). These are upper
bounds, as they include the cached ribbon being paged in.

`pakon-roll` budgets 0.5 GB per frame without ICE and 0.75 GB with it.
Frames exported at once (`workers_for`) = as many as there are cores, while
each frame's budget fits in half the physical memory, and at least 2
(`PAKON_EXPORT_THREADS` overrides). On a 14-core M4 Pro, 36 ICE frames take
16.1 s at 6 at once, 10.6 s at 10, 8.5 s at 12, 8.6 s at 14.

### Output files

- **TIFF**: our own 16-bit writer (raw negative and positive), with the
  film's name and ISO when the scan read the DX code. tifffile and Pillow
  read the tags back.
- **JPEG**: `jpeg-encoder`, 4:2:0 like Pillow, EXIF and XMP segments with
  the film name (description, caption, keyword) and ISO. The encoder's own
  downsampling keeps the top-left chroma sample of each 2×2 block, which
  aliases film grain into colour noise. `output::Rgb420` instead hands it
  the chroma averaged over each block as libjpeg's `h2v2_downsample` does (a
  box filter with alternating bias 1, 2, edges replicated, libjpeg's
  fixed-point YCbCr). The exported JPEGs then match Pillow's error against
  the TIFF to 0.001 code, with sizes within 0.1 %. The bytes differ from
  libjpeg's.
- Contact sheet and thumbnails use the `image` crate.

`cargo run -p pakon-image --example write_samples DIR` writes sample files
for checking with other readers.

### The OEM DLLs and the hardened runtime

`pakon-oem` maps each 32-bit x86 DLL at its preferred base with its own PE
loader, binds the imports to stubs and calls functions under Unicorn. Unicorn
JIT-compiles the x86 code, so the macOS app needs two hardened-runtime
entitlements (`packaging/macos/entitlements.plist`):
`com.apple.security.cs.allow-jit` and
`com.apple.security.cs.allow-unsigned-executable-memory`. Unicorn is built
from source and linked statically; the app links only system libraries, and
`build.sh` fails if it links anything else or needs a macOS newer than 13.
The DLLs stay as shipped (`oem/README.md`). Unicorn's licence is an open
issue: see [ROADMAP.md](ROADMAP.md).

### The window app

One app per user: an OS lock on `app.lock` (in `PAKON_DATA`, else a
per-user temp folder), so a crash leaves none behind; a second launch reads
`app.port` and asks the running one to come to the front. Closing the window,
Cmd-Q or a system quit ends a running scan as at the end of the roll and
waits for the teardown. Links open in the browser. The installed app writes
stdout and stderr, the scanner's lines included, to `pakon.log`.

### Firmware

On power-on the scanner is a bare FX2 bootloader (`0F05:F235`); after the
firmware load it re-enumerates as `0F05:F135`. The firmware is built from the
OEM Intel HEX files in `firmware/` (`PknLdr.hex`, the stage-1 loader taken
from the OEM `F235Ldr.sys`, and `Pakon7.hex`); the captured `f135.pakfw`
replay is the fallback. Provenance and the legal note: `firmware/README.md`.

## More documentation

| File | What's in it |
|---|---|
| [ROADMAP.md](ROADMAP.md) | What works, open work, open questions, known issues |
| [PROTOCOL.md](PROTOCOL.md) | Frames, replies, handshake, event service, DX, scan sequence, safety rules |
| [REGISTERS.md](REGISTERS.md) | Every register and command written, with encodings |
| [EEPROM.md](EEPROM.md) | The per-unit EEPROM: reading, archiving, layout |
| [IMAGING.md](IMAGING.md) | Image layout, decoder, colour pipeline, ICE, framing |
| [SCANNER_FAMILY.md](SCANNER_FAMILY.md) | F-135 / F-135+ / F-235 / F-335 differences |
| `packaging/macos/README.md`, `packaging/windows/README.md` | Building and signing the Mac app; the Windows installer and its USB driver |

Credits, licence and privacy are in the [README](../README.md).
