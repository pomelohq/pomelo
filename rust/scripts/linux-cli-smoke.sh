#!/usr/bin/env bash
# Drives the pom CLI headless against a throwaway project: ws create (worktree, Postgres database, setup), start
# --wait, the dev proxy, and an agent session (start, send, read) with a stand-in agent. Needs git, curl,
# python3 and Docker with its Compose plugin; everything it creates is removed at the end.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d /tmp/pom-smoke.XXXXXX)"
export XDG_STATE_HOME="$work/state"
export POMELO_CONFIG_DIR="$work/config"
export POM_PTY_SOCK_DIR="$work/s"
export HOME="$work/home"
export PATH="$here/cli-smoke:$PATH"
mkdir -p "$HOME" "$POM_PTY_SOCK_DIR"
project="$work/smoke"
branch="feat-smoke"

step() { printf '\n== %s\n' "$*"; }
cleanup() {
  set +e
  cd "$project" 2>/dev/null && {
    pom agent stop "$branch/smoke" >/dev/null 2>&1
    pom -w "$branch" stop >/dev/null 2>&1
    pom ws delete "$branch" >/dev/null 2>&1
    pom release --disk --yes >/dev/null 2>&1
  }
  pkill -f "$work" >/dev/null 2>&1
  rm -rf "$work"
}
trap cleanup EXIT

step "pom --version"
pom --version

mkdir -p "$project/workspace--main/api"
git -C "$project/workspace--main/api" init -q -b main
git -C "$project/workspace--main/api" config user.email smoke@example.com
git -C "$project/workspace--main/api" config user.name smoke
echo '<h1>smoke</h1>' >"$project/workspace--main/api/index.html"
git -C "$project/workspace--main/api" add -A
git -C "$project/workspace--main/api" commit -qm "Start the smoke repo"
cat >"$project/pom.yml" <<'YAML'
session: pomsmoke
default_branch: main
shared_services:
  postgres:
    image: postgres:16
repos:
  api:
    databases:
      main: "{{branch.safe}}"
    shared_services:
      - postgres
    env:
      DATABASE_URL: "postgres://{{shared.postgres.url}}/{{db.main}}"
    services:
      web:
        type: backend
        cmd: python3 -m http.server "$PORT" --bind 127.0.0.1
        healthcheck:
          http: /
YAML
cd "$project"

step "pom doctor --json"
pom doctor --json

step "pom ws create $branch"
pom ws create "$branch"
test -f "$project/workspace--$branch/api/index.html"

step "pom start --wait"
cd "$project/workspace--$branch"
pom start web --wait --timeout 120s
pom status

step "the service through the dev proxy"
url="$(pom url web | grep -o 'http://[^ ]*localhost:[0-9]*[^ ]*' | head -1)"
echo "$url"
for attempt in 1 2 3 4 5 6 7 8 9 10; do
  if curl -fsS --max-time 10 "$url" | grep -q smoke; then
    break
  fi
  [ "$attempt" -lt 10 ] || exit 1
  sleep 1
done

step "pom agent start, send, read"
pom agent start "$branch" --fresh --role smoke --json
pom agent send "$branch/smoke" "hello from the smoke test"
pom agent wait "$branch/smoke" --until idle --timeout 30s
pom agent read "$branch/smoke" | tee "$work/read.txt"
grep -q "echo: hello from the smoke test" "$work/read.txt"

step "done"
