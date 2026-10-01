# Pomelo config variables

Templates in `pom.yml` use **dot-notation**: `{{ <source>.<name>[.<field>] }}`.
One grammar resolves every value: `ResolveContext::lookup` in
`rust/crates/pom_env/src/resolver.rs`. Database name templates (`databases:`,
`shared_services: [{ <name>: { db_name: ... } }]`) only take the `branch` tokens,
resolved by `resolve_branch_tokens` in `rust/crates/pom_env/src/branch.rs`.

A token the resolver does not know is left in the output as written, so a typo
shows up verbatim in the env file instead of becoming an empty value. A
`| filter` after the key is accepted but changes nothing.

## Sources

| Token | Resolves to |
|---|---|
| `{{shared.<name>.url}}` | Shared service connection `user:pass@host:port`; just `host:port` for a service with no login (e.g. Redis); `http://127.0.0.1:<port>` for a `cmd:` shared service |
| `{{shared.<name>.host}}` | Host - always `127.0.0.1` (explicit IPv4: `localhost` may resolve to `::1`, which Docker's publish misses) |
| `{{shared.<name>.port}}` | Allocated port for the shared service |
| `{{shared.<name>.user}}` / `.pass` | Credentials from `shared_services.<name>` (`postgres` for a Postgres that sets none, empty otherwise) |
| `{{shared.<name>.slot}}` | Capacity slot index (e.g. Redis DB number) |
| `{{db.<name>}}` | Named per-branch database name (session-prefixed, branch-resolved) |
| `{{db.<name>.url}}` | Full `postgres://user:pass@host:port/<db>` URL via the shared Postgres |
| `{{<repo-alias>.<service>.url}}` | A service's base URL through the dev proxy (`http://<service>.<repo>.<branch>.localhost:<proxy port>`), or the remote URL when the active `environments` profile lists it |
| `{{<repo-alias>.<service>.path}}` | Same-origin dev-proxy path (`/_pom_dev/<repo>/<service>`) |
| `{{<repo-alias>.<service>.host}}` / `.port` / `.ws` | Host / leased port / websocket URL of a service |
| `{{secret.<NAME>}}` | Value from the secrets store (never inline a secret) |
| `{{slot.<name>}}` | Allocated slot index for a capacity-limited service |
| `{{branch}}` | The workspace branch as is |
| `{{branch.safe}}` | The branch with `/` turned into `_` (hyphens are kept): `feat/login-form` -> `feat_login-form` |
| `{{branch.host}}` | The branch as a DNS label: lowercase, other characters turned into `-`, at most 63 bytes (a hash suffix keeps long names apart) |
| `{{branch.hash}}` | First 8 hex digits of the branch's SHA-1 |
| `{{bind_ip}}` | Always `127.0.0.1` |

Service commands also get `$PORT` (their leased port) and `$BIND_IP` in their
environment; a `cmd:` shared service gets the same.

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

Every declared shared service must be referenced by the repos that use it -
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
  BILLING_URL: '{{web.api.url}}'
```

## Removed colon forms

Colon-form templates are **rejected** when the config loads (`Config::validate`
in `rust/crates/pom_config/src/validate.rs`). `config_normalize` /
`pom config normalize` rewrites the ones that have a dot replacement:

| Removed | Write instead |
|---|---|
| `{{conn:name}}` | `{{shared.name.url}}` |
| `{{host:name}}` / `{{port:name}}` | `{{shared.name.host}}` / `{{shared.name.port}}` |
| `{{user:name}}` / `{{pass:name}}` | `{{shared.name.user}}` / `{{shared.name.pass}}` |
| `{{db:name}}` | `{{db.name}}` |
| `{{slot:name}}` | `{{shared.name.slot}}` or `{{slot.name}}` |
| `{{var:NAME}}`, `{{url:...}}`, `{{ws:...}}` | no replacement: use a service ref such as `{{<repo>.<service>.url}}` |

Database name templates still accept the old spellings `{{branch_safe}}` /
`{{branch|safe}}` (and the same for `hash` and `host`); write `{{branch.safe}}`.
