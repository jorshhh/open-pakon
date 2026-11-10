# Pakon.app (macOS)

A double-clickable app for people who don't use the command line. One native
binary (`rust/crates/pakon-app`) runs the web service, the scanner driver and
the image pipeline, and shows the page from `web/static` in its own window
(WKWebView); closing the window quits. It links only system libraries (no
Python, no libusb), so the Mac needs nothing installed. macOS 13 or later:
one app for Apple silicon and a universal one that also runs on Intel Macs.

| File                 | What it is |
|----------------------|------------|
| `build.sh`           | Builds the binary for macOS 13, assembles the bundle, signs, makes the `.dmg`, notarizes |
| `entitlements.plist` | Hardened-runtime exceptions: JIT for Unicorn (the OEM DLLs' emulator) |
| `make_icon.py`       | Draws the app icon |
| `pakon_app.py`       | The Python window app, still run by the Linux and Windows packages |
| `requirements.txt`   | Its pinned pywebview, read by the Linux and Windows builds |

## Build

```sh
packaging/macos/build.sh 0.2.0        # version number
```

Output goes to `dist/macos/`: `Pakon.app` + `Pakon.dmg` (Apple silicon) and
`Pakon-Universal.app` + `Pakon-Universal.dmg` (Apple silicon and Intel; the
app inside the dmg is still `Pakon.app`). The icon is cached in
`packaging/macos/.cache/` (git-ignored). For a quicker Apple-silicon-only
build: `PAKON_ARCH=arm64 packaging/macos/build.sh 0.2.0`. Check the Intel
half on an Apple silicon Mac (with Rosetta) with
`arch -x86_64 dist/macos/Pakon-Universal.app/Contents/MacOS/Pakon`.

Needs rustup with the `aarch64-apple-darwin` target (and `x86_64-apple-darwin`
for the universal app), Xcode's command-line tools, CMake (the Unicorn crate
builds Unicorn from source) and, the first time only, Python 3 with Pillow
for the icon.

`build.sh` fails if the binary needs a newer macOS than 13 or links a library
outside the system, and lists any weak imports to check.

To try the window without building the bundle:
`cd rust && cargo run --release -p pakon-app`.

## Signing and notarization

`build.sh` signs with the first **Developer ID Application** certificate in the
keychain (or `PAKON_SIGN_IDENTITY`). Without one it signs ad hoc: the app runs
on the Mac that built it, but other Macs will refuse it.

Notarizing needs a stored App Store Connect login, once per Mac:

```sh
xcrun notarytool store-credentials pakon-notary \
    --apple-id YOU@EXAMPLE.COM --team-id T928P2TAKQ
```

Then:

```sh
PAKON_NOTARY_PROFILE=pakon-notary packaging/macos/build.sh 0.2.0
```

This submits each `.dmg`, waits for Apple, staples the ticket and checks it
with `spctl`. Upload `dist/macos/Pakon.dmg` and `dist/macos/Pakon-Universal.dmg`
to a GitHub release under those exact names: the website's and the README's
Download links point at `releases/latest/download/Pakon.dmg` and
`.../Pakon-Universal.dmg`.

### Crash reports and usage statistics (Rybbit)

The app can send opt-in crash reports and usage statistics to a
[Rybbit](https://rybbit.com) instance (`rust/crates/pakon-web/src/telemetry.rs`). Where they go is
set when building, never in the code, so a build anyone makes from this repo
sends nothing and never asks. For a release, copy
`packaging/release.env.example` to `packaging/release.env` (git-ignored; the
Linux build reads it too) and fill in:

| Variable | What |
|---|---|
| `PAKON_RYBBIT_URL` | the Rybbit instance, e.g. `https://rybbit.example.com` |
| `PAKON_RYBBIT_SITE_ID` | the Rybbit site's ID |
| `PAKON_RYBBIT_DOMAIN` | the domain the Rybbit site was created with |

`build.sh` reads `release.env` (or the same variables from the environment),
prints `reports: on` or `reports: off`, and writes them into the app's
`Info.plist`; all three are needed. No API key: the app sends to Rybbit's
public tracking endpoint, and a key would be readable by anyone with the app.

Installed apps check the latest release on launch (and every 6 hours) and
show a banner when it is newer than their own version
(`rust/crates/pakon-web/src/update.rs`), so
tag releases `vX.Y.Z` with the same number passed to `build.sh`, and do not
mark them draft or pre-release unless they should stay hidden. Users can turn
the check off with `PAKON_UPDATE_CHECK=0`.

## Where the app keeps things

- EEPROM archives: `~/Library/Application Support/Pakon/eeprom/` (the app
  bundle is read-only). Keep a copy off the machine, as with `backups/eeprom/`.
- Log: `~/Library/Logs/Pakon/pakon.log`.
- Scans: one folder per order in the work folder (`~/pakon` by default, set in
  the UI), e.g. `~/pakon/000042/2026-10-11-14-30-scan.raw`. The work folder and
  the order counter are in `~/Library/Application Support/Pakon/settings.json`.
- Saved exports: in the scan's order folder (a raw opened from elsewhere gets
  a folder named after it), under `jpeg/`, `tiff/` and `raw/`.
- Temp files (the decoded roll, ~1 GB): `/tmp/pakon_web`, emptied at launch.
