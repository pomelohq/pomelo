---
name: release-audit
description: Run before cutting a Pomelo release (make patch/minor/major). Reconcile the CHANGELOG against everything merged since the last tag so the release notes and Sparkle appcast are curated, then pick the semver bump. Use whenever asked to release, tag, or ship a version.
---

# Release audit

Releases are cut by `make patch|minor|major`, which bumps both version consts,
commits, tags `v<x>`, and pushes. CI (`release.yml`) then reads the matching
`## [<version>]` block from `CHANGELOG.md` to build the GitHub Release notes AND
the Sparkle appcast. If that block is missing, notes fall back to an auto PR list.
So the CHANGELOG must be curated BEFORE tagging — that is this skill's whole job.

## Steps

1. Confirm `main` is clean and up to date (`git fetch && git status`). Release
   from `main` only.
2. Inventory what shipped since the last release:
   - `git log $(git describe --tags --abbrev=0)..main --oneline`
   - cross-check `gh pr list --state merged --base main` for the same range.
3. Classify each item: user-facing, internal, or docs-only. Only user-facing and
   notable internal changes go in the CHANGELOG.
4. Decide the bump (semver): breaking -> major, new features -> minor, fixes only
   -> patch. When unsure, ask.
5. Curate `CHANGELOG.md`: add `## [<new-version>] - <YYYY-MM-DD>` above the previous
   entry, with `### Added / Changed / Fixed` bullets. Write at product height (what
   the user gets), not copied commit subjects; end each bullet with `(#<pr>)`.
   Keep bullets to one short line each. Commit this on `main` (or the release PR) —
   never on feature branches, to avoid conflicts on long-lived PRs.
6. Only now run `make patch|minor|major`. Verify the pushed tag points at the commit
   that contains the new CHANGELOG block (`git show <tag>:CHANGELOG.md | head`).
7. Watch the `Release` workflow to green; confirm the GitHub Release notes and the
   appcast match the CHANGELOG block.

## Guardrails

- Plain ASCII, no emoji anywhere (CHANGELOG, notes, commits).
- Never regenerate the Sparkle EdDSA key; never delete or edit old releases/tags.
- If the CHANGELOG block was forgotten and the tag is already pushed but the release
  has not published yet, fix by re-cutting cleanly (move the tag to a commit that
  has the block) — not by hand-editing a published release.
- No private/company data in the CHANGELOG or notes; use generic placeholders.
