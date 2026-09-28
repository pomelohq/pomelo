# Release process

The release is the macOS app, `Pomelo.app`. It carries its own binary for PTY holders, the MCP server and
agent hooks, so nothing else needs installing; the `pom` CLI ships inside the bundle.

## Versions

Semver in `rust/Cargo.toml` (`[workspace.package] version`), tagged `v<version>`:

- **patch**: bug fixes and small additions.
- **minor**: a significant feature or a change of surface.
- **major**: a breaking change (config schema, CLI, storage).

## Steps

1. Land the changes on `main` with CI green (`make check` locally).
2. Curate `CHANGELOG.md`: move the `## [Unreleased]` items into `## [<version>] - <YYYY-MM-DD>` and commit it
   (the `release-audit` skill checks it).
3. `make patch` (or `minor` / `major`) on `main`: bumps the version, commits `release: v<x>`, tags `v<x>` and
   pushes.
4. The tag runs `.github/workflows/release.yml` on a macOS runner:
   - builds `Pomelo.app`, signs it with the Developer ID (hardened runtime) and notarizes the DMG;
   - signs the update tarball and the DMG with the Sparkle EdDSA key;
   - publishes the GitHub Release: `Pomelo-<x>.dmg`, `Pomelo-<x>.app.tar.gz` + `.sig`, `latest.json`,
     `appcast.xml`, `checksums.txt`, with notes from the CHANGELOG block.

   Watch it with `gh run watch --repo pomelohq/pomelo`.

Secrets (Settings > Secrets > Actions): `MACOS_CERT_P12` (base64 .p12), `MACOS_CERT_PASSWORD`,
`MACOS_SIGN_IDENTITY`, `KEYCHAIN_PASSWORD`, `NOTARY_APPLE_ID`, `NOTARY_APP_PASSWORD`, `NOTARY_TEAM_ID`,
`SPARKLE_ED_PRIVATE_KEY`, and optionally `DOCS_DISPATCH_TOKEN` (rebuilds the docs site's changelog).

`make dmg` builds a local DMG for testing; there is no local publish path.

## Updates

- The app checks GitHub Releases, downloads `Pomelo-<x>.app.tar.gz`, verifies its `.sig` against the Sparkle
  public key built into the app, and only then swaps itself and relaunches.
- Apps from before the Rust rewrite (0.6.x and older) poll `.../releases/latest/download/appcast.xml` with
  Sparkle; the feed points them at the new DMG, signed with the same key, so they move over by themselves.
  The `latest` release must keep carrying `appcast.xml`.

## Rules

- Never delete old GitHub releases.
- Never regenerate the Sparkle EdDSA key. Its private half lives in the keychain and the
  `SPARKLE_ED_PRIVATE_KEY` secret; losing it breaks updates for every installed app.
