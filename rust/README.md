# Pomelo — Rust rewrite

Ground-up rewrite of Pomelo in Rust, cross-platform (Linux/macOS/Windows). This lives on the
`rust` branch, in parallel with the current Go + Swift app on `main`. We build the foundation
first, then port core functions gradually — referencing Zed's code as we go.

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

```
rust/
  crates/
    editor/   GPU editor core: rope buffer (+ multi-cursor, undo), tree-sitter highlighting,
              wgpu/glyphon renderer, user-configurable Theme. Has unit tests.
    app/      winit application (cross-platform window + input). Grows into the full UI:
              panes/tabs, file tree, terminal, then the ported core domains.
```

Run the editor: `cargo run -p pomelo -- <file>`. Test: `cargo test`.

## Theme

Colors are user-configurable (like Zed). `~/.config/pomelo/theme.json` overrides the built-in One
Dark; hex colors (`#rrggbb`), a `syntax` map of tree-sitter capture name -> color, plus
background/foreground/gutter/tab_bar/caret/selection. Missing file -> built-in One Dark.

## Roadmap (port gradually, reference Zed)

1. Foundation (this): cargo workspace, editor core in a crate, theme config, tests. [done]
2. App shell: winit window hosting the editor; pane/tab system (Zed `workspace/pane_group.rs`).
3. File tree: worktree walk + `notify` watcher + git status (Zed `worktree` + `project_panel`).
4. Terminal: `alacritty_terminal` grid + PTY, hosted in a bottom dock (Zed `terminal` + `terminal_view`).
5. Core port: workspaces / env resolution / docker / git worktrees / config (from Go `internal/*`).
6. Remaining panels: git/PR, agent, config/doctor, database.

See `docs/zed-architecture.md` for the full Zed crate map and per-subsystem deep-dive.
