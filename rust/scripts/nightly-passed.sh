#!/usr/bin/env bash
# Whether a successful Nightly run built <commit> (default HEAD), or a commit before it with only CHANGELOG.md
# changed since: nightly users ran it before stable users get it.
set -euo pipefail
cd "$(dirname "$0")/.."

COMMIT="$(git rev-parse "${1:-HEAD}")"
REPO="${RELEASE_REPO:-pomelohq/pomelo}"
command -v gh >/dev/null 2>&1 || { echo "gh is needed to read the Nightly runs" >&2; exit 1; }
BUILT="$(gh run list --repo "$REPO" --workflow nightly.yml --status success --limit 50 \
  --json headSha --jq '.[].headSha')" || { echo "could not read the Nightly runs" >&2; exit 1; }
for SHA in $BUILT; do
  git cat-file -e "$SHA^{commit}" 2>/dev/null || continue
  git merge-base --is-ancestor "$SHA" "$COMMIT" || continue
  CHANGED="$(git diff --name-only "$SHA" "$COMMIT" | grep -v '^CHANGELOG.md$' || true)"
  if [ -z "$CHANGED" ]; then
    echo "a green nightly built $SHA"
    exit 0
  fi
done
echo "no green nightly built $COMMIT (or a commit it differs from only in CHANGELOG.md)" >&2
exit 1
