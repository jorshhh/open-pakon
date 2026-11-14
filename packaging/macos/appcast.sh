#!/bin/bash
# The Sparkle feed for one published release (.github/workflows/appcast.yml):
#
#   SPARKLE_ED_PRIVATE_KEY=... packaging/macos/appcast.sh VERSION DMG NOTES.md OUT.xml
#
# One item: the universal dmg (Apple silicon and Intel) at the release's
# download URL, EdDSA-signed, with the release notes as Markdown. The feed
# is signed too. The key is the private half of sparkle-public-key
# (generate_keys -x). PAKON_DOWNLOAD_BASE replaces the download URL (a test).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
[ $# -eq 4 ] || { echo "usage: appcast.sh VERSION DMG NOTES.md OUT.xml" >&2; exit 1; }
VERSION="$1" DMG="$2" NOTES="$3" OUT="$4"
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] || { echo "bad version: $VERSION" >&2; exit 1; }
[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ] || { echo "SPARKLE_ED_PRIVATE_KEY is not set" >&2; exit 1; }
BIN="$("$HERE/sparkle.sh")/bin"
# sparkle:edSignature="..." length="..."
ENC="$(printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$BIN/sign_update" --ed-key-file - "$DMG")"
[[ "$ENC" == sparkle:edSignature=* ]] || { echo "sign_update: $ENC" >&2; exit 1; }
URL="${PAKON_DOWNLOAD_BASE:-https://github.com/jorshhh/open-pakon/releases/download}/v$VERSION/$(basename "$DMG")"
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
<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
<description sparkle:format="markdown"><![CDATA[
XML
    sed 's/]]>/]]]]><![CDATA[>/g' "$NOTES"
    cat <<XML
]]></description>
<enclosure url="$URL" type="application/octet-stream" $ENC/>
</item>
</channel>
</rss>
XML
} > "$OUT"
xmllint --noout "$OUT"
printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$BIN/sign_update" --ed-key-file - --disable-signing-warning "$OUT"
