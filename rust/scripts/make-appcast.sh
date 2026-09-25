#!/usr/bin/env bash
# Produce the auto-update artifacts for a release: a tarball of Pomelo.app (what the in-app updater
# downloads and swaps in) and a latest.json manifest. Run after make-dmg / bundle prod.
set -euo pipefail
cd "$(dirname "$0")/.."

APP="target/Pomelo.app"
[ -d "$APP" ] || APP="$(scripts/bundle-macos.sh prod)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARBALL="target/Pomelo-${VERSION}.app.tar.gz"

tar -C target -czf "$TARBALL" "$(basename "$APP")"

cat > target/latest.json <<JSON
{
  "version": "${VERSION}",
  "arch": "aarch64",
  "asset": "$(basename "$TARBALL")",
  "min_os": "14.0"
}
JSON

echo "$TARBALL"
echo "target/latest.json"
