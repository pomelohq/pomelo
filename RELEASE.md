# Release process

The release is the macOS app, `Pomelo.app`. It carries its own binary for PTY holders, the MCP server and
agent hooks, so nothing else needs installing; the `pom` CLI ships inside the bundle.

## Versions

Semver in `rust/Cargo.toml` (`[workspace.package] version`), tagged `v<version>`:

- **patch**: bug fixes and small additions.
- **minor**: a significant feature or a change of surface.
- **major**: a breaking change (config schema, CLI, storage).

## Channels

- **Stable**: the releases below, cut with `make patch|minor|major`. The default.
- **Nightly**: `.github/workflows/nightly.yml` builds `main` every night at 02:17 UTC when it moved since the
  last nightly, as `<next patch>-nightly.<YYYYMMDD>` (`0.9.9-nightly.20261024` after `v0.9.8`), signed and
  notarized like a release. It publishes into the one `nightly` prerelease, never marked latest and never
  touching `appcast.xml`: the DMG, the update tarball and the Linux tarballs, each with its `.sig`, plus
  `checksums-<v>.txt` and `nightly.json` (the version and commit). The builds of the last 7 nights stay there,
  named by version; older ones are removed. Its tag moves to the commit of the newest build.

The app picks the channel in Settings > General (`update_channel`; dev and nightly builds follow nightly
until one is picked). Stable ignores prereleases; nightly takes the highest version of both, and a release
ranks above its own nightlies (`0.9.9` > `0.9.9-nightly.20261024`), so nightly users get every stable and
every hotfix at once. Switching from nightly to stable never downgrades: it takes effect at the next stable.

Run the nightly by hand from Actions, or with `gh workflow run nightly.yml -f dry_run=true`: a dry run (the
default for a manual run) builds, signs and notarizes but publishes nothing and moves no tag; `force` builds
even when main has not moved. `release.yml` run by hand builds the current version the same way and
publishes nothing. Both call `.github/workflows/build.yml`, the one build every published download comes
from.

## Steps

1. Land the changes on `main` with CI green (`make check` locally), and let a nightly build them: a stable
   release is cut from a commit a green nightly built (or one that differs from it only in `CHANGELOG.md`).
2. Curate `CHANGELOG.md`: move the `## [Unreleased]` items into `## [<version>] - <YYYY-MM-DD>` and commit it
   (the `release-audit` skill checks it).
3. `make patch` (or `minor` / `major`) on `main`: checks the nightly rule (`scripts/nightly-passed.sh`),
   bumps the version, commits `release: v<x>`, tags `v<x>` and pushes.
