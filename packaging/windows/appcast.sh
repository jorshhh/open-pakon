#!/bin/bash
# The WinSparkle feed for one published release (.github/workflows/appcast.yml),
# the Windows counterpart of packaging/macos/appcast.sh:
#
#   SPARKLE_ED_PRIVATE_KEY=... packaging/windows/appcast.sh VERSION SETUP.exe NOTES.md OUT.xml
#
# One item: Pakon-Setup.exe at the release's download URL, EdDSA-signed
# with the Mac feed's key (Sparkle's sign_update, so it runs on a Mac),
# for 64-bit Windows, run as `/S /UPDATE` (installer.nsi: silent, waits for
# the app to quit, starts the new one), with the release notes as Markdown.
# PAKON_DOWNLOAD_BASE replaces the download URL (a test).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
[ $# -eq 4 ] || { echo "usage: appcast.sh VERSION SETUP.exe NOTES.md OUT.xml" >&2; exit 1; }
VERSION="$1" SETUP="$2" NOTES="$3" OUT="$4"
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] || { echo "bad version: $VERSION" >&2; exit 1; }
[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ] || { echo "SPARKLE_ED_PRIVATE_KEY is not set" >&2; exit 1; }
BIN="$("$HERE/../macos/sparkle.sh")/bin"
# sparkle:edSignature="..." length="..."
ENC="$(printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$BIN/sign_update" --ed-key-file - "$SETUP")"
[[ "$ENC" == sparkle:edSignature=* ]] || { echo "sign_update: $ENC" >&2; exit 1; }
URL="${PAKON_DOWNLOAD_BASE:-https://github.com/jorshhh/open-pakon/releases/download}/v$VERSION/$(basename "$SETUP")"
DATE="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')"
{
    cat <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel>
<title>Pakon</title>
<item>
<title>Pakon $VERSION</title>
<pubDate>$DATE</pubDate>
<sparkle:version>$VERSION</sparkle:version>
<sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
<description sparkle:format="markdown"><![CDATA[
XML
    sed 's/]]>/]]]]><![CDATA[>/g' "$NOTES"
    cat <<XML
]]></description>
<enclosure url="$URL" type="application/octet-stream" sparkle:os="windows-x64" sparkle:installerArguments="/S /UPDATE" $ENC/>
</item>
</channel>
</rss>
XML
} > "$OUT"
xmllint --noout "$OUT"
