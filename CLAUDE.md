# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
go build -o pom ./cmd/pom/      # Build binary
go test ./...                      # Run all tests
go vet ./...                       # Static analysis
make build                         # Same as go build
make release                       # Optimized + codesign (macOS)
make install                       # Release + copy to /usr/local/bin
```

Requires: `go` (1.26+), `zsh`, `codesign` (macOS). tmux is NOT required — services/shells run on self-managed PTY holders (`internal/ptyhost`).

## Release

Full details in `RELEASE.md`. The release IS the native macOS app.

```bash
make patch                         # 0.10.8 → 0.10.9 → bump BOTH version consts → tag → push
make minor                         # 0.10.8 → 0.11.0
make major                         # 0.10.8 → 1.0.0
```

`make patch/minor/major` bumps `cmd/pom/root.go` `version` AND `cmd/libpom/libpom.go`
`appVersion` in lockstep (`make version-check` guards drift), commits, tags, pushes.
**Release is CI-only**: pushing the `v*` tag triggers `.github/workflows/release.yml`
(→ reusable `app-build.yml` with `publish:true`) which builds, signs, notarizes, and
publishes the DMG + Sparkle appcast to the GitHub Release. There is no local publish
path. `make dmg` builds a local DMG for testing only (never publishes). The app
self-updates via Sparkle (appcast); there is no `pom update` / CLI distribution / daemon.

## Docs Rule

**Docs live in a separate repo: `toantran292/pomelo-docs`** (VitePress →
GitHub Pages at https://pomelohq.app/). They are NOT
in this repo.

**ALWAYS update the docs when a change affects user-facing behavior.** Any
breaking change, new/changed feature, config/CLI/template change, or a bug
fix that changes documented behavior MUST be accompanied by a matching
docs update in `pomelo-docs` — clone it, edit the relevant page(s) under
`guide/` / `reference/` (and `guide/architecture.md` for design/technique
changes), `npm run build` to verify, then commit + push (deploys via
Actions). Never document a feature that isn't shipped in code; if it's not
built, don't add a page for it. Purely internal refactors with no
user-visible change don't need a docs update.

## Architecture

Go core that runs dev services on self-managed PTY holders (tmux-free). **Native-first**: the
product is the macOS app (`desktop/PomeloApp`) linking the Go core via `libpom` FFI. The
**browser UI was retired** — `web-ui/` (React/Vite) and its embedded `internal/core/static/`
SPA are deleted. `internal/core` remains as the shared `Server` + business logic (consumed by
libpom's typed FFI bindings, and by `pom mcp`'s in-process `/api` handler); the `/api` mux
now backs only `pom mcp` — the app never touches it. There is **no TUI**
(removed v0.8.0). The rest of the surface is plain CLI subcommands. Config via `pom.yml`
found by walking up from CWD (the legacy `tncli.yml` name was removed in v0.10.9).

**CLI path**: `cmd/pom/main.go` (dispatch) → `internal/services/` (business logic) → `internal/ptyhost/` (PTY holders)

**App path**: `desktop/PomeloApp` (SwiftUI) → `libpom` FFI (`cmd/libpom`) → `internal/core` `Server` (typed bindings only, no `/api`) → `internal/services`/`internal/pipeline`. Streams (PTY/Claude/pipeline) go over direct FFI (`PomStream*`); the SwiftUI app IS the UI (no HTML/JS).

### Go Project Layout

```
cmd/pom/main.go              — CLI entry, command dispatch, no business logic
cmd/pom/cmd_*.go             — one cobra command per file (web, start/stop, workspace, db, …)
internal/
  config/                       — YAML parsing, template resolution, load-time validation
  lock/                         — Lock file management
  ptyhost/                      — self-managed PTY holders (tmux-free): detached process per service/shell behind a Unix socket, scrollback + attach/resize
  services/                     — Infrastructure layer (all side effects)
    services.go                 — Template resolution ({{var}}, {{conn}}, …), shared types
    network.go                  — Port allocation, session slots, shared ports
    compose.go                  — docker-compose override generation
    docker.go, git.go, files.go — Docker / Git / env-file operations
    workspace.go, registry.go   — Shared services compose, slot allocation, project registry
  pipeline/                     — Workspace lifecycle (staged create/delete + runner + events)
  jira/                         — Jira REST client (config resolve, key regex, ADF) — shared by web + archive
  archive/                      — Workspace → Markdown retrospective (claude -p); stored at ~/.local/state/pom/archives/<session>/<branch>.md. Launching a Claude window auto-grants read via --add-dir + a system-prompt hint (see startWsWindow/claudeArchiveAugment)
  mcp/                          — `pom mcp` stdio MCP server (no SDK, JSON-RPC by hand) exposing a workspace's env to agents (ports/db/services/run_in_env/config/config_doctor). PORTLESS: builds the web /api handler IN-PROCESS from the config found by walking up from CWD (no running dashboard / :8765). Auto-registered into Claude windows via --mcp-config (claudeArchiveAugment). New agent-facing endpoints live in web/mcp_endpoints.go
  web/onboard.go                — onboarding harness: an autonomous agent (onboarderDriver) analyzes cloned repos → authors pom.yml via the MCP config tools → loops config_doctor until clean. Shared agentDriver builder (fixer/onboarder). `pom onboard [--new NAME --repo P…]` (cmd/pom/cmd_onboard.go) runs it in one portless process; the native app triggers the same core after New session (role=onboarder)
  agent/
    codeagent/                  — Built-in agent registry (Claude Code) + Inject hook + state detector (used by web)
  web/                          — HTTP+WebSocket server + REST handlers; static/ = embedded built frontend
    auth.go                     — token auth middleware (all routes) + same-host Origin check
    httpjson.go                 — writeJSON / readJSON / httpErr — use these in every handler
