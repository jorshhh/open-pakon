---
name: pakon-scanner
description: >
  Working knowledge for the pakon project — a cross-platform (macOS, Windows,
  Linux) driver, decoder and app for the Kodak/Pakon F-135 / F-135+ film
  scanner, written in Rust, now working toward a replay-free client. Use when implementing, debugging,
  building, or testing any part of this repo: goal and roadmap, layering rules,
  build & test, the protocol, safety rules, and the related projects.
---

# Pakon F-135 scanner — project guide

## Start here

- `docs/ROADMAP.md` — goal, what works on which model and platform, open
  work, open questions, known issues. Read first.
- `docs/TECHNICAL.md` — the Rust crates, building, testing, the CLI tools,
  the HTTP API, settings, packaging, implementation facts.
- `docs/PROTOCOL.md` — USB, firmware load, frames, replies, handshake,
  calibration, event service, scan sequence, transport, safety rules.
- `docs/REGISTERS.md` — every register/command we write, with encodings.
- `docs/EEPROM.md` — the EEPROM layout, decoding and the archive.
- `docs/IMAGING.md` — raw stream, decode, flat field, framing, OEM colour,
  Digital ICE, output.
- `docs/SCANNER_FAMILY.md` — model differences, the F-135+, supporting
  another model.

## Goal

Replace capture replay (`.pakfw`, `.pakscan`) with commands built in code from
the EEPROM, our own calibration, and the chosen resolution. Keep the captures
as test fixtures: generated command streams must match them byte for byte.

## Related projects (in `~/Desktop/Code/pakon/`)

- **pakon-reference** — public F-X35 reference (CC BY 4.0). Use its facts.
- **libpakon-main** — independent C++ driver. **No licence: read, never copy.**
- **pakon-scanning-software** — the OEM Windows install. `TLB.dll` = F-135/F-135+
  engine; `TLA.dll` = F-235 (do not use it for 135-line facts); `TLC.dll` = F-335.
  Decompiling is for interoperability. The OEM files the colour pipeline needs
  are committed under `oem/`; the decompiler workspace stays in `re/`.

## Golden rules

- **Safety first** (pakon-reference `per-unit-data-and-safety.md`): never send
  type byte `0`; never address the bootloaders `0x22`/`0x26`/`0x42`/`0x46`; never
  write unknown bus addresses; never issue vendor request `0xA2`; replay the OEM
  TEC values, never sweep them; respect the LED current ceilings.
- **Layer separation:** transport (`rust/crates/pakon-usb`: `nusb`,
  enumeration, firmware, bulk/control I/O, no packet knowledge) vs protocol
  (`rust/crates/pakon-proto`: frames, sequences, pure logic, no USB).
  `pakon-scan` is the one crate that uses both.
- **No invented commands.** Every new frame must come from a capture, the OEM
  engine, or the reference, and should be checked against a capture in a unit
  test. A synthesized teardown built on assumptions once left the unit dark and
  jammed film.
- **Addresses come from the probes**, never constants: base `0x20`/`0x24`, Plus
  `0x40`/`0x44`.
- **Per-unit values come from the EEPROM**, never from our unit (serial 3054).
- Every transfer can be logged: `PAKON_DEBUG=0..4` (4 hexdumps).
- Hardware steps need the human: stop and give exact commands to run.

## Key protocol facts

- Frame `[type][count][data]`, length `2 + count`, no checksum; `data[0]` =
  address. Types: 1 READ, 2 WRITE, 3 STATUS, 4 COMMAND, 7 reply.
- Endpoints: `0x01` OUT command, `0x81` IN reply, `0x86` IN image.
- After each command/write, poll `03 01 <addr>` until bit 0 clears.
- Event service: poll host every 1 ms while scanning; on flag `0x80`, read reg 2
  of each controller and ack `02 05 <a> 02 06 00 <status>`.
- LEDs: LOW `0x80` enable, `0x81` currents `[B,IR,R,0,G]`, `0x82` duties.
- Motor: SCN `0xA5` speed, `0xA0` forward, `0xA1` reverse, `0xA2` stop.
- Teardown: LEDs off → ResetFifos (`02 04 10 01 84 02`, `04 03 LOW 00 8a`) →
  `92` → 20 ms → `a2`.
- End of film is detected from pixels on the host, not signalled by the scanner.

## Build & test

Deps: rustup (stable), CMake and libclang (Unicorn is built from source);
on Linux also `libgtk-3-dev libwebkit2gtk-4.1-dev`. No libusb, no Python.

```sh
cd rust && cargo test                       # hardware-free tests
cargo run --release -p pakon-app            # the app in its window
cargo run --release -p pakon-web            # the service alone, http://127.0.0.1:8000 (--host, --port)
cargo build --release -p pakon-cli          # rust/target/release/pakon_probe, pakon_replay
```

`pakon_probe` / `pakon_replay` take the C tools' flags. Not ported (C tools at
git tag `python-final`): `--read-params`, `--params-out`, `--calib-probe`,
`--film-sense`, `--trace-status`, advance scripts (`--steps`, `--limit`).
Packages: `packaging/macos/build.sh`, `packaging/windows/build.sh`,
`packaging/linux/build-deb.sh` (one native binary each). The Windows and
Linux builds have not been run on a scanner yet.

## Hardware and workflow

- Test unit here: base F-135, serial 3054 (EEPROM type 1350), connected to the
  Mac directly; macOS needs no sudo (Linux needs root or the udev rule
  `packaging/linux/60-pakon.rules`).
- EEPROM archive: the app reads it when a warm scanner connects and writes
  `<data folder>/eeprom/F135-<serial>-<date>/` (`backups/eeprom/` from a
  checkout); there is no standalone backup tool.
- F-135+ support was verified by Ali Bosworth on serial 16402 (not on this
  machine). Plus scripts are in `resources/f135plus/`; they carry that unit's
  per-unit values (motor speeds), so treat them as one unit's recording.
- Plus decode: rows have no IR block when IR is off; stride follows the
  resolution (6000/4500/3000 samples at Base 16/8/4). The decoder detects
  the layout per raw; `PAKON_LINEWIDTH=N PAKON_IR_LANE=0` forces it.
- Cold `0f05:f235` needs firmware; warm `0f05:f135` is operational. Firmware is
  RAM-only; a power cycle returns to cold.
- A truncated scan without teardown leaves the device dirty; power-cycle.
- Film stuck in the transport: on an F-135 `pakon_replay --advance-code 5
  --eeprom-dir DIR` or `--eject --eeprom-dir DIR` (or the app's Advance
  film); on an F-135+ `pakon_replay --advance --advance-seconds 3`.
- The capture tools (`tools/fx35_datalogger.py` for OEM driver-level
  captures, `tools/analyze_capture.py` for USB captures, both to `.pakscan`)
  were removed with the Python code; they are at git tag `python-final`.
- Captures and scripts live in `resources/`; large scan outputs (`*.raw`) live
  outside the repo (e.g. `/Volumes/Video/`).
- Commit only when asked; end commit messages with the Co-Authored-By trailer.
