# Pomelo (Rust) - guidelines

Scope: the `rust/` workspace, which is all of Pomelo (the app and the `pom` CLI). Read before
touching anything here. These override defaults.

## Rust

- Prioritize correctness and clarity. Speed/efficiency are secondary unless asked.
- Comments explain **why** (a trade-off, gotcha, invariant, external constraint), one short line, only when
  non-obvious. Default to zero comments; never restate what the code says. Delete a comment when its code goes.
- No panics in library code: avoid `unwrap()`/`expect()`/blind indexing — propagate with `?` or handle. In the
  binary, `expect()` is acceptable only for true startup invariants (window/GPU creation).
- Never silently discard a fallible result with `let _ =` (only for genuine teardown). Handle it: `?`, `match`,
  `if let Err(...)`, or log.
- Never create `mod.rs`. Use `src/<module>.rs`. New crates set `[lib] path = "src/<crate>.rs"` (a descriptive
  name), never `lib.rs`.
- Full words for variable names (no `q` for `queue`).
- Prefer editing existing files over spraying many tiny ones. Avoid creative additions unless requested.
- Use variable shadowing to scope clones in async contexts.

## Architecture / crates

- **Crate-per-feature.** The binary crate `pomelo` is a thin composition root — window creation + macOS
  platform glue only, no feature logic. Every feature is its own crate; split logic from its in-app view as
  `<feature>` + `<feature>_ui` (the base crate is pure logic/state with no rendering; the `_ui` crate depends
  on `ui` + the base and only builds the view). Create the `_ui` crate once the feature has a real screen.
- Shared toolkit crates have no `_ui` twin: `ui` (GPU primitives + the `div()/label()` element tree),
  `workspace` (layout/dock), `editor` (editor core).
- Build screens with the `ui` element tree (`div().col().child(label(...))`), not hand-placed rects/text.
- A screen that must float above everything is its **own OS window**, not an overlay drawn into the main
  window — `ui.render` draws all rects then all text, so an in-window overlay lets the base layer's text bleed
  through. Settings is a separate window (Cmd+,).
- Do not attribute designs to any external editor in code, comments, PRs, or chat.

## The gate — run before calling anything done

`make check` = `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test`. All must pass.
Our crates must be warning-clean; vendored deps are cap-lint-allowed so their warnings don't fail the gate.

## Build / run / release (macOS, Apple Silicon only)

- `make run` builds + launches **PomeloDev.app** (kills the previous dev instance first). Dev and prod are
  distinct bundles that coexist: `PomeloDev.app` (`com.pomelo.app.dev`) and `Pomelo.app` (`com.pomelo.app`,
  the same id as the earlier Swift app, so installs update in place and keep their permissions).
- Local builds are unsigned. `scripts/bundle-macos.sh` signs (hardened runtime) when `SIGN_ID` is set and
  `scripts/make-dmg.sh` notarizes + staples when `NOTARY_PROFILE` is set; the release workflow sets both.
- Release: `make patch|minor|major` on `main` bumps the workspace version, commits, tags `v<x>` and pushes.
  The tag triggers `.github/workflows/release.yml` (build -> sign -> notarized DMG, update tarball + `.sig`,
  Sparkle `appcast.xml` -> GitHub Release). The updater (`auto_update`) installs only a tarball whose `.sig`
  verifies against the Sparkle public key. `.github/workflows/ci.yml` runs `make check` on every push/PR
  touching `rust/**`; its `CI Gate` is the required check.

## Vendored deps

- `vendor/wgpu-hal` is a **one-line change**: `present_with_transaction: true` so live-resize frames swap
  atomically with the window (smooth resize). Re-apply the same edit if wgpu is upgraded. See
  `vendor/wgpu-hal/src/metal/surface.rs`.

## Assets

- Bundle the UI font, don't rely on system fonts (wrong optical size at small sizes): IBM Plex Sans (OFL) in
  `crates/ui/assets`, embedded via `include_bytes!`. Keep `OFL.txt` alongside it.
- The Dock/app icon is `crates/pomelo/assets/AppIcon.icns`.

## Absolute rules

- NO EMOJI anywhere: code, comments, commits, PRs, UI, CLI output. Plain ASCII (`-`, `...`, `>`); no decorative
  unicode in user-facing strings.
- PR titles: imperative, correctly capitalized, no conventional-commit prefixes (`fix:`/`feat:`), no trailing
  punctuation; optionally prefix with the crate name (`settings_ui: ...`).
