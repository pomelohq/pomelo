# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Pomelo is

A native macOS app that spins up a full, isolated, runnable dev environment for **every branch** of a multi-repo project — services, databases, shared infra, each branch a real git worktree with its own ports/DBs. The product **is** the SwiftUI app (`desktop/PomeloApp`); it links the Go core via `libpom` (c-archive FFI) and is **portless** — no dashboard, no HTTP server, no browser UI. There is also a plain `pom` CLI (same core). The old React web UI, `internal/web/`, TUI, and `pom daemon`/`pom update` are all gone.

## Commands

```bash
make build          # go build -o pom ./cmd/pom
make test           # go test ./...
make vet            # go vet ./...
make check          # build + vet + test (the Go gate)
make app            # build the native app (desktop/PomeloApp/build.sh: Go c-archive → SwiftPM link)
make app-run        # build + launch the app
make dmg            # local signed DMG for testing (never publishes)
```

- Single Go test: `go test ./internal/services/ -run TestEnvFileEntries`
- App unit tests: `cd desktop/PomeloApp && xcodebuild test -scheme PomeloApp -destination 'platform=macOS,arch=arm64' -derivedDataPath .ddata -skipPackagePluginValidation` (ViewModels vs `MockPomAPI`; add `-only-testing:PomeloAppTests/<Suite>` to scope). NOTE: `swift test` does NOT work — the CodeEditSymbols dep needs Xcode's resource pipeline. Needs `Vendor/libpom.a` (run `build.sh` once).
- Requires: Go 1.26+, `zsh`, Xcode + `codesign` (macOS, Apple Silicon). No tmux — services/shells run on self-managed PTY holders.

## Release (CI-only)

```bash
make patch          # bump BOTH version consts → commit → tag v<x> → push
make minor / major
```

`make patch/minor/major` bumps `cmd/pom/root.go` `version` **and** `cmd/libpom/libpom.go` `appVersion` in lockstep (`make version-check` guards drift). Pushing the `v*` tag is the entire release: `.github/workflows/release.yml` → `app-build.yml` (`publish:true`) builds → signs → notarizes → DMG → Sparkle appcast → GitHub Release. **No local publish path.** The app self-updates via Sparkle. Full details + required CI secrets in `RELEASE.md`. Never delete old releases; never regenerate the Sparkle EdDSA key.

## Architecture (big picture)

**One Go core, two front doors.** `internal/core` holds the `Server` + business logic. It is reached two ways, never over HTTP:

- **Native app** (`desktop/PomeloApp`, SwiftUI) → `libpom` FFI → `internal/core` → `internal/services` / `internal/pipeline` / `internal/ptyhost`.
  - Reads/actions: typed per-domain bindings in `cmd/libpom/bindings.go` (`//export Pom<Domain>` → a `Server.<Method>`) → Swift `PomCore.<domain>Data()`. Adding an endpoint = extract a `Server` data method + one `//export` + one `PomCore` method. There is **no generic `/api` bridge** — the app must not feel client-server.
  - Streams (PTY / Claude / pipeline) → direct C-FFI (`cmd/libpom/stream.go` `PomStream*`).
- **CLI**: `cmd/pom/main.go` (dispatch only) → `internal/services` → `internal/ptyhost`.
- **MCP**: `pom mcp` is a portless stdio MCP server (hand-rolled JSON-RPC) exposing a workspace's env to agents (ports/db/services/run_in_env/config/doctor). It builds its handler in-process from the `pom.yml` found by walking up from CWD — no running app needed. Auto-registered into Claude windows. Agent-facing endpoints: `internal/core/mcp_endpoints.go`.

The app re-execs its **own bundle binary** for the `pty` / `mcp` / `prepare-main` / `claude-hook` subcommands (`pombin.Path() = os.Executable()`), so it needs no external `pom` installed.

### Go layout

