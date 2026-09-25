# rust/

All of Pomelo: the macOS app (`crates/pomelo`) and the `pom` CLI (`crates/pom_cli`), one crate per feature.
See the [repo README](../README.md) for what Pomelo is, [`CLAUDE.md`](CLAUDE.md) for the layout and rules, and
[`../RELEASE.md`](../RELEASE.md) for releases.

```
make run      # build + open PomeloDev.app (com.pomelo.app.dev; runs alongside Pomelo.app)
make prod     # build + open the release Pomelo.app
make check    # fmt-check + clippy -D warnings + tests: the CI gate
make dmg      # Pomelo.app -> target/Pomelo-<ver>.dmg (signed and notarized when SIGN_ID / NOTARY_PROFILE are set)
```
