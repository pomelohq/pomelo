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

hdiutil create -volname "Pomelo" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
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
