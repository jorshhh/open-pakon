#!/bin/bash
# Prints the path of the pinned WinSparkle release's 64-bit WinSparkle.dll
# (https://winsparkle.org, the Windows port of Sparkle; pakon-app
# winsparkle.rs loads it for in-place updates), downloaded once into CACHE
# and checked against the release zip's SHA-256. Used by build.sh:
#
#   packaging/windows/winsparkle.sh CACHE
#
# The DLL links the C runtime statically and imports only Windows' own DLLs.
set -euo pipefail
VERSION=0.9.4
SHA256=6037df37fc263bd1650a1c4949681a9d40ffe991d01f35892a406cb5d103c976
[ $# -eq 1 ] || { echo "usage: winsparkle.sh CACHE" >&2; exit 1; }
DIR="$1/winsparkle-$VERSION"
DLL="$DIR/WinSparkle.dll"
if [ ! -f "$DLL" ]; then
    mkdir -p "$DIR"
    ZIP="$DIR/WinSparkle-$VERSION.zip"
    curl -fsSL -o "$ZIP" "https://github.com/vslavik/winsparkle/releases/download/v$VERSION/WinSparkle-$VERSION.zip"
    python3 - "$ZIP" "$SHA256" "WinSparkle-$VERSION/x64/Release/WinSparkle.dll" "$DLL" <<'PY' || { rm -f "$ZIP"; exit 1; }
import hashlib, sys, zipfile
zip_path, want, member, out = sys.argv[1:]
got = hashlib.sha256(open(zip_path, "rb").read()).hexdigest()
if got != want:
    sys.exit(f"WinSparkle download: wrong SHA-256 ({got})")
with zipfile.ZipFile(zip_path) as z, open(out, "wb") as f:
    f.write(z.read(member))
PY
    rm "$ZIP"
fi
echo "$DLL"
