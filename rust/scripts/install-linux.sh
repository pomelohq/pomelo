#!/bin/sh
# Install Pomelo for this user from an unpacked tarball: the app, the pom CLI, the icon and the desktop entry.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
icon="share/icons/hicolor/512x512/apps/pomelo.png"
entry="share/applications/pomelo.desktop"

install -Dm755 "$here/bin/pomelo" "$prefix/bin/pomelo"
install -Dm755 "$here/bin/pom" "$prefix/bin/pom"
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
