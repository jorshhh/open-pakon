"""
Crash reports and usage statistics, sent to a Rybbit instance
(https://rybbit.com, open-source analytics).

Where they go is set by the build, not the code: PAKON_RYBBIT_URL (the
instance), PAKON_RYBBIT_SITE_ID and PAKON_RYBBIT_DOMAIN (the Rybbit site and
its domain). Release builds bake them into the Mac app's Info.plist (build.sh,
packaging/release.env) or the Linux and Windows installs' release.env, and
pakon_app.py (or the Linux launcher) puts them back in the environment; a
build or checkout without them never sends anything and does not ask.

Opt-in: nothing is sent until the user says yes in the first-launch popup,
and each kind has its own switch (settings.json "telemetry", the Settings
page). Only the app sends (PAKON_VERSION is set from its Info.plist); a
checkout does not, unless PAKON_TELEMETRY=1. PAKON_TELEMETRY=0 turns it off
everywhere.

Events go straight to Rybbit's /api/track over HTTPS, one background thread,
dropped on any failure. Never sent: file paths (the home folder becomes ~),
images, roll names, scanner serial numbers. The user is a random install ID
that turning both switches off forgets.
"""
import faulthandler
import json
import os
import platform
import queue
import re
import ssl
import sys
import threading
import traceback
import urllib.request
import uuid
from pathlib import Path

from . import orders

RYBBIT_URL = os.environ.get("PAKON_RYBBIT_URL", "").strip().rstrip("/")
SITE_ID = os.environ.get("PAKON_RYBBIT_SITE_ID", "").strip()
HOSTNAME = os.environ.get("PAKON_RYBBIT_DOMAIN", "").strip()
ENDPOINT = RYBBIT_URL + "/api/track"
_TIMEOUT = 8
# A hard crash in native code (numpy, unicorn) kills Python before it can
# report; faulthandler writes it here and the next launch sends it. Beside
# the app's log (PAKON_LOGS, set by pakon_app.py and the Linux launcher).
CRASH_LOG = Path(os.environ.get("PAKON_LOGS") or "~/Library/Logs/Pakon").expanduser() / "crash.log"

_q: queue.Queue = queue.Queue(maxsize=200)
_started = False
_lock = threading.Lock()


# ── Consent ────────────────────────────────────────────────────────────────

def _settings() -> dict:
    return orders.setting("telemetry") or {}


def configured() -> bool:
    """The build says where reports go (all three PAKON_RYBBIT_* set)."""
    return bool(RYBBIT_URL and SITE_ID and HOSTNAME)


def available() -> bool:
    if not configured():
        return False
    flag = os.environ.get("PAKON_TELEMETRY")
    if flag is not None:
        return flag == "1"
    return bool(os.environ.get("PAKON_VERSION"))


def consent() -> dict:
    """{"available", "asked", "usage", "crashes"} for the page."""
    s = _settings()
    return {"available": available(), "asked": "usage" in s,
            "usage": bool(s.get("usage")), "crashes": bool(s.get("crashes"))}


def set_consent(usage: bool, crashes: bool) -> dict:
    s = _settings()
    s["usage"], s["crashes"] = bool(usage), bool(crashes)
    if not (usage or crashes):
        s.pop("install_id", None)          # forget this install
    elif not s.get("install_id"):
        s["install_id"] = uuid.uuid4().hex
    orders.set_setting("telemetry", s)
    return consent()


def _allowed(kind: str) -> bool:
    return available() and bool(_settings().get(kind))


# ── Scrubbing ──────────────────────────────────────────────────────────────

_HOME = str(Path.home())
_SERIAL = [(re.compile(r"(serial\s*)\d+", re.I), r"\1#"),
           (re.compile(r"(F135(?:PLUS)?-)\d+", re.I), r"\1#")]


def scrub(text: str, limit: int) -> str:
    """No home folder, no scanner serials; the tail when it is too long."""
    text = (text or "").replace(_HOME, "~")
    for pat, rep in _SERIAL:
        text = pat.sub(rep, text)
    return text if len(text) <= limit else "…" + text[-(limit - 1):]


# ── Sending ────────────────────────────────────────────────────────────────

def _context():
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except ImportError:
        return ssl.create_default_context()


