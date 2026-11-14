#!/bin/bash
# Build Pakon (macOS 13+) from the Rust port: one native binary
# (rust/crates/pakon-app: the service, the scanner driver, the image
# pipeline and the window) plus the files it reads. No Python, no libusb.
# By default both the Apple silicon app and a universal one.
#
#   packaging/macos/build.sh [VERSION]
#   PAKON_ARCH=arm64 packaging/macos/build.sh [VERSION]    Apple silicon only
#
# Needs rustup with the aarch64-apple-darwin target (and x86_64-apple-darwin
# for the universal app), cmake (Unicorn is built from source by its crate)
# and Python 3 with Pillow for the icon, the first time only.
#
# Signing, notarization and crash reports:
# PAKON_SIGN_IDENTITY or the first "Developer ID Application" identity (else
# ad hoc), PAKON_NOTARY_PROFILE, and PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN
# from the environment or packaging/release.env.
#
# Output: dist/macos/Pakon.app + Pakon.dmg (Apple silicon) and
# Pakon-Universal.app + Pakon-Universal.dmg (both; the app inside the dmg is
# still Pakon.app).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$HERE/../release.env" ]; then
    set -a; . "$HERE/../release.env"; set +a
fi
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-0.1.0}"
CACHE="$HERE/.cache"
OUT="$REPO/dist/macos"
APP="$OUT/Pakon.app"
UNIVERSAL="$OUT/Pakon-Universal.app"
DEPLOY=13.0
export MACOSX_DEPLOYMENT_TARGET=$DEPLOY

case "${PAKON_ARCH:-both}" in
    both) ARCHS="arm64 x86_64" ;;
    arm64) ARCHS="arm64" ;;
    *) echo "PAKON_ARCH must be arm64 (or unset for both apps)" >&2; exit 1 ;;
esac
triple() { [ "$1" = arm64 ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin; }

step() { printf '\n==> %s\n' "$*"; }

[ "$(uname -m)" = arm64 ] || { echo "build on Apple silicon" >&2; exit 1; }
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] \
    || { echo "version must look like 1.2 or 1.2.3, got '$VERSION'" >&2; exit 1; }
if [ -n "${PAKON_RYBBIT_URL:-}" ] && [ -n "${PAKON_RYBBIT_SITE_ID:-}" ] \
        && [ -n "${PAKON_RYBBIT_DOMAIN:-}" ]; then
    echo "reports: on (opt-in) to $PAKON_RYBBIT_URL, site $PAKON_RYBBIT_DOMAIN"
    RYBBIT=1
else
    echo "reports: off (no PAKON_RYBBIT_URL / _SITE_ID / _DOMAIN)"
    RYBBIT=0
fi
IDENTITY="${PAKON_SIGN_IDENTITY:-$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)}"
if [ -n "${PAKON_NOTARY_PROFILE:-}" ] && [ -z "$IDENTITY" ]; then
    echo "notarization needs a Developer ID Application certificate in the keychain" >&2
    exit 1
fi
mkdir -p "$CACHE" "$OUT"
rm -rf "$APP" "$UNIVERSAL" "$OUT/Pakon.dmg" "$OUT/Pakon-Universal.dmg"

# --- icon (once) -------------------------------------------------------------
if [ ! -f "$CACHE/AppIcon.icns" ]; then
    step "icon"
    PY="${PAKON_PYTHON:-}"
    for p in "$PY" "$REPO/.venv/bin/python" python3; do
        if [ -n "$p" ] && "$p" -c 'import PIL' 2>/dev/null; then PY="$p"; break; fi
    done
    "$PY" -c 'import PIL' 2>/dev/null || { echo "the icon needs Python 3 with Pillow (PAKON_PYTHON)" >&2; exit 1; }
    rm -rf "$CACHE/AppIcon.iconset"
    "$PY" -B "$HERE/make_icon.py" "$CACHE/AppIcon.iconset"
    iconutil -c icns "$CACHE/AppIcon.iconset" -o "$CACHE/AppIcon.icns"
fi

# --- the binary, per architecture --------------------------------------------
for A in $ARCHS; do
    T="$(triple "$A")"
    step "pakon_app ($A)"
    rustup target list --installed | grep -qx "$T" \
        || { echo "missing Rust target: rustup target add $T" >&2; exit 1; }
    (cd "$REPO/rust" && cargo build --release --locked -p pakon-app --target "$T")
    # Unicorn's JIT must emit this arch's code (packaging/macos/cc-x86_64)
    H="$( [ "$A" = arm64 ] && echo AARCH64 || echo X86_64 )"
    CH="$(ls -t "$REPO"/rust/target/"$T"/release/build/unicorn-engine-sys-*/out/build/config-host.h | head -1)"
    grep -q "^#define HOST_$H 1" "$CH" || { echo "Unicorn built for the wrong host ($CH):" \
        "cargo clean --release -p unicorn-engine-sys --target $T" >&2; exit 1; }
