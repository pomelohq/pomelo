#!/usr/bin/env bash
# Build + bundle the dev flavor (PomeloDev.app) and launch it, killing any previous instance of the same
# flavor first so re-running never leaves a stale window around.
set -euo pipefail
cd "$(dirname "$0")/.."
FLAVOR="${1:-dev}"
APP="$(scripts/bundle-macos.sh "$FLAVOR")"
NAME="$(basename "$APP")" # PomeloDev.app or Pomelo.app
pkill -f "$NAME/Contents/MacOS/pomelo" 2>/dev/null || true
sleep 0.3
open "$APP"
echo "launched $APP"
