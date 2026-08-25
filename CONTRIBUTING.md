# Contributing to Pomelo

Thanks for helping improve Pomelo. It's a Go core plus a native SwiftUI macOS
app; most changes touch one or the other. One rule underpins everything: the
macOS app, the `pom` CLI, and `pom mcp` are **thin clients of one core** — no
business logic in a client (enforced by `internal/arch/deps_test.go`).

## Setup

Requirements: **Go 1.26+**, **Xcode** (macOS 14+ SDK), and **zsh**. tmux is *not*
required — services run on self-managed PTY holders.

```bash
git clone https://github.com/<your-fork>/pomelo.git && cd pomelo
make build                             # Go core / CLI -> ./pom
make app-run                           # build the native app (Debug .app) and open it
```

`make app` builds an unsigned Debug `.app` bundle; `make app-run` also opens it.
The dev build runs as a real `.app` (not a bare binary) so bundle-dependent APIs
(auto-update, notifications) behave like the shipped app.

## Build & test

```bash
go build ./... && go vet ./... && go test ./...   # Go core
make check                                          # project rules (security + comment bloat)
cd desktop/PomeloApp && swift test                  # app ViewModel unit tests
```

Run all of the above before opening a PR. The app links a prebuilt `libpom.a`,
so a Go-only change still needs `desktop/PomeloApp/build.sh` to rebuild the
c-archive before the app picks it up.

## Code style

Match the surrounding code. The non-negotiables (full rules in `CLAUDE.md`):

- **Almost no comments.** A comment explains *why* / a trade-off, never *what* —
  one line, only when non-obvious. Prefer clear names over comments.
- **Go:** `gofmt`/`go vet` clean; wrap errors with context
  (`fmt.Errorf("...: %w", err)`); `exec.Command` with separate args, never a
  shell string with user input; no `panic` in libraries; file perms
  `0o644`/`0o755`, never `0o777`.
- **Swift:** keep FFI off the main thread and view bodies cheap — the app targets
  120fps. Put fetch/decode in a ViewModel with a test, not in a View.
- **Dependencies point inward** — see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), single line, no
body. Common types: `feat` `fix` `docs` `refactor` `chore` `ci`. Scope is
optional (`feat(forge): ...`).

```
feat(forge): show PR review comments in the Overview tab
fix: agent hooks fail with "node: command not found"
docs: document GitHub token setup
```

**No emoji** — not in commit messages, PR titles, or descriptions.

Every commit must be **signed off** (DCO, below):

```bash
git commit -s -m "fix: ..."
```

## Branch naming

Branch off `main`:

- `feat/pr-review-comments`
- `fix/agent-node-path`
- `docs/github-token-setup`

## Pull requests

One logical change per PR. Make sure the build is green and `make check` is
clean.

Checklist:

- [ ] `go build ./... && go vet ./... && go test ./...` pass; `make check` clean
- [ ] App touched → `desktop/PomeloApp` builds and `swift test` passes
- [ ] User-facing change → docs updated in
      [`pomelo-docs`](https://github.com/toantran292/pomelo-docs) (same PR)
- [ ] Commits are Conventional and signed off (`-s`)
- [ ] No emoji anywhere

## Project layout

```
cmd/pom/               CLI entry + one cobra command per file (no business logic)
cmd/libpom/            c-archive FFI: //export bindings + PomStream (drives the app)
internal/config/       pom.yml parsing, {{var}} resolution, load-time validation
internal/core/         shared Server + business logic (used by libpom and pom mcp)
internal/provider/     provider seams — tracker (Jira), forge (GitHub), dbclient, shell
internal/agent/claude/ built-in Claude agent (headless stream driver + hooks)
internal/pipeline/     workspace lifecycle (staged create/delete + events)
internal/commands/     CLI operation implementations
internal/services/     infrastructure — docker, git, databases, port allocation
internal/ptyhost/      self-managed PTY holders (tmux-free, durable exec)
internal/mcp/          pom mcp stdio server (agent-facing tools)
desktop/PomeloApp/     native SwiftUI macOS app (MVVM; links libpom)
docs/                  design docs + architecture diagram
scripts/               release / packaging scripts
```

## Adding an integration

Put domain logic in its own `internal/<feature>` package (model: `internal/jira`,
`internal/archive`) or behind a provider seam in `internal/provider/`. Keep the
client thin: an FFI export for the app, a cobra command for the CLI, an MCP
endpoint for agents. Never pile logic into a client, and never add a raw field to
the core `config` schema for a volatile integration. See the PLAYBOOK in
`CLAUDE.md`.

## Developer Certificate of Origin (DCO)

We do not use a CLA. Instead, sign off every commit to certify you wrote the
change (or have the right to submit it) under the project's license — the
[Developer Certificate of Origin 1.1](https://developercertificate.org/).
`git commit -s` appends:

```
Signed-off-by: Your Name <you@example.com>
```

The name/email must be real and match your Git identity. PRs whose commits are
not signed off will be asked to amend.

## Reporting bugs

Open a [GitHub issue](https://github.com/pomelohq/pomelo/issues) with:

- macOS version
- Pomelo version (top bar / `pom --version`)
- Reproduction steps
- Relevant `pom.yml` shape (redact secrets/real names)

For security issues, contact the maintainer privately rather than opening a
public issue.

## License

Contributions are licensed under [AGPL-3.0](LICENSE).