```
cmd/pom/main.go        — CLI entry, dispatch only (no business logic)
cmd/pom/cmd_*.go       — one cobra command per file (start, workspace, db, run, mcp, onboard, …)
cmd/libpom/            — c-archive FFI: bindings.go (typed reads/actions), stream.go (PomStream*), libpom.go (appVersion)
internal/
  core/                — the Server + all business logic (feature file per domain; mcp_endpoints.go, onboard.go, activity.go, control.go, run_actions.go, …)
  services/            — infra layer / side effects: env resolution, port/slot allocation, docker, git, files, holder spawn + the Holder interface
  ptyhost/             — self-managed PTY holders (tmux-free): a detached process per service/shell behind a Unix socket, ring-buffer scrollback, multi-client attach/resize
  pipeline/            — staged workspace create/delete (runner + Event channel)
  config/              — pom.yml parse, dot-notation template resolution, load-time validation; pom.d/**/*.yml deep-merge
  provider/            — shell (zsh variants), tracker (jira), forge (gh/git host), dbclient
  agent/               — built-in Claude Code agent registry + hooks; agent/claude the managed CLI launcher
  jira/ archive/ mcp/ stream/ secrets/ lock/ sessions/ workspace/ appstate/ tmpl/ paths/ pombin/ doctor/
desktop/PomeloApp/Sources/PomeloApp/{App,Features,Components,Core,ViewModels}
```

### Native app (MVVM, testable, 120fps)

- SwiftUI Views hold a `@StateObject` ViewModel (`ViewModels/*`); ViewModels depend on the `PomAPI` protocol (`Core/PomAPI.swift`, `PomCore` conforms) so tests inject `MockPomAPI`. New screen work: put fetch/decode logic in a ViewModel + a test, not the View.
- Must stay at 120fps: keep FFI off the main thread, keep binding bodies cheap.
- Go nil slice marshals to JSON `null`; Swift synthesized `Decodable` does **not** apply a property default for a present-but-null key (only for absent) → return `[]` not nil from Go for any array the app decodes.

## Key subsystems

### PTY holders + the Holder interface

Every service/shell runs as one detached ptyhost holder: a `pom pty run <name>` process (`Setsid`) hosting the command on a PTY behind a Unix socket, surviving app restarts with many clients attachable. Names are deterministic and their **prefix determines behavior**, classified in one place — `services.HolderFor(name) services.Holder` (`internal/services/holder_kind.go`):

- `ServiceHolder` (`svc-`, `ws-` incl. `claude-raw`): managed service, env-injected, **not reaped**, not auto-spawned on attach.
- `ShellHolder{kind}` (`appsh-` terminal tab, `sh-` shortcut/editor, `reposh-` repo shell): ephemeral, **reapable**; a terminal tab auto-spawns `zsh -i` on attach; a shortcut sets `KeepOpen`.

Use the interface (`Reapable()`, `AutoSpawnOnAttach()`, `KeepOpen()`, `Display()`) — never re-scatter prefix `strings.HasPrefix` checks.

### Env goes in, not sourced

Holders get their resolved workspace env **injected** via `services.SpawnHolderEnv(name, cwd, cols, rows, argv, env)` (`cmd.Env = os.Environ()+env`), where `env` comes from `services.ResolveServiceEnv` (per-service) or `services.ResolveRepoEnv` (repo-level). Direct-exec paths use `services.RunTimeoutEnv`. **Do not** hand-write `source .env.local` in a shell string — that hardcodes a filename that config can override and silently loads nothing. Shell argv comes from `provider/shell`: `Login`=`zsh -lc`, `Command`=`zsh -c`, `Interactive`=`zsh -i`.

### Ports / network state

`.pom/network.json` (per-project): `slot`, `blocks` (wsKey→block), `service_map` (svcKey→index), `shared_map`. Global: `~/.local/state/pom/{slots,shared_slots}.json`. Port registry is single-writer `ports.d/<port>` (O_EXCL). Never hand-join `"workspace--"+branch` — use `services.WorkspaceRootDir` / `RepoWorktreePath`. Always resolve env/hostnames/db-names off the **workspace branch** (folder `workspace--{branch}`), which may differ from the git branch.

### Config templates (dot-notation only)

Resolved in `services/resolve_v2.go`; validated at load (`config.Validate` — a typo fails loudly). Reference: `docs/config-variables.md` + the `configVarReference` const in `internal/core/claude_prompt.go` (injected into every agent prompt).

