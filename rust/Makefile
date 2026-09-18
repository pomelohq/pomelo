# Pomelo (Rust rewrite) — macOS / Apple Silicon. Dev bundle is PomeloDev.app (runs alongside Pomelo.app).
.PHONY: help build run dev prod bundle-dev bundle-prod dmg appcast test fmt fmt-check lint check clean \
        patch minor major version

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

help:
	@echo "Pomelo (rust) v$(VERSION) — targets:"
	@echo "  make run          build + launch PomeloDev.app (dev)"
	@echo "  make dev          alias for run"
	@echo "  make prod         build + launch Pomelo.app (release)"
	@echo "  make build        cargo build (debug)"
	@echo "  make bundle-dev   assemble PomeloDev.app"
	@echo "  make bundle-prod  assemble Pomelo.app (release)"
	@echo "  make dmg          build Pomelo.app + package Pomelo-$(VERSION).dmg"
	@echo "  make appcast      build auto-update tarball + latest.json"
	@echo "  make check        fmt-check + lint + test (the gate)"
	@echo "  make test / fmt / lint / clean"
	@echo "  make patch|minor|major   bump version, commit, tag rust-v<x>"

build:
	cargo build

run dev:
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

test:
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy --all-targets -- -D warnings

check: fmt-check lint test

clean:
	cargo clean
	rm -rf target/*.app target/*.dmg target/latest.json target/dmg-stage

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
