#!/usr/bin/env bash
# Package the native Pomelo.app into a signed + notarized + stapled DMG.
# Version comes from cmd/pom/root.go. Requires a Developer ID identity and a
# notarytool keychain profile (set once: xcrun notarytool store-credentials pomelo-notary …).
# Set DRY_RUN=1 to build + sign the .app + DMG but skip notarization.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
VERSION=$(grep '^const version' "$repo/cmd/pom/root.go" | cut -d'"' -f2)
SIGN_ID="${SIGN_ID:--}"                 # "-" = adhoc; set a Developer ID for release
NOTARY_PROFILE="${NOTARY_PROFILE:-pomelo-notary}"
DIST="$here/dist"
APP="$DIST/Pomelo.app"
DMG="$DIST/Pomelo-$VERSION.dmg"

# Adhoc builds can't be notarized — build + sign only (same as DRY_RUN).
if [ "$SIGN_ID" = "-" ]; then
  echo "==> adhoc signing (SIGN_ID=-) — skipping notarization"
  DRY_RUN=1
fi

echo "==> Pomelo $VERSION → DMG"
rm -rf "$DIST"; mkdir -p "$DIST"

echo "==> Go core (libpom.a)"
GOTOOLCHAIN=local CGO_ENABLED=1 go build -C "$repo" \
    -tags nostatic -trimpath -ldflags="-s -w" \
    -buildmode=c-archive -o "$here/Vendor/libpom.a" ./cmd/libpom/
cp "$here/Vendor/libpom.h" "$here/Sources/CPom/include/libpom.h"

# Xcode (not `swift build`): CodeEditSourceEditor pulls a sub-dep whose asset
# catalog only builds under Xcode's resource pipeline. Release config = no profiling.
echo "==> xcodebuild -configuration Release"
cd "$here"
PRODUCTS="$here/.ddata/Build/Products/Release"
rm -f "$PRODUCTS/PomeloApp" 2>/dev/null || true   # force relink (libpom.a isn't a tracked input)
xcodebuild -scheme PomeloApp -configuration Release \
    -destination 'platform=macOS,arch=arm64' \
    -derivedDataPath "$here/.ddata" \
    -skipPackagePluginValidation \
    ENABLE_TESTABILITY=NO SWIFT_ENABLE_TESTABILITY=NO CLANG_ENABLE_CODE_COVERAGE=NO \
    build 2>&1 | grep -vE "was built for newer|ld: warning:|LLVM Profile Error" || true
BIN="$PRODUCTS/PomeloApp"
test -x "$BIN" || { echo "build failed: no PomeloApp binary"; exit 1; }

echo "==> assemble Pomelo.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN" "$APP/Contents/MacOS/Pomelo"
cp "$here/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

