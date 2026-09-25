#!/usr/bin/env bash
# Bump the workspace version (patch|minor|major), commit, tag v<version> and push. Pushing the tag is what
# triggers the release workflow, so this runs on an up-to-date main only.
set -euo pipefail
cd "$(dirname "$0")/.."

KIND="${1:-patch}"
CUR="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
IFS=. read -r MA MI PA <<<"$CUR"

case "$KIND" in
  patch) PA=$((PA + 1)) ;;
  minor) MI=$((MI + 1)); PA=0 ;;
  major) MA=$((MA + 1)); MI=0; PA=0 ;;
  *) echo "usage: $0 <patch|minor|major>" >&2; exit 2 ;;
esac
NEW="${MA}.${MI}.${PA}"

if [ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then
  echo "release from main only" >&2
  exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
  echo "working tree not clean; commit or stash first" >&2
  exit 1
fi

sed -i '' "s/^version = \"${CUR}\"/version = \"${NEW}\"/" Cargo.toml
cargo update --workspace >/dev/null 2>&1

git add Cargo.toml Cargo.lock
git commit -m "release: v${NEW}"
git tag "v${NEW}"
git push origin main "v${NEW}"
echo "v${NEW} tagged and pushed; the Release workflow builds and publishes it:"
echo "  gh run watch --repo pomelohq/pomelo"
