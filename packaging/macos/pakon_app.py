"""
Pakon.app: the web service in its own window.

Runs web.app on a free localhost port and shows it with pywebview (WKWebView;
WebKitGTK on Linux, WebView2 on Windows); closing the window quits.
PyInstaller freezes this for the Mac (pakon.spec, build.sh); the Linux and
Windows packages run it as is (packaging/linux, packaging/windows). It also
runs from a checkout, for trying the window without building:

    python packaging/macos/pakon_app.py

In the Mac app the EEPROM archives go to ~/Library/Application Support/Pakon
(PAKON_DATA, web/eeprom.py): the signed bundle is read-only. The log is
~/Library/Logs/Pakon/pakon.log. On Windows both are in %LOCALAPPDATA%, the
log in Pakon/Logs.
"""
import functools
import getpass
import os
import socket
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

FROZEN = getattr(sys, "frozen", False)
# The bundle mirrors the repo layout: web/, tools/, resources/, build/, ...
ROOT = Path(sys._MEIPASS) if FROZEN else Path(__file__).resolve().parents[2]
if not FROZEN:
    sys.path.insert(0, str(ROOT))

HOME = Path.home()
LOGS = Path(os.environ["PAKON_LOGS"]) if os.environ.get("PAKON_LOGS") else None
if FROZEN:
    # The version the new-build check (web/update.py) compares against, and
    # where crash reports / usage statistics go (web/telemetry.py; only
    # release builds carry these, see build.sh).
    import plistlib
    try:
        with open(Path(sys.executable).parents[1] / "Info.plist", "rb") as f:
            info = plistlib.load(f)
        os.environ.setdefault("PAKON_VERSION", info["CFBundleShortVersionString"])
        for key, env in (("PakonRybbitURL", "PAKON_RYBBIT_URL"),
                         ("PakonRybbitSiteID", "PAKON_RYBBIT_SITE_ID"),
                         ("PakonRybbitDomain", "PAKON_RYBBIT_DOMAIN")):
            if info.get(key):
                os.environ.setdefault(env, info[key])
    except (OSError, KeyError, ValueError):
        pass
    os.environ.setdefault("PAKON_DATA", str(HOME / "Library/Application Support/Pakon"))
    LOGS = HOME / "Library/Logs/Pakon"
elif (ROOT / "release.env").exists():
    # installed by packaging/linux or packaging/windows: the version and the
    # report server, as the Mac app's Info.plist above
    for line in (ROOT / "release.env").read_text().splitlines():
        key, sep, value = line.partition("=")
        if sep and not key.startswith("#"):
            os.environ.setdefault(key.strip(), value.strip())
if sys.platform == "win32":
    _local = Path(os.environ.get("LOCALAPPDATA") or HOME / "AppData/Local") / "Pakon"
    os.environ.setdefault("PAKON_DATA", str(_local))
    LOGS = LOGS or _local / "Logs"
if os.environ.get("PAKON_DATA"):
    Path(os.environ["PAKON_DATA"]).mkdir(parents=True, exist_ok=True)
if LOGS:
    os.environ["PAKON_LOGS"] = str(LOGS)       # telemetry's crash log
    LOGS.mkdir(parents=True, exist_ok=True)
    if FROZEN or sys.stdout is None:
        # a windowed app has no terminal; uvicorn and the tools still print
        sys.stdout = sys.stderr = open(LOGS / "pakon.log", "a", buffering=1)

# One app per user: a second one would start a second service on the same
# scanner and temp folder. It brings the running window to the front instead
# (POST /api/app/focus) and quits. The lock is the OS's on an open file, so a
# crash leaves none behind; the port file says where the running one listens.
_RUN = (Path(os.environ["PAKON_DATA"]) if os.environ.get("PAKON_DATA")
        else Path(tempfile.gettempdir()) / f"pakon-{getpass.getuser()}")


def _hold_lock():
    """The open lock file while this is the only app, else None."""
    _RUN.mkdir(parents=True, exist_ok=True)
    f = open(_RUN / "app.lock", "a+")
    f.seek(0)
    try:
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(f.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        f.close()
        return None
    return f


if __name__ == "__main__":
    _LOCK = _hold_lock()
    if _LOCK is None:
        try:
            port = int((_RUN / "app.port").read_text())
            urllib.request.urlopen(urllib.request.Request(
                f"http://127.0.0.1:{port}/api/app/focus", method="POST"), timeout=5).close()
        except (OSError, ValueError):
            pass                                  # still starting: it shows itself
        print("Pakon is already running", flush=True)
        sys.exit(0)

# PyUSB looks libusb up with ctypes.util.find_library, which fails on a Mac
# without Homebrew and on Windows: point its backend at the copy in the app.
_libusb = ROOT / ("libusb-1.0.dll" if sys.platform == "win32" else "libusb-1.0.0.dylib")
if _libusb.exists():
    import usb.backend.libusb1 as _libusb1
    _libusb1.get_backend = functools.partial(_libusb1.get_backend,
                                             find_library=lambda _name: str(_libusb))

import uvicorn  # noqa: E402
import webview  # noqa: E402

import web.app as service  # noqa: E402


def _free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main():
    port = _free_port()
    server = uvicorn.Server(uvicorn.Config(service.app, host="127.0.0.1", port=port,
                                           log_level="info"))
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    deadline = time.monotonic() + 30
    while not server.started and thread.is_alive() and time.monotonic() < deadline:
        time.sleep(0.05)

    webview.settings["ALLOW_DOWNLOADS"] = True       # the frame / zip links
    if server.started:
        window = webview.create_window("Pakon", f"http://127.0.0.1:{port}/",
                                       width=1280, height=800, min_size=(960, 640))
        (_RUN / "app.port").write_text(str(port))

        def focus():                                  # a second launch
            window.restore()
            window.show()
            window.on_top = True                      # to the front, then normal
            window.on_top = False
        service.focus_window = focus
    else:
        webview.create_window("Pakon", html=(
            "<body style='font:15px -apple-system,system-ui,sans-serif;padding:2em'>"
            "<h2>Pakon could not start its scanner service.</h2>"
            f"<p>Details are in {LOGS / 'pakon.log' if LOGS else 'the terminal'}.</p></body>"))
    webview.start()

    server.should_exit = True                         # window closed: quit
    thread.join(timeout=5)


if __name__ == "__main__":
    main()
