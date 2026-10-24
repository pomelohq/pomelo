#!/usr/bin/env bash
# The rare hotfix cut from the last release instead of main, for when main is not releasable:
#   start    branch hotfix/v<next patch> from the newest v<x.y.z> tag; cherry-pick the fixes onto it
#   release  on that branch: bump to <next patch>, commit, tag v<next patch> and push the branch and the tag
# The release workflow still needs the CHANGELOG block; main then takes the version and the block in a PR.
set -euo pipefail
cd "$(dirname "$0")/.."

ACTION="${1:-}"
LAST="$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)"
[ -n "$LAST" ] || { echo "no release tag found" >&2; exit 1; }
IFS=. read -r MA MI PA <<<"${LAST#v}"
NEXT="${MA}.${MI}.$((PA + 1))"
BRANCH="hotfix/v${NEXT}"

if [ -n "$(git status --porcelain)" ]; then
  echo "working tree not clean; commit or stash first" >&2
  exit 1
fi

case "$ACTION" in
  start)
    git fetch --tags origin
    git switch -c "$BRANCH" "$LAST"
    echo "on $BRANCH from $LAST. Next:"
    echo "  git cherry-pick -x <commit>...      the fixes, from main"
    echo "  add '## [${NEXT}] - $(date +%Y-%m-%d)' to CHANGELOG.md and commit it"
    echo "  make hotfix-release"
    ;;
  release)
    if [ "$(git rev-parse --abbrev-ref HEAD)" != "$BRANCH" ]; then
      echo "run this on $BRANCH (make hotfix starts it)" >&2
      exit 1
    fi
    if ! grep -q "^## \[${NEXT}\]" ../CHANGELOG.md; then
      echo "CHANGELOG.md has no '## [${NEXT}]' block: curate it and commit it first" >&2
      exit 1
    fi
    scripts/set-version.sh "$NEXT"
    cargo update --workspace >/dev/null 2>&1
    git add Cargo.toml Cargo.lock
    git commit -m "release: v${NEXT}"
    git tag "v${NEXT}"
    git push origin "$BRANCH" "v${NEXT}"
    echo "v${NEXT} tagged and pushed; the Release workflow builds and publishes it:"
    echo "  gh run watch --repo pomelohq/pomelo"
    echo "Then open a PR to main with the version bump and the CHANGELOG block, so main's next release follows it."
    ;;
  *)
    echo "usage: $0 start|release" >&2
    exit 2
    ;;
esac
