# ADR 0001 - Pom UI Contract

Status: accepted
Date: 2026-08-27

## Context

The core (`internal/core`) is reached by the native app through a c-archive FFI
(`cmd/libpom`). Today each feature adds its own `//export Pom<X>` returning an
ad-hoc `map[string]any`, decoded by a hand-written Swift `Codable` model. 86
exports exist. Two structural problems follow:

- **DTO drift.** The response shape is declared twice (Go map + Swift model) with
  no link between them. Go `omitempty` drops a key on success, and Swift's
  synthesized `Decodable` throws on the absent key even when the property has a
  default. This has produced the same class of bug repeatedly (usage chip,
  update-main sheet, nm-store).
- **No transport discipline.** Reads, actions, and live data are mixed. Some live
  data is a real FFI stream; some is polled from the UI on a timer (usage,
  agent-states), which duplicates work across windows and adds latency.

We want one core to serve many UIs (SwiftUI now, a second native runtime later)
through a small, explicit, versioned contract. The reference pattern is a core
shipped as a thin C-ABI library (opaque handles + `*_free`) where the frontend
registers a couple of callbacks and the core drives them; we take the contract
shape and drop any rendering surface, since our UIs render themselves.

## Decision

Define the **Pom UI Contract**: a small, versioned boundary with three operation
kinds and one DTO source of truth.

### Layers

```
View            dumb, state-driven, platform-idiomatic
Store / VM       holds a projection of core state, reduces streams, issues commands
Transport adapter  the ONLY platform-specific glue: FFI <-> UI language
Core             internal/core, single source of truth
```

Only the transport adapter is platform-specific. Store/VM is thin and tested
against a mock. Views never touch the FFI directly.

### Three operation kinds

- **Query** - read a snapshot. Request/response JSON.
- **Command** - mutation. Request/response wrapped in a fixed envelope
  `{ "ok": bool, "error"?: string, "data"?: <T> }`.
- **Subscribe** - live data. The core is the producer; the adapter delivers events
  to the Store on a background executor. Nothing in the UI polls.

Live data uses a `wakeup + pull` shape (cheap signal, then fetch the delta) or an
event carrying the delta directly - a cheap wake signal plus one outbound event
callback. Existing UI polls (usage, agent-states) become subscriptions.

**Target shape (why the surface stays small).** A core-as-library keeps a tiny C
surface because it is sized by interaction verbs, not features: opaque handles hide
the domain, and every core->UI event rides one tagged-union callback instead of a
new symbol. Pomelo's 86 `Pom<X>` exports grow with features because they route by
symbol. The target is to route by data - a fixed handful of verbs
(`PomQuery(domain, params)`, `PomCommand(domain, action, params)`,
`PomSubscribe(topic)`, one outbound action callback) with the variety in the DTO,
so adding a feature is a Go handler + a DTO, not a new export. This is the LSP
model. Migration to it is incremental; new work should prefer these verbs.

### DTO source of truth

Go structs are the source. Swift (and any future UI language) models are generated
from them, not hand-written, under fixed rules:

- Go `omitempty` or pointer field -> Optional on the UI side.
- A Go nil slice marshals to `[]`, never `null`, for any array the UI decodes.
- Every response is a named Go struct, not `map[string]any`, so it has one shape.

A **contract test** runs in CI Gate: each DTO is marshaled by Go (zero value and a
populated value) to a golden fixture, and the UI decodes each fixture without
throwing. Core and UI cannot drift silently.

### Threading

The transport adapter and Store run off the main thread. Decode and reduce happen
in the background; only the final reduced delta is applied to `@Published` on the
main thread. The main-thread constraint lives in one place, not scattered.

## Where logic lives (domain vs UI/UX)

The core owns **domain logic**; the frontend owns **UI/UX logic**. Both are
genuinely complex — just on different axes. The core carries business
correctness; the frontend carries experience. The frontend is not a dumb pipe.

**Core (Go) decides — every adapter shares it:**
- Domain state and its derivation: service status, which workspaces are
  unoptimized, PR health, DB results.
- Classification / state machines: agent-state meaning, PR-state -> severity,
  sort order, filters, derived flags.
- **View-ready DTOs**: the core hands back what the UI renders, including a
  semantic status token (`ok` / `warn` / `danger`), never a color value.

**Frontend renders and interacts — its own real complexity:**
- Layout, sizing, animation, gestures/reorder, scroll, focus, keyboard, 120fps.
- Theme mapping: token `danger` -> the platform color for the active theme. This
  is rendering, not a decision.
- Ephemeral interaction state: selected tab, drawer open/height, hover, scroll
  position, window layout. This is inherently per-UI and stays in the frontend.

The rule: **the core decides, the frontend presents and interacts.** A new
adapter reimplements presentation + interaction, never domain logic. Concretely,
move derivation/classification out of view models into the core's DTOs (e.g. the
core returns `entry.orphan`, a sorted list, or `pr.severity`); the view model
reduces deltas and the view binds DTO -> widget and token -> color.

## Consequences

- The whole Decodable bug class is eliminated at the generator + contract test.
- Adding a UI = one transport adapter + Store, reusing the contract and DTOs.
- Views stop calling `PomCore.shared`; logic moves to testable Stores.
- The 86 ad-hoc map responses migrate to named DTOs incrementally; the contract
  test grows with each one.
- The old Wails plan (`web-ui` + `internal/web`) is superseded and not revived.

## Migration

1. This ADR.
2. Named Go DTOs + golden-fixture contract test in CI Gate (start with the
   surfaces that have drifted; extend mechanically).
3. Ban `PomCore.shared` in Views: `scripts/lint-view-ffi.sh` (in CI Gate) fails on
   any View-layer file not in `scripts/view-ffi-allowlist.txt`. The Store/VM layer
   (`*Store.swift`, `*ViewModel.swift`, `AppState`, `Core/`) may call the core.
   Migrate a grandfathered file onto a Store, then delete its allowlist line.
4. Convert UI polls (usage, agent-states) to subscriptions.
5. Split god views (RootView) as they are touched.
6. Later: a second transport adapter (Gio) to validate the contract cross-platform.
