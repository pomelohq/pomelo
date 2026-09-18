#!/usr/bin/env bash
# Assemble a macOS .app bundle (Apple Silicon only). Flavor decides name + bundle id so the dev build
# (PomeloDev.app) installs alongside the release build (Pomelo.app). Unsigned for now; signing hooks are
# marked with TODO(sign).
set -euo pipefail
cd "$(dirname "$0")/.."

FLAVOR="${1:-dev}"
case "$FLAVOR" in
  dev)
    cargo build -p pomelo
    BIN="target/debug/pomelo"
    APP_NAME="PomeloDev"
    DISPLAY_NAME="Pomelo Dev"
    BUNDLE_ID="app.pomelo.dev"
    ;;
  prod)
    cargo build -p pomelo --release
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

# TODO(sign): codesign --deep --options runtime --sign "Developer ID Application: ..." "$APP"; then notarize.
touch "$APP"
echo "$APP"