- `{{shared.<name>.url}}` (+ `.host/.port/.user/.pass/.slot`) — shared service.
- `{{db.<name>}}` / `{{db.<name>.url}}` — named DB (session-prefixed, branch-resolved).
- `{{<repo-alias>.<service>.url}}` / `.path` / `.host` / `.port` — another repo service (dev-proxy aware; `.path` = same-origin `/_pom_dev/<repo>/<svc>`); `environments:<profile>` switches local↔remote.
- `{{secret.<NAME>}}`, `{{slot.<name>}}`, `{{branch.safe/host/hash}}`, `{{bind_ip}}`(=127.0.0.1).

**Never author colon forms** (`{{conn:x}}`, `{{db:x}}`, `{{branch_safe}}`, …) — dot-notation only; migrate any you find. **Routing is system-managed** — never author a `proxy:` or `webhook:` block; Pomelo auto-routes `/_pom_dev/<repo>/<svc>` and fans out webhooks. Well-known shared services (`postgres`/`redis`/`minio`/`opensearch`) get built-in defaults (`config/shared_defaults.go`). Config may be split into `pom.d/**/*.yml` (deep-merged, order-preserving); `pom config split` produces the tidy layout.

## Docs

Docs live in a **separate repo** `pomelohq/pomelo-docs` (VitePress → https://pomelohq.app/), NOT here. Any user-facing change (new/changed feature, config/CLI/template change, behavior-changing bug fix) MUST ship a matching docs update same day. Purely internal refactors don't. Never document an unshipped feature.

## Coding rules

### Comments — write almost none
A comment answers **WHY / a trade-off, never WHAT**, and is **one short line**, only when non-obvious (a gotcha, invariant, workaround). Default to zero; make names self-documenting. No doc-comment blocks on every function. Delete a comment when its code goes. Trim over-commented code as you touch it.

### Go
- `cmd/pom/main.go` is dispatch only; business logic in `internal/*`. One package per concern, no circular imports. Receiver methods for behavior, not free functions taking config first. Interfaces defined by the consumer, only when a test needs a mock.
- Wrap errors with context (`fmt.Errorf("...: %w", err)`). No panics in libs; `fatal()`/`log.Fatalf` only in `main.go`. Never ignore a subprocess error on a happy path (`_ =` only for teardown).
- `s.cfg()` returns the config via an atomic pointer (hot-swapped on reload) — read through it; never mutate the shared `*config.Config` in a handler (put per-run state on `Server` behind a mutex).
- Centralize template/branch conversions: `ResolveEnvTemplates` etc. and `BranchSafe()` — never inline `strings.ReplaceAll(branch, "/", "_")` or `strings.ReplaceAll("{{...}}", ...)`. Extract on the 3rd copy.
- Pipeline stages emit `Event` structs over a channel; parallel stages use `sync.WaitGroup` + a mutex for errors. File-lock shared state (`WithProjectLock`, `withSlotLock`).

### Security
- `exec.Command` with **separate args**, never a shell string with raw user input. Multi-command pipelines (`cd && source && cmd`) build the string only after sanitizing branch input via `BranchSafe()`.
- Anything touching the network (git remotes, `gh`, docker pulls) MUST use `services.RunTimeout` / `RunTimeoutEnv` / `exec.CommandContext` — a bare `exec.Command` can hang forever.
- Validate paths, reject `..`. Perms `0o644`/`0o755`, never `0o777`. Secrets are never stored in config — store an env-var **name** (`token_env`) and read it at use time; a `Resolve(cfg)` returns nil when unconfigured so the feature silently no-ops.
- `sudo` is allowed **only** in `pom setup`. Runtime commands never elevate.

### Never touch a user's real project
When Pomelo is pointed at a real project, only `Read` its `pom.yml` for context — output a diff for the user to apply, never edit it. In public docs/examples/commits/screenshots use generic placeholders (`myproject`, `api`, `web`, `feat-login`, `PROJ-101`) — never a real repo name, branch, URL, ticket id, or credential-shaped value. No emoji in PRs/commits/UI.
