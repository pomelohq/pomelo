#!/usr/bin/env bash
# Build the prod bundle (Pomelo.app) and package it into a compressed DMG for distribution.
set -euo pipefail
cd "$(dirname "$0")/.."

APP="$(scripts/bundle-macos.sh prod)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DMG="target/Pomelo-${VERSION}.dmg"
STAGE="target/dmg-stage"

rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

# CI runners sometimes report "Resource busy" while another process still holds the volume; it passes on retry.
for attempt in 1 2 3 4 5; do
  if hdiutil create -volname "Pomelo" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 5 ]; then
    echo "hdiutil create failed $attempt times" >&2
    exit 1
  fi
  echo "hdiutil create failed (attempt $attempt), retrying" >&2
  sleep $((attempt * 5))
done
rm -rf "$STAGE"

if [ -n "${SIGN_ID:-}" ]; then
  codesign --force --timestamp --sign "$SIGN_ID" "$DMG" >&2
fi
if [ -n "${NOTARY_PROFILE:-}" ]; then
  # notarytool exits 0 even when Apple rejects the upload, so read the verdict.
  RESULT="$(xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1)"
  echo "$RESULT" >&2
  echo "$RESULT" | grep -q "status: Accepted" || { echo "notarization failed" >&2; exit 1; }
  xcrun stapler staple "$DMG" >&2
  # The ticket covers the app inside too; staple the bundle the update tarball is made from.
  xcrun stapler staple "$APP" >&2
fi
echo "$DMG"
