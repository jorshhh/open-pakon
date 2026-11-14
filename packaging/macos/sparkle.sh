#!/bin/bash
# Prints the folder of the pinned Sparkle release (Sparkle.framework and
# bin/sign_update), downloaded once into packaging/macos/.cache and checked
# against its SHA-256. Used by build.sh and appcast.sh.
set -euo pipefail
VERSION=2.10.1
SHA256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
CACHE="$(cd "$(dirname "$0")" && pwd)/.cache"
DIR="$CACHE/sparkle-$VERSION"
if [ ! -x "$DIR/bin/sign_update" ]; then
    mkdir -p "$CACHE"
    TAR="$CACHE/sparkle-$VERSION.tar.xz"
    curl -fsSL -o "$TAR" "https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz"
    echo "$SHA256  $TAR" | shasum -a 256 -c --quiet >&2 || { rm -f "$TAR"; echo "Sparkle download: wrong SHA-256" >&2; exit 1; }
    rm -rf "$DIR" && mkdir -p "$DIR"
    tar -xJf "$TAR" -C "$DIR"
    rm "$TAR"
fi
echo "$DIR"
