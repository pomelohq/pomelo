#!/usr/bin/env bash
# What a change since <compare-rev> needs checked, as key=value lines (the format of $GITHUB_OUTPUT):
#   run_tests          any Rust or workflow change
#   run_action_checks  a workflow change
#   filterset          a nextest filterset of the changed crates and their dependents; empty runs every test
#   affected           those crates, space-separated and padded; empty when every crate is affected
# --worktree also counts uncommitted and untracked files. Used by CI and by `make check-changed`.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$(git rev-parse --show-toplevel)"

compare="${1:?usage: changed-crates.sh <compare-rev> [--worktree]}"
if [ "${2:-}" = "--worktree" ]; then
  changed="$( { git -C "$repo" diff --name-only "$compare"; git -C "$repo" ls-files --others --exclude-standard; } | sort -u)"
else
  changed="$(git -C "$repo" diff --name-only "$compare" HEAD)"
fi

flag() {
  if grep -qE "$2" <<<"$changed"; then echo "$1=true"; else echo "$1=false"; fi
}
flag run_action_checks '^\.github/'
code="$(grep -E '^(rust/|\.github/)' <<<"$changed" | grep -vE '\.md$' || true)"
if [ -z "$code" ]; then
  printf 'run_tests=false\nfilterset=\naffected=\n'
  exit 0
fi
echo "run_tests=true"

# Anything outside a crate (toolchain, cargo or nextest config, the lockfile, workflows, scripts) can change every crate.
if grep -vqE '^rust/crates/[^/]+/' <<<"$code"; then
  printf 'filterset=\naffected=\n'
  exit 0
fi
dirs="$(sed -E 's|^rust/crates/([^/]+)/.*|\1|' <<<"$code" | sort -u)"
metadata="$(cargo metadata --format-version 1 --no-deps)"
packages="$(jq -r --argjson dirs "$(jq -R . <<<"$dirs" | jq -s .)" '
  .packages[] | (.manifest_path | capture("/crates/(?<dir>[^/]+)/Cargo\\.toml$").dir) as $dir
  | select($dirs | index($dir)) | .name' <<<"$metadata" | sort -u)"
if [ -z "$packages" ]; then
  printf 'filterset=\naffected=\n'
  exit 0
fi
echo "filterset=$(sed 's/.*/rdeps(&)/' <<<"$packages" | paste -sd'|' - | sed 's/|/ | /g')"
# Workspace crates reach each other only by path, so the path dependencies are the whole graph.
affected="$(jq -r --argjson changed "$(jq -R . <<<"$packages" | jq -s .)" '
  [.packages[] | {name, uses: [.dependencies[] | select(.path) | .name]}] as $graph
  | def grow: . as $set
      | ($set + [$graph[] | select(any(.uses[]; . as $use | $set | index($use) != null)) | .name] | unique) as $next
      | if ($next | length) == ($set | length) then $set else ($next | grow) end;
  $changed | unique | grow | .[]' <<<"$metadata")"
echo "affected= $(tr '\n' ' ' <<<"$affected")"
