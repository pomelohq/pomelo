#!/usr/bin/env bash
# ADR 0001: Views must not touch the FFI directly - fetch/decode/commands belong in
# a Store/ViewModel. This ratchets that rule: the Store/VM layer (Core/, AppState,
# *Store.swift, *ViewModel.swift, main.swift) may call PomCore; every other file
# must be grandfathered in scripts/view-ffi-allowlist.txt. New violations fail CI.
# Migrate a file, then delete its line from the allowlist.
set -euo pipefail
cd "$(dirname "$0")/.."
BASE=desktop/PomeloApp/Sources/PomeloApp
ALLOW=scripts/view-ffi-allowlist.txt

offenders=()
while IFS= read -r f; do
  rel=${f#"$BASE"/}
  case "$rel" in App/main.swift|App/AppState.swift|*Store.swift|*ViewModel.swift) continue;; esac
  # A real call is `PomCore.shared.<member>` (fetch/decode/command). The bare
  # `= PomCore.shared` VM-default has no trailing dot; `.session` is a synchronous
  # identity constant, not data traffic, so it is exempt.
  calls=$(grep -oE "PomCore\.shared\.[A-Za-z_][A-Za-z0-9_]*" "$f" | sort -u | grep -vx "PomCore.shared.session" || true)
  if [ -n "$calls" ]; then
    offenders+=("$rel")
  fi
done < <(grep -rl "PomCore\.shared\." "$BASE/Features" "$BASE/Components" "$BASE/App" 2>/dev/null)

fail=0
for rel in "${offenders[@]:-}"; do
  [ -z "$rel" ] && continue
  if ! grep -qxF "$rel" "$ALLOW"; then
    echo "error: $rel calls PomCore.shared directly; route it through a Store/ViewModel (ADR 0001)."
    fail=1
  fi
done

# Flag stale allowlist entries so migrated files get removed and the list shrinks.
while IFS= read -r rel; do
  [ -z "$rel" ] && continue
  printf '%s\n' "${offenders[@]:-}" | grep -qxF "$rel" || echo "note: $rel is allowlisted but no longer calls PomCore.shared; remove it from $ALLOW."
done < "$ALLOW"

exit $fail
