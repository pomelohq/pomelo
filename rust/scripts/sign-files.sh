#!/usr/bin/env bash
# Sign each file with the Sparkle EdDSA key into <file>.sig, the base64 signature the updaters check.
# Needs SIGN_UPDATE (Sparkle's sign_update) and SPARKLE_ED_KEY_FILE.
set -euo pipefail
[ "$#" -gt 0 ] || { echo "usage: $0 <file>..." >&2; exit 2; }
for file in "$@"; do
  # sign_update prints: sparkle:edSignature="<base64>" length="<bytes>"
  signature="$("$SIGN_UPDATE" -f "$SPARKLE_ED_KEY_FILE" "$file" | sed -n 's/.*edSignature="\([^"]*\)".*/\1/p')"
  [ -n "$signature" ] || { echo "could not sign $file" >&2; exit 1; }
  printf '%s\n' "$signature" > "${file}.sig"
  echo "${file}.sig"
done
