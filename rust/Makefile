# Pomelo - macOS / Apple Silicon. Dev bundle is PomeloDev.app (runs alongside Pomelo.app).
.PHONY: help build run dev prod bundle-dev bundle-prod dmg appcast test fmt fmt-check hakari-check lint check clean gc trim \
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
	@echo "  make gc           drop the dev/test build cache (target/debug); keeps release builds and the apps"
	@echo "  make test / fmt / lint / clean"
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

appcast:
	scripts/make-appcast.sh

test: trim
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
