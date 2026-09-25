# pom.yml — canonical schema

**`pom.yml` is the config to RUN THE PROJECT — not config for the app.** Only
keys that directly serve *building & running* the project belong here. Anything
about the app's own behavior, side features, or personal machine prefs lives in
the app's Settings, never in `pom.yml`.

Legend: `[name]` = arbitrary map key · `<x>` = a value you fill · `<a|b>` = enum
(pick one) · `?` = optional · `[ <x> ]` = list.

```yaml
session: <string>                     # namespace: DB prefix + hostnames
default_branch: <branch>

repos:
  [repo_name]:                        # = clone folder name
    alias: <string>?                  # default = [repo_name]
    default_branch: <branch>?
    databases: { [db_name]: <base_db_name> }?     # → {{db.<db_name>}}
    presets: [ <preset_name> ]?
    seed_from_main: <bool>?           # clone DBs from the main workspace instead of empty
    env:
      [ENV_KEY]: <string|template>
      # | [".env.file"]: { [KEY]: <val> } ; "*": { [KEY]: <val> }   # "*" = shared base
    services:
      [service_name]:
        type: <backend|frontend|worker>
        cmd: <string>
        dir: <relpath>?               # monorepo sub-app
        port: <bool>?                 # override $PORT assignment
        depends_on: [ <service_name> ]?   # start order (B waits for A)
        env: { [KEY]: <string|template> }?
        profiles: [ <profile> ]?      # which environments profiles this service can switch to (gates the env picker)
        modes: { [mode_name]: <cmd> }?    # named alternate run-commands, switched live in the app (no config edit)
        mode: <mode_name>?                # default mode
    lifecycle:                        # how the repo is built & run
      pre_start: <cmd>?               # runs before EVERY service (nvm use / source …)
      commands: { [op_name]: <cmd> }? # SINGLE SOURCE of named ops (install/generate/migrate/seed/test/lint/build/…) — each auto-becomes a shortcut
      create:  [ <op_name> ]?         # WHEN creating a workspace   (default: install, generate, migrate, seed)
      refresh: [ <op_name> ]?         # WHEN a pull brings new commits (default: migrate)
      pre_delete: [ <op_name|cmd> ]?
      copy: [ <glob> ]?

shared_services:
  [name]:                             # postgres|redis|minio|opensearch → image/ports/creds/healthcheck auto-filled
    type: <string>?                   # well-known key when the service name differs
    image: <string>?
    ports: [ <"host:container"> ]?
    environment: { [KEY]: <val> }?
    volumes: [ <string> ]?
    command: <string>?
    healthcheck: { <...> }?
    db_user: <string>? ; db_password: <string>?
    capacity: <int>?                  # slot-limited → {{slot.<name>}}

environments:                         # local↔remote switchboard: DEFINE profiles
  [profile]:
    [<repo>.<service>]: <remote_url>  # only listed services switch; the rest stay local

presets:                              # reusable repo fragments, composable
  [preset_name]:
    presets: [ <preset_name> ]?
    services: { <same shape as repos[].services> }?
    env: { [KEY]: <val> }?
    lifecycle: { <same shape as repos[].lifecycle> }?

seed: [ <cmd> ]?                      # workspace-level: runs once in the workspace root
prepare_main: [ <reset|migrate|seed> ]?
```

## Template grammar — dot-notation ONLY

```
{{shared.<name>.url|host|port|user|pass|slot}}
{{db.<name>}} | {{db.<name>.url}}
{{<repo>.<service>.url|path|host|port|ws}}     # .path = same-origin /_pom_dev/<repo>/<svc>
{{secret.<NAME>}} | {{slot.<name>}}
{{branch.safe|host|hash}} | {{bind_ip}}
```

Colon forms (`{{var:}}` `{{host:}}` `{{port:}}` `{{conn:}}` `{{db:}}` `{{user:}}`
`{{pass:}}` `{{slot:}}` `{{url:}}` `{{ws:}}`) are **removed** — `config.Validate`
rejects them.

## Removed (not "config to run the project")

- **App config** — `ui`, `code_agents` (→ app Settings, not a shared artifact).
- **Features / integrations** — `jira`, `archive`, `plugins` (`sync` stays: it schedules Keep Main Fresh).
- **Orchestration** — `combinations`, `workspaces`.
- **System-managed routing** — `proxy`, `webhook` (Pomelo auto-routes
  `/_pom_dev/<repo>/<svc>` + `<svc>.<repo>.<branch>.localhost`, webhooks at
  `/<repo>/<svc>`).
- **`e2e`** (deleted) and its enabler `exposes:` + `{{var:}}`.
- **Redundant** — per-service `shortcuts` + flat `setup`/`migrate`/`seed`/
  `pre_start` (folded into `lifecycle`), per-repo env whitelist (per-service
  `profiles:` suffices).
- **Legacy** — `schema_version`, `shared_stable_ports`, `global_services`,
  per-repo `env_switch`.
```
