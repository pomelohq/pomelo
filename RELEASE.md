# Release process

**The release is the native macOS app.** The app is self-contained — it re-execs
its own bundle binary for the `pty` / `mcp` / `prepare-main` subcommands
(`pombin.Path() = os.Executable()`), so it needs **no external `pom` CLI installed**.
There is no separate CLI distribution anymore (the old `make publish` / Homebrew
tap / `tncli-releases` bridge / `pom update` self-update / `pom daemon` were all
removed in v0.10.9).

One version covers both the app and the in-tree CLI: same `vMAJOR.MINOR.PATCH`
(the two constants stay in lockstep).

## Version naming (semver)

- **patch** (`0.10.8 → 0.10.9`) — bug fix, no new user-facing surface.
- **minor** (`0.10.8 → 0.11.0`) — new feature / new surface, backward-compatible.
- **major** (`0.10.8 → 1.0.0`) — breaking change (config schema, CLI, storage).

Tag is `v<version>` (e.g. `v0.10.9`). The version lives in **two** constants that
must always match — `make patch/minor/major` bumps both and `make version-check`
guards against drift:

- `cmd/pom/root.go` `const version`
- `cmd/libpom/libpom.go` `const appVersion` — drives the DMG name + Sparkle appcast.

## Steps (CI — the default)

1. Land your changes on `main` (green: `go build ./... && go vet ./... && go test ./...`;
   app touched → `swift build && swift test` in `desktop/PomeloApp`).
2. Update `CHANGELOG.md`: move the `## [Unreleased]` items into a new
   `## [<version>] - <YYYY-MM-DD>` section. Claude drafts this from the
   Conventional-Commit history since the last tag (`git log <lastTag>..HEAD`);
   review the wording, then commit it just before the bump.
3. **One command:** `make patch` (or `minor` / `major`). Bumps both consts, commits
   `release: v<x>`, tags `v<x>`, pushes to `main`.
4. That's it — pushing the tag triggers **`.github/workflows/release.yml`**, which
   calls the reusable **`app-build.yml`** (`publish: true`) on a GitHub-hosted
   `macos-26` runner (Xcode pinned by `.github/actions/setup-xcode`): build → sign →
   notarize → DMG → Sparkle appcast → publish the `pomelohq/pomelo` GitHub Release
   with `Pomelo-<x>.dmg` + `appcast.xml` + `checksums.txt`. Watch it:
   `gh run watch --repo pomelohq/pomelo`.

The docs landing version is **not** bumped by hand — `pomelo-docs`
(`PomHome.vue`) reads it live from `releases/latest`.

CI needs these repo secrets (Settings ▸ Secrets ▸ Actions): `MACOS_CERT_P12`
(base64 .p12), `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY`, `KEYCHAIN_PASSWORD`,
`NOTARY_APPLE_ID`, `NOTARY_APP_PASSWORD`, `NOTARY_TEAM_ID`, `SPARKLE_ED_PRIVATE_KEY`.

Every PR (and push to `main`) runs **`ci.yml`** → the same `app-build.yml` with
`publish: false`, which compiles the app unsigned as a merge gate. `make dmg`
builds a local DMG for testing only — there is no local publish path.

## Rules

- Never delete old GitHub releases — keep the full history.
- Auto-update feed is `…/releases/latest/download/appcast.xml` (302, no-cache) on
  `pomelohq/pomelo`; the `latest` release must carry the `appcast.xml` asset.
- The Sparkle EdDSA private key: locally in the macOS keychain, in CI as the
  `SPARKLE_ED_PRIVATE_KEY` secret — do not lose it; without it `sign_update` can't
  sign the DMG and auto-update breaks.