web-ui/                         — React/Vite frontend source (built into internal/core/static/)
```

### Native app (desktop/PomeloApp) — MVVM + testable

The macOS app links the Go core via `libpom` (c-archive FFI), portless. Direction
is **native-first** (web UI paused; see memory). Architecture:

- **Streams** (PTY / Claude / pipeline) → direct C-FFI (`cmd/libpom/stream.go`
  `PomStream*`).
- **Reads/actions** → typed per-domain FFI bindings in `cmd/libpom/bindings.go`
  (`//export Pom<Domain>` → an extracted `Server.<Method>` the `pom mcp` handler
  also shares) → Swift `PomCore.<domain>Data()`. This is the ONLY read/action path
  — there is no generic `/api` bridge (`PomHandle` was removed); the app must NOT
  feel client-server. Add an endpoint = extract a `Server` data method + one
  `//export` + one `PomCore` method (+ a domain-protocol entry if a VM needs it).
- **MVVM/testability**: SwiftUI Views hold a `@StateObject` ViewModel
  (`Sources/PomeloApp/ViewModels/*`); ViewModels depend on the `PomAPI` protocol
  (`Core/PomAPI.swift`, PomCore conforms), so unit tests inject `MockPomAPI` (no
  FFI). Tests in `Tests/PomeloAppTests` run via `swift test`. New screen work:
  extract logic into a ViewModel + a test, don't put fetch/decode in the View.
- Build: `desktop/PomeloApp/build.sh` (Go c-archive → SwiftPM link). Must run at
  120fps (see memory) — keep FFI off the main thread, bodies cheap.
- iPhone (planned): a thin LAN/VPN bridge (chat → Claude pty relay + Tailscale/
  Cloudflare tunnel for services), NOT a full API client.

### Web architecture notes

