#!/usr/bin/env bash
# Assemble a Linux tarball: the app and the pom CLI under bin/, the desktop entry and icon under share/, and
# install.sh, which copies them into ~/.local (or $PREFIX) and points the desktop entry at the installed app.
set -euo pipefail
cd "$(dirname "$0")/.."

FLAVOR="${1:-prod}"
case "$FLAVOR" in
  dev)
    cargo build -p pomelo -p pom_cli
    BIN_DIR="target/debug"
    ;;
  prod)
    cargo build -p pomelo -p pom_cli --release
    BIN_DIR="target/release"
    ;;
  *)
    echo "usage: $0 <dev|prod>" >&2
    exit 2
    ;;
esac

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(uname -m)"
STAGE="target/linux-stage/pomelo-${VERSION}"
rm -rf "target/linux-stage"
install -Dm755 "$BIN_DIR/pomelo" "$STAGE/bin/pomelo"
install -Dm755 "$BIN_DIR/pom" "$STAGE/bin/pom"
install -Dm644 crates/pomelo/assets/pomelo.desktop "$STAGE/share/applications/pomelo.desktop"
install -Dm644 crates/pomelo/assets/pomelo.png "$STAGE/share/icons/hicolor/512x512/apps/pomelo.png"
install -Dm755 scripts/install-linux.sh "$STAGE/install.sh"

TARBALL="target/Pomelo-${VERSION}-linux-${ARCH}.tar.gz"
tar -C "target/linux-stage" -czf "$TARBALL" "pomelo-${VERSION}"
echo "$TARBALL"
