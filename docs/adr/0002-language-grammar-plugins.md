# ADR 0002 - Language grammar plugins (WASM tree-sitter)

Status: proposed
Date: 2026-09-15

## Context

The Files pane now has an in-app code editor (ADR-adjacent work). Two highlighting
engines exist:

- **Read-only preview** uses a hand-written regex highlighter (`CodeView`) that
  colors many languages approximately.
- **Edit mode** uses `CodeEditSourceEditor` (tree-sitter) — but the vendored
  `LocalPackages/CodeEditLanguages` was deliberately trimmed to **SQL only** to
  keep the binary small (the SQL editor was the sole original consumer).

We want Zed-quality syntax highlighting (accurate tree-sitter) for the languages
in our repos (Ruby, TypeScript/TSX, JavaScript, JSON, YAML, HTML, CSS, Markdown,
...) and, per product direction, the ability to **install more languages later**
without shipping a new app build.

Naive static vendoring was evaluated and rejected:

- Grammar `parser.c` files are large and getting larger: current
  `tree-sitter-ruby` `parser.c` is ~15 MB, `tree-sitter-typescript` / `-tsx`
  ~8.7 MB each. Bundling a full stack is tens to hundreds of MB of C source →
  large binary, slow compiles.
- Query files (`highlights.scm`) are pinned to specific grammar commits in
  CodeEditLanguages; pairing them with an arbitrary upstream `parser.c` breaks
  query compilation (unknown node types).
- Several grammars ship C++ external scanners (`scanner.cc`), adding build
  complexity.
- Downloaded **native** code (a `.dylib` per grammar) is the obvious "plugin"
  shape but is blocked by Gatekeeper/notarization: unsigned dynamic libraries
  loaded at runtime are refused on a notarized, hardened-runtime app.

The distribution problem — "add a language at runtime without native code
signing" — is exactly what WASM solves, and is how Zed ships language grammars.

## Decision

Introduce a **language grammar plugin system** built on **WASM tree-sitter
grammars** loaded at runtime. A "language plugin" is data, not native code:

```
<plugin>/
  manifest.json        name, extensions, grammar ABI version, comment tokens, ...
  grammar.wasm         tree-sitter grammar compiled to WASM
  queries/
    highlights.scm
    injections.scm
    locals.scm
```

Plugins live under `~/.local/state/pom/grammars/<language>/` and are resolved by
file extension at open time. A small set ships **built-in** (bundled in the app
bundle) so common languages work offline out of the box; the rest are installable
from a registry.

### Components

1. **WASM tree-sitter runtime.** tree-sitter's C library has a WASM feature
   (`ts_wasm_store_new`, `ts_parser_set_wasm_store`, `ts_wasm_store_load_language`)
   backed by a wasm engine (wasmtime). We link tree-sitter with the wasm feature
   and vendor a wasm engine, exposed to Swift through a thin C shim + Swift
   wrapper (SwiftTreeSitter today has no WASM API, so this is new binding work).

2. **Grammar store + resolver.** `GrammarStore` maps a file extension to an
   installed plugin, lazily instantiates its `TSLanguage` from `grammar.wasm`
   (cached per language), and loads its queries. Built-ins are seeded from the app
   bundle; user-installed ones from the state dir.

3. **Registry + downloader.** A static manifest (JSON, hosted alongside releases)
   lists available languages -> {wasm URL, queries URL, sha256, abi}. The
   downloader fetches + verifies (sha256) into the state dir. No code execution
   on install; WASM is sandboxed data.

4. **Editor integration.** Extend the editor path so a language can be supplied
   at runtime (a `Language` + query set) rather than only from the compile-time
   `CodeLanguage` enum. The read-only preview switches to the tree-sitter editor
   (read-only) when a grammar is available for the file, and falls back to the
   regex `CodeView` otherwise — so nothing regresses for unmapped types.

5. **UI.** A "Languages" settings section: list built-in + installed + available
   grammars, install/remove, show size. Optional: offer to install a grammar the
   first time an unknown file type is opened.

### Why WASM over the alternatives

- **vs. static vendoring:** no binary bloat, no per-grammar build, languages are
  added without an app release.
- **vs. downloaded dylibs:** WASM is data, so it sidesteps notarization/Gatekeeper
  entirely and is sandboxed (a malformed grammar can't crash/execute native code
  arbitrarily).
- Matches Zed's proven model, so grammar + query artifacts can track the same
  ecosystem.

## Phasing

- **P0 - runtime spike.** Link tree-sitter with the wasm feature + a wasm engine;
  a Swift shim that loads one `grammar.wasm` and parses a buffer. Validate size
  and parse correctness. This is the make-or-break unknown.
- **P1 - built-in stack.** Ship WASM grammars + queries for the core stack
  (Ruby, TS, TSX, JS, JSON, YAML, HTML, CSS, Markdown) in the app bundle; wire
  the resolver + editor integration; preview uses tree-sitter where available.
- **P2 - registry + install.** Manifest, downloader (sha256-verified), state-dir
  storage, "Languages" settings UI, install-on-open prompt.
- **P3 - depth (optional).** Injections (e.g. JS-in-HTML), per-theme query
  tuning, and reuse for other tree-sitter-driven features (structure outline,
  Cmd+F symbol jump).

## Risks / unknowns

- **SwiftTreeSitter has no WASM API.** We must bind tree-sitter's wasm C API
  ourselves and vendor a wasm engine (wasmtime C API, a few MB). P0 must prove
  this integrates cleanly with the SwiftPM/xcodebuild build used by `build.sh`.
- **`CodeEditSourceEditor` assumes compile-time languages.** Feeding a
  runtime-built `Language` + queries needs a small fork/extension of the local
  package (it already renders from a `Language` + `EditorTheme`, so the surface is
  small, but it is a change to a vendored dependency).
- **ABI/version matching.** grammar.wasm and its queries must come from the same
  grammar version; the manifest pins both + a tree-sitter ABI version, and the
  loader refuses mismatches.
- **wasm engine size / startup.** wasmtime adds a few MB and a one-time
  instantiation cost per language; grammars are instantiated lazily and cached.
- **Notarizing built-in `.wasm` resources** is fine (they are resources, not
  code), but confirm the hardened runtime does not object to the wasm engine's
  JIT (may require the allow-jit entitlement, or use the interpreter/Cranelift
  without JIT).

## Alternatives considered

- **Static vendoring of the full stack** - rejected (size, build time, query
  version matching, C++ scanners). See Context.
- **Downloaded native `.dylib` grammars** - rejected (Gatekeeper/notarization
  blocks unsigned runtime-loaded native code; sandbox/safety).
- **Pin a small, query-matched static grammar set for the core stack only** -
  viable as a *stopgap* if the WASM runtime spike (P0) proves too costly: it
  gives real tree-sitter for the core stack with a bounded (few-MB) size increase
  but no runtime install. Kept as the fallback, not the goal.

## Consequences

- One highlighting engine (tree-sitter) becomes the default wherever a grammar is
  available, preview and edit alike; the regex `CodeView` stays only as the
  fallback for unmapped types.
- Adding a language becomes a data operation (ship/download a plugin), not a code
  change — the same infrastructure can later host other tree-sitter features.
- New runtime dependency (a wasm engine) and a small fork of
  `CodeEditSourceEditor`/`CodeEditLanguages` to accept runtime languages.
