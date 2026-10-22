#!/bin/sh
# Install Pomelo for this user from an unpacked tarball: the app, the pom CLI, the icon and the desktop entry.
# --cli-only (or a tarball without the app) installs pom alone, for a server without a desktop.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
icon="share/icons/hicolor/512x512/apps/pomelo.png"
entry="share/applications/pomelo.desktop"

cli_only=false
for arg in "$@"; do
  case "$arg" in
    --cli-only) cli_only=true ;;
    *) echo "usage: $0 [--cli-only]" >&2; exit 2 ;;
  esac
done
[ -f "$here/bin/pomelo" ] || cli_only=true

install -Dm755 "$here/bin/pom" "$prefix/bin/pom"
if [ "$cli_only" = true ]; then
  echo "pom installed under $prefix/bin"
  case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "add $prefix/bin to your PATH to run pom" ;;
  esac
  exit 0
fi

install -Dm755 "$here/bin/pomelo" "$prefix/bin/pomelo"
install -Dm644 "$here/$icon" "$prefix/$icon"
mkdir -p "$prefix/share/applications"
# The desktop shell's PATH often lacks ~/.local/bin, and it drops an entry whose Exec it cannot find, icon and all.
sed "s|^Exec=.*|Exec=$prefix/bin/pomelo|; s|^TryExec=.*|TryExec=$prefix/bin/pomelo|" \
  "$here/$entry" >"$prefix/$entry"
chmod 644 "$prefix/$entry"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$prefix/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
fi
echo "Pomelo installed under $prefix"
