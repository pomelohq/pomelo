#!/usr/bin/env bash
# CI only. Cargo rebuilds a crate when a source is newer than its last build, and a checkout makes every file
# new: unchanged files get an old time, files changed since the cached build's commit the current one.
set -euo pipefail
cd "$(dirname "$0")/.."

git ls-files -z | xargs -0 touch -t 200001010000
built="$(cat target/.built-from 2>/dev/null || true)"
if [ -n "$built" ] && git cat-file -e "$built^{commit}" 2>/dev/null; then
  echo "cached build is from $built"
  git diff --name-only --relative "$built" HEAD | while IFS= read -r file; do
    if [ -e "$file" ]; then touch "$file"; fi
  done
else
  echo "no usable cached build: everything is new"
  git ls-files -z | xargs -0 touch
fi
