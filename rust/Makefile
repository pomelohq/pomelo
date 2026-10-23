# Pomelo - macOS / Apple Silicon. Dev bundle is PomeloDev.app (runs alongside Pomelo.app).
.PHONY: help build run dev prod bundle-dev bundle-prod dmg linux-tarball appcast test test-cargo fmt fmt-check hakari-check lint check check-changed clean gc trim \
        patch minor major version

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

help:
	@echo "Pomelo v$(VERSION) - targets:"
	@echo "  make run          build + launch PomeloDev.app (dev)"
	@echo "  make dev          alias for run"
	@echo "  make prod         build + launch Pomelo.app (release)"
	@echo "  make build        cargo build (debug)"
	@echo "  make bundle-dev   assemble PomeloDev.app"
	@echo "  make bundle-prod  assemble Pomelo.app (release)"
	@echo "  make dmg          build Pomelo.app + package Pomelo-$(VERSION).dmg"
	@echo "  make appcast      build auto-update tarball + latest.json"
	@echo "  make check        fmt-check + lint + test (the gate)"
	@echo "  make check-changed  fmt-check, then lint + test only the crates changed since origin/main and their dependents"
	@echo "  make gc           drop the dev/test build cache (target/debug); keeps release builds and the apps"
	@echo "  make test / test-cargo / fmt / lint / clean"
	@echo "  make patch|minor|major   bump version, commit, tag v<x>, push (on main)"

# Dev builds first drop a cache left useless by a toolchain change or grown past its limit.
trim:
	@scripts/trim-target.sh

build: trim
	cargo build

run dev: trim
	scripts/run-macos.sh dev

prod:
	scripts/run-macos.sh prod

bundle-dev:
	scripts/bundle-macos.sh dev

bundle-prod:
	scripts/bundle-macos.sh prod

dmg:
	scripts/make-dmg.sh

linux-tarball:
	scripts/bundle-linux.sh prod

appcast:
	scripts/make-appcast.sh

# Each test in its own process, all binaries at once; .config/nextest.toml keeps apart the ones that share a resource.
# TEST_FILTER, a nextest filterset, narrows the run (CI passes the changed crates and their dependents).
test: trim
	@if cargo nextest --version >/dev/null 2>&1; then \
		cargo nextest run --workspace --no-tests=pass $${TEST_FILTER:+-E "$$TEST_FILTER"} && cargo test --doc --workspace; \
	else \
		echo "cargo-nextest not installed (cargo install cargo-nextest): running plain cargo test"; \
		cargo test; \
	fi

test-cargo: trim
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint: trim
	cargo clippy --all-targets -- -D warnings

# Fails when a dependency changed without regenerating the workspace-hack; skipped where cargo-hakari isn't installed.
hakari-check:
	@if cargo hakari --version >/dev/null 2>&1; then \
		cargo hakari generate --diff && cargo hakari manage-deps --dry-run; \
	else \
		echo "cargo-hakari not installed (cargo install cargo-hakari): skipping the workspace-hack check"; \
	fi

check: fmt-check hakari-check lint test

check-changed: trim
	@scripts/check-changed.sh

clean:
	cargo clean
	rm -rf target/*.app target/*.dmg target/latest.json target/dmg-stage

# The dev/test cache only grows (every toolchain or flag change leaves a full copy behind); it rebuilds on demand.
gc:
	@du -sh target/debug 2>/dev/null || true
	rm -rf target/debug target/release/incremental
	@echo "dev build cache removed; the next build or test rebuilds it"

version:
	@echo $(VERSION)

patch:
	scripts/bump-version.sh patch

minor:
	scripts/bump-version.sh minor

major:
	scripts/bump-version.sh major

snapshot:
	cargo run -p ui_snapshot -- /tmp/pomelo-ui.png "$${CAT:-1}"
