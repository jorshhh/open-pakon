#!/bin/bash
# Build Pakon (macOS 13+) with PyInstaller: by default both the Apple silicon
# app and a universal one (Apple silicon and Intel).
#
#   packaging/macos/build.sh [VERSION]
#   PAKON_ARCH=arm64 packaging/macos/build.sh [VERSION]    Apple silicon only
#
# The app is frozen once per architecture (the Intel half with
# python-build-standalone's x86_64 Python, run under Rosetta, and x86_64
# wheels); the universal app fuses the two: every Mach-O pair goes through
# lipo, every other file must be the same in both.
#
# Signing: uses the first "Developer ID Application" identity in the keychain
# (or PAKON_SIGN_IDENTITY); without one it signs ad hoc, which runs only on
# this Mac. Notarization runs when PAKON_NOTARY_PROFILE names a profile made
# with `xcrun notarytool store-credentials`. See packaging/macos/README.md.
#
# Crash reports and usage statistics (web/telemetry.py, opt-in) go to a Rybbit
# instance only when PAKON_RYBBIT_URL, PAKON_RYBBIT_SITE_ID and
# PAKON_RYBBIT_DOMAIN are set, from the environment or from packaging/release.env
# (git-ignored; see release.env.example there). Without them the app
# never sends any.
#
# Output: dist/macos/Pakon.app + Pakon.dmg (Apple silicon) and
# dist/macos/Pakon-Universal.app + Pakon-Universal.dmg (both; the app inside
# the dmg is still Pakon.app).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$HERE/../release.env" ]; then
    set -a; . "$HERE/../release.env"; set +a
fi
if [ -n "${PAKON_RYBBIT_URL:-}" ] && [ -n "${PAKON_RYBBIT_SITE_ID:-}" ] \
        && [ -n "${PAKON_RYBBIT_DOMAIN:-}" ]; then
    echo "reports: on (opt-in) to $PAKON_RYBBIT_URL, site $PAKON_RYBBIT_DOMAIN"
else
    echo "reports: off (no PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN)"
fi
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-0.1.0}"
CACHE="$HERE/.cache"
OUT="$REPO/dist/macos"
APP="$OUT/Pakon.app"
DEPLOY=13.0
export MACOSX_DEPLOYMENT_TARGET=$DEPLOY

UNIVERSAL="$OUT/Pakon-Universal.app"
case "${PAKON_ARCH:-both}" in
    both) ARCHS="arm64 x86_64" ;;
    arm64) ARCHS="arm64" ;;
    *) echo "PAKON_ARCH must be arm64 (or unset for both apps)" >&2; exit 1 ;;
esac

# Python from python-build-standalone: built for old macOS, unlike Homebrew's,
# and the frozen app inherits its libpython. One per architecture.
PY_BASE="https://github.com/astral-sh/python-build-standalone/releases/download/20260924"
py_url() {  # arch
    case "$1" in
        arm64)  echo "$PY_BASE/cpython-3.12.14%2B20260924-aarch64-apple-darwin-install_only.tar.gz" ;;
        x86_64) echo "$PY_BASE/cpython-3.12.14%2B20260924-x86_64-apple-darwin-install_only.tar.gz" ;;
    esac
}
py_sha() {  # arch
    case "$1" in
        arm64)  echo 9763f43db2481a6af36af82ec40302aab7a73632f880129d07a6e81aec846277 ;;
        x86_64) echo 0d6a4a299908123f00bc844df737603f047ff9eba14fda6cad83f3cf3cb3a2af ;;
    esac
}
USB_URL="https://github.com/libusb/libusb/releases/download/v1.0.30/libusb-1.0.30.tar.bz2"
USB_SHA=fea36f34f9156400209595e300840767ab1a385ede1dc7ee893015aea9c6dbaf

step() { printf '\n==> %s\n' "$*"; }

fetch() {  # url sha256 dest
    if [ ! -f "$3" ] || ! echo "$2  $3" | shasum -a 256 -c --status; then
        curl -fL --retry 3 -o "$3.part" "$1"
        echo "$2  $3.part" | shasum -a 256 -c --status \
            || { echo "checksum mismatch: $1" >&2; rm -f "$3.part"; exit 1; }
        mv "$3.part" "$3"
    fi
}

