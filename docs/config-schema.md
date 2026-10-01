# pom.yml - canonical schema

**`pom.yml` is the config to RUN THE PROJECT - not config for the app.** Only
keys that directly serve *building & running* the project belong here. Anything
about the app's own behavior, side features, or personal machine prefs lives in
the app's Settings, never in `pom.yml`.

Parser: `rust/crates/pom_config/src/schema.rs`; load-time checks:
`rust/crates/pom_config/src/validate.rs`; built-in shared service defaults:
`rust/crates/pom_config/src/presets.rs`.

Legend: `[name]` = arbitrary map key - `<x>` = a value you fill - `<a|b>` = enum
(pick one) - `?` = optional - `[ <x> ]` = list.

```yaml
session: <string>?                    # namespace: DB prefix + hostnames (default: pomelo)
default_branch: <branch>?

repos:
  [repo_name]:                        # = clone folder name
    alias: <string>?                  # name templates use for this repo (default = [repo_name])
    default_branch: <branch>?
    preset: <preset_name> | [ <preset_name> ]?   # presets to apply (they only fill what is unset)
    databases: { [db_name]: <name_template> }?   # -> {{db.<db_name>}}; e.g. main: "{{branch.safe}}"
    shared_services: [ <name> | { <name>: { db_name: <template> } } ]?   # the shared services it uses
    seed_from_main: <bool>?           # copy DBs from the main workspace instead of starting empty
    profiles: [ <profile> ]?          # environments profiles this repo's services may switch to
    proxy_port: <port>?               # port the dev proxy uses when no service port is leased
    shell_env: <string>?              # "KEY=value ..." prefixed to every service command
    env:
      [ENV_KEY]: <string|template>    # flat map -> .env.local
      # or per file: { "*": { [KEY]: <val> }, ".env.test": { [KEY]: <val> } }   # "*" = base for every file
    services:
      [service_name]: <cmd>           # shorthand for { cmd: <cmd> }
      [service_name]:
        type: <backend|frontend|worker>?   # backend/frontend get a $PORT unless port: false
        cmd: <string>
        dir: <relpath>?               # monorepo sub-app
        port: <bool>?                 # override the $PORT assignment
        depends_on: [ <service_name> ]?   # start order (B waits for A)
        env: { [KEY]: <string|template> }?
        pre_start: <cmd>?             # replaces the repo's pre_start for this service
        shell_env: <string>?          # replaces the repo's shell_env for this service
        proxy_port: <port>?
        profiles: [ <profile> ]?      # which environments profiles this service can switch to
        modes: { [mode_name]: <cmd> }?    # named alternate run-commands, switched live in the app
        mode: <mode_name>?                # default mode
        tasks: [ { key: <k>, desc: <text>, cmd: <cmd> } ]?   # service quick commands
    lifecycle:                        # how the repo is built & run (the flat keys below mean the same)
      pre_start: <cmd>?               # runs before EVERY service (nvm use / ...)
      commands: { [op_name]: <cmd> }? # named ops; each becomes a shortcut. install/generate/migrate run at create
      setup: [ <cmd> ]?               # replaces commands install+generate+migrate at workspace create
      migrate: [ <cmd> ]?             # replaces commands.migrate
      seed: [ <cmd> ]?                # runs after setup at create (skipped when DBs come from main)
      pre_delete: [ <cmd> ]?          # runs before the workspace is deleted
      copy: [ <glob> ]?               # files copied from the main checkout into a new workspace
      tasks: [ { key: <k>, desc: <text>, cmd: <cmd> } ]?   # extra quick commands (old name: shortcuts)
    # pre_start, commands, setup, migrate, seed, pre_delete, copy, tasks also work directly on the repo;
    # a lifecycle: value wins over the flat one.

shared_services:
  [name]:                             # postgres|redis|minio|opensearch|zincsearch -> image/ports/creds/healthcheck auto-filled
    type: <string>?                   # well-known key when the service name differs
    image: <string>?
    ports: [ <"host:container"> ]?
    environment: { [KEY]: <val> }?
    volumes: [ <string> ]?
    command: <string>?
    healthcheck: { test: <cmd|[CMD, ...]>, interval: <dur>?, timeout: <dur>?, retries: <int>? }?
    db_user: <string>? ; db_password: <string>?
    capacity: <int>?                  # slot-limited -> {{slot.<name>}}
  [name]:                             # OR a command: one process for every workspace (no Docker)
    cmd: <shell string>               # instead of image; gets $PORT and $BIND_IP
    repo: <repo>?                     # runs in that repo's main-workspace checkout; else the project folder
    port: <int>?                      # the port it is told (fixed); else one is leased
    environment: { [KEY]: <val> }?    # $PORT / ${PORT} in a value become the port
    healthcheck: { test: <cmd> }?     # waited for (up to 30s) before services that use it start
                                      # image XOR cmd; ports/volumes/command/capacity/db_* are image-only,
                                      # repo/port are cmd-only; {{shared.<name>.url}} = http://127.0.0.1:<port>

environments:                         # local<->remote switchboard: DEFINE profiles
  [profile]:
    [<repo>.<service>]: <remote_url>  # only listed services switch; the rest stay local

presets:                              # reusable repo fragments, composable
  [preset_name]:
    preset: <preset_name> | [ <preset_name> ]?
    services: { <same shape as repos[].services> }?
    env: { [KEY]: <val> }?
    pre_start / commands / setup / migrate / seed / pre_delete / copy / tasks / seed_from_main: <as on a repo>?

preset: <preset_name> | [ <preset_name> ]?   # workspace level: these presets' services run in every workspace
seed: [ <cmd> ]?                      # workspace-level: runs once in the workspace root at create
prepare_main: [ <reset|migrate|seed> ]?   # Prepare main phases (default: reset, migrate, seed)
sync:
  auto_push: <bool>? ; interval_sec: <int>?
  refresh_main: <bool>? ; refresh_interval_sec: <int>?   # Keep Main Fresh
```

