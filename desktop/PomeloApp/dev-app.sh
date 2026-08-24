#!/usr/bin/env bash
# Assemble a runnable .app for local testing — Debug build, no hardened runtime,
# adhoc-signed. Unlike package.sh's release .app, this loads fine with a plain
# adhoc identity (hardened runtime + adhoc = dyld library-validation refuses to
# load Sparkle.framework into the app process). Never notarize/ship this.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
DIST="$here/dist-dev"
APP="$DIST/Pomelo.app"
PRODUCTS="$here/.ddata/Build/Products/Debug"

echo "==> Go core (libpom.a)"
GOTOOLCHAIN=local CGO_ENABLED=1 go build -C "$repo" \
    -tags nostatic -trimpath -ldflags="-s -w" \
    -buildmode=c-archive -o "$here/Vendor/libpom.a" ./cmd/libpom/
cp "$here/Vendor/libpom.h" "$here/Sources/CPom/include/libpom.h"

echo "==> xcodebuild -configuration Debug"
cd "$here"
rm -f "$PRODUCTS/PomeloApp" 2>/dev/null || true   # force relink (libpom.a isn't a tracked input)
xcodebuild -scheme PomeloApp -configuration Debug \
    -destination 'platform=macOS,arch=arm64' \
    -derivedDataPath "$here/.ddata" \
    -skipPackagePluginValidation \
    build 2>&1 | grep -vE "was built for newer|ld: warning:|LLVM Profile Error" || true
BIN="$PRODUCTS/PomeloApp"
test -x "$BIN" || { echo "build failed: no PomeloApp binary"; exit 1; }

echo "==> assemble Pomelo Dev.app"
rm -rf "$DIST"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN" "$APP/Contents/MacOS/Pomelo"
cp "$here/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || true

for b in "$PRODUCTS"/*.bundle; do
  [ -e "$b" ] || continue
  cp -R "$b" "$APP/Contents/MacOS/"; cp -R "$b" "$APP/Contents/Resources/"
done

cp -R "$PRODUCTS/Sparkle.framework" "$APP/Contents/Frameworks/"
for f in "$PRODUCTS"/*.framework; do
  case "$f" in *Sparkle.framework) continue;; esac
  [ -d "$f" ] && cp -R "$f" "$APP/Contents/Frameworks/"
done
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/Pomelo" 2>/dev/null || true

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key><string>Pomelo Dev</string>
	<key>CFBundleExecutable</key><string>Pomelo</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIdentifier</key><string>com.pomelo.app.dev</string>
	<key>CFBundleName</key><string>Pomelo Dev</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>0.0.0-dev</string>
	<key>CFBundleVersion</key><string>0.0.0-dev</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "==> adhoc sign (no hardened runtime, so Sparkle loads locally)"
for b in "$APP/Contents/MacOS/"*.bundle "$APP/Contents/Resources/"*.bundle; do
  [ -e "$b" ] && codesign --force --sign - "$b"
done
for f in "$APP/Contents/Frameworks/"*.framework; do
  [ -d "$f" ] && codesign --force --sign - "$f"
done
codesign --force --sign - "$APP/Contents/MacOS/Pomelo"
codesign --force --sign - "$APP"
codesign --verify --deep --strict "$APP"

echo "==> open $APP"
open "$APP"
