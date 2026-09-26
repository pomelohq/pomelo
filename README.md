<p align="center">
  <img src=".github/assets/logo.png" width="120" height="120" alt="Pomelo">
</p>

<h1 align="center">Pomelo</h1>

<p align="center">
  A dev environment. One per branch.<br>
  A native macOS app for multi-repo projects - free and open source.
</p>

<p align="center">
  <a href="https://github.com/pomelohq/pomelo/releases/latest"><img src="https://img.shields.io/github/v/release/pomelohq/pomelo" alt="Release"></a>
  <a href="https://www.gnu.org/licenses/agpl-3.0"><img src="https://img.shields.io/badge/License-AGPL_v3-blue.svg" alt="License: AGPL v3"></a>
  <a href="#install"><img src="https://img.shields.io/badge/platform-macOS%2014%2B%20%7C%20Apple%20Silicon-lightgrey.svg" alt="Platform: macOS"></a>
  <a href="https://pomelohq.app"><img src="https://img.shields.io/badge/docs-pomelohq.app-d9b45b.svg" alt="Docs"></a>
</p>

<p align="center">
  <img src=".github/assets/app.png" width="820" alt="Pomelo: workspaces on the left, the code editor and a terminal on the right">
</p>

## About

Pomelo spins up a full, isolated, runnable environment for **every branch** of a multi-repo project:
services, databases and shared infrastructure, wired automatically. Each branch is a real git worktree with
its own services, ports and databases, so two branches never collide. No port juggling, no hand-written env
files.

## What's inside

- **One branch, one full stack.** Every workspace is a real git worktree with its own services, ports and
  databases. A workspace checks out only the repos its work needs.
- **Fast switches.** New workspaces clone databases from a prepared `main` and materialize dependencies with
  APFS copy-on-write: seconds, not rebuilds.
- **Native services, Docker only for data.** Your repos' services run as native processes on self-managed PTY
  holders that survive app restarts. Only databases and shared infra (Postgres, Redis, MinIO, OpenSearch) run
  in Docker.
- **A real editor.** A GPU-rendered code editor with tabs and splits, tree-sitter highlighting, language
  servers, project search, diffs and a Git panel, next to terminals that keep running.
- **Same-origin networking.** A built-in dev proxy serves a frontend and its backends under one origin (no
  CORS), and a webhook relay fans inbound events out to every branch.
- **Database browser.** Inspect and query each branch's Postgres and Redis without leaving the app.
- **Agent-ready.** Run Claude Code (or any AI CLI) in a workspace terminal, wired to that environment through
  pom's MCP tools, with its state (working, needs input, idle) shown per workspace.
- **Pull requests and tickets.** Each workspace shows its pull requests with checks and conversation, and its
  Jira ticket.

## Why Pomelo

Writing code is no longer the bottleneck; knowing a change is *correct* is. That needs ground truth, not
another opinion. Every branch has a real, running environment: database, services, ports, resolved env. So
when you (or an AI agent) make a change, you can **run it and know**, not guess from a diff.

The loop: **you reason, the agent types, the env proves, you judge.** Two control points stay human: the shape
of the commit or PR, and judging review feedback.

## Architecture

Pomelo is one Rust program. The app (`rust/crates/pomelo`) is a thin composition root over one crate per
feature: a GPU UI toolkit (`ui`, wgpu + winit), the workspace layout, the editor, and the core crates that
hold the logic (`pom_core`, `pom_config`, `pom_services`, `pom_workspace`, `pom_proxy`, `pom_mcp`, ...). The
`pom` CLI (`rust/crates/pom_cli`) drives the same crates. There is no daemon and no localhost server between
the UI and the engine.

The app re-runs its own binary for PTY holders, the MCP server and agent hooks, so nothing else has to be
installed. Config lives in a `pom.yml` (plus an optional `pom.d/` of fragments), found by walking up from the
current directory. See [`docs/config-schema.md`](docs/config-schema.md) and
[`docs/config-variables.md`](docs/config-variables.md).

## Install

Download the latest signed, notarized DMG from
[Releases](https://github.com/pomelohq/pomelo/releases/latest), drag **Pomelo** into **Applications** and open
it. macOS 14+, Apple Silicon. The app updates itself.

Full docs: **https://pomelohq.app**

## Build from source

Requires Rust (the toolchain in `rust/rust-toolchain.toml` installs itself through `rustup`), Xcode command
line tools, Docker and `zsh`.

```bash
make run      # build and open PomeloDev.app (runs alongside an installed Pomelo.app)
make check    # fmt, clippy -D warnings, tests: the gate CI runs
make prod     # build and open the release Pomelo.app
```

See [`RELEASE.md`](RELEASE.md) for how releases are cut.

## Contributing

Contributions are welcome. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the workflow, code style and the DCO
sign-off we require on every commit. By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Copyright (C) 2026 Toan Tran.

Pomelo is free software licensed under the [GNU AGPL-3.0](LICENSE). Commercial licensing is available from the
copyright holder.
