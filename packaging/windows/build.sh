#!/bin/bash
# Build the Windows installer of Pakon (64-bit Windows 10 or 11) on Linux.
#
#   packaging/windows/build.sh [VERSION]
#
# Nothing here runs on Windows: the C tools are cross-compiled with MinGW
# (static, with libusb's Windows build), Python is python.org's embeddable
# 3.12 with the Windows wheels of web/requirements.txt + pywebview (uv picks
# them for Windows), Pakon.exe (launcher.c) starts packaging/macos/pakon_app.py
# with it, pakon_driver.exe (pakon_driver.c) sets up the scanner's WinUSB
# driver, and NSIS (installer.nsi) packs it all into a per-user installer.
# Downloads are pinned and checked against their SHA-256.
#
# Crash reports and usage statistics (web/telemetry.py, opt-in): as the Linux
# build, PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN from the environment or
# packaging/release.env go into release.env in the install, with PAKON_VERSION.
#
# Needs: gcc-mingw-w64-x86-64 cmake pkg-config nsis p7zip-full unzip curl
# python3-venv; without MinGW or NSIS it builds in Docker (Dockerfile here).
# Output: dist/windows/Pakon-Setup.exe
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-$(git -C "$REPO" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || true)}"
VERSION="${VERSION:-0.1.0}"
OUT="$REPO/dist/windows"
CACHE="$OUT/.cache"
APP="$OUT/stage/Pakon"
if [ -f "$REPO/packaging/release.env" ]; then
    set -a; . "$REPO/packaging/release.env"; set +a
fi

PY_URL="https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip"
PY_SHA=4acbed6dd1c744b0376e3b1cf57ce906f9dc9e95e68824584c8099a63025a3c3
USB_URL="https://github.com/libusb/libusb/releases/download/v1.0.30/libusb-1.0.30.7z"
USB_SHA=7fb1dfec805b97983763d7d0ae244320da12add1003d4249c96cc4d586398c79
CC=x86_64-w64-mingw32-gcc

step() { printf '\n==> %s\n' "$*"; }

fetch() {  # url sha256 dest
    if [ ! -f "$3" ] || ! echo "$2  $3" | sha256sum -c --status; then
        curl -fL --retry 3 -o "$3.part" "$1"
        echo "$2  $3.part" | sha256sum -c --status \
            || { echo "checksum mismatch: $1" >&2; rm -f "$3.part"; exit 1; }
        mv "$3.part" "$3"
    fi
}

[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] \
    || { echo "version must look like 1.2 or 1.2.3, got '$VERSION'" >&2; exit 1; }

if ! command -v "$CC" >/dev/null || ! command -v makensis >/dev/null; then
    command -v docker >/dev/null || { echo "needs MinGW and NSIS (sudo apt install" \
        "gcc-mingw-w64-x86-64 cmake pkg-config nsis p7zip-full unzip) or Docker" >&2; exit 1; }
    step "no MinGW or NSIS here: building in Docker"
    docker build -q -t pakon-win-build "$HERE" >/dev/null
    exec docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp \
        -e PAKON_RYBBIT_URL -e PAKON_RYBBIT_SITE_ID -e PAKON_RYBBIT_DOMAIN \
        -v "$REPO:$REPO" -w "$REPO" pakon-win-build "$HERE/build.sh" "$VERSION"
fi

rm -rf "$OUT/stage" && mkdir -p "$APP" "$CACHE"

step "downloads"
fetch "$PY_URL" "$PY_SHA" "$CACHE/python-embed.zip"
fetch "$USB_URL" "$USB_SHA" "$CACHE/libusb.7z"

step "pakon_probe, pakon_replay (MinGW, static)"
# libusb's static library only, so the tools need no DLL next to them
USB="$OUT/libusb"
rm -rf "$USB" && mkdir -p "$USB/x" "$USB/lib" "$USB/include/libusb-1.0"
7z x -y -o"$USB/x" "$CACHE/libusb.7z" >/dev/null
cp "$USB/x/MinGW64/static/libusb-1.0.a" "$USB/lib/"
cp "$USB/x/include/libusb.h" "$USB/include/libusb-1.0/"
cat > "$USB/libusb-1.0.pc" <<PC
Name: libusb-1.0
Description: libusb (Windows build)
Version: 1.0.30
Libs: -L$USB/lib -lusb-1.0
Cflags: -I$USB/include/libusb-1.0
PC
PKG_CONFIG_LIBDIR="$USB" cmake -S "$REPO" -B "$OUT/cbuild" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_SYSTEM_NAME=Windows -DCMAKE_C_COMPILER="$CC" \
    -DCMAKE_EXE_LINKER_FLAGS=-static >/dev/null
