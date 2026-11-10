# Pakon for Windows

`Pakon-Setup.exe` installs Pakon for the current user on 64-bit Windows 10 or
11: no administrator rights, nothing else to install first. It is the same
app as the Mac one: one native binary (`rust/crates/pakon-app`) that runs the
web service, the scanner driver and the image pipeline, and shows the page
in its own window with Microsoft's WebView2 (part of Windows 11 and of
current Windows 10).

| File            | What it is |
|-----------------|------------|
| `build.sh`      | Builds the installer on Linux or a Mac (in Docker when MinGW, NSIS or Rust's Windows target is missing) |
| `installer.nsi` | The NSIS installer: files, shortcuts, the entry in Settings > Apps |
| `pakon_driver.c` | `pakon_driver.exe`: installs the scanner's USB driver (see below) |
| `pakon-winusb.inf` | That driver: Windows' WinUSB for the scanner's two USB IDs |
| `Dockerfile`    | Rust, MinGW, NSIS and the rest, for `build.sh` |

## Build

On Linux or a Mac (no Windows needed):

```sh
packaging/windows/build.sh 0.7.0        # version number
```

Output: `dist/windows/Pakon-Setup.exe`. With rustup's `x86_64-pc-windows-gnu`
target, MinGW and NSIS installed (`sudo apt install gcc-mingw-w64-x86-64
g++-mingw-w64-x86-64 cmake libclang-dev nsis python3-venv`) it builds
directly; otherwise in Docker, from the `Dockerfile` here.

- `Pakon.exe` is cross-compiled with MinGW and linked statically; it needs
  only Windows' own DLLs and `WebView2Loader.dll` (Microsoft's, shipped
  beside it; `build.sh` checks the imports).
- Python with Pillow draws the icon at build time only.
- Crash reports and usage statistics: `packaging/release.env`, as for the Mac
  build (see `packaging/macos/README.md`).
- Installing over the earlier Python app removes its files (the embedded
  Python, the C tools, `web\`, `tools\`).

The installer is not signed, so Windows SmartScreen warns about it the first
time ("More info", then "Run anyway").

## The USB driver

Pakon talks to the scanner through Microsoft's generic **WinUSB** driver; Windows has none for the scanner but Kodak's.
The installer sets it up once: `driver\pakon_driver.exe` (`pakon_driver.c`)
asks for administrator rights and installs `pakon-winusb.inf`, which puts
WinUSB on both of the scanner's USB IDs, `0F05:F235` (before the firmware
load) and `0F05:F135` (after it), connected or not. It replaces Kodak's
driver if that was installed.

Windows installs only signed driver packages, so `pakon_driver.exe` signs it
on the PC, as [Zadig](https://zadig.akeo.ie) does: a self-signed certificate
("Pakon USB driver (this computer only)") goes into the machine's Root and
TrustedPublisher stores, signs the package's catalog, and has its private key
deleted, so it cannot sign anything else. Nothing of ours runs in the kernel:
the driver is Windows' own `winusb.sys`. Updates skip this step; uninstalling
Pakon offers to remove the driver and the certificate.

If it fails (it says why), or to set it up by hand: run
`%LOCALAPPDATA%\Programs\Pakon\driver\pakon_driver.exe install`, or use
Zadig. In Zadig, **Options > List All Devices**, pick `0F05 F235`, choose
**WinUSB** and **Install Driver**; after loading the firmware in Pakon, do the
same for `0F05 F135`.

To go back to Kodak's software, uninstall Pakon with the driver (or run
`pakon_driver.exe uninstall`), then reinstall Kodak's.

### In a virtual machine

Pass both USB IDs through to the VM: `0f05:f235` (before the firmware load)
and `0f05:f135` (after it). In VirtualBox, add a USB filter for each (vendor
`0f05`, products `f235` and `f135`) and use a USB 2.0 or 3.0 controller. The
scanner disconnects and reconnects when the firmware is loaded, so a VM that
passes through only the one device it saw first loses it then.
In virt-manager, redirect it again after the firmware load (*Virtual Machine >
Redirect USB device*), or attach the new ID with *Add Hardware > USB Host
Device*.

## Where the app keeps things

- The app: `%LOCALAPPDATA%\Programs\Pakon`. Uninstall it from
  Settings > Apps; that removes only this folder.
- Settings and EEPROM archives: `%LOCALAPPDATA%\Pakon` (`settings.json`,
  `eeprom\`). Keep a copy of the EEPROM archives off the machine.
- Log: `%LOCALAPPDATA%\Pakon\Logs\pakon.log`.
- Scans: one folder per order in the work folder (`%USERPROFILE%\pakon` by
  default, set in the app), as on the Mac.
- Temp files (the decoded roll, ~1 GB): `%TEMP%\pakon_web`, emptied at launch.
