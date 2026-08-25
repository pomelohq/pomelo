# Changelog

All notable changes to Pomelo are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Pomelo follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Optimistic, animated start/stop for a repo's services — cards flip to an
  immediate starting…/stopping… state and animate between states. (#20)

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
