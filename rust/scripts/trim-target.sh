#!/usr/bin/env bash
# Runs before dev builds. A toolchain change makes every cached artifact useless, and the cache otherwise only
# grows (a new copy per flag or dependency change), so the dev cache goes first when the toolchain changed or it
# passed POM_TARGET_LIMIT_GB (default 20). Release builds and the bundled apps are never touched.
set -euo pipefail
cd "$(dirname "$0")/.."

DEBUG="target/debug"
STAMP="target/.toolchain"
CURRENT="$(rustc -V)"
if [ -d "$DEBUG" ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" != "$CURRENT" ]; then
  echo "toolchain changed ($(cat "$STAMP") -> $CURRENT): dropping the old dev build cache" >&2
  rm -rf "$DEBUG"
fi
mkdir -p target
printf '%s\n' "$CURRENT" > "$STAMP"

[ -d "$DEBUG" ] || exit 0
LIMIT_GB="${POM_TARGET_LIMIT_GB:-20}"
SIZE_KB="$(du -sk "$DEBUG" | cut -f1)"
if [ "$SIZE_KB" -gt $((LIMIT_GB * 1024 * 1024)) ]; then
  echo "dev build cache is $((SIZE_KB / 1024 / 1024)) GB (limit ${LIMIT_GB} GB): dropping it before building" >&2
  rm -rf "$DEBUG"
fi
