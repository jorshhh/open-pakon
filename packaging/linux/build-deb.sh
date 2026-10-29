#!/bin/bash
# Build a .deb of Pakon for this Debian/Ubuntu machine (amd64/arm64).
#
#   packaging/linux/build-deb.sh [VERSION]
#
# Installs the repo files the app needs to /opt/pakon, the C tools (Release)
# to /opt/pakon/build, and a venv with web/requirements.txt + pywebview at
# /opt/pakon/venv (system site-packages for python3-gi). The venv is made in
# the staging dir and run only as `venv/bin/python`, which finds its
# site-packages from pyvenv.cfg, so moving it to /opt/pakon is fine.
# Also: /usr/bin/pakon, a .desktop entry, an icon, and a udev rule so the
# logged-in user reaches the scanner without sudo.
#
# Crash reports and usage statistics (web/telemetry.py, opt-in): the same
# PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN as the Mac build, from the environment
# or packaging/release.env. They go into /opt/pakon/release.env with
# PAKON_VERSION, which the launcher loads; without them the app never sends any.
#
# Needs: build-essential cmake pkg-config libusb-1.0-0-dev python3-venv
# dpkg-dev. Output: dist/linux/pakon_<VERSION>_<arch>.deb
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-$(git -C "$REPO" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || true)}"
VERSION="${VERSION:-0.1.0}"
ARCH="$(dpkg --print-architecture)"
OUT="$REPO/dist/linux"
STAGE="$OUT/stage"
APPDIR="$STAGE/opt/pakon"
PY="${PYTHON:-python3}"
if [ -f "$REPO/packaging/release.env" ]; then
    set -a; . "$REPO/packaging/release.env"; set +a
fi

step() { printf '\n==> %s\n' "$*"; }

rm -rf "$STAGE" && mkdir -p "$APPDIR" "$OUT"

step "pakon_probe, pakon_replay"
cmake -S "$REPO" -B "$OUT/cbuild" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$OUT/cbuild" --target pakon_probe pakon_replay -j >/dev/null
mkdir -p "$APPDIR/build"
install -m755 "$OUT/cbuild/pakon_probe" "$OUT/cbuild/pakon_replay" "$APPDIR/build/"

step "app files"
# same set as packaging/macos/pakon.spec
mkdir -p "$APPDIR"/{web,tools,resources/f135plus,firmware,packaging/macos}
cp "$REPO"/web/*.py "$APPDIR/web/"
cp -r "$REPO/web/static" "$APPDIR/web/"
cp "$REPO"/tools/*.py "$APPDIR/tools/"
cp "$REPO"/resources/*.pakscan "$REPO"/resources/*.pakfw "$APPDIR/resources/"
cp "$REPO"/resources/f135plus/*.pakscan "$APPDIR/resources/f135plus/"
cp "$REPO"/firmware/*.hex "$REPO/firmware/README.md" "$APPDIR/firmware/"
cp -r "$REPO/profiles" "$REPO/oem" "$APPDIR/"
cp "$REPO/LICENSE" "$REPO/NOTICE" "$APPDIR/"
cp "$REPO/packaging/macos/pakon_app.py" "$APPDIR/packaging/macos/"
find "$APPDIR" -name __pycache__ -prune -exec rm -rf {} +

# what the Mac app reads from Info.plist (pakon_app.py)
{
    echo "PAKON_VERSION=$VERSION"
    if [ -n "${PAKON_RYBBIT_URL:-}" ] && [ -n "${PAKON_RYBBIT_SITE_ID:-}" ] \
            && [ -n "${PAKON_RYBBIT_DOMAIN:-}" ]; then
        echo "reports: on (opt-in) to $PAKON_RYBBIT_URL, site $PAKON_RYBBIT_DOMAIN" >&2
        echo "PAKON_RYBBIT_URL=$PAKON_RYBBIT_URL"
        echo "PAKON_RYBBIT_SITE_ID=$PAKON_RYBBIT_SITE_ID"
        echo "PAKON_RYBBIT_DOMAIN=$PAKON_RYBBIT_DOMAIN"
    else
        echo "reports: off (no PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN)" >&2
    fi
} > "$APPDIR/release.env"

step "Python venv"
"$PY" -m venv --system-site-packages "$APPDIR/venv"
"$APPDIR/venv/bin/python" -m pip install --quiet --disable-pip-version-check \
    --no-cache-dir -r "$REPO/web/requirements.txt" \
    "$(grep '^pywebview==' "$REPO/packaging/macos/requirements.txt")"
find "$APPDIR/venv" -name __pycache__ -prune -exec rm -rf {} +
PYVER="$("$PY" -c 'import sys; print(f"{sys.version_info[0]}.{sys.version_info[1]}")')"

step "launcher, desktop entry, icon, udev rule"
install -Dm755 "$HERE/pakon" "$STAGE/usr/bin/pakon"
install -Dm644 "$HERE/pakon.desktop" "$STAGE/usr/share/applications/pakon.desktop"
install -Dm644 "$REPO/brand/logo.png" "$STAGE/usr/share/pixmaps/pakon.png"
install -Dm644 "$HERE/60-pakon.rules" "$STAGE/usr/lib/udev/rules.d/60-pakon.rules"

step "control"
mkdir -p "$STAGE/DEBIAN"
SIZE="$(du -sk "$STAGE" | cut -f1)"
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: pakon
Version: $VERSION
Architecture: $ARCH
Maintainer: Pakon <https://github.com/jorshhh/open-pakon>
Installed-Size: $SIZE
Depends: python$PYVER, python3-gi, gir1.2-webkit2-4.1, libusb-1.0-0
Section: graphics
Priority: optional
Homepage: https://github.com/jorshhh/open-pakon
Description: Driver and app for the Kodak Pakon F-135 / F-135+ film scanner
 Scan film on a Kodak Pakon F-135 or F-135+ without the original Windows
 software: firmware load, scan, framing and export, with Kodak's colour.
CONTROL
cat > "$STAGE/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if [ "$1" = configure ] && command -v udevadm >/dev/null; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=usb --attr-match=idVendor=0f05 || true
fi
POSTINST
chmod 755 "$STAGE/DEBIAN/postinst"

step "deb"
DEB="$OUT/pakon_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$DEB" >/dev/null
rm -rf "$STAGE"
du -sh "$DEB"
