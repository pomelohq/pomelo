# Contributing to Pomelo

Thanks for helping improve Pomelo. It is one Rust workspace under `rust/`: the macOS app and the `pom` CLI,
built from one crate per feature. The app is a thin composition root; logic lives in the feature crates, and
each feature's view in its `_ui` crate. Read `rust/CLAUDE.md` for the layout and rules.

## Setup

Requirements: macOS 14+ on Apple Silicon, Xcode command line tools, `rustup` (the pinned toolchain in
`rust/rust-toolchain.toml` installs itself), Docker for shared services, and `zsh`.

```bash
git clone https://github.com/<your-fork>/pomelo.git && cd pomelo
make run      # build and open PomeloDev.app; it runs alongside an installed Pomelo.app
```

The dev build is a real `.app` bundle (not a bare binary), so bundle-dependent features (notifications,
updates) behave like the shipped app.

## Build and test

```bash
make check    # cargo fmt --check, cargo clippy --all-targets -D warnings, cargo test
```

This is exactly what CI runs; run it before opening a PR. Tests must not start login shells or leave
processes behind: terminal tests use a plain `/bin/sh`, service tests kill their holders on drop.

## Code style

Match the surrounding code. The non-negotiables (full rules in `rust/CLAUDE.md`):

- **Almost no comments.** A comment explains *why* (a trade-off, gotcha, constraint), never *what*; one line,
  only when non-obvious. Prefer clear names.
- **No panics in library code**: no `unwrap()`/`expect()`/blind indexing; propagate with `?` or handle it.
  Never discard a fallible result with `let _ =` outside genuine teardown.
- **Crate per feature**: a new feature is its own crate (`<feature>` + `<feature>_ui` once it has a screen),
  `[lib] path = "src/<crate>.rs"`, never `mod.rs`.
- **UI** is built with the `ui` element tree (`div().col().child(label(...))`) and the shared components and
  theme tokens, not hand-placed rects or new colors.
- **Processes** get separate args, never a shell string built from user input; network calls have timeouts.

## Commits

Short imperative subject, capitalized, no conventional-commit prefix, no trailing period
(`Scroll the workspace list`, `pom_config: check edits before saving`). **No emoji** anywhere.

Every commit must be **signed off** (DCO, below):

```bash
git commit -s -m "Fix the palette losing keys"
```

## Pull requests

Branch off `main`, one logical change per PR, `make check` green.

- [ ] `make check` passes
- [ ] UI touched: checked in `make run`
- [ ] User-facing change: docs updated in [`pomelo-docs`](https://github.com/pomelohq/pomelo-docs)
- [ ] Commits signed off (`-s`), no emoji

## Project layout

```
rust/crates/pomelo/        the app: windows, macOS glue, wiring (no feature logic)
rust/crates/pom_cli/       the pom CLI
rust/crates/ui/            GPU UI toolkit: wgpu/winit rendering + the element tree
rust/crates/workspace/     window layout: docks, pane groups, tabs, palette, WORKSPACES panel
rust/crates/editor/        editor core: buffer, syntax, display
rust/crates/*_ui/          a feature's views (files, git, services, database, settings, ...)
rust/crates/pom_*/         core logic: config, services, workspaces, PTY holders, proxy, MCP, agents, ...
rust/vendor/wgpu-hal/      vendored with a one-line change (see rust/CLAUDE.md)
docs/                      config reference (schema and template variables)
```

## Developer Certificate of Origin (DCO)

We do not use a CLA. Sign off every commit to certify you wrote the change (or have the right to submit it)
under the project's license, per the [Developer Certificate of Origin 1.1](https://developercertificate.org/).
`git commit -s` appends:

```
Signed-off-by: Your Name <you@example.com>
```

The name and email must be real and match your Git identity.

## Reporting bugs

Open a [GitHub issue](https://github.com/pomelohq/pomelo/issues) with your macOS version, the Pomelo version
(Settings > General, or `pom version`), reproduction steps, and the relevant `pom.yml` shape (redact secrets
and real names). For security issues, contact the maintainer privately.

## License

Contributions are licensed under [AGPL-3.0](LICENSE).
