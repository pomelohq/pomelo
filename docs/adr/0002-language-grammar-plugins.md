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

## Spike results (P0)

Validated the make-or-break unknown — running tree-sitter WASM grammars natively
in this toolchain:

- **Version pin.** tree-sitter 0.25.10 (what SwiftTreeSitter / CodeEditLanguages
  use) pins **wasmtime 29.0.1**. Its `wasm_store.c` does NOT compile against the
  latest wasmtime (v48): the C API changed (`wasmtime_func_t.__private`
  `uint32_t` -> `void*`, 8 errors). Against the **v29.0.1 C API it compiles
  clean**. So match wasmtime to tree-sitter, not to "latest" — upgrading wasmtime
  alone breaks the contract; upgrading tree-sitter would ripple through the whole
  editor stack.
- **Build + link + run works.** Compiling `lib/src/lib.c` with
  `-DTREE_SITTER_FEATURE_WASM` (it `#include`s `wasm_store.c`) plus the vendored
  `wasm/{stdlib-symbols.txt,wasm-stdlib.h}`, linking `libwasmtime` (v29), produces
  a binary that creates a `wasm_engine` + `TSWasmStore` and reaches
  `ts_wasm_store_load_language`. Must build with the **Xcode toolchain**, not
  CommandLineTools (the CLT SDK has the known `arm64e.x1` malformed-tbd bug).
- **Size.** wasmtime v29 `libwasmtime.dylib` is **13 MB** (v48 was 24 MB) — bundle
  the dylib, not the ~52 MB static archive.
- **Grammar pipeline proven end-to-end.** The npm `tree-sitter-wasms` artifact
  fails to load (`failed to parse dylink section` — its `dylink.0` is from a
  different tree-sitter). Building the grammar with the matching CLI instead —
  `npx tree-sitter-cli@0.25.10 build --wasm` (emscripten runs in the
  `emscripten/emsdk` docker image, a dev/CI tool, never shipped) — produced a
  5.6 KB `tree-sitter-json.wasm` that the spike **loaded and parsed correctly**:
  `(document (object (pair key: (string ...) value: (array (number) (number)
  (true) (null))) ...))`. Compiled grammar `.wasm` files are small (KB–low MB),
  far smaller than their C source (TS `parser.c` is 8.7 MB).

Conclusion: the WASM approach is fully validated — build (emscripten), bundle
(wasmtime v29 dylib), and runtime load+parse all work. No research risk remains;
what's left is app integration.

### Remaining P1 work (integration, no longer research-risky)

1. **Grammar artifacts.** A build step (local or CI) runs `tree-sitter build
   --wasm` per language to produce `<lang>.wasm` + copy queries, for the core
   stack (ruby, typescript, tsx, javascript, json, yaml, html, css, markdown).
   Emscripten stays in CI so contributors need no docker; the small `.wasm` files
   are committed/shipped.
2. **Runtime.** Add `libwasmtime` (v29 dylib) as a binary dependency and build
   tree-sitter with `-DTREE_SITTER_FEATURE_WASM` (a small patch to the vendored
   `TreeSitter` C target: include `wasm_store.c` + `wasm/` generated files, add
   the wasmtime header/lib paths). Confirm the hardened runtime accepts the wasm
   engine (JIT entitlement or interpreter).
3. **Loader + editor.** A `GrammarStore` (Swift) creating a shared engine +
   `TSWasmStore`, lazily instantiating a `TSLanguage` per file type from its
   `.wasm`, with queries; wire it into `CodeEditLanguages`/`CodeEditSourceEditor`
   so the editor accepts a runtime language. Preview switches to tree-sitter where
   a grammar exists, regex `CodeView` otherwise.
4. **Registry/install (P2)** builds on top once the built-in path works.

## Consequences

- One highlighting engine (tree-sitter) becomes the default wherever a grammar is
  available, preview and edit alike; the regex `CodeView` stays only as the
  fallback for unmapped types.
- Adding a language becomes a data operation (ship/download a plugin), not a code
  change — the same infrastructure can later host other tree-sitter features.
- New runtime dependency (a wasm engine) and a small fork of
  `CodeEditSourceEditor`/`CodeEditLanguages` to accept runtime languages.
