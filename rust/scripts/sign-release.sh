#!/usr/bin/env bash
# Signs a release's downloads with the Sparkle EdDSA key (never regenerated; the installed apps trust it):
#   - Pomelo-<v>.app.tar.gz.sig  the in-app updater checks it before swapping the app
#   - appcast.xml                the feed apps from before the rewrite poll, pointing them at the new DMG
# Needs SIGN_UPDATE (Sparkle's sign_update) and SPARKLE_ED_KEY_FILE; RELEASE_NOTES is optional.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DMG="target/Pomelo-${VERSION}.dmg"
TARBALL="target/Pomelo-${VERSION}.app.tar.gz"
REPO="${RELEASE_REPO:-pomelohq/pomelo}"
for file in "$DMG" "$TARBALL"; do
  [ -f "$file" ] || { echo "missing $file" >&2; exit 1; }
done

# sign_update prints: sparkle:edSignature="<base64>" length="<bytes>"
TARBALL_SIG="$("$SIGN_UPDATE" -f "$SPARKLE_ED_KEY_FILE" "$TARBALL" | sed -n 's/.*edSignature="\([^"]*\)".*/\1/p')"
[ -n "$TARBALL_SIG" ] || { echo "could not sign $TARBALL" >&2; exit 1; }
printf '%s\n' "$TARBALL_SIG" > "${TARBALL}.sig"

DMG_SIG="$("$SIGN_UPDATE" -f "$SPARKLE_ED_KEY_FILE" "$DMG")"
DMG_URL="https://github.com/${REPO}/releases/download/v${VERSION}/Pomelo-${VERSION}.dmg"
DESC="<p>Pomelo ${VERSION}</p>"
if [ -n "${RELEASE_NOTES:-}" ]; then
  ITEMS=$(printf '%s\n' "$RELEASE_NOTES" | sed -n 's/^[-*][[:space:]][[:space:]]*\(.*\)/<li>\1<\/li>/p' | tr -d '\n')
  [ -n "$ITEMS" ] && DESC="<h3>What's new in ${VERSION}</h3><ul>${ITEMS}</ul>"
fi
cat > target/appcast.xml <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>Pomelo</title>
    <item>
      <title>${VERSION}</title>
      <sparkle:version>${VERSION}</sparkle:version>
      <sparkle:shortVersionString>${VERSION}</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
      <description><![CDATA[${DESC}]]></description>
      <enclosure url="${DMG_URL}" ${DMG_SIG} type="application/octet-stream" />
    </item>
  </channel>
</rss>
XML
echo "${TARBALL}.sig"
echo "target/appcast.xml"
