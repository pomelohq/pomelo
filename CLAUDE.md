# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Pomelo is

A native macOS app that spins up a full, isolated, runnable dev environment for **every branch** of a
multi-repo project: services, databases, shared infra, each branch a real git worktree with its own ports and
databases. It is one Rust workspace under `rust/`: the app (`crates/pomelo`) and the `pom` CLI
(`crates/pom_cli`) over the same feature crates. There is no daemon and no localhost server between UI and core.

**Read `rust/CLAUDE.md` before touching code**: crate-per-feature layout, Rust rules, the `make check` gate,
build/run, vendored deps.

## Commands

```bash
make run        # build + open PomeloDev.app (dev bundle, coexists with Pomelo.app)
make prod       # build + open the release Pomelo.app
make check      # cargo fmt --check + clippy -D warnings + cargo test (the gate CI runs)
make snapshot   # render a settings page headless (ui_snapshot) for visual checks
```

All targets forward to `rust/`. Tests must not spawn login shells (PTY exhaustion); terminal tests set a
plain `/bin/sh`.

## Release (CI-only)

`make patch|minor|major` on an up-to-date `main`: bumps `rust/Cargo.toml`, commits `release: v<x>`, tags
`v<x>`, pushes. The tag runs `.github/workflows/release.yml`: build, Developer ID sign, notarize the DMG, sign
the update tarball and the Sparkle `appcast.xml` with the Sparkle EdDSA key, publish the GitHub Release. **No
local publish path.** Details and required secrets: `RELEASE.md`. Never delete old releases; never regenerate
the Sparkle EdDSA key (installed apps verify updates with its public half).

Before cutting a release, run the `release-audit` skill: release notes and the Sparkle feed come from the
`## [<version>]` block of `CHANGELOG.md`, which must be curated and committed before the tag is pushed.

**Never touch `CHANGELOG.md` in a fix/feature PR.** It is user-owned: do not add, edit or restore the
`## [Unreleased]` block while coding. Only curate the `## [<version>]` block at release time, when asked.

## Architecture (big picture)

- **Crate-per-feature.** `crates/pomelo` is a thin composition root (windows + macOS glue). Each feature is a
  crate, split into logic (`<feature>`) and its view (`<feature>_ui`). Toolkit crates: `ui` (wgpu/winit GPU
  primitives + the `div()/label()` element tree), `workspace` (layout, docks, panes), `editor`.
- **Core crates**: `pom_config` (pom.yml + pom.d parse, templates, validation, maintenance edits), `pom_core`
  (projects, scaffolding, adding/removing repos), `pom_services` (service runner, env files, ports, shared
  Docker services), `pom_workspace` (staged create/delete), `pom_ptyhost` (self-managed PTY holders),
  `pom_proxy` (dev proxy + webhook relay), `pom_mcp` (stdio MCP server for agents), `pom_agent` (agent
  launch, hooks, state), `pom_detect` (stack detection for new projects), `pom_doctor`.
- **Self re-exec.** The app runs its own binary for `pty`, `mcp`, `claude-hook` and friends, so it needs no
  external `pom` installed.
- **PTY holders.** Every service and shell is a detached holder process behind a Unix socket, surviving app
  restarts; names encode their kind (`svc-`, `ws-`, `appsh-`...).
- **Env goes in, not sourced.** Services get their resolved env injected at spawn; env files are generated
  from the config. Never hand-write `source .env.local` in a shell string.
- **Config templates are dot-notation only** (`{{shared.postgres.url}}`, `{{db.main}}`, `{{api.web.url}}`,
  `{{secret.NAME}}`, `{{branch.safe}}`), validated at load. Never author colon forms or `proxy:` / `webhook:`
  blocks. Reference: `docs/config-schema.md`, `docs/config-variables.md`.

## Docs

Docs live in a separate repo, `pomelohq/pomelo-docs` (VitePress, https://pomelohq.app/). Any user-facing
change (feature, config, CLI, behavior-changing fix) ships a matching docs update the same day. Never
document an unshipped feature.

## Rules

- **Comments:** why only, one short line, only when non-obvious. Default to none; prefer clear names.
- **Security:** processes with separate args, never a shell string built from user input; anything touching
  the network has a timeout; validate paths and reject `..`; file perms `0o644`/`0o755`; secrets are never
  stored in config (store an env-var name, read it at use time).
- **Never touch a user's real project.** When pointed at a real project, only read its config; output a diff
  for the user to apply. In docs, examples, commits and screenshots use placeholders (`myproject`, `api`,
  `web`, `feat-login`, `PROJ-101`), never real repo names, branches, URLs, ticket ids or credentials.
- **No emoji** in code, UI, commits or PRs.
