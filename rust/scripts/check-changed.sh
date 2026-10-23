#!/usr/bin/env bash
# The gate for the crates a change touches: fmt on everything, then clippy and tests on the changed crates and
# every crate that depends on them. `make check` is still the gate before a commit.
set -euo pipefail
cd "$(dirname "$0")/.."

base="${POM_CHECK_BASE:-origin/main}"
fork="$(git merge-base "$base" HEAD)"
changed="$( { git diff --name-only --relative "$fork"; git ls-files --others --exclude-standard; } | sort -u)"

if ! cargo nextest --version >/dev/null 2>&1; then
  echo "check-changed needs cargo-nextest: cargo install cargo-nextest (or brew install cargo-nextest)" >&2
  exit 1
fi

cargo fmt --check

if [ -z "$changed" ]; then
  echo "nothing changed since $base"
  exit 0
fi

metadata="$(cargo metadata --format-version 1 --no-deps)"
workspace_root="$(jq -r '.workspace_root' <<<"$metadata")"

# A file outside every crate (the root manifest, the lockfile, the toolchain, test or lint config) can change any crate.
everything=0
packages=()
while IFS= read -r file; do
  package="$(jq -r --arg file "$workspace_root/$file" '
    [.packages[] | (.manifest_path | rtrimstr("/Cargo.toml")) as $dir
      | select($file | startswith($dir + "/")) | {name, length: ($dir | length)}]
    | max_by(.length) | .name // empty' <<<"$metadata")"
  if [ -z "$package" ]; then
    case "$file" in
      docs/* | *.md | scripts/*) ;;
      *) everything=1 ;;
    esac
  else
    packages+=("$package")
  fi
done <<<"$changed"

if grep -qE '(^|/)Cargo\.(toml|lock)$' <<<"$changed"; then
  make --no-print-directory hakari-check
fi

if [ "$everything" = 1 ]; then
  echo "a workspace-wide file changed: checking every crate"
  cargo clippy --all-targets -- -D warnings
  cargo nextest run --workspace
  cargo test --doc --workspace
  exit 0
fi

if [ "${#packages[@]}" = 0 ]; then
  echo "no crate changed since $base"
  exit 0
fi

affected="$(for package in $(printf '%s\n' "${packages[@]}" | sort -u); do
  cargo tree --workspace --invert "$package" --edges normal,build,dev --prefix none --format '{p}' \
    | grep -F "($workspace_root/" | cut -d' ' -f1
done | sort -u)"
echo "changed: $(printf '%s\n' "${packages[@]}" | sort -u | tr '\n' ' ')"
echo "checking $(wc -l <<<"$affected" | tr -d ' ') crates: $(tr '\n' ' ' <<<"$affected")"

selection=()
for package in $affected; do selection+=(-p "$package"); done
cargo clippy --all-targets "${selection[@]}" -- -D warnings
cargo nextest run --no-tests=pass "${selection[@]}"

libraries=()
for package in $affected; do
  if jq -e --arg name "$package" \
    '.packages[] | select(.name == $name) | .targets[] | select(.kind | index("lib"))' <<<"$metadata" >/dev/null; then
    libraries+=(-p "$package")
  fi
done
if [ "${#libraries[@]}" -gt 0 ]; then
  cargo test --doc "${libraries[@]}"
fi