done
BIN="$CACHE/pakon_app"
rm -f "$BIN.universal"   # an earlier universal build's, not this one's
if [ "$ARCHS" = "arm64 x86_64" ]; then
    lipo -create "$REPO/rust/target/aarch64-apple-darwin/release/pakon_app" \
        "$REPO/rust/target/x86_64-apple-darwin/release/pakon_app" -output "$BIN.universal"
fi
cp "$REPO/rust/target/aarch64-apple-darwin/release/pakon_app" "$BIN.arm64"

# --- the bundle -----------------------------------------------------------------
# Contents/Resources mirrors the repo layout the service reads (paths.rs
# find_root): web/static, resources, firmware, profiles, oem.
assemble() {  # binary app
    local app="$2" res="$2/Contents/Resources"
    step "bundle $(basename "$app")"
    mkdir -p "$app/Contents/MacOS" "$res/web" "$res/resources/f135plus" "$res/firmware"
    cp "$1" "$app/Contents/MacOS/Pakon"
    cp -R "$REPO/web/static" "$res/web/static"
    # the scan scripts and the captured firmware load, not the captures
    cp "$REPO"/resources/*.pakscan "$REPO"/resources/*.pakfw "$res/resources/"
    cp "$REPO"/resources/f135plus/*.pakscan "$res/resources/f135plus/"
    cp "$REPO"/firmware/*.hex "$REPO/firmware/README.md" "$res/firmware/" 2>/dev/null || true
    cp -R "$REPO/profiles" "$REPO/oem" "$res/"
    cp "$REPO/LICENSE" "$REPO/NOTICE" "$res/"
    cp "$CACHE/AppIcon.icns" "$res/AppIcon.icns"
    local rybbit=""
    if [ "$RYBBIT" = 1 ]; then
        rybbit="    <key>PakonRybbitURL</key><string>$PAKON_RYBBIT_URL</string>
    <key>PakonRybbitSiteID</key><string>$PAKON_RYBBIT_SITE_ID</string>
    <key>PakonRybbitDomain</key><string>$PAKON_RYBBIT_DOMAIN</string>"
    fi
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Pakon</string>
    <key>CFBundleDisplayName</key><string>Pakon</string>
    <key>CFBundleIdentifier</key><string>com.pakon.scanner</string>
    <key>CFBundleExecutable</key><string>Pakon</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>$DEPLOY</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>AGPL-3.0-or-later. Not affiliated with Kodak or Pakon.</string>
$rybbit
</dict>
</plist>
PLIST
    plutil -lint -s "$app/Contents/Info.plist"
}

assemble "$BIN.arm64" "$APP"
[ -f "$BIN.universal" ] && assemble "$BIN.universal" "$UNIVERSAL"

if [ -n "$IDENTITY" ]; then
    SIGN=(codesign --force --options runtime --timestamp --sign "$IDENTITY")
    echo "identity: $IDENTITY"
else
    SIGN=(codesign --force --options runtime --sign -)
    echo "no Developer ID Application identity: signing ad hoc (runs on this Mac only)"
fi

# --- check, sign, dmg and notarize one app ---------------------------------------
package() {  # app dmg-name
    local app="$1" dmg="$OUT/$2" exe="$1/Contents/MacOS/Pakon"
    step "check ($(basename "$app"))"
    local m
    for m in $(otool -l "$exe" | awk '/LC_BUILD_VERSION/{b=1} b&&/minos/{print $2; b=0}'); do
        [ "$(printf '%s\n%s\n' "$m" "$DEPLOY" | sort -V | tail -1)" = "$DEPLOY" ] \
            || { echo "needs macOS $m, newer than $DEPLOY" >&2; exit 1; }
    done
    # a weak import newer than $DEPLOY passes the minos check and crashes
    # on older macOS at the call (as libusb's pipe2 once did)
    if dyld_info -imports "$exe" | grep -q 'weak-import'; then
        echo "weak imports (check each is guarded):"
        dyld_info -imports "$exe" | grep 'weak-import'
    fi
    # only system libraries (library lines are indented; a fat binary also
    # prints a header line per architecture)
    if otool -L "$exe" | grep -E '^\s' | grep -v -E '^\s+(/System/Library/|/usr/lib/)'; then
        echo "links a library outside the system" >&2; exit 1
    fi

    step "sign ($(basename "$app"))"
    # the hardened runtime needs JIT for Unicorn (the OEM DLLs' emulator)
    "${SIGN[@]}" --entitlements "$HERE/entitlements.plist" "$app"
    codesign --verify --deep --strict "$app"

    step "dmg ($2)"
    local stage="$CACHE/dmg"
    rm -rf "$stage" && mkdir -p "$stage"
    cp -R "$app" "$stage/Pakon.app"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname Pakon -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
    rm -rf "$stage"
    if [ -n "$IDENTITY" ]; then
        codesign --force --timestamp --sign "$IDENTITY" "$dmg"
    fi
    if [ -n "${PAKON_NOTARY_PROFILE:-}" ]; then
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