## Template grammar - dot-notation ONLY

```
{{shared.<name>.url|host|port|user|pass|slot}}
{{db.<name>}} | {{db.<name>.url}}
{{<repo>.<service>.url|path|host|port|ws}}     # .path = same-origin /_pom_dev/<repo>/<svc>
{{secret.<NAME>}} | {{slot.<name>}}
{{branch}} | {{branch.safe|host|hash}} | {{bind_ip}}
```

Full table: `docs/config-variables.md`. Colon forms (`{{var:}}` `{{host:}}` `{{port:}}`
`{{conn:}}` `{{db:}}` `{{user:}}` `{{pass:}}` `{{slot:}}` `{{url:}}` `{{ws:}}`) are
**rejected** at load (`Config::validate`); `config_normalize` / `pom config normalize`
migrates the ones that have a dot replacement.

## Removed (not "config to run the project")

The parser still reads some of these so an old file loads, but nothing uses them;
`config_normalize` deletes `schema_version`, `plugins`, `combinations`, `proxy`, `webhook`
and `exposes`.

- **App config** - `ui`, `code_agents` (-> app Settings, not a shared artifact).
- **Features / integrations** - `jira`, `archive`, `plugins` (`sync` stays: it schedules Keep Main Fresh).
- **Orchestration** - `combinations`, `workspaces`.
- **System-managed routing** - `proxy`, `webhook` (Pomelo auto-routes
  `/_pom_dev/<repo>/<svc>` + `<svc>.<repo>.<branch>.localhost`, webhooks at
  `/<repo>/<svc>`).
- **`e2e`** and its enabler `exposes:` + `{{var:}}`.
- **Legacy** - `schema_version`, `shared_stable_ports`, `global_services`,
  per-repo `env_switch`, lifecycle `create:` / `refresh:` lists.
