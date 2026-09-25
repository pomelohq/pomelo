#!/usr/bin/env bash
# Assemble a macOS .app bundle (Apple Silicon only). Flavor decides name + bundle id so the dev build
# (PomeloDev.app) installs alongside the release build (Pomelo.app). With SIGN_ID set (a Developer ID
# identity in the keychain) the binaries and the bundle are signed with the hardened runtime.
set -euo pipefail
cd "$(dirname "$0")/.."

FLAVOR="${1:-dev}"
case "$FLAVOR" in
  dev)
    cargo build -p pomelo -p pom_cli
    BIN="target/debug/pomelo"
    APP_NAME="PomeloDev"
    DISPLAY_NAME="Pomelo Dev"
    BUNDLE_ID="app.pomelo.dev"
    ;;
  prod)
    cargo build -p pomelo -p pom_cli --release
    BIN="target/release/pomelo"
    APP_NAME="Pomelo"
    DISPLAY_NAME="Pomelo"
    BUNDLE_ID="app.pomelo"
    ;;
  *)
    echo "usage: $0 <dev|prod>" >&2
    exit 2
    ;;
esac

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/${APP_NAME}.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/pomelo"
cp "$(dirname "$BIN")/pom" "$APP/Contents/MacOS/pom"
cp crates/pomelo/assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${DISPLAY_NAME}</string>
  <key>CFBundleDisplayName</key><string>${DISPLAY_NAME}</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>pomelo</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

if [ -n "${SIGN_ID:-}" ]; then
  # Inner executables first: signing the bundle does not re-sign what it contains.
  for BIN in "$APP/Contents/MacOS/pom" "$APP/Contents/MacOS/pomelo"; do
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$BIN" >&2
  done
  codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP" >&2
  codesign --verify --strict --verbose=2 "$APP" >&2
fi
touch "$APP"
echo "$APP"