machos() {  # every Mach-O file under $1, one per line
    find "$1" -type f -print0 | xargs -0 file -F '|' \
        | grep -v 'for architecture' | grep 'Mach-O' | cut -d'|' -f1
}

[ "$(uname -m)" = arm64 ] || { echo "build on Apple silicon" >&2; exit 1; }
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] \
    || { echo "version must look like 1.2 or 1.2.3, got '$VERSION'" >&2; exit 1; }
IDENTITY="${PAKON_SIGN_IDENTITY:-$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)}"
if [ -n "${PAKON_NOTARY_PROFILE:-}" ] && [ -z "$IDENTITY" ]; then
    echo "notarization needs a Developer ID Application certificate in the keychain" \
         "(see packaging/macos/README.md)" >&2
    exit 1
fi
mkdir -p "$CACHE" "$OUT"
rm -f "$CACHE/AppIcon.icns"                          # made again with the first build's Python
rm -rf "$APP" "$OUT/Pakon.dmg" "$UNIVERSAL" "$OUT/Pakon-Universal.dmg"

# --- one architecture: libusb, the C tools, Python, PyInstaller -------------
# -> .cache/<arch>/dist/Pakon.app (unsigned)
build_arch() {  # arch
    local A="$1" AC="$CACHE/$1"
    mkdir -p "$AC"
    # --- libusb, built for $DEPLOY (Homebrew's targets the running macOS) -------
    step "libusb"
    USB_PREFIX="$AC/libusb-$DEPLOY-nopipe2"
    if [ ! -f "$USB_PREFIX/lib/libusb-1.0.0.dylib" ]; then
        fetch "$USB_URL" "$USB_SHA" "$CACHE/libusb.tar.bz2"
        rm -rf "$AC/libusb-src" && mkdir -p "$AC/libusb-src"
        tar -xjf "$CACHE/libusb.tar.bz2" -C "$AC/libusb-src" --strip-components 1
        # pipe2 is macOS 27+. Against a newer SDK configure finds it, libusb_init
        # calls it unguarded, and older macOS jumps to the null weak import.
        (cd "$AC/libusb-src" \
            && CC="clang -arch $A" ./configure --prefix="$USB_PREFIX" --disable-static --quiet \
                --host="${A/arm64/aarch64}-apple-darwin" ac_cv_func_pipe2=no \
            && make -s -j"$(sysctl -n hw.ncpu)" && make -s install)
    fi
    # minos alone misses this: a weak import still passes the check below
    if dyld_info -imports "$USB_PREFIX/lib/libusb-1.0.0.dylib" | grep -q 'weak-import'; then
        echo "libusb weak-imports symbols newer than $DEPLOY:"
        dyld_info -imports "$USB_PREFIX/lib/libusb-1.0.0.dylib" | grep 'weak-import'
        exit 1
    fi

    # --- C tools, linked against that libusb by rpath --------------------------
    step "pakon_probe, pakon_replay ($A)"
    BIN="$AC/bin"
    rm -rf "$BIN" && mkdir -p "$BIN"
    PKG_CONFIG_PATH="$USB_PREFIX/lib/pkgconfig" cmake -S "$REPO" -B "$AC/cbuild" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_DEPLOYMENT_TARGET=$DEPLOY \
        -DCMAKE_OSX_ARCHITECTURES=$A >/dev/null
    cmake --build "$AC/cbuild" --target pakon_probe pakon_replay -j >/dev/null
    cp "$USB_PREFIX/lib/libusb-1.0.0.dylib" "$BIN/"
    chmod u+w "$BIN/libusb-1.0.0.dylib"
    install_name_tool -id @rpath/libusb-1.0.0.dylib "$BIN/libusb-1.0.0.dylib"
    for t in pakon_probe pakon_replay; do
        cp "$AC/cbuild/$t" "$BIN/"
        install_name_tool -change "$USB_PREFIX/lib/libusb-1.0.0.dylib" \
            @rpath/libusb-1.0.0.dylib "$BIN/$t"
        install_name_tool -add_rpath @loader_path/.. "$BIN/$t"   # build/ -> bundle root
    done

    # --- build Python with wheels for $DEPLOY ----------------------------------
    step "Python ($A)"
    PYROOT="$AC/python"
    if [ ! -x "$PYROOT/bin/python3" ]; then
        fetch "$(py_url "$A")" "$(py_sha "$A")" "$AC/python.tar.gz"
        tar -xzf "$AC/python.tar.gz" -C "$AC"                  # -> .cache/<arch>/python
    fi
    VENV="$AC/venv"
    STAMP="$VENV/.requirements"
    if ! cmp -s "$HERE/requirements.txt" "$STAMP" \
            || ! cmp -s "$REPO/web/requirements.txt" "$STAMP.web"; then
        rm -rf "$VENV"
        "$PYROOT/bin/python3" -m venv "$VENV"
        SITE="$("$VENV/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')"
        # pywebview's proxy_tools ships as source only; --platform needs wheels
        rm -rf "$AC/wheels"
        "$VENV/bin/python" -m pip wheel --quiet --disable-pip-version-check \
            --no-deps proxy_tools -w "$AC/wheels"
        # --platform picks wheels built for $DEPLOY, not for the macOS doing the build
        (cd "$HERE" && "$VENV/bin/python" -m pip install --quiet \
            --disable-pip-version-check --no-cache-dir --upgrade \
            --find-links "$AC/wheels" \
            --target "$SITE" --platform "macosx_${DEPLOY/./_}_$A" \
            --python-version 3.12 --implementation cp --only-binary=:all: \
            -r requirements.txt)
        cp "$HERE/requirements.txt" "$STAMP"
        cp "$REPO/web/requirements.txt" "$STAMP.web"
    fi
    PY="$VENV/bin/python"

    # --- freeze --------------------------------------------------------------------
    if [ ! -f "$CACHE/AppIcon.icns" ]; then
        rm -rf "$CACHE/AppIcon.iconset"
        "$PY" -B "$HERE/make_icon.py" "$CACHE/AppIcon.iconset"
        iconutil -c icns "$CACHE/AppIcon.iconset" -o "$CACHE/AppIcon.icns"
    fi
    step "PyInstaller ($A)"
    rm -rf "$AC/dist"
    PAKON_BIN_DIR="$BIN" PAKON_VERSION="$VERSION" PAKON_ICON="$CACHE/AppIcon.icns" \
        PAKON_TARGET_ARCH="$A" "$PY" -m PyInstaller --noconfirm --clean --log-level WARN \
        --distpath "$AC/dist" --workpath "$AC/pyinstaller" "$HERE/pakon.spec"
    rm -rf "$AC/dist/Pakon"                              # the bare COLLECT folder
}

