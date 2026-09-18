# Pomelo — Rust rewrite

Ground-up rewrite of Pomelo in Rust, cross-platform (Linux/macOS/Windows). This lives on the
`rust` branch, in parallel with the current Go + Swift app on `main`. We build the foundation
first, then port core functions gradually.

## What the current system (main) has — what we need to port

**Go core** (`internal/`, `cmd/`): the business logic. Reached via `libpom` FFI (Swift app) and the
`pom` CLI. Key pieces to port:
- `internal/core` — the Server + per-domain logic (workspaces, onboarding, activity, run actions).
- `internal/services` — env resolution, port/slot allocation, docker, git worktrees, files, PTY holder spawn.
- `internal/ptyhost` — self-managed PTY holders (tmux-free): detached process per service/shell behind a Unix socket.
- `internal/pipeline` — staged workspace create/delete (event channel).
- `internal/config` — `pom.yml` parse, dot-notation template resolution, `pom.d/**` deep-merge.
- `internal/provider` — shell / tracker (jira) / forge (gh) / dbclient.
- `internal/{jira,secrets,sessions,workspace,mcp,doctor}` — supporting domains.
- `cmd/pom` — CLI dispatch; `pom mcp` portless stdio MCP server.

**Swift app** (`desktop/PomeloApp`): the SwiftUI UI to replace with a Rust UI —
workspaces list, files editor + tree, terminal, git panel, PR/GitHub management, agent (Claude) panel,
config/doctor, services home, database pane.

## Rust workspace layout

Crate-per-feature: the binary is named after the product (`pomelo`) and is a thin composition root — no
feature logic lives in it. Each feature is its own crate, and logic is split from its in-app UI as
`<feature>` + `<feature>_ui` (e.g. `auto_update` + `auto_update_ui`): the base crate holds pure logic/state
with no rendering, the `_ui` crate depends on `ui` + the base crate and only builds the view. Library
crates use `[lib] path` with a descriptive file name (`src/<crate>.rs`), never `lib.rs`/`mod.rs`.

```
rust/
  crates/
    pomelo/           the binary (main.rs + macOS platform glue). Composition root only.
    ui/               minimal GPU framework: colored rects + text on a wgpu surface.
    workspace/        layout/dock system (top bar, docks, pane/tab groups).
    editor/           GPU editor core: rope buffer (+ multi-cursor, undo), syntax highlighting, Theme.
    auto_update/      self-update logic: check GitHub Releases -> download -> swap -> relaunch.
    auto_update_ui/   the "update available / restart" banner built from auto_update's status.
    settings/         typed user config loaded from/saved to ~/.config/pomelo/settings.json.
    settings_ui/      the settings screen (overlay) built from settings::Settings. Cmd+, toggles it.
```

Run the editor: `cargo run -p pomelo -- <file>`. Test: `cargo test`.

## Build & release (macOS, Apple Silicon only)

Everything runs through the `Makefile` (targets wrap `scripts/*.sh`):

```
make run          # build + launch PomeloDev.app (dev, debug) — runs alongside a released Pomelo.app
make prod         # build + launch Pomelo.app (release)
make dmg          # Pomelo.app -> target/Pomelo-<ver>.dmg
make appcast      # target/Pomelo-<ver>.app.tar.gz + latest.json (auto-update payload)
make check        # fmt-check + clippy -D warnings + test (the CI gate)
make patch|minor|major   # bump workspace version, commit, tag rust-v<ver>
```

Dev vs prod are two distinct bundles so they coexist: `PomeloDev.app` (`app.pomelo.dev`) and
`Pomelo.app` (`app.pomelo`). Both currently ship **unsigned** — signing/notarization hooks are marked
`TODO(sign)` in `scripts/bundle-macos.sh` and `.github/workflows/rust-release.yml`.

**Release flow:** `make patch` bumps `Cargo.toml`, commits, and tags `rust-v<ver>` (kept distinct from
main's `v*` tags). Pushing that tag triggers `.github/workflows/rust-release.yml` (macos-14 / arm64):
build -> DMG + update tarball -> GitHub Release with the artifacts attached. `rust-ci.yml` runs the gate
on every push/PR under `rust/**`.

**Auto-update** (`crates/app/src/updater.rs`): on launch the *production* bundle asks the GitHub Releases
API for the newest `rust-v*`, and if it's higher than the running version it downloads the `.app.tar.gz`,
swaps it in, and relaunches. Dev bundle and `cargo run` never self-update. Disable with
`POMELO_AUTO_UPDATE=0`; override the source repo with `POMELO_UPDATE_REPO=owner/name` at build time.

## Theme

Colors are user-configurable. `~/.config/pomelo/theme.json` overrides the built-in One Dark; hex colors
(`#rrggbb`), a `syntax` map of tree-sitter capture name -> color, plus
background/foreground/gutter/tab_bar/caret/selection. Missing file -> built-in One Dark.

## Roadmap (port gradually)

1. Foundation (this): cargo workspace, editor core in a crate, theme config, tests. [done]
2. App shell: winit window hosting the editor; pane/tab system in `workspace`.
3. File tree: worktree walk + `notify` watcher + git status (`project_panel` + `project_panel_ui`).
4. Terminal: `alacritty_terminal` grid + PTY, hosted in a bottom dock (`terminal` + `terminal_ui`).
5. Core port: workspaces / env resolution / docker / git worktrees / config (from Go `internal/*`).
6. Remaining panels: git/PR, agent, config/doctor, database.

See `docs/architecture.md` for the target crate map and per-subsystem plan.
