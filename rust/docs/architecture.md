# Pomelo (Rust) architecture

Target layout for the Rust rewrite: a cross-platform (currently macOS/Apple-Silicon) GPU app that replaces
the Swift/AppKit shell and, over time, ports the Go core.

## Crate conventions

- The binary crate is named after the product (`pomelo`) and is a **thin composition root** — it wires
  feature crates together and owns platform glue (window, Dock icon, traffic lights). No feature logic.
- **One crate per feature.** Split pure logic/state from its in-app view:
  - `<feature>` — logic and state, no rendering. Depends only on what the logic needs.
  - `<feature>_ui` — the view. Depends on `ui` + `<feature>`; turns state into rects/text (and later
    interactive elements). Only created once a feature actually has an on-screen surface.
- Shared low-level crates have no `_ui` twin because they *are* the toolkit: `ui` (GPU primitives),
  `workspace` (layout/dock), `editor` (editor core).
- Library crates use `[lib] path = "src/<crate>.rs"` (descriptive name), never `lib.rs` / `mod.rs`.

## Current crates

| Crate | Role |
|---|---|
| `pomelo` | binary; composition root + macOS platform glue |
| `ui` | GPU framework: colored rects + text on a wgpu surface (glyphon) |
| `workspace` | layout/dock system: top bar, docks, and (next) pane/tab groups |
| `editor` | editor core: rope buffer, multi-cursor, undo, tree-sitter highlighting, Theme |
| `auto_update` | self-update logic: check GitHub Releases, download, swap, relaunch |
| `auto_update_ui` | "update available / restart" banner built from `auto_update` status |

## Subsystems to build (each as `<feature>` + `<feature>_ui` where it has a view)

| Subsystem | Crates | Notes |
|---|---|---|
| Editor | `editor` (+ a pane host in `workspace`) | rope + display map + selections + scroll; per-frame: layout -> prepaint (visible rows, line layouts) -> paint (bg, gutter, text, highlights, cursors, scrollbars) |
| Pane / dock | `workspace` | recursive flex layout over a `Pane | Axis` tree; docks host panels; tab bar + active item per pane |
| Terminal | `terminal` + `terminal_ui` | PTY + `alacritty_terminal` grid in the base; the view batches visible cells into text runs, paints bg/text/cursor/selection |
| File tree | `project_panel` + `project_panel_ui` | fs walk + `notify` watcher + git status in the base; the view is a uniform list (indent, icon, name, git badge) |
| Core port | new crates mirroring Go `internal/*` | workspaces, env resolution, ports/slots, docker, git worktrees, config, ptyhost |

## Per-frame flow (today)

`winit` event -> `pomelo` updates `workspace::Layout` state -> `Layout::build(w, h)` returns `Vec<Rect>` +
`Vec<Text>` -> `ui::UiRenderer::render` fills quads (wgpu) and glyphs (glyphon) to the surface. Live-resize
is smooth because the surface presents transaction-coupled (see the vendored `wgpu-hal` patch).
