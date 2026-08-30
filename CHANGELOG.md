# Changelog

All notable changes to Pomelo are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Pomelo follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Service URLs route again after Pomelo restarts while an agent window is still open. Each `pom mcp` server was claiming the dev-proxy and webhook-relay ports, so an MCP process that outlived the app kept them and every workspace service answered "no dev-proxy route" — the MCP server no longer starts those listeners. The app also logs when the dev-proxy port is already taken instead of failing silently.

## [0.4.3] - 2026-08-30

### Fixed
- A repo with no runnable services (for example an infrastructure or config-only repo) can now be added to a workspace instead of failing with "no repos selected" — it gets a worktree and stays in sync, just with nothing to run. (#74)

## [0.4.2] - 2026-08-30

### Fixed
- Keep main fresh no longer fails when two Pomelo instances run against the same project: the golden-source refresh now serializes across processes instead of racing git and leaving a stale lock. (#73)
- A failed per-repo pull shows its git error inline in the sync popover, so it explains itself instead of just reading "failed". (#73)

## [0.4.1] - 2026-08-30

### Added
- Review Model tab: a data-model (ER) diagram of the entities a change touches — boxes with primary/foreign-key fields, relationship edges, and added/changed highlighting; click an entity to peek its definition. (#72)

### Changed
- The review narrative is selectable as one document (drag across the whole thing, not one paragraph at a time) and can link a phrase straight to the Flow or Model diagram instead of restating it. (#72)

### Fixed
- An open review updates when it is regenerated, and switching between Narrative, Flow, and Model no longer reloads the step timeline. (#72)

## [0.4.0] - 2026-08-29

### Added
- Split diff: a side-by-side view that bridges each change with a curved connector ribbon and keeps matching lines anchored while you scroll; choose Unified or Split as the default in Settings > Appearance. (#70)
- Open the agent beside a function pane (Cmd-I) in a resizable split that persists per workspace, with a top-bar status pill that lists active agents and jumps to one. (#69)
- Attachments in Jira and PR views open inline images in a viewer. (#67)

### Changed
- The services home is a reorderable kanban board, the sidebar reveals on hover when collapsed, and the command palette shows each workspace's agent state and PR/ticket status. (#69)
- Jira comments are styled like Jira, with a resizable comments column. (#68)
- Split diff tints each line by change kind with word-level highlights and syncs horizontal scroll across both panes. (#70)

### Fixed
- PRs, Jira status, and severity dots appear from cache on launch instead of a multi-second "loading" spinner. (#71)

## [0.3.6] - 2026-08-29

### Added
- Commits tab: click a commit to see the diff it introduced, in the same tree + viewer as Files (Esc to go back). (#58)
- A wrap-vs-scroll preference for read-only code views (Settings > Appearance). (#58)
- Agent usage: click the top-bar meter for a card with each window's used %, reset time (absolute + countdown), and the signed-in account. (#65)
- Dependency store shortcut (Shift-Cmd-D), listed in Settings > Shortcuts. (#65)

### Changed
- Selecting code in a unified diff now copies just the code — line numbers and +/- markers are drawn in the margin, not part of the text. (#58)
- Every read-only code surface (review peek, unified and split diff) is drawn by one renderer, so highlighting, line height, selection, and theming match everywhere. (#63)
- The Review tab is hidden on the main workspace (it reviews a branch's changes), while the agent is now available there. (#64)
- Top bar uses the Claude mark for the usage meter and a distinct shared-services icon (no longer clashing with the Database tab). (#65)

### Fixed
- Diff line backgrounds no longer show faint horizontal stripes on rows revealed while scrolling. (#63)
- Code and section headers recolor immediately when switching theme (dark, light, sepia) instead of keeping the previous palette. (#63)
- Reviews are found by workspace name (not the git branch), so a generated review reliably appears in the Review tab. (#64)
- The dev-proxy routes to a service's allocated port immediately, instead of briefly latching onto a transient build socket and needing several reloads to settle. (#66)

## [0.3.5] - 2026-08-28

### Changed
- The review peek and PR diff now share one code renderer, so syntax highlighting, line height, and selection behave identically across both. (#62)

### Fixed
- Diff line backgrounds no longer show faint horizontal stripes on rows revealed while scrolling. (#62)
- Code text in the diff and peek, and the LOCAL CHANGES / PULL REQUESTS section headers, recolor immediately when you switch theme (dark, light, sepia) instead of keeping the previous palette. (#62)

## [0.3.4] - 2026-08-28

### Added
- Review tab (Cmd-5): a per-workspace companion for understanding and reviewing agent-written, multi-repo changes. An authored narrative links each claim to real code via repo-qualified anchors; clicking peeks the exact file range in a side pane with syntax highlighting. (#61)
- Flow view: an agent-authored sequence diagram (participants are repos/services, per-step precise code ranges, call/return arrows, and critical/opt/loop boundary fragments) paired with a step timeline that shows each step's code; hovering or paging keeps the diagram and timeline in sync. (#61)
- Review notes anchored to selected lines (add, reply, resolve), and an Ask agent action that opens the workspace agent with the prompt pre-filled. (#61)
- The pom-review skill is installed into the agent automatically; the MCP diagnostics pane reports its status. (#61)

### Changed
- Workspace panes stay mounted per workspace, so the active tab and its scroll/selection survive switching panes and workspaces. (#61)

## [0.3.3] - 2026-08-28

### Added
- SQL editor: run just the statement under the cursor (or the current selection) with Cmd-Return; Cmd-Shift-Return runs the whole buffer. (#60)

### Changed
- Smarter SQL autocomplete: fuzzy ranking so "users" finds "partner_users", table/column lists that follow the clause (FROM, SELECT, WHERE, `table.`), a manual trigger (Esc) that lists everything, and a popup that tracks the caret and reappears after you delete and retype. (#60)
- The SQL editor and Database navigator honor the active theme (including sepia) and recolor immediately when you switch themes. (#60)

### Fixed
- Pull request Files and Commits reflect the pushed PR even when the local checkout is behind, by diffing the pushed refs and refreshing them in the background. (#60)
- Global keyboard shortcuts work while a text field or the SQL editor is focused, and the SQL editor's line-number gutter no longer overlaps the results grid. (#60)
- App data still decodes when the backend omits an optional field, and repeated polling no longer re-renders the whole window on every tick. (#60)

## [0.3.2] - 2026-08-28

### Added
- Database navigator rebuilt on a native outline view: smooth row reuse for large schemas, correct nested indentation (server > database > tables), native disclosure, and drag-to-reorder that keeps working even with tables expanded. (#59)

### Changed
- The main workspace's "keep fresh" status now shows the real sync outcome and how fresh it is ("synced 2m ago", "N updated", or a failure) instead of a bare countdown; the next-run timer moves into a per-repo popover. Each repo is pulled onto its own default branch, and a pull runs immediately on launch. (#59)
- Consistent loading vs empty states across the PR tabs, Activity, Secrets, and diffs, plus a shared UI kit (spinner, cards, pills, section headers) drawn without native controls for a uniform look. (#59)

### Fixed
- Pull request Files and Commits now match the PR even when the local worktree is behind, by diffing the pushed PR refs. (#59)
- The PR conversation loads reliably (its timeline is now assembled by the core), and PR and Jira details are cached to disk so reopening a workspace is instant. (#59)
- Global keyboard shortcuts (settings, shared services, and friends) fire even while a text field or the SQL editor is focused. (#59)
- Crash log output wraps instead of scrolling sideways. (#59)

## [0.3.1] - 2026-08-28

### Fixed
- A running service no longer loses its allocated port when the process briefly stops accepting connections (e.g. a dev server reloading): the port is only reclaimed after it stays unreachable for a sustained window, so the dev-proxy keeps routing to it instead of falling back to a costly live scan that could spike CPU. (#56)

## [0.3.0] - 2026-08-28

### Added
- Add repos to an existing workspace: right-click a workspace and pick "Add repo..." to fork more repos onto its branch, wiring up their env, ports, and services without touching the repos already there. (#54)

### Changed
- PR and Jira panes are far more readable: a GitHub-style conversation timeline with avatars, review threads, and inline comments, laid out in a centered reading column, plus sepia theme fixes. Review bodies are now surfaced from the core. (#49)

### Fixed
- Shared services now connect over the explicit IPv4 loopback (127.0.0.1) instead of "localhost", avoiding IPv6 connection failures on Docker Desktop where only some clients could reach the port. (#54)
- A service that just crashed no longer briefly shows as running: holder liveness now treats an exited-but-unreaped process as dead. (#55)

## [0.2.5] - 2026-08-27

### Fixed
- Diff viewer: long lines scroll horizontally in both unified and split views instead of getting cut off, and the added/removed tint spans the full line. (#48)
- Workspace sidebar scrolls smoothly; a fast flick no longer leaves a phantom blank gap below the list. (#50)
- Shared services now hand out a connection URL on the port the container is actually published on, including capacity>1 services where a workspace is pinned to a second instance. (#50, #51)
- Creating a workspace whose repo selection matches nothing now fails with a clear message instead of leaving an empty, unusable workspace. (#50)
- Dependency board shows only the current project's caches and fits to the window when opened. (#48)
- Diff view renders renames compactly and groups file-tree paths correctly. (#36)

### Changed
- Internal: the core-to-UI boundary is now data-routed through a few verbs (query/command/fetch/subscribe) instead of ~86 typed exports, and domain logic (PR status, diff parsing, dependency ordering, agent notifications) lives in the Go core per ADR 0001. No user-facing behavior change. (#39, #45, #48, #50, #52)

## [0.2.4] - 2026-08-27

### Fixed
- Keeping the golden source fresh now updates a main that has diverged from origin (upstream rebase or force-push) by mirroring origin, instead of silently skipping it. (#37)

### Changed
- Internal: standardized the core-to-UI boundary with DTO contract tests and a view-model data layer; no user-facing behavior change. (#35)

## [0.2.3] - 2026-08-27

### Fixed
- Opening a service in the browser now builds a URL that resolves even when two workspaces share a ticket prefix; it falls back to the full branch host instead of an ambiguous short one. (#34)

## [0.2.2] - 2026-08-27

### Added
- Claude usage meter in the top bar: 5h session and weekly windows with a compact bar, color-coded by load. (#32)
- PR view: a file tree with a local-changes sidebar, and tooltips across the app. (#33)

### Changed
- The compacting agent state now has its own distinct pulsing orb instead of a plain grey one. (#31)

## [0.2.1] - 2026-08-27

### Added
- Notification sounds: pick a sound per event (or several, played at random), save them as switchable sound sets, and upload your own audio. Delivery has a master toggle and an option to alert even while you're viewing the workspace. (#29)

### Fixed
- The Database pane shortcut (Cmd-4) is now listed in Settings > Shortcuts. (#30)

## [0.2.0] - 2026-08-26

### Added
- Dependency Store: a global node_modules cache board (node_modules -> hash -> workspaces) with Optimize to capture hand-installed deps and Dedupe to reclaim disk via CoW. (#27)
- Update from origin for the golden source: a main-only action with a per-repo progress sheet. (#27)

### Changed
- Workspace, repo-column, and database-tree reordering is gesture-driven now. (#27)
- CI is path-gated behind a single CI Gate; onboarding seeds pom.yml via detect. (#27)

### Fixed
- Config tree fits the sidebar width instead of clipping long fragment names. (#27)
- Golden-source update pulls only the default branch, avoiding the fast-forward-to-multiple-branches failure. (#27)

## [0.1.7] - 2026-08-26

### Added
- Optimistic, animated start/stop for a repo's services — cards flip to an
  immediate starting…/stopping… state and animate between states. (#20)

### Changed
- Create workspace: the sprint picker uses a themed dropdown, and the ticket
  suggestions render as a solid card with hover, close on pick, and no longer
  overlap the hint. (#21)

### Fixed
- A stopped service could keep showing "running" after the OS recycled the dead
  holder's pid; the pidfile is now removed on kill. (#19)
- Quit no longer hangs while tearing down a workspace's ephemeral shells. (#18)

## [0.1.6] - 2026-08-26

### Added
- Jira pane: a read-only "Web links" section listing an issue's remote links. (#16)
- App icon shown in the session chip and the create-workspace sheet. (#11)

### Changed
- Shortcuts keep their tab and output open after finishing (Ctrl+D to close). (#13)
- Holder lifecycle unified behind a single interface; shells receive injected
  env instead of sourcing a hardcoded .env.local. (#15)

### Fixed
- Activity Monitor no longer blanks a workspace's process group. (#14)
- Attaching to a stopped service waits for its holder instead of spawning a bare
  shell over it, and shells are no longer reaped on app launch. (#10)
- The port reaper only reclaims ports Pomelo allocated — never a user's own
  running process. (#9)
- Jira tickets with zero comments now render instead of failing to load. (#16)

### Build
- Styled DMG installer window, built headlessly for CI. (#12)

## [0.1.5] - 2026-08-25

### Fixed
- Re-derive the description, name, and slug when the Jira ticket changes. (#8)

## [0.1.4] - 2026-08-25

### Changed
- Point the in-app update feed at the pomelohq/pomelo releases. (#7)

### Documentation
- README with a hero image, app screenshot, and architecture diagram. (#6)

## [0.1.3] - 2026-08-25

### Changed
- One unified "New session" flow; renamed Project to Session. (#5)

## [0.1.2] - 2026-08-25

### Changed
- Purged legacy branding; fixed the session root and session switching. (#4)

## [0.1.1] - 2026-08-25

### Added
- "Open a project…" entry in the session dropdown. (#2)
