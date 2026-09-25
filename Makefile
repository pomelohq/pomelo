# Pomelo lives in rust/; these targets forward there so the usual commands work from the repo root.
.PHONY: help build run dev prod dmg check test fmt lint clean snapshot patch minor major version

help build run dev prod dmg check test fmt lint clean snapshot version:
	@$(MAKE) --no-print-directory -C rust $@

# Release: bump the version, commit, tag v<x> and push; CI builds, signs, notarizes and publishes.
patch minor major:
	@$(MAKE) --no-print-directory -C rust $@
