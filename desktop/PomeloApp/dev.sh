#!/usr/bin/env bash
# Deploy a DEV build as "Pomelo Dev.app" (com.pomelo.app.dev) so it never clashes
# with the installed final Pomelo. Assembles the debug binary + Sparkle.framework,
# ad-hoc signs, and launches.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
"$here/build.sh" >/dev/null 2>&1
PRODUCTS="$here/.ddata/Build/Products/Debug"
BIN="$PRODUCTS/PomeloApp"
FW="$PRODUCTS/Sparkle.framework"
APP="/tmp/Pomelo Dev.app"
# Kill only this dev build by its path; both apps share the executable name
# "Pomelo", so `pkill -x Pomelo` would also nuke the installed production app.
pkill -9 -f "$APP/Contents/MacOS/Pomelo" 2>/dev/null || true; sleep 0.5
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN" "$APP/Contents/MacOS/Pomelo"
cp -R "$FW" "$APP/Contents/Frameworks/"
# SwiftPM resource bundles (Bundle.module) must sit next to the executable AND in
# Resources — the generated accessor checks both. Frameworks (tree-sitter container)
# go in Frameworks/ (rpath already points there).
for b in "$PRODUCTS"/*.bundle; do
  [ -e "$b" ] || continue
  cp -R "$b" "$APP/Contents/MacOS/"; cp -R "$b" "$APP/Contents/Resources/"
done
for f in "$PRODUCTS"/*.framework; do
  case "$f" in *Sparkle.framework) continue;; esac
  [ -d "$f" ] && cp -R "$f" "$APP/Contents/Frameworks/"
done
cp "$here/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
V=$(grep '^const version' "$here/../../cmd/pom/root.go" | cut -d'"' -f2)
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDisplayName</key><string>Pomelo Dev</string>
<key>CFBundleExecutable</key><string>Pomelo</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleIdentifier</key><string>com.pomelo.app.dev</string>
<key>CFBundleName</key><string>Pomelo Dev</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>${V}-dev</string>
<key>CFBundleVersion</key><string>${V}-dev</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PL
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/Pomelo" 2>/dev/null || true
SF="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
for h in "$SF/XPCServices/"*.xpc "$SF/Autoupdate" "$SF/Updater.app" "$APP/Contents/Frameworks/Sparkle.framework"; do
  [ -e "$h" ] && codesign --force -s - "$h" >/dev/null 2>&1
done
# Ad-hoc sign every other bundled framework (e.g. the tree-sitter container).
for f in "$APP/Contents/Frameworks/"*.framework; do
  case "$f" in *Sparkle.framework) continue;; esac
  codesign --force -s - "$f" >/dev/null 2>&1
done
# Sign nested resource bundles too, else signing the main binary fails on them.
for b in "$APP/Contents/MacOS/"*.bundle "$APP/Contents/Resources/"*.bundle; do
  [ -e "$b" ] && codesign --force -s - "$b" >/dev/null 2>&1
done
codesign --force -s - "$APP/Contents/MacOS/Pomelo" >/dev/null 2>&1
codesign --force -s - "$APP" >/dev/null 2>&1
open "$APP"
echo "launched: Pomelo Dev ($V-dev)"
