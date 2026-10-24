#!/usr/bin/env bash
# Builds the gallery into "Pomelo Gallery.app" and runs it from there, so the Dock, the app
# switcher and Mission Control show its name and icon instead of a bare binary's.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p gallery

APP="target/release/Pomelo Gallery.app"
ICON=crates/gallery/assets/gallery-icon.png
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/gallery "$APP/Contents/MacOS/gallery"

if [[ ! -f "$APP/Contents/Resources/AppIcon.icns" || "$ICON" -nt "$APP/Contents/Resources/AppIcon.icns" ]]; then
  ICONSET="$(mktemp -d)/AppIcon.iconset"
  mkdir -p "$ICONSET"
  for size in 16 32 128 256; do
    sips -z "$size" "$size" "$ICON" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$ICON" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  cp "$ICON" "$ICONSET/icon_512x512.png"
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  rm -r "$(dirname "$ICONSET")"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Pomelo Gallery</string>
  <key>CFBundleDisplayName</key><string>Pomelo Gallery</string>
  <key>CFBundleIdentifier</key><string>com.pomelo.gallery</string>
  <key>CFBundleExecutable</key><string>gallery</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

exec "$APP/Contents/MacOS/gallery"