# Two frozen bundles -> one: Mach-O pairs through lipo, other files must match.
# Allowed to differ: PyInstaller's ad hoc signature (signing replaces it), a
# wheel's install metadata (its platform tag; the arm64 copy is kept), and a
# zip whose members are the same (base_library.zip: only timestamps differ).
zip_same() {  # a.zip b.zip: the same members with the same contents
    local t
    t="$(mktemp -d)"
    unzip -q "$1" -d "$t/a" && unzip -q "$2" -d "$t/b" && diff -rq "$t/a" "$t/b" >/dev/null
    local rc=$?
    rm -rf "$t"
    return $rc
}

fuse() {  # arm64.app x86_64.app out.app
    rm -rf "$3"
    ditto "$1" "$3"
    rm -rf "$3/Contents/_CodeSignature"
    local bad=0 rel a b
    while IFS= read -r -d '' b; do
        rel="${b#"$2"/}"
        a="$3/$rel"
        case "$rel" in
            Contents/_CodeSignature/*|*.dist-info/RECORD|*.dist-info/WHEEL) continue ;;
        esac
        if [ ! -e "$a" ]; then
            echo "only in the Intel build: $rel" >&2; bad=1
        elif file -b "$b" | grep -q 'Mach-O'; then
            lipo -create "$a" "$b" -output "$a.fat" && mv "$a.fat" "$a"
        elif cmp -s "$a" "$b"; then
            :
        elif [[ "$rel" == *.zip ]] && zip_same "$a" "$b"; then
            :
        else
            echo "differs between the builds: $rel" >&2; bad=1
        fi
    done < <(find "$2" -type f -print0)
    while IFS= read -r -d '' a; do
        rel="${a#"$1"/}"
        case "$rel" in Contents/_CodeSignature/*) continue ;; esac
        [ -e "$2/$rel" ] || { echo "only in the Apple silicon build: $rel" >&2; bad=1; }
    done < <(find "$1" -type f -print0)
    return $bad
}

for A in $ARCHS; do
    build_arch "$A"
done
ditto "$CACHE/arm64/dist/Pakon.app" "$APP"
if [ "$ARCHS" = "arm64 x86_64" ]; then
    step "fuse (universal)"
    fuse "$CACHE/arm64/dist/Pakon.app" "$CACHE/x86_64/dist/Pakon.app" "$UNIVERSAL"
fi

if [ -n "$IDENTITY" ]; then
    SIGN=(codesign --force --options runtime --timestamp --sign "$IDENTITY")
    echo "identity: $IDENTITY"
else
    SIGN=(codesign --force --options runtime --sign -)
    echo "no Developer ID Application identity: signing ad hoc (runs on this Mac only)"
fi

# --- check, sign, dmg and notarize one app ------------------------------------
package() {  # app dmg-name
    local app="$1" dmg="$OUT/$2"
    step "check minimum macOS ($(basename "$app"))"
    local bad
    bad=$(machos "$app" | while read -r f; do
            m=$(otool -l "$f" | awk '/LC_BUILD_VERSION/{b=1} b&&/minos/{print $2; exit}')
            if [ -n "$m" ] && [ "$(printf '%s\n%s\n' "$m" "$DEPLOY" | sort -V | tail -1)" != "$DEPLOY" ]; then
                echo "$m ${f#"$app"/}"
            fi
        done)
    [ -z "$bad" ] || { echo "needs newer macOS than $DEPLOY:"; echo "$bad"; exit 1; }

    step "sign ($(basename "$app"))"
    # every Mach-O inside, then the app (its executable gets the entitlements).
    # pakon_probe and pakon_replay are executables of their own: they need the
    # entitlements too, or an ad hoc build cannot load the bundle's libusb.
    machos "$app" | grep -v "/Contents/MacOS/Pakon$" | while read -r f; do
        case "$f" in
            */build/pakon_probe|*/build/pakon_replay) ent=(--entitlements "$HERE/entitlements.plist") ;;
            *) ent=() ;;
        esac
        "${SIGN[@]}" ${ent[@]+"${ent[@]}"} "$f" 2>&1 | { grep -v "replacing existing signature" || true; }
        [ "${PIPESTATUS[0]}" -eq 0 ] || { echo "signing failed: $f" >&2; exit 1; }
    done
    "${SIGN[@]}" --entitlements "$HERE/entitlements.plist" "$app"
    codesign --verify --deep --strict "$app"

    step "dmg ($2)"
    local stage="$CACHE/dmg"
    rm -rf "$stage" && mkdir -p "$stage"
    cp -R "$app" "$stage/Pakon.app"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname Pakon -srcfolder "$stage" -fs HFS+ \
        -format UDZO -ov "$dmg"
    rm -rf "$stage"
    if [ -n "$IDENTITY" ]; then
        codesign --force --timestamp --sign "$IDENTITY" "$dmg"
    fi

    if [ -n "${PAKON_NOTARY_PROFILE:-}" ]; then
        [ -n "$IDENTITY" ] || { echo "notarization needs a Developer ID identity" >&2; exit 1; }
        step "notarize ($2)"
        xcrun notarytool submit "$dmg" --keychain-profile "$PAKON_NOTARY_PROFILE" --wait
        xcrun stapler staple "$dmg"
        spctl --assess --type open --context context:primary-signature -v "$dmg"
    fi
}

package "$APP" Pakon.dmg
[ -d "$UNIVERSAL" ] && package "$UNIVERSAL" Pakon-Universal.dmg

step "done"
du -sh "$OUT"/*.app "$OUT"/*.dmg
