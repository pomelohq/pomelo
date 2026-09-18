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
echo "$DMG"
