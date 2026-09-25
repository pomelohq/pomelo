# Pomelo config variables (v2)

Templates in `pom.yml` / `pom.d/**` use **dot-notation**: `{{ <source>.<name>[.<field>] }}`.
One grammar resolves every value — resolver: `services/resolve_v2.go`
(`ResolveCtx.lookup`). Validated at load by `config.Validate` (a typo / renamed
alias fails loudly).

> This file is the single source of truth for the grammar. Keep it in sync with
> `configVarReference` in `internal/web/claude_prompt.go` (injected into every
> Claude / Doctor / onboarder prompt) and the summary in `CLAUDE.md`.

## Sources

| Token | Resolves to |
|---|---|
| `{{shared.<name>.url}}` | Shared service connection `user:pass@host:port` |
| `{{shared.<name>.host}}` | Host — always `127.0.0.1` (explicit IPv4: `localhost` may resolve to `::1`, which Docker's publish misses) |
| `{{shared.<name>.port}}` | Allocated port for the shared service |
| `{{shared.<name>.user}}` / `.pass` | Credentials from `shared_services.<name>` |
| `{{shared.<name>.slot}}` | Capacity slot index (e.g. Redis DB number) |
| `{{db.<name>}}` | Named per-branch database name (session-prefixed, branch-resolved) |
| `{{db.<name>.url}}` | Full `postgres://…/<db>` URL via the shared postgres |
| `{{<repo-alias>.<service>.url}}` | A service's base URL (env-aware: local port or remote per active profile) |
| `{{<repo-alias>.<service>.path}}` | Same-origin dev-proxy path (`/_pom_dev/<repo>/<service>`) |
| `{{<repo-alias>.<service>.host}}` / `.port` / `.ws` | Host / port / websocket URL of a service |
| `{{secret.<NAME>}}` | Value from the secrets store (never inline a secret) |
| `{{slot.<name>}}` | Allocated slot index for a capacity-limited service |
| `{{branch.safe}}` | Workspace branch with `/` and `-` → `_` |
| `{{branch.host}}` / `{{branch.hash}}` | Branch-derived hostname / short hash |
| `{{bind_ip}}` | Always `127.0.0.1` |

## Databases

Declare a repo's databases so `{{db.<name>}}` has something to resolve:

```yaml
repos:
  api:
    databases:
      main: "{{branch.safe}}"        # name template; the session prefix is added
      test: "{{branch.safe}}_test"
```

## Env wiring examples

Every declared shared service must be referenced by the repos that use it —
otherwise services boot with no connection info (`config_doctor` flags this as
`shared.unwired`).

```yaml
env:
  DATABASE_URL: postgresql://{{shared.postgres.url}}/{{db.main}}?schema=public
  DATABASE_TRANSACTION_URL: postgres://{{shared.postgres.url}}/{{db.main_tx}}
  REDIS_URL: redis://{{shared.redis.host}}:{{shared.redis.port}}/{{shared.redis.slot}}
  OPENSEARCH_URL: http://{{shared.opensearch.host}}:{{shared.opensearch.port}}
  MINIO_URL: http://{{shared.minio.host}}:{{shared.minio.port}}
  # cross-service (env-switchable via environments: profiles)
  COMMUNICATION_SERVICE_URL: '{{comm.api.url}}'
```

## Switchable variables: `{{var:NAME}}` (still current)

Separate from the dot grammar, a service can publish a variable with
`exposes: NAME`; consumers reference `{{var:NAME}}` and an `environments:<profile>`
entry overrides it (local↔remote). This is **not** legacy — but for a plain
cross-service URL prefer the dot service-ref `{{<repo>.<service>.url}}`, which is
dev-proxy aware and switches via profiles too.

## Legacy (prefer the dot form)

These colon forms still resolve for back-compat but have dot replacements:

| Legacy | Write instead |
|---|---|
| `{{conn:name}}` | `{{shared.name.url}}` |
| `{{host:name}}` / `{{port:name}}` | `{{shared.name.host}}` / `{{shared.name.port}}` |
| `{{db:name}}` | `{{db.name}}` |
| `{{slot:name}}` | `{{slot.name}}` |
| `{{branch_safe}}` / `{{branch_host}}` | `{{branch.safe}}` / `{{branch.host}}` |

Removed entirely in v2: `{{url:}}` / `{{ws:}}` string templates, positional
`{{db:N}}`, `global_services`, the per-repo `env_switch` bool.
