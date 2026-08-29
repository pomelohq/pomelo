---
name: pom-review
description: Author a Pomelo multi-repo review for the current workspace — a narrative that links each claim to real code across every repo in the workspace. Use when asked to "review this workspace/branch" or "write a review" inside a Pomelo workspace.
---

# pom-review

Write a review artifact that Pomelo's Review tab renders. Split the work by what each
surface does best, and never say the same thing twice:

- The **narrative** (`doc`) carries only what a picture cannot — the intent, the one key
  modelling decision, the trade-offs, and the risks a reviewer must check.
- The **diagram** carries the flow and the structure (who calls whom, in what order).
- The **anchors** link every claim to the real code.

Do NOT restate in prose what a diagram or an anchor already shows. If you catch yourself
narrating the call sequence step by step, stop — that is the sequence diagram's job (its
per-step notes explain each call); the prose then only says why the flow is shaped that way
and where it can break. A Pomelo workspace spans several repos on one branch, so anchors
must name the repo.

Author ONLY the views this change actually needs — do not fill in everything. Read the
diff first, then decide: the narrative + anchors are the backbone (always); a diagram is
added only when the change has a real multi-step flow / lock / transaction / schema touch.
A small one-file change is just prose + a couple of anchors. Pomelo hides any view you
leave empty.

Keep it tight. The narrative is a few short sections of 2-4 sentences or bullets, not an
essay — aim well under a screenful. Cut any sentence that re-describes a diagram step or an
anchored line; link the phrase to the code instead of paraphrasing it.

## Where to write

The artifact is one JSON file at the project root:

```
<project-root>/.pom/reviews/<name>.json
```

`<name>` MUST be the workspace name, which can differ from the git branch — the app
reads the file by workspace name, so a branch-named file will not show up. The repos
sit under `<project-root>/workspace--<name>/<repo>`, so derive both from any repo:

```
repo_top=$(git -C <repo> rev-parse --show-toplevel)   # .../workspace--<name>/<repo>
ws_dir=$(dirname "$repo_top")                          # .../workspace--<name>
name=$(basename "$ws_dir"); name=${name#workspace--}   # <name>
root=$(dirname "$ws_dir")                               # <project-root>
mkdir -p "$root/.pom/reviews"                           # write $root/.pom/reviews/$name.json
```

Do not name the file from `git rev-parse --abbrev-ref HEAD`.

## What to inspect

For each repo in the workspace, read what the branch changed against its base:

```
git -C <repo> diff -M origin/<default-branch>...HEAD      # files + hunks
git -C <repo> log origin/<default-branch>..HEAD --oneline # commits
```

Resolve `<default-branch>` per repo (it may be `main` in one repo and `master` in
another) via `git -C <repo> symbolic-ref --short refs/remotes/origin/HEAD`.

## Schema

```json
{
  "exists": true,
  "id": "<short-slug>",
  "title": "<one line>",
  "doc": "<markdown>",
  "anchors": [
    { "id": "a1", "repo": "<repo dir name>", "path": "<path within that repo>",
      "start": <first line>, "end": <last line>, "side": "head",
      "note": "<one-line caption for the guided tour stop>" }
  ],
  "diagram": {
    "title": "<one line, e.g. 'Create an order'>",
    "participants": [
      { "id": "api",  "label": "<endpoint>",  "repo": "<repo>" },
      { "id": "svc",  "label": "<service>",   "repo": "<repo>" },
      { "id": "lock", "label": "<lock/cache>" }
    ],
    "steps": [
      { "from": "api", "to": "svc", "kind": "call", "label": "<method>",
        "repo": "<repo>", "path": "<path>", "start": <n>, "end": <m>,
        "note": "<optional short caption>" },
      { "from": "svc", "to": "api", "kind": "return", "label": "<result>",
        "repo": "<repo>", "path": "<path>", "start": <n>, "end": <m> }
    ],
    "scopes": [
      { "kind": "critical", "label": "<lock key or transaction>", "from": 3, "to": 9 }
    ]
  },
  "model": {
    "title": "<one line, e.g. 'Order data model'>",
    "entities": [
      { "id": "order", "label": "Order", "repo": "<repo>", "path": "<path>", "start": <n>, "end": <m>,
        "changed": true, "note": "<why this entity matters>",
        "fields": [
          { "name": "id", "type": "uuid", "key": "pk" },
          { "name": "customer_id", "type": "uuid", "key": "fk" },
          { "name": "status", "type": "enum", "changed": true, "note": "<what changed>" }
        ] },
      { "id": "item", "label": "OrderItem", "repo": "<repo>", "path": "<path>", "start": <n>, "end": <m>,
        "fields": [ { "name": "order_id", "type": "uuid", "key": "fk" }, { "name": "sku", "type": "text" } ] }
    ],
    "relations": [
      { "from": "item", "to": "order", "kind": "n-1", "label": "<verb, e.g. belongs to>" }
    ]
  }
}
```

- `doc` is GitHub-flavored markdown. Link a phrase to code with an anchor URL:
  `[the phrase](pom://code?repo=<repo>&path=<path>&start=<n>&end=<m>)`.
  The `repo`/`path`/`start`/`end` MUST match an entry in `anchors`.
- `repo` is the repo's directory name as it appears in the workspace (e.g. `api`,
  `web`, `worker`) — the same name Pomelo shows. `path` is relative to that repo.
- Prefer a handful of high-signal anchors over many. The prose covers intent, the key
  modelling decision, and the risks — NOT a step-by-step of the flow (the diagram carries
  that). Link to code with the anchor URL; link to the flow with `pom://flow` (jumps to the
  Flow tab) or `pom://flow?step=<n>` (opens the diagram focused on that step) instead of
  re-narrating it.