- Server is stateless per-request; live data (workspaces, panes, PRs, ports) via `/api/*`, terminals via `/ws`.
- **Auth** (`internal/core/auth.go`): cross-origin requests are rejected on every route (Origin must match Host) — this alone secures loopback, so `127.0.0.1` needs no token. LAN clients (non-loopback `RemoteAddr`) require a per-install token (launch URL `?token=` → cookie). Only PWA manifest/icons are exempt. Never bypass the Origin check; never trust `X-Forwarded-For` for loopback detection.
- **Config access is race-safe**: handlers/loops read via `s.cfg()` (atomic pointer, hot-swapped on reload). NEVER add a raw `Config` field access or mutate the shared `*config.Config` in a handler — put per-run mutable state on `Server` behind a mutex (see `modeOverrides`).
- **Network-dependent subprocesses need timeouts**: use `services.RunTimeout(...)` (or `exec.CommandContext`) for anything touching git remotes / gh / docker pulls. A bare `exec.Command` on the network can hang a request forever.
- Frontend polling pauses when the tab is hidden (`usePageVisible`) and is scoped/batched (e.g. `peek-all` only for the focused workspace's running windows) to avoid API spam.
- PR data (`gh`) is fetched with bounded concurrency + served stale + background-warmed to stay fast and avoid GitHub 429s (`internal/core/code.go`).
- Version is surfaced to the UI via `web.Version` (set from the CLI `version` const) → `/api/panes` → sidebar footer.
- Workspace/worktree paths come from `services.WorkspaceRootDir` / `services.RepoWorktreePath` — never hand-join `"workspace--"+branch` in web code.

## Adding an integration module (PLAYBOOK)

Follow this recipe for any new integration (ticketing, notifications, …).
It exists because past modules paid for missing seams — every gotcha below
actually happened.

**Backend**
1. Domain logic goes in its own package `internal/<feature>/` (model:
   `internal/jira`, `internal/archive`). The web layer must stay a thin
   adapter — handlers parse/validate/serialize only.
2. Config: add a struct + field in `internal/config/config.go`. Secrets are
   NEVER stored in config — store an env-var *name* (`token_env` pattern) and
   read it at use time. Provide a `Resolve(cfg)` that returns nil when
   unconfigured so the feature silently no-ops (model: `jira.Resolve`).
3. Handlers live in one new file `internal/core/<feature>.go` with their own
   `func (s *Server) <feature>Routes(mux *http.ServeMux)` — then add ONE line
   to the register slice in `server.go Run()`. Use `writeJSON`/`readJSON`/
   `httpErr` from httpjson.go; read config via `s.cfg()`.
4. Slow/remote data: cache + serve-stale + background-warm (model: PR cache).
   Bound every subprocess with `services.RunTimeout`.
5. CLI (if any): new `cmd/pom/cmd_<feature>.go`, register in `root.go`.
   **Gotcha**: `noConfigCmds` matches leaf names but only for top-level
   commands — a nested subcommand may share a name (`archive list` vs `list`).

**Frontend**
6. API calls: new `web-ui/src/api/<feature>.ts` (fetchers + their response
   interfaces), re-export from `api/index.ts`. Never add raw `fetch()` inside
   components.
7. Polled data: use the `makePolled` factory (`hooks/makePolled.ts`) — one
   hook, ~10 lines. **Gotcha**: key per-workspace maps by `wsKey(w)`, NOT by
   branch (a branch/wsKey mismatch silently renders nothing).
8. Modals: add a variant to the store's `modal` union + one case in App's
   modal switch — no new useState booleans, no callback props through Sidebar.
9. New tab kind: add to the `TAB_KINDS` registry + `Tab` union — don't
   copy-paste another `open*` store action.
10. Settings form: extend `CfgDoc` + one section in Settings; a "Test
    connection" button calls a backend `/api/<feature>/test` endpoint that
    reads the env token server-side (never send secrets from the browser).
11. Colors: CSS vars only (all three themes). No hex in components.

**Ship checklist (every change)**
- `go build ./... && go vet ./... && go test ./...` + `make check`
- App touched → `swift build && swift test` in `desktop/PomeloApp`
- User-facing change → update `pomelo-docs` (separate repo) same day
- Release = `make patch` (bump BOTH version consts + tag + push). The tag push is
  the whole release: CI (`release.yml` → `app-build.yml`) builds, signs, notarizes,
  and publishes the DMG + Sparkle appcast. No local publish. See `RELEASE.md`.
- Test before commit; never commit without the user's go-ahead on behavior
  changes

### Sudo Rule

`sudo` is only allowed in `pom setup` (one-time global setup). Runtime commands (`start`, `workspace create`, `proxy`, etc.) must NEVER require sudo.

### Real user config

Never modify a real project a user has pointed Pomelo at — only `Read` its `pom.yml` for context; output a diff for the user to apply. In public docs/examples use generic placeholders (`myproject`, `api`, `web`, `feat-login`), never a real project's repo names, branches, URLs, or any credential-shaped values.

### Workspace Branch vs Git Branch

**Always use workspace branch** (from folder name `workspace--{branch}`) for env resolution, hostnames, database names. Git branch may differ (e.g., workspace `feat-42` but git branch `feat-42-add-login-flow`). Use `workspace_branch(wt)` helper, fallback to `wt.branch`.

### Config Templates (schema v2)

Templates use **dot-notation** and are resolved in `services/resolve_v2.go`
(`ResolveCtx.resolve`). Full reference (kept in sync for humans + agents):
[`docs/config-variables.md`](docs/config-variables.md) and the
`configVarReference` const in `internal/core/claude_prompt.go` (injected into every
Claude/Doctor/onboarder prompt). All references are validated at load
(`config.Validate`) — a typo / renamed alias fails loudly.

- `{{shared.<name>.url}}` — shared service conn `user:pass@host:port`; also `.host` `.port` `.user` `.pass` `.slot` (redis DB index).
- `{{db.<name>}}` — named database from a repo's `databases:` map (session-prefixed + branch-resolved); `{{db.<name>.url}}` = full postgres URL. NOT positional.
- `{{<repo-alias>.<service>.url}}` / `.path` / `.host` / `.port` / `.ws` — another repo service's address (dev-proxy aware; `.path` = same-origin `/_pom_dev/<repo>/<svc>`); `environments:<profile>` switches url/host/port local↔remote.
- `{{secret.<NAME>}}` — value from the secrets store (never inline a secret).
- `{{slot.<name>}}` — allocated slot index for capacity-limited services.
- `{{branch.safe}}` — branch with `/` and `-` → `_`; `{{branch.host}}` DNS label; `{{branch.hash}}` short hash; `{{bind_ip}}` — always `127.0.0.1`.

**Only dot-notation is allowed. NEVER author or keep colon forms** — they are
being removed, not "legacy back-compat." Migrate every one you encounter:
`{{conn:x}}`→`{{shared.x.url}}`, `{{host:x}}`/`{{port:x}}`→`{{shared.x.host}}`/`.port`,
`{{user:x}}`/`{{pass:x}}`/`{{slot:x}}`→`{{shared.x.user}}`/`.pass`/`.slot`,
`{{db:x}}`→`{{db.x}}`, `{{branch_safe}}`→`{{branch.safe}}`. Already gone entirely:
`{{url:}}`/`{{ws:}}`, positional `{{db:N}}`, `global_services`, per-repo
`env_switch`, `schema_version`, `shared_stable_ports`. The one colon form not yet
migrated is `{{var:NAME}}`+`exposes:` (the local↔remote switchboard, no dot
equivalent yet, still used internally by `pom e2e`) — avoid it in new config.

**Routing is system-managed — NOT user config.** Never author a `proxy:` or
`webhook:` block. Pomelo auto-routes dev services (`/_pom_dev/<repo>/<svc>` +
`<svc>.<repo>.<branch>.localhost`) and fans out webhooks (`/<repo>/<svc>`) with
zero config.

Config may be **split**: a `pom.d/**/*.yml` dir next to `pom.yml` is deep-merged
(yaml.Node level, order-preserving, walked recursively) on load. `pom config
split` produces the tidy layout: `pom.d/repos/NN-<name>.yml` (NN keeps
RepoOrder) + `pom.d/{environments,presets,shared-services}.yml`, small stuff
stays in root. See `config.MergedYAML`/`SplitToFragments`
(`internal/config/include.go`). Web config editor is read-only when split
(writing the merged view back would re-inline fragments) — `handleConfigRead`
`read_only` flag.

**Well-known shared services** (`postgres`/`redis`/`minio`/`opensearch`) have
built-in defaults (`internal/config/shared_defaults.go`,
`applyWellKnownDefaults`): name the service (or set `type:`) and Pomelo fills
image/ports/env/volumes/healthcheck/creds; user fields override, `environment`
merges. Keep these in sync with the frontend `PRESETS` in
`settings/SharedServices.tsx`.

The `env:` key is unified (no separate `env_output`): a flat `{KEY: val}` map → `.env.local`; or a file-keyed map `{".env.x": {...}}` with an optional `"*"` entry as the shared base across files. Profiles are switched per env-file (per-service for `dir:` services, per-repo for services sharing the repo's env file). See `parseEnv` in `config/config_parse.go`.

### Network State

`.pom/network.json` — per-project state:
- `slot`: current runtime slot (0 or 1)
- `blocks`: wsKey → block index (for workspace port blocks)
- `service_map`: svcKey → index within block (stable)
- `shared_map`: shared service name → offset from sharedBase (stable)

`~/.local/state/pom/slots.json` — global session slot leases
`~/.local/state/pom/shared_slots.json` — capacity-based slot allocations (Redis DB indexes)

### PTY Holders (tmux-free)

Each service/shell = one detached ptyhost holder (`internal/ptyhost`): a `pom pty run <name>` process (setsid) hosting the command on a PTY behind a Unix socket, so it survives a pom restart and many clients can attach. Holder names are deterministic (`services.ServiceHolderName`/`WsServiceHolderName`, `sh-<wsKey>-…` for shells). Services run via `zsh -ic`; `pre_start` runs after `cd` before `cmd`. The web attaches over `/ws/pty`; liveness = holder alive (`ptyhost.HolderAlive`) + port; preview = holder scrollback (`ptyhost.Snapshot`). `pom ps` monitors holder CPU/RAM.

## Coding Rules (read before writing ANY code)

These are non-negotiable and apply to every language (Go + `web-ui/`). The
four pillars: **optimal, secure, clean, maintainable.**

### Comments — write almost none

**HARD RULE: a comment answers WHY or a trade-off — NEVER WHAT. Max one line
(a short sentence). No paragraphs. If it restates the code or narrates behaviour,
delete it.** Verbose comments are a defect; keep them terse and rare.

Comments cost tokens and rot. Default to **zero comments**; make the code
self-documenting instead (clear names, small functions).

- **Never comment WHAT the code does** — the code says that. No
  restating a signature, no narrating steps, no section banners.
- **Only comment WHY / a trade-off**, and only when non-obvious: a gotcha, an
  invariant, a workaround, a "why not the obvious way". **One line, max a short
  sentence** — never a multi-line paragraph explaining a design. If the reason
  is obvious from the code, say nothing.
- **No doc-comment blocks** on every function. A handler named
  `handleRefresh` needs no `// handleRefresh does X` — only add a note if
  there's a subtle contract a caller can't infer.
- Deleting code → delete its comments too. Never leave a comment explaining
  something that's gone.
- When editing existing over-commented code, trim the noise as you touch it.

### Optimal

- Web handlers never block on slow subprocess (docker/git/gh) — bounded,
  parallel, or background + serve-stale.
- No needless allocation/copies in hot paths; reuse buffers; prefer streaming.
- Frontend: poll politely (pause when hidden, batch/scope), one endpoint over N.

### Secure

- `exec.Command` with separate args — NEVER a shell string with user input.
  Multi-command pipelines: sanitize (`BranchSafe`) before building the string.
- Validate/clean paths, reject `..`. File perms `0o644`/`0o755`, never `0o777`.
- `sudo` only in `pom setup`. Runtime never elevates.

### Clean & maintainable

- Small, single-purpose functions. SOLID. DRY — extract on the 3rd copy.
- Match the surrounding code's style/naming/idiom exactly.
- Wrap errors with context (`fmt.Errorf("...: %w", err)`). No panics in libs;
  `fatal()` only in `main.go`.
- Interfaces defined by the consumer, only when a test needs a mock.
- Guard external/nil data at the boundary (Go nil slice → JSON `null` →
  frontend `.length`/`for..of` crash: return `[]`, guard with `|| []` /
  `Array.isArray`).

## Go Code Rules

### Structure & SOLID

- **One package per concern** — `config` parses YAML, `ptyhost` runs PTY holders, `services` handles infra. No circular imports.
- **`cmd/pom/main.go` = dispatch only** — no business logic, just parse args → call internal packages → print output.
- **Composition over embedding** — Config/Dir/Service are plain structs. Add methods for resolution. No deep type hierarchies.
- **Interfaces defined by consumer, not provider** — if `pipeline` needs to run a holder, it imports `services`/`ptyhost` directly (small project). Only add interfaces when testing requires mocking.
- **Receiver methods for behavior** — `(c *Config).ResolveService()`, `(m *Model).DoStart()`. Not standalone functions taking config as first arg.

### Error Handling

- **Wrap errors with context** — `fmt.Errorf("git worktree add for %s: %w", dirName, err)`. Never lose the original error.
- **Return errors from subprocess calls** — only use `_ =` for cleanup paths (`os.Remove` on teardown). Never ignore `exec.Command().Run()` in happy paths.
- **`fatal()` only in `main.go`** — internal packages return errors. Never `os.Exit()` from library code.
- **No panic** — use `log.Fatalf()` or return error. Panic only for "impossible" programmer errors.

### Security & Subprocess

- **`exec.Command()` only, NEVER shell interpolation** — pass args as separate strings: `exec.Command("git", "-C", dir, "checkout", branch)`. Never `exec.Command("sh", "-c", "git checkout " + branch)`.
- **Exception: multi-command pipelines** — when chaining `cd && source && export && cmd`, use `exec.Command("zsh", "-ic", fullCmd)` with pre-built string. Never interpolate user input directly — sanitize via `BranchSafe()` first.
- **Validate paths** — reject `..` in branch names before `filepath.Join`. Use `filepath.Clean()` on user-provided paths.
- **File permissions** — dirs: `0o755`, config/state: `0o644`, scripts: `0o755`. Never `0o777`.
- **Sudo only in `pom setup`** — runtime commands must never require sudo.

### DRY & Templates

- **Template resolution centralized** — `ResolveEnvTemplates`, `ResolveConfigTemplates`, `ResolveDBTemplates`, `ResolveSlotTemplates` in `services/services.go`. Never hand-roll `strings.ReplaceAll("{{bind_ip}}", ...)` outside these functions.
- **Extract on 3rd occurrence** — two similar blocks = ok. Third = extract function.
- **`BranchSafe()` for all branch→filename/dbname conversions** — never inline `strings.ReplaceAll(branch, "/", "_")`.

### Concurrency

- **Web handlers stay responsive** — heavy work (docker, git, gh) runs bounded/parallel or in the background; never block a request on a slow subprocess (see PR fetch: background warm + serve-stale).
- **Pipeline events via channel** — `RunCreatePipeline(ctx, ch)` / `RunDeletePipeline` send `Event` structs. Consumer (CLI prints; web streams over `/ws/workspace/{create,delete}`) reads channel.
- **File locks for shared state** — `WithProjectLock()` for `network.json`, `withSlotLock()` for `shared_slots.json`. Never read-modify-write without lock.
- **`sync.WaitGroup` for parallel stages** — `stageSourceParallel`, `stageConfigureParallel`. Collect errors via mutex.

### Web frontend (React, `web-ui/`)

- **State via Zustand + hooks** — `store.ts` for tabs/view/selection; data hooks (`useWorkspaces`, `useWorkspacePRs`) own polling.
- **Poll politely** — pause when the tab is hidden (`usePageVisible`), batch/scope requests, prefer one endpoint over N. Reads go through `api.ts`.
- **After editing `web-ui/`, run `npm run build`** (writes `internal/core/static/`) and commit those assets — the release binary embeds them.
- **Theme** — semantic CSS variables in `styles.css` driven by `data-theme`; never hardcode colors in components.
