#!/bin/bash
# Build a .deb of Pakon (amd64 or arm64) from the Rust port: one native
# binary (rust/crates/pakon-app: the service, the scanner driver, the image
# pipeline and the window, in WebKitGTK) plus the files it reads.
#
#   packaging/linux/build-deb.sh [VERSION]
#   PAKON_DEB_ARCH=amd64 packaging/linux/build-deb.sh [VERSION]   (in Docker)
#
# Installs the binary and its files to /opt/pakon, /usr/bin/pakon (the
# launcher: data in ~/.local/share/pakon, the log in
# ~/.local/state/pakon/pakon.log), a .desktop entry, an icon, and a udev
# rule so the logged-in user reaches the scanner without sudo. Depends only
# on the system's GTK 3 and WebKitGTK 4.1 (dpkg-shlibdeps works them out).
#
# Crash reports and usage statistics (opt-in): the same PAKON_RYBBIT_URL /
# _SITE_ID / _DOMAIN as the Mac build, from the environment or
# packaging/release.env. They go into /opt/pakon/release.env with
# PAKON_VERSION, which the app reads; without them it never sends any.
#
# Needs on Debian/Ubuntu: cargo (rustup), build-essential cmake pkg-config
# libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev dpkg-dev. Without them (or on a Mac) it
# builds in Docker from the Dockerfile here, for PAKON_DEB_ARCH (default:
# this machine's; the other one runs emulated, much slower), with
# PAKON_UBUNTU_MIRROR in place of archive.ubuntu.com if that is out of reach.
# Output: dist/linux/pakon_<arch>.deb
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-$(git -C "$REPO" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || true)}"
VERSION="${VERSION:-0.1.0}"
OUT="$REPO/dist/linux"
if [ -f "$REPO/packaging/release.env" ]; then
    set -a; . "$REPO/packaging/release.env"; set +a
fi

step() { printf '\n==> %s\n' "$*"; }

[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] \
    || { echo "version must look like 1.2 or 1.2.3, got '$VERSION'" >&2; exit 1; }

native() {
    [ "$(uname -s)" = Linux ] && command -v cargo >/dev/null && command -v dpkg-deb >/dev/null \
        && pkg-config --exists webkit2gtk-4.1 gtk+-3.0 2>/dev/null \
        && { [ -z "${PAKON_DEB_ARCH:-}" ] || [ "$PAKON_DEB_ARCH" = "$(dpkg --print-architecture)" ]; }
}
if ! native; then
    command -v docker >/dev/null || { echo "needs cargo, dpkg-dev and the GTK 3 / WebKitGTK 4.1" \
        "headers (see above), or Docker" >&2; exit 1; }
    case "${PAKON_DEB_ARCH:-$(uname -m)}" in
        amd64|x86_64) A=amd64 ;;
        arm64|aarch64) A=arm64 ;;
        *) echo "PAKON_DEB_ARCH must be amd64 or arm64" >&2; exit 1 ;;
    esac
    step "building in Docker (linux/$A)"
    docker build -q --platform "linux/$A" --build-arg UBUNTU_MIRROR="${PAKON_UBUNTU_MIRROR:-}" \
        -t "pakon-deb-build:$A" "$HERE" >/dev/null
    mkdir -p "$OUT/.cache/cargo-registry-$A"
    exec docker run --rm --platform "linux/$A" -u "$(id -u):$(id -g)" -e HOME=/tmp \
        -e PAKON_RYBBIT_URL -e PAKON_RYBBIT_SITE_ID -e PAKON_RYBBIT_DOMAIN \
        -e PAKON_DEB_ARCH="$A" \
        -v "$OUT/.cache/cargo-registry-$A:/usr/local/cargo/registry" \
        -v "$REPO:$REPO" -w "$REPO" "pakon-deb-build:$A" "$HERE/build-deb.sh" "$VERSION"
fi

ARCH="$(dpkg --print-architecture)"
STAGE="$OUT/stage-$ARCH"
APPDIR="$STAGE/opt/pakon"
# its own target folder: the Mac's (rust/target) holds other platforms' build scripts
TARGET="$OUT/target-$ARCH"
rm -rf "$STAGE" && mkdir -p "$APPDIR" "$OUT"

step "pakon_app ($ARCH)"
(cd "$REPO/rust" && CARGO_TARGET_DIR="$TARGET" cargo build --release --locked -p pakon-app)
install -m755 "$TARGET/release/pakon_app" "$APPDIR/pakon"
strip "$APPDIR/pakon"

step "app files"
# /opt/pakon mirrors the repo layout the service reads (paths.rs find_root):
# web/static, resources, firmware, profiles, oem; as the Mac and Windows builds
mkdir -p "$APPDIR"/{web,resources/f135plus,firmware}
cp -r "$REPO/web/static" "$APPDIR/web/"
cp "$REPO"/resources/*.pakscan "$REPO"/resources/*.pakfw "$APPDIR/resources/"
cp "$REPO"/resources/f135plus/*.pakscan "$APPDIR/resources/f135plus/"
cp "$REPO"/firmware/*.hex "$REPO/firmware/README.md" "$APPDIR/firmware/" 2>/dev/null || true
cp -r "$REPO/profiles" "$REPO/oem" "$APPDIR/"
cp "$REPO/LICENSE" "$REPO/NOTICE" "$APPDIR/"

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
} > "$APPDIR/release.env"

step "launcher, desktop entry, icon, udev rule"
install -Dm755 "$HERE/pakon" "$STAGE/usr/bin/pakon"
install -Dm644 "$HERE/pakon.desktop" "$STAGE/usr/share/applications/pakon.desktop"
install -Dm644 "$REPO/brand/logo.png" "$STAGE/usr/share/pixmaps/pakon.png"
install -Dm644 "$HERE/60-pakon.rules" "$STAGE/usr/lib/udev/rules.d/60-pakon.rules"

step "control"
# the shared libraries the binary links, as package names (GTK, WebKitGTK, glibc)
SHL="$OUT/shlibs-$ARCH"
rm -rf "$SHL" && mkdir -p "$SHL/debian"
printf 'Source: pakon\n\nPackage: pakon\nArchitecture: any\n' > "$SHL/debian/control"
DEPENDS="$(cd "$SHL" && dpkg-shlibdeps -O "$APPDIR/pakon" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
rm -rf "$SHL"
[ -n "$DEPENDS" ] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
echo "Depends: $DEPENDS"
mkdir -p "$STAGE/DEBIAN"
SIZE="$(du -sk "$STAGE" | cut -f1)"
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: pakon
Version: $VERSION
Architecture: $ARCH
Maintainer: Pakon <https://github.com/jorshhh/open-pakon>
Installed-Size: $SIZE
Depends: $DEPENDS
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
# no version in the name: the README links to releases/latest/download/
DEB="$OUT/pakon_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$DEB" >/dev/null
rm -rf "$STAGE"
du -sh "$DEB"