# SwiftPM resource bundles (Bundle.module) sit next to the executable AND in Resources.
for b in "$PRODUCTS"/*.bundle; do
  [ -e "$b" ] || continue
  cp -R "$b" "$APP/Contents/MacOS/"; cp -R "$b" "$APP/Contents/Resources/"
done

# Sparkle (in-app auto-update) + any other bundled framework (tree-sitter container).
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
	<key>CFBundleDisplayName</key><string>Pomelo</string>
	<key>CFBundleExecutable</key><string>Pomelo</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIdentifier</key><string>com.pomelo.app</string>
	<key>CFBundleName</key><string>Pomelo</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
	<key>SUFeedURL</key><string>https://github.com/pomelohq/pomelo/releases/latest/download/appcast.xml</string>
	<key>SUPublicEDKey</key><string>tGnmpupAySzHVMfQcDqtlMFoxuSLC9Pl6TtF4DmGECY=</string>
	<key>SUEnableAutomaticChecks</key><true/>
	<key>SUScheduledCheckInterval</key><integer>86400</integer>
</dict>
</plist>
PLIST

# Strip local/debug symbols (~30MB of __LINKEDIT) BEFORE codesign — strip mutates
# the binary and would invalidate a signature. -x keeps external symbols (the
# libpom c-archive exports Swift links against), drops only local ones.
echo "==> strip -x"
strip -x "$APP/Contents/MacOS/Pomelo"

echo "==> codesign (hardened runtime; inside-out, then app)"
SF="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
for h in "$SF/XPCServices/"*.xpc "$SF/Autoupdate" "$SF/Updater.app" "$APP/Contents/Frameworks/Sparkle.framework"; do
  [ -e "$h" ] && codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$h"
done
# Other bundled frameworks + nested resource bundles must be signed before the binary.
for f in "$APP/Contents/Frameworks/"*.framework; do
  case "$f" in *Sparkle.framework) continue;; esac
  [ -d "$f" ] && codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$f"
done
for b in "$APP/Contents/MacOS/"*.bundle "$APP/Contents/Resources/"*.bundle; do
  [ -e "$b" ] && codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$b"
done
codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/MacOS/Pomelo"
codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP"
codesign --verify --strict --verbose=2 "$APP"

if [ "${DRY_RUN:-0}" != "1" ]; then
  echo "==> notarize .app (submit + wait)"
  ditto -c -k --keepParent "$APP" "$DIST/Pomelo.zip"
  xcrun notarytool submit "$DIST/Pomelo.zip" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  rm -f "$DIST/Pomelo.zip"
fi

echo "==> build DMG"
STAGE="$DIST/stage"; rm -rf "$STAGE"; mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/Pomelo.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Pomelo" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force --sign "$SIGN_ID" --timestamp "$DMG"

if [ "${DRY_RUN:-0}" != "1" ]; then
  echo "==> notarize DMG (submit + wait)"
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
  echo "==> verify"
  spctl -a -t open --context context:primary-signature -vv "$DMG" || true
fi

# Sparkle appcast: sign the DMG (EdDSA, private key from keychain) → appcast.xml.
# Upload dist/appcast.xml as an asset on the release SUFeedURL points at
# (/releases/latest/download/appcast.xml → 302 no-cache, unlike raw.git which
# CDN-caches for 5min and ignores query strings — the update-lag we hit).
echo "==> appcast (Sparkle)"
SIGN_UPDATE="$here/.ddata/SourcePackages/artifacts/sparkle/Sparkle/bin/sign_update"
# Local: EdDSA private key lives in the keychain. CI: pass it via SPARKLE_ED_KEY_FILE.
if [ -n "${SPARKLE_ED_KEY_FILE:-}" ]; then
  SIG=$("$SIGN_UPDATE" -f "$SPARKLE_ED_KEY_FILE" "$DMG" 2>/dev/null)
else
  SIG=$("$SIGN_UPDATE" "$DMG" 2>/dev/null)   # → sparkle:edSignature="…" length="…"
fi
DMG_URL="https://github.com/pomelohq/pomelo/releases/download/v$VERSION/Pomelo-$VERSION.dmg"

# Sparkle shows <description> (HTML) in the update dialog. Render RELEASE_NOTES
# (optional; CI leaves it unset and uses --generate-notes) as a bullet list, else generic.
DESC="<p>Pomelo $VERSION</p>"
if [ -n "${RELEASE_NOTES:-}" ]; then
  LIS=$(printf '%s\n' "$RELEASE_NOTES" | sed -n 's/^[-*][[:space:]][[:space:]]*\(.*\)/<li>\1<\/li>/p' | tr -d '\n')
  if [ -n "$LIS" ]; then DESC="<h3>What's new in $VERSION</h3><ul>$LIS</ul>"; else DESC="<p>$RELEASE_NOTES</p>"; fi
fi
cat > "$DIST/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>Pomelo</title>
    <item>
      <title>$VERSION</title>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
      <description><![CDATA[$DESC]]></description>
      <enclosure url="$DMG_URL" $SIG type="application/octet-stream" />
    </item>
  </channel>
</rss>
XML
echo ">> appcast written: $DIST/appcast.xml (upload as a release asset: gh release upload vX $DIST/appcast.xml)"

echo "==> done: $DMG"