def _user_agent() -> str:
    # Rybbit reads the OS from the user agent: a Mac's, Linux's (X11, as
    # WebKitGTK sends it, with the machine's architecture) or Windows' (10 and
    # 11 both say NT 10.0)
    if sys.platform.startswith("linux"):
        osinfo = f"X11; Linux {platform.machine() or 'x86_64'}"
    elif sys.platform == "win32":
        arm = platform.machine().upper() == "ARM64"
        osinfo = "Windows NT 10.0; " + ("ARM64" if arm else "Win64; x64")
    else:
        mac = platform.mac_ver()[0] or "10.15.7"
        osinfo = f"Macintosh; Intel Mac OS X {mac.replace('.', '_')}"
    return (f"Mozilla/5.0 ({osinfo}) "
            f"AppleWebKit/605.1.15 (KHTML, like Gecko) "
            f"Pakon/{os.environ.get('PAKON_VERSION', 'dev')}")


def _linux_release() -> str | None:
    """'Linux Mint 22' from /etc/os-release, on Linux; else None."""
    if not sys.platform.startswith("linux"):
        return None
    try:
        return platform.freedesktop_os_release().get("PRETTY_NAME")
    except OSError:
        return None


def _sender():
    ctx = _context()
    while True:
        body = _q.get()
        req = urllib.request.Request(
            ENDPOINT, data=json.dumps(body).encode(), method="POST",
            headers={"Content-Type": "application/json",
                     "User-Agent": body["user_agent"]})
        try:
            urllib.request.urlopen(req, timeout=_TIMEOUT, context=ctx).close()
        except Exception as exc:            # offline etc.: drop it
            print(f"telemetry: not sent ({exc})", flush=True)


def _send(kind: str, payload: dict) -> None:
    global _started
    if not _allowed(kind):
        return
    with _lock:
        if not _started:
            threading.Thread(target=_sender, daemon=True).start()
            _started = True
    body = {"site_id": SITE_ID, "hostname": HOSTNAME, "pathname": "/",
            "user_id": _settings().get("install_id"),
            "user_agent": _user_agent(), "language": "en", **payload}
    try:
        _q.put_nowait(body)
    except queue.Full:
        pass


def page(name: str) -> None:
    """A page of the app (home, scan, process, frames, export, settings)
    was opened."""
    if name in ("home", "scan", "process", "frames", "export", "settings"):
        _send("usage", {"type": "pageview", "pathname": "/" + name,
                        "page_title": name.capitalize()})


def event(name: str, page_name: str = "", **props) -> None:
    """A usage event; props must be small and anonymous (no paths)."""
    props["version"] = os.environ.get("PAKON_VERSION", "dev")
    _send("usage", {"type": "custom_event", "event_name": name,
                    "pathname": "/" + page_name if page_name else "/",
                    "properties": json.dumps(props)[:2048]})


def error(kind: str, message: str, stack: str = "", page_name: str = "") -> None:
    """A crash or failure report: Rybbit keeps 500 chars of message and 2000
    of stack (here a traceback, or the tail of a scanner log)."""
    props = {"message": scrub(message, 500), "stack": scrub(stack, 2000),
             "version": os.environ.get("PAKON_VERSION", "dev")}
    _send("crashes", {"type": "error", "event_name": kind[:256],
                      "pathname": "/" + page_name if page_name else "/",
                      "properties": json.dumps(props)[:4096]})


def exception(exc: BaseException, where: str = "", page_name: str = "") -> None:
    tb = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))
    error(type(exc).__name__, f"{where}: {exc}" if where else str(exc), tb, page_name)


# ── Startup: hooks and the last hard crash ─────────────────────────────────

def start() -> None:
    """Install the exception hooks, send app_open, and report a hard crash
    the last run left in CRASH_LOG. Call once at startup."""
    if not available():
        return
    prev_hook = sys.excepthook

    def _hook(t, v, tb):
        exception(v, "unhandled")
        prev_hook(t, v, tb)
    sys.excepthook = _hook
    threading.excepthook = lambda a: exception(a.exc_value, f"thread {a.thread.name}")

    try:
        if CRASH_LOG.exists() and CRASH_LOG.stat().st_size:
            tail = CRASH_LOG.read_text(errors="replace")
            first = next((ln for ln in tail.splitlines() if ln.startswith("Fatal")),
                         "hard crash")
            error("NativeCrash", first, tail)
            CRASH_LOG.write_text("")
        CRASH_LOG.parent.mkdir(parents=True, exist_ok=True)
        faulthandler.enable(open(CRASH_LOG, "a"))
    except OSError:
        pass
    event("app_open", "home", macos=platform.mac_ver()[0] or None,
          linux=_linux_release(),
          windows=platform.version() if sys.platform == "win32" else None)

