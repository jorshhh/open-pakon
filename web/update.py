"""
New-build check: is there a GitHub release newer than this app?

The app's version comes from PAKON_VERSION (pakon_app.py sets it from the
bundle's Info.plist); a checkout has none and never checks. The latest
release is asked from the GitHub API once at startup and then every few
hours, in the background; any failure (offline, rate limit) just means no
banner. PAKON_UPDATE_CHECK=0 turns the check off.
"""
import json
import os
import re
import ssl
import threading
import time
import urllib.request

REPO = "jorshhh/open-pakon"
_API = f"https://api.github.com/repos/{REPO}/releases/latest"
RELEASES = f"https://github.com/{REPO}/releases"
_EVERY = 6 * 3600          # seconds between checks
_TIMEOUT = 8

_lock = threading.Lock()
_state = {"latest": None, "url": None, "checked": 0.0}


def current() -> str | None:
    v = os.environ.get("PAKON_VERSION", "").strip()
    return v or None


def enabled() -> bool:
    return current() is not None and os.environ.get("PAKON_UPDATE_CHECK", "1") != "0"


def _parse(v: str) -> tuple[int, ...] | None:
    """'v0.3.0' / '0.3' -> (0, 3, 0); None when it is not a plain version."""
    m = re.fullmatch(r"v?(\d+)(?:\.(\d+))?(?:\.(\d+))?", v.strip())
    return tuple(int(x or 0) for x in m.groups()) if m else None


def newer(latest: str, installed: str) -> bool:
    a, b = _parse(latest), _parse(installed)
    return a is not None and b is not None and a > b


def _context():
    # The app's Python has no system CA bundle; certifi ships one.
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except ImportError:
        return ssl.create_default_context()


def check() -> None:
    """Ask GitHub for the latest release (blocking; run it in a thread)."""
    if not enabled():
        return
    with _lock:
        if time.time() - _state["checked"] < _EVERY:
            return
        _state["checked"] = time.time()
    req = urllib.request.Request(_API, headers={
        "Accept": "application/vnd.github+json",
        "User-Agent": f"Pakon/{current()}",
    })
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT, context=_context()) as r:
            rel = json.load(r)
    except Exception as exc:          # offline, rate limited, ...: no banner
        print(f"update check failed: {exc}", flush=True)
        return
    tag, url = rel.get("tag_name") or "", rel.get("html_url") or ""
    if rel.get("draft") or rel.get("prerelease") or _parse(tag) is None:
        return
    with _lock:
        _state["latest"] = tag.lstrip("v")
        # only ever open this repo's release pages
        _state["url"] = url if url.startswith(RELEASES + "/") else RELEASES + "/latest"


def status() -> dict | None:
    """{"current", "latest", "url"} when a newer release is out, else None.
    Starts a background check when the last one is old."""
    if not enabled():
        return None
    if time.time() - _state["checked"] >= _EVERY:
        threading.Thread(target=check, daemon=True).start()
    with _lock:
        latest, url = _state["latest"], _state["url"]
    if latest and newer(latest, current()):
        return {"current": current(), "latest": latest, "url": url}
    return None