4. The tag runs `.github/workflows/release.yml` on a macOS runner:
   - builds `Pomelo.app`, signs it with the Developer ID (hardened runtime) and notarizes the DMG;
   - signs the update tarball and the DMG with the Sparkle EdDSA key;
   - publishes the GitHub Release: `Pomelo-<x>.dmg`, `Pomelo-<x>.app.tar.gz` + `.sig`, `latest.json`,
     `appcast.xml`, `checksums.txt`, with notes from the CHANGELOG block;
   - adds the Linux tarballs `Pomelo-<x>-linux-<arch>.tar.gz` and `pom-<x>-linux-<arch>.tar.gz`, each with a
     `.sig` made with the same key (signed on macOS, where Sparkle's `sign_update` runs), and
     `checksums-linux.txt`.

   Watch it with `gh run watch --repo pomelohq/pomelo`.
5. Regenerate the docs site's reference pages from the tag: in a `pomelo-docs` checkout,
   `cargo run --manifest-path <pomelo>/rust/Cargo.toml -p docs_gen -- --out reference`, then open a docs PR
   with what changed. The docs repo's `Reference drift` check fails until the pages match the latest tag.

Secrets (Settings > Secrets > Actions): `MACOS_CERT_P12` (base64 .p12), `MACOS_CERT_PASSWORD`,
`MACOS_SIGN_IDENTITY`, `KEYCHAIN_PASSWORD`, `NOTARY_APPLE_ID`, `NOTARY_APP_PASSWORD`, `NOTARY_TEAM_ID`,
`SPARKLE_ED_PRIVATE_KEY`, and optionally `DOCS_DISPATCH_TOKEN` (rebuilds the docs site's changelog).

`make dmg` builds a local DMG for testing; there is no local publish path.

## Hotfixes

By default a hotfix is a normal release from `main`, which is kept releasable: land the fix, curate its
CHANGELOG block, then `make patch HOTFIX=1`, which skips only the nightly rule (CI still has to pass). Nightly
users get it at once, since it ranks above their nightly.

Only when `main` is not releasable, cut it from the last release instead:

1. `make hotfix`: branches `hotfix/v<next patch>` from the newest `v<x.y.z>` tag.
2. `git cherry-pick -x <commit>` the fixes from `main`; add the `## [<next patch>]` CHANGELOG block and commit.
3. `make hotfix-release`: bumps the version, commits `release: v<x>`, tags it and pushes the branch and tag;
   the Release workflow publishes it as usual.
4. Open a PR to `main` with the version bump and the CHANGELOG block, so main's next release follows it.

## Updates

- The app checks GitHub Releases for its channel, downloads `Pomelo-<x>.app.tar.gz`, verifies its `.sig`
  against the Sparkle public key built into the app, and only then swaps itself and relaunches.
- On Linux an install made by `install.sh` (it writes `<prefix>/share/pomelo/installed-by`) does the same with
  `Pomelo-<x>-linux-<arch>.tar.gz`, running the new tarball's install.sh at restart; `pom self-update
  [--channel] [--version]` updates pom (and the app beside it) the same way, and `install.sh --channel` /
  `--version` hand off to it. An install a package manager made never replaces itself.
- "Update pom on host" installs the app's own version on the host when it is still published (its stable
  release, or a nightly the nightly release keeps), else the newest nightly, after its `.sig` verifies. The
  same version works fully; different versions use only what the host's HELLO declares, and the app offers
  the update once the major or minor version differs.
- Apps from before the Rust rewrite (0.6.x and older) poll `.../releases/latest/download/appcast.xml` with
  Sparkle; the feed points them at the new DMG, signed with the same key, so they move over by themselves.
  The `latest` release must keep carrying `appcast.xml`.

## Grammar packages

Grammars the app doesn't compile in are packages it downloads: `rust/grammars/manifest.toml` lists them, each
built from the crate version and checksum it names, with the queries and editing config in
`rust/grammars/languages/<id>/`. `rust/grammars/expected/<id>.json` records what the compiled-in language gave
for its sample before it left the app; `verify` holds every package to it. `.github/workflows/grammars.yml`
builds and checks them on every change. On main it then publishes, by itself, the packages whose inputs changed
(crate, queries and config, compile flags, wasi-sdk; each package records them as `input_hash`), signed, to the
one `grammars` release, updated in place; unchanged packages keep their published file and signature, and a
pull request's run lists what would be published. Run it by hand with `publish=true` to publish every package
again. The release is never marked latest, because the latest release must keep carrying `appcast.xml`, and
there is only ever one so it never pushes app releases out of the updater's view. Old archives stay on it, since
an app may still hold an index that names them.

The app reads `https://github.com/pomelohq/pomelo/releases/download/grammars/index.json` at most once a day, and
falls back to the copy built into it. `release.yml` downloads the published index, refuses it unless every
package is signed by the app's key, and builds it into the release; `rust/grammars/index.json` is only the copy
dev builds carry, and the fallback when the release can't download.

One-time setup, before the first publish:

1. Make the signing key: `cd rust && cargo run -p grammar_packager -- keygen`. It prints the secret and the
   public key. This is a key of its own, not the Sparkle key.
2. Add the secret as the repository secret `GRAMMARS_SIGNING_KEY`. Keep a copy somewhere safe; like the Sparkle
   key, replacing it later means shipping an app with the new public key before packages signed with it work.
3. Put the public key in `GRAMMARS_PUBLIC_KEY` in `rust/crates/grammars/src/grammars.rs`. Until it is set the
   app downloads nothing and never suggests a package; once set, every package must match its sha256 and its
   signature or it is refused.

Locally: `cargo run -p grammar_packager -- build` then `-- verify` (needs network for crates.io and wasi-sdk);
`-- check-index --index FILE` checks an index is signed by the app's key.

## Rules

- Never delete old GitHub releases.
- Never regenerate the Sparkle EdDSA key. Its private half lives in the keychain and the
  `SPARKLE_ED_PRIVATE_KEY` secret; losing it breaks updates for every installed app.