cmake --build "$OUT/cbuild" --target pakon_probe pakon_replay -j >/dev/null
mkdir -p "$APP/build"
cp "$OUT/cbuild/pakon_probe.exe" "$OUT/cbuild/pakon_replay.exe" "$APP/build/"
x86_64-w64-mingw32-strip "$APP"/build/*.exe
# PyUSB's (web/eeprom.py), found by pakon_app.py
cp "$USB/x/MinGW64/dll/libusb-1.0.dll" "$APP/"

step "Python $(basename "$PY_URL" .zip)"
unzip -q -o "$CACHE/python-embed.zip" -d "$APP"
# the import path: the stdlib, the install folder (web/, tools/) and the
# wheels; `import site` reads the wheels' .pth files (pythonnet, pywin32)
printf 'python312.zip\r\n.\r\nLib\\site-packages\r\nimport site\r\n' > "$APP/python312._pth"

step "wheels for Windows"
BENV="$OUT/buildenv"
[ -x "$BENV/bin/uv" ] || { python3 -m venv "$BENV" &&
    "$BENV/bin/pip" install --quiet --disable-pip-version-check uv==0.13.6 pillow; }
# Windows wheels only, so nothing is compiled here for Linux; proxy_tools
# (pywebview's) is pure Python but has no wheel, so it gets one here
[ -f "$CACHE/wheels/proxy_tools-0.1.0-py3-none-any.whl" ] ||
    "$BENV/bin/pip" wheel --quiet --disable-pip-version-check --no-deps \
        -w "$CACHE/wheels" proxy_tools==0.1.0
UV_CACHE_DIR="$CACHE/uv" "$BENV/bin/uv" pip install --quiet --python "$BENV/bin/python" \
    --target "$APP/Lib/site-packages" --python-platform x86_64-pc-windows-msvc \
    --python-version 3.12 --only-binary :all: --find-links "$CACHE/wheels" \
    -r "$REPO/web/requirements.txt" \
    "$(grep '^pywebview==' "$REPO/packaging/macos/requirements.txt")"

step "app files"
# same set as packaging/macos/pakon.spec and packaging/linux/build-deb.sh
mkdir -p "$APP"/{web,tools,resources/f135plus,firmware,packaging/macos}
cp "$REPO"/web/*.py "$APP/web/"
cp -r "$REPO/web/static" "$APP/web/"
cp "$REPO"/tools/*.py "$APP/tools/"
cp "$REPO"/resources/*.pakscan "$REPO"/resources/*.pakfw "$APP/resources/"
cp "$REPO"/resources/f135plus/*.pakscan "$APP/resources/f135plus/"
cp "$REPO"/firmware/*.hex "$REPO/firmware/README.md" "$APP/firmware/"
cp -r "$REPO/profiles" "$REPO/oem" "$APP/"
cp "$REPO/LICENSE" "$REPO/NOTICE" "$APP/"
cp "$REPO/packaging/macos/pakon_app.py" "$APP/packaging/macos/"
find "$APP" -name __pycache__ -prune -exec rm -rf {} +

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
} > "$APP/release.env"

step "Pakon.exe"
"$BENV/bin/python" - "$REPO/brand/logo.png" "$OUT/pakon.ico" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert("RGBA")
side = max(img.size)                      # square, centred
sq = Image.new("RGBA", (side, side))
sq.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
sq.save(sys.argv[2], sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
PY
cp "$OUT/pakon.ico" "$APP/"
echo '1 ICON "pakon.ico"' > "$OUT/launcher.rc"
(cd "$OUT" && x86_64-w64-mingw32-windres launcher.rc -O coff -o launcher.res)
"$CC" -O2 -Wall -Wextra -municode -mwindows -static -o "$APP/Pakon.exe" \
    "$HERE/launcher.c" "$OUT/launcher.res"
x86_64-w64-mingw32-strip "$APP/Pakon.exe"

step "pakon_driver.exe (the scanner's WinUSB driver)"
mkdir -p "$APP/driver"
cp "$HERE/pakon-winusb.inf" "$APP/driver/"
printf '1 24 "%s"\n' "$HERE/pakon_driver.manifest" > "$OUT/driver.rc"
(cd "$OUT" && x86_64-w64-mingw32-windres driver.rc -O coff -o driver.res)
"$CC" -O2 -Wall -Wextra -municode -mwindows -static -o "$APP/driver/pakon_driver.exe" \
    "$HERE/pakon_driver.c" "$OUT/driver.res" -lcrypt32 -lwintrust -lsetupapi
x86_64-w64-mingw32-strip "$APP/driver/pakon_driver.exe"

step "installer"
makensis -V2 -DVERSION="$VERSION" -DSTAGE="$APP" -DOUTFILE="$OUT/Pakon-Setup.exe" \
    "$HERE/installer.nsi"
chmod +x "$OUT/Pakon-Setup.exe"      # Samba runs it from a share only then
du -sh "$OUT/Pakon-Setup.exe"
