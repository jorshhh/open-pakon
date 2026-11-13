#!/bin/bash
# Build the Windows installer of Pakon (64-bit Windows 10 or 11) on Linux or
# a Mac.
#
#   packaging/windows/build.sh [VERSION]
#
# Nothing here runs on Windows. Pakon.exe is the Rust app (rust/crates/
# pakon-app: the service, the scanner driver, the image pipeline and the
# window, in WebView2) cross-compiled with MinGW, next to WebView2Loader.dll
# (Microsoft's, from the webview2-com-sys crate) and the files it reads;
# pakon_driver.exe (pakon_driver.c) sets up the scanner's WinUSB driver; and
# NSIS (installer.nsi) packs it all into a per-user installer.
#
# Crash reports and usage statistics (opt-in): as the Linux build,
# PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN from the environment or
# packaging/release.env go into release.env in the install, with PAKON_VERSION.
#
# Needs: rustup with the x86_64-pc-windows-gnu target, gcc-mingw-w64-x86-64
# g++-mingw-w64-x86-64 cmake libclang-dev nsis python3-venv; without them it builds in
# Docker (Dockerfile here). Output: dist/windows/Pakon-Setup.exe
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-$(git -C "$REPO" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || true)}"
VERSION="${VERSION:-0.1.0}"
OUT="$REPO/dist/windows"
CACHE="$OUT/.cache"
APP="$OUT/stage/Pakon"
TRIPLE=x86_64-pc-windows-gnu
if [ -f "$REPO/packaging/release.env" ]; then
    set -a; . "$REPO/packaging/release.env"; set +a
fi
CC=x86_64-w64-mingw32-gcc

step() { printf '\n==> %s\n' "$*"; }

[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] \
    || { echo "version must look like 1.2 or 1.2.3, got '$VERSION'" >&2; exit 1; }

if ! command -v "$CC" >/dev/null || ! command -v makensis >/dev/null \
        || ! rustup target list --installed 2>/dev/null | grep -qx "$TRIPLE"; then
    command -v docker >/dev/null || { echo "needs MinGW, NSIS and rustup's $TRIPLE target" \
        "(see above), or Docker" >&2; exit 1; }
    step "no MinGW, NSIS or $TRIPLE here: building in Docker"
    docker build -q -t pakon-win-build "$HERE" >/dev/null
    mkdir -p "$CACHE/cargo-registry"
    exec docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp \
        -e PAKON_RYBBIT_URL -e PAKON_RYBBIT_SITE_ID -e PAKON_RYBBIT_DOMAIN \
        -v "$CACHE/cargo-registry:/usr/local/cargo/registry" \
        -v "$REPO:$REPO" -w "$REPO" pakon-win-build "$HERE/build.sh" "$VERSION"
fi

rm -rf "$OUT/stage" && mkdir -p "$APP" "$CACHE"

step "icon"
BENV="$OUT/buildenv"
[ -x "$BENV/bin/python" ] && "$BENV/bin/python" -c 'import PIL' 2>/dev/null || {
    python3 -m venv "$BENV" &&
    "$BENV/bin/pip" install --quiet --disable-pip-version-check pillow; }
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
echo '1 ICON "pakon.ico"' > "$OUT/app.rc"
(cd "$OUT" && x86_64-w64-mingw32-windres app.rc -O coff -o app-res.o)

step "Pakon.exe ($TRIPLE)"
# its own target folder: rust/target holds the Mac's build scripts
TARGET="$OUT/target"
# bindgen (Unicorn's bindings) parses the headers as MinGW's GCC sees them
MINGW_INC="$("$CC" -print-file-name=include)"
MINGW_SYSROOT="$(dirname "$(dirname "$(command -v "$CC")")")/x86_64-w64-mingw32"
[ -d "$MINGW_SYSROOT" ] || MINGW_SYSROOT=/usr/x86_64-w64-mingw32
# Unicorn's build script links pthread as a DLL (-lpthread, which rustc
# wraps in -Bdynamic): a folder searched first whose "import library" is
# the static libwinpthread.a, so Pakon.exe needs no libwinpthread-1.dll
STATIC="$OUT/static-libs"
mkdir -p "$STATIC"
cp "$("$CC" -print-file-name=libwinpthread.a)" "$STATIC/libpthread.dll.a"
cp "$STATIC/libpthread.dll.a" "$STATIC/libpthread.a"
(cd "$REPO/rust" && CARGO_TARGET_DIR="$TARGET" \
    CC_x86_64_pc_windows_gnu="$CC" CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
    BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnu="-I$MINGW_INC --sysroot=$MINGW_SYSROOT" \
    cargo rustc --release --locked -p pakon-app --bin pakon_app --target "$TRIPLE" \
        -- -L "native=$STATIC" -C link-arg="$OUT/app-res.o" -C target-feature=+crt-static)
cp "$TARGET/$TRIPLE/release/pakon_app.exe" "$APP/Pakon.exe"
x86_64-w64-mingw32-strip "$APP/Pakon.exe"
# WebView2's loader: MinGW links the DLL (MSVC would link it statically)
WV="$(sed -n '/^name = "webview2-com-sys"$/{n;s/^version = "\(.*\)"$/\1/p;}' "$REPO/rust/Cargo.lock")"
LOADER="$(ls "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/webview2-com-sys-"$WV"/x64/WebView2Loader.dll | head -1)"
cp "$LOADER" "$APP/"
# only Windows' own DLLs, and the loader
BAD="$(x86_64-w64-mingw32-objdump -p "$APP/Pakon.exe" | sed -n 's/^\s*DLL Name: //p' \
    | grep -v -i -E '^(kernel32|user32|gdi32|advapi32|shell32|ole32|oleaut32|ws2_32|bcrypt|bcryptprimitives|ntdll|userenv|winusb|setupapi|cfgmgr32|comctl32|combase|rpcrt4|uxtheme|dwmapi|shlwapi|version|imm32|crypt32|secur32|ncrypt|iphlpapi|winmm|propsys|uiautomationcore|api-ms-win-[a-z0-9-]*|msvcrt|ucrtbase|webview2loader)\.dll$' || true)"
[ -z "$BAD" ] || { echo "Pakon.exe needs DLLs Windows does not have:" $BAD >&2; exit 1; }

step "app files"
# the install folder mirrors the repo layout the service reads (paths.rs
# find_root): web/static, resources, firmware, profiles, oem; as the Mac and
# Linux builds
mkdir -p "$APP"/{web,resources/f135plus,firmware}
cp -r "$REPO/web/static" "$APP/web/"
cp "$REPO"/resources/*.pakscan "$REPO"/resources/*.pakfw "$APP/resources/"
cp "$REPO"/resources/f135plus/*.pakscan "$APP/resources/f135plus/"
cp "$REPO"/firmware/*.hex "$REPO/firmware/README.md" "$APP/firmware/" 2>/dev/null || true
cp -r "$REPO/profiles" "$REPO/oem" "$APP/"
cp "$REPO/LICENSE" "$REPO/NOTICE" "$APP/"

# what the Mac app reads from Info.plist
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