- `anchors` ORDER is the guided-tour order: Pomelo lets a reviewer step prev/next
  through the stops. Order them as a reading path through the change (entry point
  first, then the flow, then the risky bits). `note` is the short caption shown at
  each stop — say why this spot matters. Put the same anchors you link inline here,
  in tour order.

## Sequence diagram (optional but recommended for cross-repo flows)

`diagram` renders as a sequence diagram in the Review's "Flow" tab — the fastest way
to check the control flow of a change that hops across repos/services.

- `participants` are the actors in the flow: a controller/endpoint, a model/service,
  a job, a lock, an external system, a DB. Give each a stable `id`, a short `label`,
  and (when it lives in a repo) the `repo` so the reader sees which codebase it is.
- `participants` render left-to-right; put them in call order (caller-most first).
- `steps` are the messages in EXECUTION ORDER (top to bottom). `from`/`to` are
  participant `id`s (a self-call where `from == to` is fine — e.g. a model re-reading
  itself under a lock). `label` is the call name.
- `kind` is `call` (solid arrow) or `return` (dashed, muted). Use `return` for the
  value flowing back so the diagram reads like a real UML sequence, not just one-way.
- CRITICAL — each step carries its OWN PRECISE code range (`repo`/`path`/`start`/`end`):
  the exact lines that implement THAT message, usually 1-8 lines, found by READING the
  file. Do NOT reuse one broad method range across many steps — the Flow timeline shows
  each step's own slice, so "step 4 = the reload line" must point at the reload line, not
  the whole method. Different steps in the same method get different narrow ranges.
- `note` — give EVERY step a one-line note explaining what this call does and why it
  matters (the invariant it holds, the race it closes, the side effect). This is the
  reviewer's plain-language explanation; the Flow timeline shows it under each step, so a
  step without a note reads as an unexplained arrow. Keep it to one tight sentence.
- `scopes` are UML combined-fragment boundaries around a run of steps (`from`/`to` are
  1-based step numbers, inclusive) — draw them for the spans a reviewer must reason about
  as a unit. Use the standard operator as `kind`:
  - `critical` — a held lock or a DB transaction (mutual exclusion / atomic region). This
    is the big one for concurrency review: wrap acquire..release or the transaction body.
  - `opt` — a conditional block; `loop` — a repeated block; `alt` — mutually exclusive paths.
  `label` names it (the lock key, the transaction, the condition). Add scopes only where
  the boundary is real and load-bearing for correctness.
- Trace the flow by READING the code (entry point -> each call), don't invent it. Only
  emit `diagram` when there is a real multi-step flow; skip it for trivial changes.

## Data-model (ER) diagram (optional — for schema / data-model / mapping changes)

`model` renders as an entity-relationship diagram in the Review's "Model" tab — boxes of
entities with their fields, joined by relationship edges. Use it whenever the change is
about the SHAPE of data (a new table/column, a new mapping/source dimension, a foreign
key, a record embedded on another) — that shape is far clearer as a picture than as prose,
so let the diagram carry it and keep the narrative to why the shape changed.

- `entities` are the records/tables the change touches — NOT the whole schema. Give each a
  stable `id`, a `label`, and (when it lives in code) `repo`/`path`/`start`/`end` so a click
  peeks the definition. Set `changed: true` on an entity the branch adds or alters.
- `fields` are the columns/attributes that matter for this change (skip the noise). Each has
  a `name`, optional `type`, optional `key` (`pk` or `fk`), and `changed: true` for a field
  the branch adds or alters. A one-line `note` explains a non-obvious field.
- `relations` are the edges: `from`/`to` are entity `id`s, `kind` is the cardinality
  (`1-1` | `1-n` | `n-n`), `label` is the verb (e.g. "maps to", "owns").
- Emit `model` only when the data shape is a real part of the change; a pure control-flow or
  UI change does not need it. A change can have both a sequence diagram and a model diagram.

## Which view carries what

- Sequence diagram (`diagram`): control flow — who calls whom across repos/services, locks,
  transactions. Reach for it on multi-step / concurrency / cross-repo changes.
- Data-model diagram (`model`): data shape — entities, fields, foreign keys, cardinality.
  Reach for it on schema / migration / mapping / new-source changes.
- Narrative (`doc`): only the reasoning neither picture shows — intent, the key decision,
  trade-offs, risks. Link `pom://flow` / `pom://flow?step=<n>` / `pom://model` from the prose
  to send the reader to the right diagram instead of describing it in words.

## Ground DB / schema claims via the pom MCP

Pomelo manages this branch's databases, so read the REAL schema instead of guessing.
The `pom` MCP server (already registered) exposes read-only tools:

- `db_list` -> the branch's databases (use a `name` as `db` below).
- `db_tables` / `db_columns` -> tables and exact columns/types.
- `db_query` -> run a SELECT to confirm data shape.

Use these to verify any claim about tables, columns, indexes, or migrations before you
write it, and to make a migration/data-model anchor accurate. If the change adds a
column or index, confirm it landed with `db_columns`/`db_query`.

## Rules

- VALIDATE every anchor before writing: the file exists in that repo and the line
  range is within the file. Read the file to confirm, do not guess line numbers.
- Repo-qualified always — never emit an anchor without a `repo`.
- Plain ASCII, no emoji.
- Overwrite the file if it already exists (a review is regenerated per workspace).

## After writing

Tell the user the review is ready and to open the Review tab (Cmd-5) on this
workspace in Pomelo.
