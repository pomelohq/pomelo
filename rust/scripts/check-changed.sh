#!/usr/bin/env bash
# The gate for what a branch touches: fmt and clippy on everything (unchanged crates are already checked, so
# that costs little), the tests of the changed crates and every crate that depends on them, and the
# workspace-hack check when a Cargo file changed. `make check` is still the gate before a commit.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! cargo nextest --version >/dev/null 2>&1; then
  echo "check-changed needs cargo-nextest: cargo install cargo-nextest (or brew install cargo-nextest)" >&2
  exit 1
fi

base="${POM_CHECK_BASE:-origin/main}"
fork="$(git merge-base "$base" HEAD)"
plan="$(scripts/changed-crates.sh "$fork" --worktree)"
value() { sed -n "s/^$1=//p" <<<"$plan"; }

cargo fmt --check
if [ "$(value run_tests)" != true ]; then
  echo "no Rust change since $base"
  exit 0
fi
if git diff --name-only "$fork" -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml' | grep -q . \
  || git ls-files --others --exclude-standard -- 'crates/*/Cargo.toml' | grep -q .; then
  make --no-print-directory hakari-check
fi
cargo clippy --all-targets -- -D warnings

filterset="$(value filterset)"
if [ -n "$filterset" ]; then
  echo "tests: $filterset"
  cargo nextest run --workspace --no-tests=pass -E "$filterset"
else
  echo "tests: everything"
  cargo nextest run --workspace
fi
cargo test --doc --workspace
