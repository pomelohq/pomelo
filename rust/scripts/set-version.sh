#!/usr/bin/env bash
# Stamp the workspace version a build carries (a nightly's `<x.y.z>-nightly.<date>`); nothing is committed.
set -euo pipefail
cd "$(dirname "$0")/.."

NEW="${1:?usage: $0 <version>}"
if ! printf '%s' "$NEW" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
  echo "not a version: $NEW" >&2
  exit 2
fi
CUR="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ "$CUR" = "$NEW" ] && exit 0
sed "s/^version = \"${CUR}\"/version = \"${NEW}\"/" Cargo.toml > Cargo.toml.new
mv Cargo.toml.new Cargo.toml
echo "version $CUR -> $NEW"
