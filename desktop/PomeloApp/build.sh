#!/usr/bin/env bash
# Build the Pomelo native app: Go core → c-archive → Xcode build (SwiftPM package).
# Xcode (not `swift build`) because CodeEditSourceEditor pulls a sub-dep whose asset
# catalog only builds under Xcode's resource pipeline.
# Usage: ./build.sh [run|selftest|selftest-pty]
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
export PRODUCTS="$here/.ddata/Build/Products/Debug"

echo "==> Building Go core (libpom.a, lean: -tags nostatic)"
GOTOOLCHAIN=local CGO_ENABLED=1 go build -C "$repo" \
    -tags nostatic -trimpath -ldflags="-s -w" \
    -buildmode=c-archive -o "$here/Vendor/libpom.a" ./cmd/libpom/
cp "$here/Vendor/libpom.h" "$here/Sources/CPom/include/libpom.h"

echo "==> xcodebuild"
cd "$here"
# SwiftPM doesn't track the prebuilt libpom.a as an input, so a Go-only change
# won't relink. Force it by removing the linked binary.
rm -f "$PRODUCTS/PomeloApp" 2>/dev/null || true
xcodebuild -scheme PomeloApp -configuration Debug \
    -destination 'platform=macOS,arch=arm64' \
    -derivedDataPath "$here/.ddata" \
    -skipPackagePluginValidation \
    build 2>&1 | grep -vE "was built for newer|ld: warning:|LLVM Profile Error" || true

test -x "$PRODUCTS/PomeloApp" || { echo "build failed: no PomeloApp binary"; exit 1; }

# Assemble a real .app bundle. A bare binary crashes on APIs that require a
# bundle identity (Sparkle, UserNotifications), so the dev build must run as an
# .app — launched via `open` so LaunchServices registers it. Distinct bundle id +
# executable name from the shipped app so the dev build runs ALONGSIDE a released
# Pomelo (dogfooding) without either killing/refocusing the other.
VERSION=$(grep '^const version' "$repo/cmd/pom/root.go" | cut -d'"' -f2)
APP="$PRODUCTS/PomeloDev.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$PRODUCTS/PomeloApp" "$APP/Contents/MacOS/PomeloDev"
cp "$here/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || true
for b in "$PRODUCTS"/*.bundle; do
  [ -e "$b" ] || continue
  cp -R "$b" "$APP/Contents/MacOS/"; cp -R "$b" "$APP/Contents/Resources/"
done
for f in "$PRODUCTS"/*.framework; do
  [ -d "$f" ] && cp -R "$f" "$APP/Contents/Frameworks/"
done
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/PomeloDev" 2>/dev/null || true

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key><string>Pomelo (dev)</string>
	<key>CFBundleExecutable</key><string>PomeloDev</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIdentifier</key><string>com.pomelo.app.dev</string>
	<key>CFBundleName</key><string>Pomelo Dev</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
	<key>SUEnableAutomaticChecks</key><false/>
</dict>
</plist>
PLIST

codesign -s - --force --deep "$APP" >/dev/null 2>&1 || true
echo "Build complete! -> $APP"

case "${1:-}" in
    run)          exec open -n "$APP" ;;
    selftest)     exec "$PRODUCTS/PomeloApp" --selftest ;;
    selftest-pty) exec "$PRODUCTS/PomeloApp" --selftest-pty ;;
esac
