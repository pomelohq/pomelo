# Changelog

All notable changes to Pomelo are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Pomelo follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.11] - 2026-10-28

### Added
- A remote project's editor shows the git gutter, blame and line staging, run on the host; an older pom there leaves the gutter empty. (#445)
- A shared service with a Postgres or PostGIS image gets 1 GiB of `/dev/shm`, so many workspaces querying it at once no longer fail with `could not resize shared memory segment`. Any image service can set its own `shm_size`. (#496)

### Changed
- One design system across the app: every label is on one type scale, gaps and padding on one spacing scale, corners, shadows and motion on one set of steps, and every color comes from the theme. Buttons, pickers, menus, popovers, tabs, chips, pills, progress and empty states are shared components, so they look and behave the same in every panel. (#464, #466, #467, #468, #469, #470, #471, #472, #473, #475, #476, #477, #478)
- The Services panel is simpler: a header with running/total and Start all / Stop all, one strip per problem with its fix, and quieter service and shared rows. (#448)
- The Git panel shows each changed file once, ticked when staged and dashed when partly staged; Staged / Not staged sections remain under View Options. (#487)
- The Git panel commit footer is a message box and one split button naming the repos it commits to, with Amend, Signoff and Skip Hooks in its menu, a warning only when a repo is off the workspace branch, and Undo right after a commit. (#486)
- A database that fails to open shows its cause in one line with the main fix as a split button; the details open on click, and databases waiting on the same down server say "Same cause as ...". (#489)
- Take over on an agent someone else drives runs without freezing the window, shows that it is running, and says why when it fails. (#483)

### Fixed
- `pom agent send` presses the retry Enter for a prompt the agent folded to `[Image #N]`, so a pasted image path no longer stays unsent. (#490)
- The Git panel no longer keeps an old HEAD when a refresh comes in while it is reading, so Undo after a commit finds the commit. (#497)
- Workspace switcher rows have room for their status pill. (#488)
- An empty state's action button sits centered under its title. (#474)

## [0.9.10] - 2026-10-24

### Security
- The dev proxy asks for its token on any request that carries `X-Forwarded-For`, `X-Real-IP`, `Forwarded` or `X-Forwarded-Host`, also one from this machine, so a tunnel or proxy on loopback no longer passes as local. `pom doctor` warns (`proxy.untrusted_forwarder`) about a peer that sends those headers without being in `proxy.trusted_proxies`. (#401)
- wasmtime, which loads grammar packages, moves to 48.0.7 and tokio to 1.54.1, fixing their published advisories. (#400)

### Changed
- The app and pom server agree on a protocol both speak, so a newer app keeps working with an older pom on a host. A host with a newer pom than the app now says to update the app instead of offering to replace pom there with an older one. (#402)
- Buttons, selects, menus, chips, pills, tooltips and status dots each come from one shared component, so a control looks the same in every panel. Chips (branches, key kinds, plans, a detected stack) now share one shape. (#420, #421, #422, #433, #434, #435, #436)
- A Settings dropdown without room below opens above its control or lines up with its right edge, and keeps its search line next to the control. (#423, #428, #430)

### Fixed
- Settings descriptions and section notes sit on the same line height as the rest of the small text, so long pages no longer drift lower note by note. (#410)
- Starting several services at once no longer fails now and then with "No such file or directory" when two of them set up their socket folder at the same moment. (#425)

## [0.9.9] - 2026-10-24

### Security
- The dev proxy asks for its token on every request a reverse proxy forwards, also one running on the same machine: only a request from this machine to a `localhost` name goes without it, and a peer listed in `proxy.trusted_proxies` never counts as this machine. With a real `proxy.domain`, `pom proxy` now refuses to start without a token (`pom proxy token new`) or `allow_unauthenticated`, even when bound to loopback. (#382)
- An agent can only deny another agent's permission prompt; an allow comes from a person or an orchestrator, unless pom.yml sets `agents.peer_approve: true`. (#383)
- Importing a workspace archive unpacks only plain files: untracked files never land under a `.git`, links and other special entries stop the import, setuid bits are dropped, and the manifest's paths and repo names are checked. (#384)
- The secrets key is no longer replaced when it cannot be read or has the wrong length, which lost every stored secret; it is created only when missing. (#381)
- Update pom on a host installs only a tarball whose signature verifies, and picks the app's own version (or, for a nightly no longer kept, the newest nightly). (#390)

### Added
- Update channels: Settings > General > Update Channel picks stable or nightly. Stable never offers a nightly; nightly takes the newest of both, so a later stable replaces it and switching back never downgrades. Each night main is built, signed and notarized into one rolling `nightly` prerelease. (#386, #388)
- Linux updates: Linux tarballs ship with a signature, the app installed by `install.sh` updates itself like on macOS, and `pom self-update [--channel] [--version] [--check]` verifies before it installs. `install.sh` takes `--channel` and `--version`. A package-managed install is never replaced. (#389)

### Changed
- Settings opened from a remote window has two tabs in its title bar, This Mac and the host. The host tab changes the host's own settings (workspaces, pom server, agent command, terminal shell, dev services, Jira) and opens its settings.json in the remote editor; This Mac notes which sections apply only here. Needs this pom on the host. (#393)
- In a remote window, the main agent starts with the host's Agent Command and new terminals use the host's Terminal Shell, read from the host's own settings. Needs this pom on the host. (#371, #349)
- SECURITY.md describes the screen fallback, the secrets key and imports as they work now. (#385)

### Fixed
- An agent opened from a remote window opens where Open Agents In says, next to the code or in the dock. (#348)

## [0.9.8] - 2026-10-24

### Added
- The Git panel in a remote window: the same panel, branch menu and branch diff as a local one, with git running on the host through pom server; fetch, pull and push run there with progress. A host with an older pom says to update it; a failed sign-in points to `pom doctor` on the host. Needs this pom on the host. (#335, #338, #339)
- Pull requests in a remote window: summaries on the workspace rows and the PR blocks and tabs in the Git panel, using the GitHub token of the `<project>@<host>` session, else the project's own. (#343)
- `pom doctor` checks that git can sign in to each repo's origin (`git.auth:<repo>`, with a fix for ssh or https); `--quick` skips it. (#345)
- Pasting an image (Cmd+V or Ctrl+V with a screenshot on the clipboard) into a terminal or agent tab of a remote window uploads it to a private folder on the host and pastes its path, so the agent there attaches it. Needs this pom on the host. (#342)
- SECURITY.md: how to report a vulnerability, and a threat model per surface with its known gaps. (#331)

### Changed
- An agent opened from the app in a remote window starts as it does locally, with your MCP servers and plugins; agents started by `pom agent start` or by another agent stay isolated. Needs this pom on the host; restart agents already running. (#336)
- The screen-reading fallback never types an allow into an agent's dialog: `pom agent allow` on a dialog no hook holds fails with `allow_needs_hook`, nothing is typed while a person drives, and every key is sent only if a fresh read shows the same dialog. `pom agent start --trust` trusts the folder through the agent's config. (#332)
- An agent's `config_set` can no longer change the `agents` section of pom.yml or overwrite a pom.yml it cannot read. (#334)
- Env files are written 0600, and credential headers in Dev Requests show `[redacted]`. (#334)
- Pomelo refuses a terminal socket folder that is a symlink or owned by another user. (#334)

### Fixed
- A remote file tree that stayed empty on a large workspace: the host watches only the folders the tree shows, never node_modules, and one workspace's watch no longer holds up the others. Needs this pom on the host. (#333)
- Switching workspaces in a remote window no longer loses the terminal, editor and agent tabs; an agent tab moved next to the code comes back there. (#341, #337)

## [0.9.7] - 2026-10-23

### Added
- Project search and replace in a remote window: the search runs on the host and results stream in; replace-all writes there and reports files that changed meanwhile. (#322)
- Jira in a remote window uses the host's own Jira settings and calls Jira from there; Settings > Integrations edits them on the host, and Settings > Project shows the remote project. (#324)
- The title-bar Claude account chip and today's cost in a remote window show the host's account, limits and cost. (#321)
- The node_modules Store page in a remote window shows the host's store and runs its actions there. (#325)
- `pom proxy token [new|rm]` and the `proxy.token_env` / `proxy.allow_unauthenticated` settings. (#329)

### Changed
- A dev proxy bound beyond loopback no longer starts without a token (or `allow_unauthenticated`); other machines pass it once as `?pom_token=` (kept as a cookie) or in `X-Pom-Token`. (#329)
- The dev proxy answers only its own host names (421 otherwise), keeps `/_pom/` for itself, and refuses cross-site browser requests to `/_pom/` and the webhook relay. (#329)
- A workspace on both this machine and a host sends its requests to the host while its service is down here; the 503 page links the `<svc>.<repo>.<ws>.<host>.localhost` address. (#327)
- `@prisma` and `prisma` are kept in the node_modules store but each workspace gets its own writable copy; `modules_private` names starting with `.` are never stored, others are copied privately, and presets can set it. (#318)
- With several lockfiles, the node_modules store picks the package manager from the setup's install command, then `packageManager` in package.json. (#318)
- The remote file tree loads much faster; gitignored folders load when opened. The file tree builds faster locally too. (#319)

### Fixed
- `prisma generate` no longer fails with EACCES on its second run in a workspace using hard links. (#318)
- The Agent button, Cmd+? and the Agent menu item open the host's agent in a remote window. (#323)
- The Store page in a remote window no longer waits forever on "Reading lockfiles". (#325)
- The Dev Requests list no longer scrolls over its toolbar. (#320)
- The "reachable here" notice for a host's shared services shows once per window instead of on every workspace switch or reconnect. (#320)
- `pom -c pom.yml` with a relative path no longer prints "cannot make an empty path absolute". (#326)
- Settings opened from a remote window no longer edit another window's local project, and quick Jira edits reach the host in order. (#324, #328)

## [0.9.6] - 2026-10-23

### Added
- Files on a remote project: the host's file tree updates live, files open and save there with conflicts caught instead of overwritten, tree changes (create, rename, copy, delete, trash), saves that wait for the connection, and unsaved edits kept across restarts. (#299, #301, #307)
- Ports for a remote project: the local dev proxy forwards a host's workspace addresses, so `svc.repo.ws.localhost:8767` opens here as it does locally; the shown workspace's shared services are forwarded too, with a Ports tab and `pom -H <host> forward` / `pom host ports`. (#305, #306)
- A remote project's Services panel and service tab are the same as a local one's, in the left dock. (#304, #308)
- `repos.<name>.modules_private` keeps per-workspace folders out of the shared node_modules store; `.prisma` is always kept out. (#316)
- `pom queue --redis <name>`; queue commands find the shared Redis from the env and Bull v4 queues without a meta key. (#302)

### Changed
- `pom queue counts` and `wait-idle` count only due work: repeatable jobs and delayed jobs not yet due no longer keep a service busy. Their JSON is now `pom.queue/v2`. (#302)
- `pom proxy fault add`, `rm` and `clear` return once the running proxy has applied the change, with `applied` in `--json`. (#303)
- `--json` transfers (`ws export|import`, `project export|import`) print one JSON document on stdout, also on errors, with progress as JSON lines on stderr. (#303)
- Projects created or used from the CLI appear in `pom project ls` and Connect to host. (#302)

### Fixed
- `pom agent send` never presses its retry Enter into a dialog the agent drew meanwhile, so it cannot approve a tool by accident. (#315)
- `pom agent ls` shows a turn as running right after `send`, and `agent wait` no longer sees a stale state after a prompt is answered. (#303, #309)

## [0.9.5] - 2026-10-23

### Added
- `pom agent allow` / `deny <handle> <id>` answer the agent's own permission prompts from another client; `pom agent ls --json` shows the waiting prompt (`pending_permission`: tool, input, id). (#293, #297, #298)
- `pom agent watch --live` and `pom agent read --live` stream the reply the agent is writing. (#293)
- `--as-person` on `agent send`, `ask`, `interrupt`, `allow` and `deny` lets the person's other client (web, phone) act for them without changing who drives the session. (#291)
- `pom agent ls --json` adds `running` and `phase`, with the full list of states and reasons documented. (#291)
- `pom commands -o json` lists every declared command, and `pom run <name>` runs any of them, picking the repo that declares it. (#292)
- `pom ws ls --json` adds `last_activity_at` and `last_activity_source`. (#292)
- `pom ps --orphans [--kill]` finds sessions whose workspace is gone; `pom doctor` reports them. (#294)
- `pom doctor --quick`, and `pom doctor` warns when another `pom` earlier on the PATH shadows this one. (#294)

### Changed
- A remote project shows its host in the title bar and a compact page with one place for the connection status. (#295)
- `pom doctor` runs its checks side by side with a time limit each and reports how long each took. (#294)
- `agent wait --until turn-end` waits for the turn after the call, not one that already ended; `--turn n` waits for exactly turn n. (#291)
- Self-built `pom` reports `<version>-dev+<commit>`. (#294)

### Fixed
- Deleting a workspace stops every agent session in it, not just the main one. (#294)
- `pom run` found custom commands declared in a repo other than the first, and ones sharing a command with another name. (#292)
- `pom agent start --trust` no longer answers No when the agent redraws its trust prompt, and reads the prompt and permission dialogs of current agent versions. (#297, #298)

## [0.9.4] - 2026-10-23

### Added
- Work on a project that runs on another machine: `pom host add` and `pom -H <host> <command>` drive pom over ssh with the JSON and exit code passed through, and the app's Connect to host... opens a project on that host. (#267, #268, #269)
- On a remote project the app opens terminals, service output and the agent dock (with leases and New agent), creates, sleeps, wakes and pulls workspaces with live progress, and shows live state, all through `pom server` over one ssh session. (#279-#286)
- A dropped connection reconnects by itself and terminals, logs and jobs carry on without losing or repeating output; a host with an older pom shows Update pom on <host>, which installs the matching release after checking its checksum. (#287, #288)
- `pom server install-unit` (Linux, opt-in) runs the server under systemd; `pom doctor` reports the server, its socket folder and lingering. (#289)
- The dev proxy can be reached from another machine behind a reverse proxy: `proxy.bind`, `proxy.domain`, public scheme and port, and trusted proxies, with forwarded headers and Secure cookies kept on https. (#266)
- Workspace limits on Linux: `limits: { cpu, memory }` in pom.yml caps each workspace, and `pom ps` sums CPU and memory per workspace. (#272)
- `pom ws sleep` and `pom ws wake` stop a workspace's services and agents and bring back what ran, and a request through the dev proxy wakes a sleeping workspace; `idle_sleep_minutes` and `max_running` put idle workspaces to sleep and queue the rest. (#273, #274)
- `pom ws export` / `pom ws import` (and `--to <host>`) move a workspace with its unpushed commits, uncommitted and untracked files, database data and the secrets it uses; `pom project export/import` moves a project. (#275, #276)
- `pom ws gc` cleans up workspaces whose pull requests merged, and `workspaces.auto_cleanup` can ask or do it in the app. (#270)
- Sync on push: a signed GitHub webhook to `/_pom/github` fast-forwards a clean workspace and restarts only the services it touched, or notifies; `pom github webhook setup` and `pom sync status`. (#277, #278)

### Changed
- `pom disk` and `pom doctor` warn when a disk or LVM thin pool is over 80% full, with the commands to free space. (#270)
- `pom doctor` on Linux warns about Docker nested in LXC, high load and processes stuck in disk wait. (#271)
- Deleting a workspace also removes its agent session files and the Docker volumes labelled for it. (#270)
- `pom modules prune` runs at most once a day on its own. (#270)

### Fixed
- A terminal that falls behind no longer silently loses output: it resumes where it stopped. (#281)
- A terminal could briefly look stopped right after it started. (#290)
- A GitHub delivery that arrived as the relay started was refused. (#290)

## [0.9.3] - 2026-10-22

### Added
- Linux: Pomelo runs natively on Linux (Wayland and X11), with its own title bar, Ctrl keys, desktop notifications and the login shell; each release ships x86_64 and aarch64 tarballs with an `install.sh`. (#261, #262)
- A standalone `pom` CLI tarball for Linux servers (`install.sh --cli-only`), with no display libraries needed; `pom doctor --json` and `pom --version`. (#261, #262)
- `pom run --profile test` runs a command with the repo's `.env.test` values while keeping the workspace's own database, Redis, URLs and ports; a `test:` block in pom.yml makes `pom run test` use it, and `--explain-env` shows where each variable comes from. (#265)
- `pom agent ls --json` says why a session holds: `pending_approval`, `idle_notification`, `stale`. (#263)

### Changed
- Shared Docker services are checked from the host (Postgres handshake, Redis PING, TCP), and their compose healthchecks are turned off, so pom never runs `docker exec` on a timer. (#264)
- `pom start --wait` also waits for shared services to answer, and `pom status` shows one that runs but does not answer yet as starting. (#264)
- `pom doctor` on Linux checks Docker Engine and the Compose plugin instead of asking for Docker Desktop or OrbStack. (#261)
- `pom agent send --queue` waits at most 30 minutes by default (`--timeout` to change it). (#263)

### Fixed
- An agent idle for a long time can be sent to again: Claude's idle reminder no longer counts as a permission prompt. (#263)
- Opening a file from outside the workspace, like pom.yml, in an external editor uses the right path. (#261)
- Shared Redis slots are no longer used up when Docker cannot be reached. (#261)
- The dev proxy serves a project that was started from the CLI without opening it in the app. (#261)

## [0.9.2] - 2026-10-22

### Fixed
- Memory no longer climbs while the Mac sleeps or a window is hidden, and no longer spikes after waking: a hidden window stops drawing and redraws once when shown. (#260)

## [0.9.1] - 2026-10-22

### Changed
- Switch workspaces from a picker that lists the ones needing you first, with key hints. (#247)
- Every dialog and confirmation shares one look, the merge confirmation included. (#249, #250)
- The New project page, the Store and Dev Requests tabs, the welcome page and the workspace row menus are redrawn. (#251, #252, #255)
- The welcome page flags a missing Docker, git or agent CLI with a button to fix it. (#255)
- The command palette, Go to file, notifications, toasts and the update button are redrawn; a palette with no match says so. (#254)
- The Database, Environment, Secrets, ticket and project search tabs are redrawn; search results show two lines of context. (#256, #257)
- The editor and terminal are polished: clearer selected rows in pickers and completions, a centered message when the shell fails to start, and the review box uses the UI font. (#258)

### Fixed
- Memory no longer grows over long sessions from files changing on disk, background terminals and caches. (#248)
- `pom ws create` no longer hangs at Provisioning. (#253)
- Drawing a frame takes less CPU. (#253)
- Markdown table cells that fit stay on one line. (#258)
- Restore File and Add to .gitignore find the right repository for the file. (#258)

## [0.9.0] - 2026-10-21

### Added
- Merge a pull request from the app: squash, merge commit or rebase, with the reasons shown when merging is blocked. (#240)
- Review a pull request in the app: reply to and resolve threads, and submit Comment, Request changes or Approve. (#240)
- Resolve merge conflicts in the editor with Use ours / Use theirs / Use both per conflict, Mark Resolved in the Git panel, or Ask Claude to merge. (#240)
- Stage or unstage just the selected lines of a change, and a strip over the change under the caret to stage, restore or jump between hunks. (#237, #238)
- New themes: Catppuccin Mocha, Macchiato, Frappe and Latte, Tokyo Night, Nord and Rose Pine Dawn, each with its own code colors. (#245)

### Changed
- New look: Pomelo Dark and Pomelo Light are the default themes, and panels sit as rounded surfaces with gaps instead of lines. One Dark, One Light, Ayu Mirage and Gruvbox Dark are rebuilt on the same layers. (#234, #235, #245)
- The title bar shows the branch and a pill for the workspace that needs you; every tab bar uses one tab style. (#235, #243)
- The workspaces sidebar says one thing per workspace, and the folded rail flags only what needs you: a waiting agent, PR trouble, running services. (#236)
- The Git panel is redesigned: repo cards in Changes, a clearer commit footer, Remote cards that say what to do next, and History by repo or as one timeline. (#237, #238)
- The pull request tab shows its state, blockers, checks and conversation more clearly. (#239)
- The Settings window uses inset panels with collapsible categories, and a service tab shows its facts as one card. (#241, #244, #246)

### Fixed
- The Git panel stays readable when narrow, and branch file folders start collapsed. (#242)
- Translucent colors blend like a browser, so highlights are no longer too strong. (#236)

## [0.8.9] - 2026-10-09

### Changed
- A browser opening a service that is starting, stopped or not answering gets a page saying what is wrong and how to start it, and it reloads by itself once the service is up. (#232)

### Fixed
- Markdown preview tables size their columns to the content and wrap cell text, instead of running text into the next column. (#231)

## [0.8.8] - 2026-10-06

### Added
- Drive Pomelo from the keyboard: context menus, the file tree, the Git, Services and Database panels and the Settings window move with arrows, Enter and Escape. (#218, #219, #221, #222, #225)
- `ctrl-tab` switches to recent tabs, `cmd-alt-up` / `cmd-alt-down` to the previous or next workspace, and `cmd-alt-a` to one whose agent waits for you. (#220)
- Answer agents and notices from the keyboard: `cmd-k y` / `cmd-k n` allow or deny, `cmd-k o` takes over, `cmd-k enter` runs a notice's action. (#223)
- Close tab sets (`cmd-alt-t`, `cmd-k e` / `t` / `u`), move between regions with `cmd-k tab` and `cmd-escape`, and see each button's key in its tooltip. (#224)
- Vim mode (Settings > Keymap): Normal, Insert and Visual modes, motions, operators, registers, `.` repeat, search, and vim keys in panel lists. (#226, #227, #228, #229)

## [0.8.7] - 2026-10-02

### Added
- Settings > Agents > Open Agents In: open agents as a tab next to your code instead of in the agent dock. (#217)

## [0.8.6] - 2026-10-02

### Added
- Database snapshots for a workspace: `pom db snapshot`, `restore`, `snapshots` and `snapshot drop` cover every database at once; a restore stops and restarts the workspace's services. (#210)
- `prepare-main` keeps a `main__baseline` and new workspaces copy from it without disconnecting main, then keep a `ws__baseline`; `pom db baseline` and `pom db reseed` rebuild them, also as MCP tools. (#211)
- Service healthchecks (`healthcheck: http | cmd`), `pom start --wait` and `pom status -o json` with up, ready and healthy. (#213)
- `pom mark` with `pom logs --since` and `pom db stats --since` to read one test step's logs and queries, repeated queries included; `describe workspace` and `env ls` print JSON. (#214)
- `pom queue wait-idle` and `queue counts` for Sidekiq and BullMQ queues in the workspace's Redis. (#215)
- `pom proxy fault add` injects errors and delays into one workspace's proxied requests, with rules that expire. (#216)

### Fixed
- Redis slots are kept per project, a released slot's data is cleared, slots of workspaces deleted by hand are reclaimed, and unused Redis containers go away; one Redis now holds 64 workspaces. (#212)

## [0.8.5] - 2026-10-02

### Added
- `pom agent` lets scripts, orchestrators and other agents drive the coding agents of one workspace: start, send, wait, ask, read, watch, interrupt, approve, take over and release, with stable JSON and exit codes. (#202, #203, #204, #205, #208)
- Agents get `agent_*` MCP tools to list and message the other sessions of their own workspace, with limits on rate and depth. (#206)
- A workspace policy (`agents.policy` in pom.yml) can allow, deny or ask before each tool call an agent makes; a failing policy denies. (#202, #204)
- An agent tab shows who drives it with Take over, lists pending approvals with Allow / Deny, and the workspace dot reflects every session. (#207)
- Every language has its own file icon in the file tree, tabs and language picker, and `.env`, Dockerfile, Makefile and git message files get one by name. (#201)

### Fixed
- Two agent sessions in one workspace, or the same branch in two projects, no longer overwrite each other's state. (#202)
- Skills that run their own scripts work in agent tabs. (#209)

## [0.8.4] - 2026-10-02

### Added
- Pick the active file's language from the status bar, `cmd-k m` or the command palette ("Select a language..."); a language that comes as a package installs first. (#198)

### Changed
- Service tab controls light up on hover and press, and the pointer turns into a hand over them. (#199)

## [0.8.3] - 2026-10-02

### Added
- Agents can read the full pom.yml and template reference with the new `config_reference` MCP tool. (#195)

### Fixed
- Service URLs no longer end in "backend not reachable" while the service runs: every process now shares one port per service, the proxy follows the port the service really listens on (IPv4 or IPv6), and leftover port leases are cleaned up. (#197)
- A stopped or still-building service now says so in the browser instead of "backend not reachable". (#197)
- Resolving a port conflict restarts the services it moves. (#197)
- Opening a high-resolution image no longer crashes or freezes Pomelo; it is shown scaled down with its original size in the tab. (#196)

## [0.8.2] - 2026-10-01

### Added
- After a crash at launch, Pomelo looks for a fixed release first and reopens windows without the tabs that crashed it, with Reopen Tabs. (#190)
- Settings > General > Restore on Startup chooses whether the last session's tabs reopen. (#190)
- A shared service can be a command (`cmd:`) run once for every workspace, like a mock auth server; agents start and stop it with the new shared_* tools. (#188)

### Fixed
- Symbols the Claude Code TUI draws as text, such as its line markers, showed as color emoji in the terminal. (#185)
- Scrolling over a service tab's header scrolled its log. (#186)
- Stop Agent in an agent tab's menu pinned the tab instead of stopping the agent. (#187)

## [0.8.1] - 2026-10-01

### Fixed
- 0.8.0 quit as soon as a file opened: the release is now signed so language packages can run, and built-in languages no longer depend on them. (#183)

## [0.8.0] - 2026-10-01

### Added
- Settings > Languages & Tools: turn language servers on or off and choose them per language. (#146)
- The status bar lists the running language servers with their progress, and downloads missing npm servers. (#147)
- Each file runs every server its language uses, each in its project's own folder (nearest Gemfile, package.json, Cargo.toml...). (#157)
- Tailwind CSS and ESLint language servers for web projects. (#157, #168)
- Ruby files run solargraph, or ruby-lsp when chosen in settings. (#155)
- A Project Diagnostics tab lists every error and warning; open it from the status bar or Cmd+Shift+M. (#156)
- Language server messages and questions show as notifications; a too-large Ruby workspace offers to switch to ruby-lsp. (#161)
- View Logs for each language server, a notice with the install command when one is missing, and a note when it can't go to a definition. (#166)
- Preview tabs: a single click in the file tree opens an italic preview that the next one replaces; edit or double-click to keep it. (#165)
- Editors get a breadcrumb bar with search and editor controls, and an optional minimap. (#141, #142, #145)
- Cursor shape, blink and animation, the multi-cursor modifier and reduce motion are settings. (#136)
- Shared service tabs show the status, image, port, connection URL and users above their logs. (#170)
- Rarely used languages (Kotlin, Swift, OCaml, Lua and 17 more) install their highlighting on first use and update themselves. (#175, #178, #182)
- Stop an agent from its tab's menu, and choose what closing an agent tab does. (#135)

### Changed
- The app is about 30 MB smaller, now that rare languages' grammars are downloads. (#178)
- Edit in settings.json opens the file in a Pomelo tab, and hand edits apply as soon as you save. (#158, #159)
- Settings keep keys they don't know, so an older version no longer drops a newer version's settings. (#162)
- Clicking another workspace's ticket switches to that workspace first; only the ticket status opens it. (#165, #171)
- Long tab titles trail off, and file finder paths shorten to fit their row. (#163, #165)
- Highlighting follows each grammar's own queries, and the editor's text size is set apart from the UI's. (#138, #140)
- Status bar buttons can each be turned off in Settings, and every font setting sits under Appearance. (#139, #143)
- Completions fade a long label's tail and show the details a server adds. (#151)

### Fixed
- The Start, Stop and Restart buttons of a service tab could stop responding. (#169)
- Typing into the command palette went to the console of the tab under it. (#160)
- Holding Cmd+V, Cmd+X or Cmd+Z stuttered instead of repeating smoothly. (#152)
- Shift+click extends the selection, and Cmd+click follows a link on release instead of selecting it. (#153, #154)
- Items of a submenu opened to the left could not be clicked. (#148)
- Gitignored files showed a git gutter. (#150)
- Indent guides drew over the gutter when the editor scrolled sideways. (#137)
- A window moved to another display redraws at that display's scale. (#134)
- Updates install by rename and say why an install failed. (#133)

## [0.7.7] - 2026-09-30

### Added
- Table tabs edit data: double-click a cell, Review SQL, then Apply runs it all in one transaction. (#132)
- A Details side shows the whole value (JSON as a tree) or the whole row and what points at it. (#132)
- Foreign keys open the row they point at; Structure and DDL views, column filters; tabs reopen next session. (#132)
- Redis keyspaces open as a tab: keys by prefix, values by type, TTL editing and deletes. (#132)
- Buckets and folders open as a folder tab with find, Upload here and a preview side. (#132)
- Settings > Dev Services toggles the proxy and webhook relay and sets their ports; a Dev Requests tab lists their traffic. (#127)
- The shared node_modules store works on macOS and Linux, with a store tab and size and age limits. (#128)
- Create Workspace picks tickets from a sprint, the backlog or your own, and chooses environment and data. (#130)

### Changed
- Windows reopen where you left them, and status bar and title bar toggles apply right away. (#126)
- JSON request and response bodies in Dev Requests are colored like the editor colors JSON. (#131)

## [0.7.6] - 2026-09-30

### Added
- Your own themes: theme files in ~/.config/pomelo/themes join the built-in ones, name only the colors they change, and apply as soon as you save them. theme_overrides in settings.json adjusts any theme. (#122)
- Theme Mode Dynamic picks a light and a dark theme, and System follows macOS as it switches. (#122)
- The editor and terminal fonts have a family, weight and line height, and every font takes OpenType features and fallback families. (#124)

### Changed
- The renderer moves to wgpu 30 and a newer text engine; live resize stays smooth. (#123)

### Fixed
- Double-clicking anywhere on the title bar zooms the window as System Settings says, and dragging there moves it. (#121)
- Edit in settings.json on the font rows opens the settings file. (#124)

## [0.7.5] - 2026-09-30

### Added
- A macOS menu bar: Pomelo, File, Edit, View, Go, Window and Help, each command with the key bound to it now. (#119)
- keymap.json accepts secondary- (Command on macOS, Control elsewhere) and ["pane::ActivateItem", n]; tab switching can be rebound. (#118)

### Changed
- Window commands use Command on macOS: Git cmd-shift-c, Services cmd-shift-s, Database cmd-shift-d, Pull Requests cmd-shift-r, Switch Workspace cmd-alt-o, tabs cmd-1 to cmd-9 and cmd-0 for the last. (#118)
- The app icon is sized like other Mac apps in the Dock and cmd-tab. (#116)

### Fixed
- Cmd+? opens the agent. (#117)

## [0.7.4] - 2026-09-30

### Added
- Updates download in the background and wait for you: the title bar shows the download, then Restart to Update. Quitting installs a staged update too, and afterwards a notification offers the release notes. (#112)
- The app menu has Release Notes, and Settings shows the version with Check Now, Restart to Update or Try Again. (#112)
- A running service's tab is its live console: type into prompts and REPLs, paste, and use the mouse. Filter the logs to get the log list back. (#114)
- Read-only tabs show a lock next to the name. (#115)

### Changed
- Automatic update checks run hourly and stay quiet; only Check for Updates reports checking and up to date. (#112)
- Unwrapped service logs keep the time column in place and scroll the text sideways; wrapped rows line the time up with the first line. (#114)

### Fixed
- Very large files (millions of lines) open in a fraction of a second, scroll smoothly anywhere and take keystrokes instantly, without growing to gigabytes of memory. (#115)
- Typing in a large unsaved file no longer freezes the app while it keeps the changes for the next launch. (#115)
- A long value in a text field stays inside its box and keeps the caret in view. (#113)

## [0.7.3] - 2026-09-29

### Fixed
- Check for Updates says what it found: checking, up to date, downloading, or why it failed. (#110)
- After installing an update, Pomelo opens again by itself instead of staying closed. (#110)
- cmd-? opens the agent again. (#110)
- A long repo name no longer pushes Publish out of its card in the Git panel's Remote tab. (#110)
- The window buttons stay centered in the title bar. (#110)

## [0.7.2] - 2026-09-29

### Fixed
- The editor shows its change strips again: added, modified and deleted lines are marked in the gutter. (#109)
- Clicking the status bar button of the panel already open closes the left panel column, like the other docks. (#109)
- A narrow agent dock no longer pushes Send to main and Archive out of the side agent bar. (#109)

## [0.7.1] - 2026-09-29

### Fixed
- Pomelo 0.6.x can update to this version: 0.7.0's app left out the update key 0.6.x checks, so its updater reported "improperly signed". (#108)

## [0.7.0] - 2026-09-29

### Changed
- Pomelo is rebuilt in Rust: one native app with a GPU-drawn UI and the core built in, no daemon or local server. Installs update in place and keep their permissions. (#107)
- A project is one pom.yml; templates use dot notation (`{{shared.postgres.url}}`), and every save is checked before it applies. (#107)
- A repo's quick commands are `tasks:` in pom.yml (the repo menu's Run Task); `shortcuts:` still reads. (#107)
- Updates install only when their signature verifies. (#107)

### Added
- A full code editor: language servers, tree-sitter highlighting, multiple cursors, project search, split panes, Markdown preview. (#107)
- New project setup: pick the repos, then Claude Code, Codex or Gemini CLI writes pom.yml and Pomelo verifies every service boots. (#107)
- Side agents next to the main agent: Ask, Review, Second opinion or Fix, started as a fork, a compacted fork or fresh. (#107)
- Agent usage: Claude plan limits in the title bar, today's cost in the status bar, and a usage tab by day, workspace and model. (#107)
- The Services panel shows what needs attention with its fix, and a service opens as a tab with its live logs. (#107)
- A Git panel that stages and commits across every repo of the branch, with unified and split branch diffs. (#107)
- Database consoles, table grids with filters and CSV export, and Redis and MinIO browsing. (#107)

### Removed
- The iOS remote app. (#107)

## [0.6.1] - 2026-09-16

### Fixed
- Fixed a crash on macOS 27 that could close the app when opening the Files editor or the Database SQL editor. The editor no longer re-triggers a layout pass from within one, so short files and switching panes are stable.

## [0.6.0] - 2026-09-15

### Added
- The Files pane is now a full code editor: edit and save in place (Cmd+S), with native tree-sitter syntax highlighting for JSON, JavaScript, TypeScript/TSX, Python, Ruby, Java, and SQL. No more opening another editor just to read or tweak a file.
- Indentation guides and a git change gutter in the editor: a colored bar marks added, modified, and deleted lines against HEAD, and the caret's line shows an inline git-blame annotation (author and how long ago).
- The file tree tints files with uncommitted changes, and the folders that contain them, so you can see at a glance where your edits are.
- A themed right-click menu for the tree and the editor (Cut/Copy/Paste, Copy Path, Copy Relative Path, Reveal in Finder, Open in Terminal). Open in Terminal opens Pomelo's built-in terminal in that folder, and right-clicking in the editor moves the caret to the click.
- Editor font size shortcuts (Cmd +, Cmd -, Cmd 0), and the Files pane now remembers the open file and which folders are expanded per workspace.

### Changed
- The Files editor is always editable now (the separate read-only mode is gone); the Save button appears only when there are unsaved edits.

## [0.5.10] - 2026-09-14

### Fixed
- A service that takes a while to start no longer 502s through the dev-proxy while it is still building. A slow first build (for example a Nest service that runs a full build before it starts listening) could lose its allocated port mid-build, so the workspace URL returned "no dev-proxy route" even though the service was on its way up. The port is now held for as long as the service's process is alive, and the proxy reports "still starting" (retriable) instead of a route error while it builds.
- The workspace list no longer shows the red "awaiting input" dot when the agent is simply idle. Claude's idle notification ("Claude is waiting for your input") was misread as a prompt; only a real permission or tool-input request now lights the dot.

## [0.5.9] - 2026-09-14

### Added
- The Files pane tracks the filesystem: a file created, deleted, or renamed on disk appears in the tree without reopening the pane, and open folders stay open across a refresh. Changes under .git and node_modules are ignored, so ordinary git and dev-server churn costs nothing.
- Markdown files in the Files pane render as formatted text, with a Preview / Raw toggle to switch to the highlighted source.

### Fixed
- The Files pane lists files that sit directly in the workspace folder rather than inside a repo (CLAUDE.md, docker-compose.yml, and the like), and no longer hides dot-entries at that level while showing them inside repos.
- Shortcuts, terminals, and services now launch with the same tool-augmented PATH the agent uses, so commands like `npm`, `node`, and `pnpm` from a version manager (nvm, fnm, volta, asdf) are found instead of failing with `command not found`.

## [0.5.8] - 2026-09-10

### Fixed
- The agent no longer fails with `exec: "claude": executable file not found in $PATH` when the Claude CLI is installed in ~/.local/bin (the native installer's location). Pomelo now probes the well-known install paths and an interactive shell to find it, and never caches a failed lookup.

## [0.5.7] - 2026-09-05

### Fixed
- Creating your first session no longer fails with "no server". Session creation bootstraps a project before any server is running, so it now runs standalone instead of requiring one, and the app boots straight into the new session.

## [0.5.6] - 2026-09-05

### Added
- The iOS remote terminal now renders on the GPU (Metal) like the Mac, so scrolling is smooth and Nerd Font icons (devicons, folder, Material Design glyphs) render instead of showing blanks. (#85)
- Files pane: browse everything in the workspace folder from one tree spanning every repo, and preview a file without opening an editor. Code renders with syntax highlighting and line numbers, images get a zoomable viewer, and other binaries show their type instead of garbage. Right-click a file or folder to reveal it in Finder or copy its path, and select lines in a code preview to ask the agent about that block. (#84)

### Changed
- The Metal terminal, theme, and design system moved into shared cross-platform packages (PomeloTerminalKit / PomeloUI / PomeloCore) used by both the Mac app and the iOS remote. The macOS app is unchanged; the split is the groundwork for sharing UI across macOS/iPadOS/iOS. (#85)
- The PRs and Git panes are merged into one Git pane. Local changes now show git status inline in the diff file tree — stage, unstage, and discard on hover, commit staged changes, and push — instead of a separate flat changes list. (#82)

### Fixed
- Pomelo.app no longer drops a `default.profraw` file into whatever directory Claude Code happens to be working in. The globally installed Claude Code hook re-execs the app binary on every tool-use event, and both the Debug and Release builds were compiled with code coverage instrumentation on, so each re-exec wrote a coverage file to the current directory — for every project on the machine, not just Pomelo workspaces. (#83)

## [0.5.4] - 2026-09-04

### Changed
- The GPU (Metal) terminal renderer is now the default. It reaches parity with the previous renderer and adds glyph-atlas rendering: scrollback, text selection that sticks to the text as it scrolls (with drag-to-select auto-scroll past the top and bottom edges), bold / dim / inverse / italic / underline / strikethrough, wide (CJK) characters, a cursor that dims when the terminal is unfocused, and Cmd+K to clear. (#81)

### Added
- Nerd Font icons render in the terminal, so TUIs show their devicons, folder, and Material Design glyphs. Missing glyphs fall back through installed Nerd Fonts and are scaled to fit the cell instead of clipping. (#81)
- Terminal font setting: pick any installed monospaced or Nerd Font family for the terminal. (#81)

### Fixed
- The terminal, service log peek, split handle, and icon buttons re-tint immediately on a light/dark theme switch instead of keeping stale colors. (#81)
- Squared the top corners of the Metal terminal and the service peek log, and matched the agent and golden-source header heights. (#81)

## [0.5.3] - 2026-09-03

### Added
- PR conversation reads like GitHub: a review's inline comments are grouped into threads nested under the review, with the root comment and its replies in one card, relative timestamps, and Bot/Author tags. Resolved threads collapse by default so a long review stays scannable, and each thread expands on its own without re-rendering the rest of the timeline. (#80)
- Experimental GPU (Metal) terminal renderer, opt-in behind a flag: scrollback, text selection that sticks to the text as it scrolls, drag-to-select auto-scroll past the top and bottom edges, bold / dim / inverse styling, SF Mono at the configured font size, and Cmd+K to clear. (#80)

### Fixed
- The app no longer writes a performance log to /tmp on every launch; that log is created only while the Perf HUD is enabled and removed when it is turned off. (#80)

## [0.5.2] - 2026-09-03

### Fixed
- The app stays responsive under a busy workspace. State updates no longer re-render the whole window on every poll: the app-wide store and its hot view models moved to per-property observation, so a view only redraws when a value it reads changes. The agent status orb now animates on the compositor instead of a run-loop animation that pinned the display at max refresh, and the terminal mirror coalesces a log firehose instead of feeding the main thread every frame. Sidebar transitions, the services grid, and the command palette are smooth again and battery use drops. (#79)

## [0.5.1] - 2026-09-01

### Changed
- The agent terminal mirror reconnects seamlessly. The PTY holder now frames its scrollback snapshot and streams a byte offset, so a reattaching client resumes from where it left off instead of replaying the whole buffer over its live screen. A client that is caught up replays nothing. (#78)

### Fixed
- iOS terminal no longer garbles after the app is backgrounded or reloads when switching tabs. A background assertion keeps a brief app switch connected, and a real reconnect resets the buffer before the replay. (#78)
- iOS home list refreshes every paired Mac concurrently and no longer sticks on "connecting" while one is slow or offline. (#78)

### Added
- iOS pairing scanner gets continuous autofocus with an ultra-wide fallback for close QR codes, tap-to-focus, and a Camera-style pinch-zoom bar. The agent terminal supports swipe-to-scroll for full-screen agents. (#78)

## [0.5.0] - 2026-08-31

### Added
- Remote control: pair a phone over LAN or Tailscale and drive Pomelo from an iOS companion app - dashboard, live agent terminal, PR and Jira views, create and rename workspaces. Trust is a pinned self-signed certificate over a bearer-authenticated, allowlisted control API (no browser, no HTTP port). (#77)
- iOS app: dark, light, and sepia themes, a home-screen widget, and a Live Activity / Dynamic Island that shows active agent status and Claude usage. (#77)
- Git panel: a Git tab in the workspace pane with file status and stage, commit, and push actions. (#77)

### Fixed
- Service URLs route again after Pomelo restarts while an agent window is still open. Each `pom mcp` server was claiming the dev-proxy and webhook-relay ports, so an MCP process that outlived the app kept them and every workspace service answered "no dev-proxy route" - the MCP server no longer starts those listeners. The app also logs when the dev-proxy port is already taken instead of failing silently. (#76)
- The PRs pane stays usable at narrow split widths. (#75)

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
