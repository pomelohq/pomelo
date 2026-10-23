//! Render a screen headlessly to a PNG so the UI can be eyeballed without launching a window. Usage:
//!   cargo run -p ui_snapshot -- [out.png] [category_index]
//! Defaults: /tmp/pomelo-ui.png, the Appearance page (all categories expanded).
//!   cargo run -p ui_snapshot -- --scene <id> <out.png> [--theme dark|light] [--size WxH]
//! renders the screen a design board stands for, at the board's size and a device scale of 1, so the two can
//! be compared pixel for pixel.

use std::io::BufWriter;

mod e2e;
mod editor_fixture;
mod git_fixture;
mod modal_fixture;
mod remote_fixture;

/// The project the services scenes draw, local and remote alike.
const SERVICES_YML: &str = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\n  mail:\n    image: axllent/mailpit\n  mock-as:\n    cmd: node scripts/mock-as.js\n    repo: api\nworkspace_services:\n  gateway:\n    cmd: caddy run\n    port: 8080\nrepos:\n  api:\n    env:\n      .env.development:\n        RAILS_ENV: development\n    shared_services: [postgres, redis, files, mock-as]\n    services:\n      server:\n        type: backend\n        port: true\n        mode: dev\n        modes:\n          dev: bin/rails server -p $PORT\n      jobs:\n        mode: dev\n        modes:\n          dev: bundle exec sidekiq\n      mailer: bin/mailer\n  web:\n    services:\n      app:\n        type: frontend\n        port: true\n        mode: hmr\n        modes:\n          hmr: npm run dev -- --port $PORT\n      storybook:\n        type: frontend\n        cmd: npm run storybook -- -p $PORT\n";
mod terminal_fixture;

/// A design board's screen: the settings category it shows and the switches that put it in the board's state.
/// Local git whose scans and answers go through JSON, as a remote window receives them.
struct WireGit;

fn over_wire<T: serde::Serialize + serde::de::DeserializeOwned>(value: T) -> T {
    serde_json::to_value(&value)
        .and_then(serde_json::from_value)
        .unwrap_or(value)
}

impl git::access::GitAccess for WireGit {
    fn scan(&self, repo: &git::access::RepoSource) -> git::scan::RepoScan {
        over_wire(git::access::LocalGit.scan(repo))
    }

    fn act(
        &self,
        repo: &git::access::RepoSource,
        op: git::access::GitOp,
    ) -> Result<(), git::working_copy::CommandError> {
        git::access::LocalGit.act(repo, over_wire(op))
    }

    fn read(
        &self,
        repo: &git::access::RepoSource,
        query: git::access::GitRead,
    ) -> Result<git::access::GitAnswer, String> {
        git::access::LocalGit
            .read(repo, over_wire(query))
            .map(over_wire)
    }

    fn remote(
        &self,
        repo: &git::access::RepoSource,
        command: git::access::RemoteCommand,
        progress: &(dyn Fn(&str) + Sync),
    ) -> Result<git::working_copy::CommandOutput, git::working_copy::CommandError> {
        git::access::LocalGit.remote(repo, over_wire(command), progress)
    }
}

fn scene(id: &str) -> Option<(usize, &'static [(&'static str, &'static str)])> {
    Some(match id {
        "settings-appearance" => (settings_ui::APPEARANCE, &[]),
        "settings-keymap" => (settings_ui::KEYMAP, &[]),
        "settings-agent" => (settings_ui::AGENT, &[]),
        "settings-languages" => (settings_ui::LANGUAGES_TOOLS, &[]),
        "settings-search" => (settings_ui::APPEARANCE, &[("SETTINGSSEARCH", "font")]),
        // The main window as the window boards draw it: rail folded, the Files tree, three tabs, the agent dock.
        "window-proposed" => (0, &[("MAINVIEW", "window"), ("RAIL", "1")]),
        "project-sidebar-window" => (0, &[("MAINVIEW", "window")]),
        "project-sidebar-window-folded" => (0, &[("MAINVIEW", "window"), ("RAIL", "1")]),
        // The strip over an agent someone else drives while a tool call waits, at the dock's width.
        "agents-strip" => (0, &[("AGENTSTRIP", "1")]),
        // cmd-alt-o over the window boards' workspaces, alone at the picker's width; SWITCHER is the query.
        "project-switcher" => (0, &[("SWITCHER", "")]),
        // Each window modal alone at its width, in the board's state; MODAL names it, PROMPT a confirmation.
        "project-create" => (0, &[("MODAL", "create")]),
        "project-create-branch" => (0, &[("MODAL", "create-branch")]),
        "project-create-taken" => (0, &[("MODAL", "create-taken")]),
        "project-rename" => (0, &[("MODAL", "rename")]),
        "project-add-repo" => (0, &[("MODAL", "add-repo")]),
        "git-use-branch" => (0, &[("MODAL", "use-branch")]),
        "project-delete" => (0, &[("PROMPT", "delete")]),
        "git-discard-all" => (0, &[("PROMPT", "discard-all")]),
        "git-discard-file" => (0, &[("PROMPT", "discard-file")]),
        "git-pr-merge-confirm" => (0, &[("MERGECONFIRM", "1")]),
        "project-switcher-filtered" => (0, &[("SWITCHER", "rate")]),
        // cmd-shift-p and cmd-p alone at their width; PICKER is the picker and its query.
        "system-palette" => (0, &[("PICKER", "palette:work")]),
        "system-palette-empty" => (0, &[("PICKER", "palette:deploy prod")]),
        "system-file-finder" => (0, &[("PICKER", "finder:users")]),
        // A notification card, a toast or the title bar's update button alone; NOTICE names which.
        "system-notifications" => (0, &[("NOTICE", "update-ready")]),
        "system-notifications-toast" => (0, &[("NOTICE", "toast")]),
        "system-notifications-update" => (0, &[("NOTICE", "update-ready-button")]),
        "system-invalid-config" => (0, &[("NOTICE", "invalid-config")]),
        "system-update-states" => (0, &[("NOTICE", "update-states")]),
        "data-table" => (0, &[("DATABASE", "board-table")]),
        "data-keyspace" => (0, &[("DATABASE", "board-keyspace")]),
        "data-bucket" => (0, &[("DATABASE", "board-bucket")]),
        "data-failed" => (0, &[("DBFAILURE", "1")]),
        // The Environment and Secrets tab bodies; ENVTAB=board-* draws the boards' rows.
        "data-env" => (0, &[("ENVTAB", "board-env")]),
        "data-secrets" => (0, &[("ENVTAB", "board-secrets")]),
        // The editor and terminal boards in the window at the boards' size; CROP keeps the part each one is about.
        "editor-open" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "open"),
                ("CROP", "342,40,1092,832"),
            ],
        ),
        "editor-split" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "split"),
                ("CROP", "342,40,1092,832"),
            ],
        ),
        "editor-tabs" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "tabs"),
                ("CROP", "496,80,448,176"),
            ],
        ),
        "editor-find" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "find"),
                ("CROP", "342,106,1092,85"),
            ],
        ),
        "editor-diagnostics" => (0, &[("DIAGBOARD", "1")]),
        "editor-language" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "language"),
                ("CROP", "448,80,544,376"),
            ],
        ),
        "editor-goto" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "goto"),
                ("CROP", "528,80,384,59"),
            ],
        ),
        "editor-goto-states" => (0, &[("PICKER", "goto-states")]),
        "editor-readonly" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "readonly"),
                ("READONLY", "1"),
                ("CROP", "56,40,1384,860"),
            ],
        ),
        "editor-markdown" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "markdown"),
                ("CROP", "342,40,1092,832"),
            ],
        ),
        "editor-image" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "image"),
                ("CROP", "342,40,1092,832"),
            ],
        ),
        "editor-vim-normal" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "vim-normal"),
                ("CROP", "56,40,1384,860"),
            ],
        ),
        "editor-vim-visual" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "vim-visual"),
                ("CROP", "56,40,1384,860"),
            ],
        ),
        "editor-hover" => (0, &[("PICKER", "hover")]),
        "editor-completion" => (0, &[("PICKER", "completion:fi")]),
        "terminal-dock" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-dock"),
                ("SETTLE", "80"),
                ("CROP", "342,512,1092,360"),
            ],
        ),
        "terminal-split" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-split"),
                ("SETTLE", "80"),
                ("CROP", "342,512,1092,360"),
            ],
        ),
        "terminal-zoomed" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-zoomed"),
                ("SETTLE", "80"),
                ("CROP", "56,48,1378,824"),
            ],
        ),
        "terminal-search" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-search"),
                ("SETTLE", "80"),
                ("CROP", "342,512,1092,360"),
            ],
        ),
        "terminal-exited" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-exited"),
                ("SETTLE", "80"),
                ("CROP", "342,512,1092,360"),
            ],
        ),
        "terminal-spawn-failed" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "term-spawn-failed"),
                ("SETTLE", "80"),
                ("CROP", "342,512,1092,360"),
            ],
        ),
        "editor-tree-menu" => (
            0,
            &[
                ("MAINVIEW", "window"),
                ("RAIL", "1"),
                ("EDITORBOARD", "tree-menu"),
                ("CROP", "200,209,203,449"),
            ],
        ),
        // The ticket tab body from a recorded issue, and with no Jira site set up.
        "data-ticket" => (0, &[("TICKETTAB", "board")]),
        "data-ticket-unset" => (0, &[("TICKETTAB", "board-unset")]),
        // cmd-shift-f over a fixture of the board's three files, and before the first search.
        "editor-search" => (
            0,
            &[("SEARCH", "User.where(active: true)"), ("SEARCHBOARD", "1")],
        ),
        "editor-search-empty" => (0, &[("SEARCH", ""), ("SEARCHBOARD", "1")]),
        // The welcome page alone at the editor area's size, the pointer on New project; WELCOME=docker
        // has Docker stopped.
        "project-welcome" => (0, &[("WELCOME", "ok")]),
        "project-welcome-docker" => (0, &[("WELCOME", "docker")]),
        "project-row-menu" => (0, &[("ROWMENU", "workspace")]),
        "project-row-menu-main" => (0, &[("ROWMENU", "main")]),
        // The New project tab's page alone, at the size of the board's page; ONBOARDSCENE is its state.
        "system-onboarding-repos" => (0, &[("ONBOARDSCENE", "repos")]),
        "system-onboarding-setup" => (0, &[("ONBOARDSCENE", "setup")]),
        "system-onboarding-review" => (0, &[("ONBOARDSCENE", "review")]),
        "system-onboarding-running" => (0, &[("ONBOARDSCENE", "progress")]),
        "system-onboarding-repair" => (0, &[("ONBOARDSCENE", "fixing")]),
        "system-onboarding-done" => (0, &[("ONBOARDSCENE", "done")]),
        "data-store" => (0, &[("PAGESCENE", "store")]),
        "data-store-measuring" => (0, &[("PAGESCENE", "store-measuring")]),
        "data-store-optimize" => (0, &[("PAGESCENE", "store-optimize")]),
        "services-requests" => (0, &[("PAGESCENE", "requests")]),
        "services-requests-empty" => (0, &[("PAGESCENE", "requests-empty")]),
        // A remote window's Ports tab below its tab bar, over a fixed list of forwards.
        "remote-ports" => (0, &[("PAGESCENE", "ports")]),
        "remote-ports-moved" => (0, &[("PAGESCENE", "ports-moved")]),
        "remote-ports-error" => (0, &[("PAGESCENE", "ports-error")]),
        "remote-ports-empty" => (0, &[("PAGESCENE", "ports-empty")]),
        // One tab bar as every pane draws it: the agent dock's, and the design system's sample row.
        // A service tab's facts card, running, stopped and crashed, at the boards' card size.
        "services-running" => (0, &[("SERVICES", "api/server"), ("FACTS", "674")]),
        "services-stopped" => (
            0,
            &[
                ("SERVICES", "web/app"),
                ("FACTS", "674"),
                ("STOPPED", "web/app"),
            ],
        ),
        "services-crashed" => (0, &[("SERVICES", "api/jobs"), ("FACTS", "674")]),
        // The same cards over a project on another machine: the panel reads what the host's server reports.
        "remote-services-running" => (
            0,
            &[
                ("SERVICES", "api/server"),
                ("FACTS", "674"),
                ("REMOTE", "1"),
            ],
        ),
        "remote-services-stopped" => (
            0,
            &[
                ("SERVICES", "web/app"),
                ("FACTS", "674"),
                ("STOPPED", "web/app"),
                ("REMOTE", "1"),
            ],
        ),
        "remote-services-crashed" => (
            0,
            &[("SERVICES", "api/jobs"), ("FACTS", "674"), ("REMOTE", "1")],
        ),
        "agent-dock-tabs" => (0, &[("TABBAR", "agent")]),
        "components-tabs" => (0, &[("TABBAR", "sample")]),
        // The Git panel over scratch repos in the board's state; GITVIEW=tree|ungrouped, GITAMEND=1, GITSELECT=<rows down>.
        "git-changes" => (0, &[("GITFIXTURE", "unstaged"), ("GITSELECT", "2")]),
        // The same board read as a remote window does: every scan crosses the wire as JSON first.
        "git-changes-remote" => (
            0,
            &[
                ("GITFIXTURE", "unstaged"),
                ("GITSELECT", "2"),
                ("GITWIRE", "1"),
            ],
        ),
        "git-unserved" => (
            0,
            &[
                ("GITFIXTURE", "unstaged"),
                (
                    "GITUNSERVED",
                    "pom 0.9.6 on dev cannot show its repos here. Update pom there to use Git in this window.",
                ),
            ],
        ),
        "git-staged" => (0, &[("GITFIXTURE", "staged")]),
        "git-staged-all" => (0, &[("GITFIXTURE", "staged-all")]),
        "git-committed" => (0, &[("GITFIXTURE", "committed")]),
        "git-commit-amend" => (0, &[("GITFIXTURE", "staged"), ("GITAMEND", "1")]),
        "git-tree-view" => (0, &[("GITFIXTURE", "unstaged"), ("GITVIEW", "tree")]),
        "git-grouped-none" => (0, &[("GITFIXTURE", "staged"), ("GITVIEW", "ungrouped")]),
        // Remote and History over api, web and worker with origins: GITPRS draws the boards' pull requests,
        // GITREVIEWED marks branch files reviewed, GITOPEN=<repo>:<index> opens a branch commit.
        "git-remote" => (
            0,
            &[
                ("GITFIXTURE", "remote:differ"),
                ("TAB", "remote"),
                ("GITPRS", "1"),
                (
                    "GITREVIEWED",
                    "0:app/controllers/users_controller.rb,0:spec/requests/users_spec.rb",
                ),
            ],
        ),
        // The Remote tab at 300px over long repo, branch, file and pull request names.
        "git-remote-narrow" => (
            0,
            &[
                ("GITFIXTURE", "remote:narrow"),
                ("TAB", "remote"),
                ("GITPRS", "1"),
                ("GITVIEW", "tree"),
                ("WSBRANCH", git_fixture::NARROW_BRANCH),
            ],
        ),
        "git-changes-narrow" => (
            0,
            &[
                ("GITFIXTURE", "remote:narrow"),
                ("GITVIEW", "tree"),
                ("WSBRANCH", git_fixture::NARROW_BRANCH),
            ],
        ),
        "git-history-narrow" => (
            0,
            &[
                ("GITFIXTURE", "remote:narrow"),
                ("TAB", "history"),
                ("GITPRS", "1"),
                ("WSBRANCH", git_fixture::NARROW_BRANCH),
            ],
        ),
        "git-in-sync" => (
            0,
            &[
                ("GITFIXTURE", "remote:in-sync"),
                ("TAB", "remote"),
                ("GITPRS", "1"),
                (
                    "GITREVIEWED",
                    "0:app/controllers/users_controller.rb,0:spec/requests/users_spec.rb",
                ),
            ],
        ),
        "git-history" => (
            0,
            &[
                ("GITFIXTURE", "remote:differ"),
                ("TAB", "history"),
                ("GITPRS", "1"),
            ],
        ),
        "git-history-timeline" => (
            0,
            &[
                ("GITFIXTURE", "remote:differ"),
                ("TAB", "history"),
                ("GITTIMELINE", "1"),
                ("GITPRS", "1"),
            ],
        ),
        "git-commit-view" => (
            0,
            &[
                ("GITFIXTURE", "remote:differ"),
                ("GITOPEN", "0:0"),
                ("GITPRS", "1"),
            ],
        ),
        "git-drift" => (0, &[("GITFIXTURE", "drift")]),
        // The PR tab with the boards' pull request #482 on api.
        "git-pr-overview" => (0, &[("PRFIXTURE", "overview")]),
        "git-pr-checks" => (0, &[("PRFIXTURE", "checks")]),
        "git-pr-loading" => (0, &[("PRFIXTURE", "loading")]),
        "git-pr-none" => (0, &[("PRFIXTURE", "none")]),
        "git-pr-error" => (0, &[("PRFIXTURE", "error")]),
        "git-pr-empty" => (0, &[("PRFIXTURE", "empty")]),
        // A project on another machine from a stand-in source; REMOTEVIEW names the state. The window is laid
        // out at the boards' 1440x900 and cropped to the part a board is about.
        "remote-connected" => (0, &[("REMOTEVIEW", "connected")]),
        "remote-disconnected" => (
            0,
            &[("REMOTEVIEW", "disconnected"), ("CROP", REMOTE_CENTER)],
        ),
        "remote-version" => (0, &[("REMOTEVIEW", "version"), ("CROP", REMOTE_CENTER)]),
        "remote-version-commands" => (
            0,
            &[("REMOTEVIEW", "version-commands"), ("CROP", REMOTE_CENTER)],
        ),
        "remote-version-updating" => (
            0,
            &[("REMOTEVIEW", "version-updating"), ("CROP", REMOTE_CENTER)],
        ),
        "remote-reconnecting" => (
            0,
            &[("REMOTEVIEW", "reconnecting"), ("CROP", REMOTE_CENTER)],
        ),
        "remote-terminal-disconnected" => (
            0,
            &[
                ("REMOTEVIEW", "terminal-disconnected"),
                ("CROP", REMOTE_CENTER),
            ],
        ),
        "remote-terminal" => (
            0,
            &[("REMOTEVIEW", "terminal"), ("CROP", "256,512,1178,360")],
        ),
        "remote-agents" => (0, &[("REMOTEVIEW", "agents"), ("CROP", "1054,40,380,832")]),
        "remote-create" => (0, &[("REMOTEVIEW", "create"), ("CROP", "0,40,256,860")]),
        "remote-files-scanning" => (
            0,
            &[("REMOTEVIEW", "files-scanning"), ("CROP", REMOTE_FILES)],
        ),
        "remote-file-open" => (0, &[("REMOTEVIEW", "files-open"), ("CROP", REMOTE_FILES)]),
        "remote-file-saving" => (0, &[("REMOTEVIEW", "files-saving"), ("CROP", REMOTE_FILES)]),
        "remote-file-offline" => (
            0,
            &[("REMOTEVIEW", "files-offline"), ("CROP", REMOTE_FILES)],
        ),
        "remote-file-too-large" => (
            0,
            &[("REMOTEVIEW", "files-too-large"), ("CROP", REMOTE_FILES)],
        ),
        "remote-file-conflict" => (0, &[("PROMPT", "save-conflict")]),
        "remote-files-old-server" => (
            0,
            &[("REMOTEVIEW", "files-old-server"), ("CROP", REMOTE_CENTER)],
        ),
        "remote-restarted" => (
            0,
            &[("REMOTEVIEW", "restarted"), ("CROP", "496,814,449,52")],
        ),
        "remote-port-ready" => (0, &[("NOTICE", "port-ready")]),
        // Connect to Host alone at its width, in the step a board shows.
        "remote-connect" => (0, &[("MODAL", "connect-hosts")]),
        "remote-connect-empty" => (0, &[("MODAL", "connect-empty")]),
        "remote-connect-loading" => (0, &[("MODAL", "connect-loading")]),
        "remote-connect-error" => (0, &[("MODAL", "connect-error")]),
        "remote-connect-projects" => (0, &[("MODAL", "connect-projects")]),
        "remote-welcome" => (0, &[("WELCOME", "remote")]),
        // Every state of the title bar's host stacked, as one alone is too short to score.
        "remote-host-states" => (0, &[("HOSTSTATES", "1")]),
        _ => return None,
    })
}

/// A remote window right of its workspaces: the file tree, the editor and the status bar.
const REMOTE_FILES: &str = "256,40,1184,860";

/// The center pane of a remote window at the boards' size, with the terminal dock when one is open.
const REMOTE_CENTER: &str = "256,40,1178,832";

/// `--scene <id> <out.png> [--theme dark|light] [--size WxH]`: sets the switches the rest of `main` reads and
/// returns the output path and category.
fn scene_args(args: &[String]) -> anyhow::Result<Option<(String, usize)>> {
    if args.get(1).map(String::as_str) != Some("--scene") {
        return Ok(None);
    }
    let (Some(id), Some(out)) = (args.get(2), args.get(3)) else {
        anyhow::bail!("usage: --scene <id> <out.png> [--theme dark|light] [--size WxH]");
    };
    let Some((category, switches)) = scene(id) else {
        anyhow::bail!("unknown scene {id}");
    };
    let mut theme = "Pomelo Dark";
    let mut size = "1180x760".to_string();
    let mut rest = args[4..].iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next()) {
            // The canvas names themes by slug ("one-dark"); "dark" and "light" are Pomelo's own.
            ("--theme", Some(value)) => {
                theme = match value.as_str() {
                    "dark" => "Pomelo Dark",
                    "light" => "Pomelo Light",
                    slug => ui::builtin_theme_names()
                        .into_iter()
                        .map(|(name, _)| name)
                        .find(|name| name.to_lowercase().replace(' ', "-") == slug)
                        .ok_or_else(|| anyhow::anyhow!("unknown theme {slug}"))?,
                }
            }
            ("--size", Some(value)) => size = value.clone(),
            _ => anyhow::bail!("unexpected argument {flag}"),
        }
    }
    std::env::set_var("THEME", theme);
    std::env::set_var("THEMESELECTION", "dynamic");
    std::env::set_var("SCENESIZE", size);
    if let Some((_, board)) = switches.iter().find(|(key, _)| *key == "EDITORBOARD") {
        editor_board_fixture(board)?;
    } else if switches.iter().any(|(key, _)| *key == "MAINVIEW") {
        window_fixture()?;
    }
    std::env::set_var("EXPANDED", category.to_string());
    for (key, value) in switches {
        std::env::set_var(key, value);
    }
    if let Some((_, state)) = switches.iter().find(|(key, _)| *key == "GITFIXTURE") {
        let roots = if let Some(remote) = state.strip_prefix("remote:") {
            let Some(remote) = git_fixture::RemoteState::parse(remote) else {
                anyhow::bail!("unknown git fixture state {state}");
            };
            git_fixture::create_remote(remote)?
        } else if *state == "drift" {
            git_fixture::create_drift()?
        } else {
            let Some(state) = git_fixture::State::parse(state) else {
                anyhow::bail!("unknown git fixture state {state}");
            };
            git_fixture::create(state)?
        };
        let roots: Vec<String> = roots
            .iter()
            .map(|root| root.display().to_string())
            .collect();
        std::env::set_var("GITPANEL", roots.join(","));
    }
    Ok(Some((out.clone(), category)))
}

/// The pull request the PR boards show, in the state `view` names; the tab never reaches GitHub.
fn pr_fixture(view: &str) -> pull_request_ui::PrItem {
    // No token in this process, so the lookup the tab starts can only fail; the loading scene renders before it does.
    std::env::remove_var("GH_TOKEN");
    std::env::remove_var("GITHUB_TOKEN");
    let dir = std::env::temp_dir().join(format!("pom-snapshot-pr-{}", std::process::id()));
    let target = pom_forge::PrTarget {
        repo: "api".into(),
        owner: "acme".into(),
        name: "api".into(),
        head: "feat-login".into(),
    };
    let check = |name: &str, workflow: &str, result: &str, took: (&str, &str)| pom_forge::Check {
        name: name.into(),
        workflow_name: workflow.into(),
        details_url: "https://example.com".into(),
        result: result.into(),
        started_at: took.0.into(),
        completed_at: took.1.into(),
        ..pom_forge::Check::default()
    };
    let empty = view == "empty";
    let mut pr = pom_forge::PullRequest {
        id: "PR_482".into(),
        head_ref_oid: "4f2a9c1".into(),
        number: 482,
        title: "Paginate the users index".into(),
        state: "OPEN".into(),
        is_draft: empty,
        mergeable: "MERGEABLE".into(),
        merge_state_status: if empty { "BEHIND" } else { "CLEAN" }.into(),
        head_ref_name: "feat-login".into(),
        base_ref_name: "main".into(),
        author: Some(pom_forge::Actor {
            login: "alex".into(),
            avatar_url: String::new(),
        }),
        additions: 51,
        deletions: 4,
        ..pom_forge::PullRequest::default()
    };
    if !empty {
        pr.body = "Adds page and per params to the users index, with an X-Total-Count header for the client. Closes PROJ-101.".into();
        pr.labels = vec![
            pom_forge::Label {
                name: "backend".into(),
                color: "86b6ec".into(),
            },
            pom_forge::Label {
                name: "api".into(),
                color: "8fc27a".into(),
            },
        ];
        pr.status_check_rollup = vec![
            check(
                "rspec",
                "CI / test",
                "fail",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:02:41Z"),
            ),
            check(
                "rubocop",
                "CI / lint",
                "pass",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:00:38Z"),
            ),
            check(
                "brakeman",
                "CI / security",
                "pass",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:00:51Z"),
            ),
            check(
                "deploy-preview",
                "Preview",
                "pending",
                ("2026-10-20T09:00:00Z", ""),
            ),
        ];
        pr.checks = "fail".into();
        pr.review = "changes".into();
        pr.reviewers = vec![
            pom_forge::Reviewer {
                name: "bea".into(),
                state: "changes".into(),
            },
            pom_forge::Reviewer {
                name: "sam".into(),
                state: "pending".into(),
            },
        ];
        pr.timeline = vec![
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Review,
                author: "bea".into(),
                body: "Clamp per so a client cannot ask for every row at once.".into(),
                at: "2026-10-20T09:12:00Z".into(),
                state: "CHANGES_REQUESTED".into(),
                threads: vec![pom_forge::ReviewThread {
                    id: "PRRT_1".into(),
                    path: "app/controllers/users_controller.rb".into(),
                    line: Some(7),
                    resolved: true,
                    comments: Vec::new(),
                }],
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Inline,
                author: "alex".into(),
                body: "Done, capped at 100 and covered in the request spec.".into(),
                at: "2026-10-20T10:03:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Comment,
                author: "sam".into(),
                body: "Can the client read the total from a header instead?".into(),
                at: "2026-10-20T10:40:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
        ];
    }
    // The merge confirmation needs a pull request with nothing blocking it.
    if view == "merge-confirm" {
        pr.checks = "pass".into();
        pr.review = "approved".into();
        pr.review_decision = "APPROVED".into();
    }
    let mut item = pull_request_ui::PrItem::new(
        pom_paths::StateDir::new(dir.join("state")),
        "myproject".into(),
        std::sync::Arc::new(|| {}),
        target,
        None,
    );
    match view {
        "loading" => {}
        "none" => item.show(None),
        "error" => item.show_error("No GitHub token: set GH_TOKEN or the session's github secret"),
        _ => item.show(Some(pr)),
    }
    if view == "checks" {
        item.show_checks();
    }
    item
}

pub fn fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/pomelo-qa-fixture/{}", std::process::id()))
}

struct FixtureCleanup;

impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(fixture_dir());
    }
}

/// The files the window boards show, written to a scratch folder the window scene opens.
fn fixture_root() -> std::path::PathBuf {
    fixture_dir().join("myproject")
}

fn editor_board_fixture(board: &str) -> anyhow::Result<()> {
    let root = fixture_root();
    editor_fixture::create(&root)?;
    let open = |paths: &[&str]| {
        paths
            .iter()
            .map(|path| root.join(path).display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let controller = "api/app/controllers/users_controller.rb";
    let (files, select) = match board {
        "language" => (open(&["web/src/api/users"]), "6,0,0"),
        "markdown" => (open(&["api/README.md"]), "6,8,0"),
        "image" => (open(&["web/public/logo.png"]), "0,0,0"),
        "readonly" => (open(&[controller]), "5,4,0"),
        "tabs" => (
            open(&[
                controller,
                "api/config/routes.rb",
                "api/README.md",
                "web/src/api/users.ts",
                "pom.yml",
            ]),
            "5,4,0",
        ),
        _ => (
            open(&[
                controller,
                "api/config/routes.rb",
                "api/README.md",
                controller,
            ]),
            "5,4,0",
        ),
    };
    std::env::set_var("FILESROOT", &root);
    std::env::set_var("OPENFILES", files);
    std::env::set_var("SELECT", select);
    let shell = match board_terminals(board) {
        Some(terms) => terminal_fixture::create(&fixture_dir(), &terms)?,
        None if board == "term-spawn-failed" => "/usr/local/bin/fish".into(),
        None => return Ok(()),
    };
    std::env::set_var("TERMSHELL", shell);
    Ok(())
}

fn board_terminals(board: &str) -> Option<Vec<terminal_fixture::Term>> {
    use terminal_fixture::*;
    let term = |dir, program, argument, lines, exit| Term {
        dir,
        program,
        argument,
        lines,
        exit,
    };
    let server = || term("api", "rails", Some("server"), server_log(), None);
    let shell_tab = || term(".", "zsh", None, shell(), None);
    Some(match board {
        "term-dock" => vec![
            server(),
            term("web", "pnpm", Some("dev"), dev_server(), None),
            shell_tab(),
        ],
        "term-split" => vec![
            server(),
            shell_tab(),
            term("web", "pnpm", Some("test"), dev_server(), None),
        ],
        "term-zoomed" => {
            let mut lines = shell();
            lines.truncate(8);
            lines.extend(rspec());
            vec![
                term("api", "bundle", Some("exec"), lines, None),
                shell_tab(),
            ]
        }
        "term-search" => vec![
            server(),
            term("web", "pnpm", Some("dev"), dev_server(), None),
        ],
        "term-exited" => vec![
            server(),
            term("api", "rails", Some("db:migrate"), migrate(), Some(1)),
        ],
        _ => return None,
    })
}

fn move_caret(view: &mut workspace::WorkspaceView, line: usize, column: usize) {
    view.editor_key(workspace::EditKey::DocumentStart, false);
    for _ in 0..line {
        view.editor_key(workspace::EditKey::Down, false);
    }
    for _ in 0..column {
        view.editor_key(workspace::EditKey::Right, false);
    }
}

type BoardStep = Box<dyn Fn(&mut workspace::WorkspaceView)>;

fn editor_board_steps(board: &str) -> Vec<BoardStep> {
    use workspace::pane::PaneCommand;
    let root = fixture_root();
    let open = |path: &'static str| -> BoardStep {
        let path = root.join(path);
        Box::new(move |view| view.open_file(&path))
    };
    match board {
        "split" => {
            let (spec, other) = (
                root.join("api/spec/requests/users_spec.rb"),
                root.join("api/spec/models/user_spec.rb"),
            );
            vec![Box::new(move |view| {
                view.pane_command(PaneCommand::Split(
                    workspace::pane_group::SplitDirection::Right,
                ));
                view.open_file(&spec);
                view.open_file(&other);
                view.pane_command(PaneCommand::ActivateItem(0));
                view.pane_command(PaneCommand::CloseActiveItem);
                view.pane_command(PaneCommand::ActivateItem(0));
                move_caret(view, 8, 16);
            })]
        }
        "tabs" => vec![
            Box::new(|view| view.deploy_project_diagnostics(false)),
            open("pom.yml"),
            open("web/src/api/users.ts"),
            open("api/README.md"),
            open("api/config/routes.rb"),
            open("api/app/controllers/users_controller.rb"),
            Box::new(|view| {
                view.run_action(workspace::keymap::Action::ToggleTabSwitcher);
            }),
        ],
        "find" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::DeploySearch, false);
            }),
            Box::new(|view| {
                view.editor_text("user");
                view.editor_key(workspace::EditKey::ToggleSearchWholeWord, false);
                view.editor_key(workspace::EditKey::ToggleSearchReplace, false);
            }),
            Box::new(|view| {
                view.editor_key(workspace::EditKey::Tab, false);
                view.editor_text("account");
                view.editor_key(workspace::EditKey::Tab, false);
            }),
        ],
        "language" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::ToggleLanguageSelector, false);
            }),
            Box::new(|view| {
                view.editor_text("t");
            }),
        ],
        "goto" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::ToggleGoToLine, false);
            }),
            Box::new(|view| {
                view.editor_text("26:5");
            }),
        ],
        "image" => {
            let (logo, broken) = (
                root.join("web/public/logo.png"),
                root.join("web/public/broken.png"),
            );
            vec![
                Box::new(move |view| {
                    view.pane_command(PaneCommand::Split(
                        workspace::pane_group::SplitDirection::Right,
                    ));
                    view.open_file(&broken);
                    view.pane_command(PaneCommand::ActivateItem(0));
                    view.pane_command(PaneCommand::CloseActiveItem);
                }),
                Box::new(move |view| {
                    view.pane_command(PaneCommand::ActivatePane(
                        workspace::pane_group::SplitDirection::Left,
                    ));
                    let _ = &logo;
                }),
            ]
        }
        "markdown" => vec![Box::new(|view| {
            view.editor_key(workspace::EditKey::OpenMarkdownPreviewToTheSide, false);
        })],
        "vim-normal" => vec![Box::new(|view| move_caret(view, 5, 13))],
        "vim-visual" => vec![Box::new(|view| {
            move_caret(view, 5, 2);
            view.editor_text("Vjj");
        })],
        board if board.starts_with("term-") => {
            let mut steps: Vec<BoardStep> = board_terminals(board)
                .unwrap_or_else(|| {
                    vec![terminal_fixture::Term {
                        dir: ".",
                        program: "",
                        argument: None,
                        lines: Vec::new(),
                        exit: None,
                    }]
                })
                .iter()
                .map(|term| -> BoardStep {
                    let dir = root.join(term.dir);
                    Box::new(move |view| view.open_terminal_in(dir.clone()))
                })
                .collect();
            let first = |view: &mut workspace::WorkspaceView| {
                view.pane_command(PaneCommand::ActivateItem(0));
            };
            match board {
                "term-dock" | "term-search" => steps.push(Box::new(first)),
                "term-split" => {
                    steps[1] = Box::new(|view| {
                        view.pane_command(PaneCommand::Split(
                            workspace::pane_group::SplitDirection::Right,
                        ));
                    });
                    steps.push(Box::new(first));
                }
                "term-zoomed" => steps.push(Box::new(|view| {
                    view.pane_command(PaneCommand::ActivateItem(0));
                    view.pane_command(PaneCommand::ToggleZoom);
                })),
                _ => {}
            }
            if board == "term-search" {
                steps.push(Box::new(|view| {
                    view.editor_key(workspace::EditKey::DeploySearch, false);
                }));
                steps.push(Box::new(|view| {
                    view.editor_text("Completed");
                    view.editor_key(workspace::EditKey::ToggleSearchCaseSensitive, false);
                }));
            }
            steps
        }
        "tree-menu" => vec![Box::new(|view| {
            view.right_click(200.0, 205.0);
        })],
        _ => Vec::new(),
    }
}

fn window_fixture() -> anyhow::Result<()> {
    let root = fixture_root();
    let root = root.as_path();
    let _ = std::fs::remove_dir_all(root);
    let files: [(&str, &str); 7] = [
        (
            "api/app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  before_action :authenticate_user!\n\n  def index\n    @users = User.where(active: true).order(:name)\n    render json: @users\n  end\n\n  def show\n    @user = User.find(params[:id])\n  end\nend",
        ),
        (
            "api/app/controllers/sessions_controller.rb",
            "class SessionsController < ApplicationController\nend\n",
        ),
        ("api/app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("api/config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("api/spec/users_spec.rb", "\n"),
        ("api/Gemfile", "source \"https://rubygems.org\"\n"),
        ("web/README.md", "# web\n"),
    ];
    for (path, text) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    let open = |path: &str| root.join(path).display().to_string();
    std::env::set_var("FILESROOT", root);
    std::env::set_var(
        "OPENFILES",
        [
            open("api/app/controllers/users_controller.rb"),
            open("api/config/routes.rb"),
            open("web/README.md"),
            open("api/app/controllers/users_controller.rb"),
        ]
        .join(","),
    );
    std::env::set_var("SELECT", "7,0,0");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let _cleanup = FixtureCleanup;
    let args: Vec<String> = std::env::args().collect();
    let (out, category) = match scene_args(&args)? {
        Some(scene) => scene,
        None => (
            args.get(1)
                .cloned()
                .unwrap_or_else(|| "/tmp/pomelo-ui.png".into()),
            args.get(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or(settings_ui::APPEARANCE),
        ),
    };
    // BUFFERFEATURES='{"calt": false}': OpenType features for the code font.
    if let Ok(features) = std::env::var("BUFFERFEATURES") {
        let value: serde_json::Value = serde_json::from_str(&features)?;
        ui::set_font_settings(
            ui::TextFont::Buffer,
            ui::FontSettings {
                features: ui::parse_font_features(&value).0,
                ..Default::default()
            },
        );
    }

    if std::env::var("MENUKEYS").is_ok() {
        let item = |id: u64, text: &'static str, sep: bool, disabled: bool| workspace::MenuItem {
            id,
            label: text.into(),
            checked: false,
            sep,
            disabled,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        let mut items = vec![
            item(workspace::MENU_EDIT_CUT, "Cut", false, false),
            item(workspace::MENU_EDIT_COPY, "Copy", false, false),
            item(workspace::MENU_EDIT_PASTE, "Paste", false, false),
            item(
                workspace::MENU_COPY_REL_PATH,
                "Copy Relative Path",
                true,
                false,
            ),
            item(workspace::MENU_REVEAL, "Reveal in Finder", false, false),
            item(workspace::MENU_TREE_RENAME, "Rename", true, false),
            item(workspace::MENU_TREE_DELETE, "Delete", false, false),
            item(
                workspace::MENU_EDIT_GO_TO_DECLARATION,
                "Go to Declaration",
                true,
                false,
            ),
        ];
        items[0].hint = None;
        let painted = workspace::context_menu(40.0, 40.0, 40.0, 400.0, 400.0, &items, None, None);
        let (width, height) = (400.0_f32, 400.0_f32);
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(repos) = std::env::var("E2E_REPOS") {
        return e2e::run(out.trim_end_matches(".png"), &repos);
    }

    let usage_page = std::env::var("USAGEPAGE").is_ok();
    let store_page = std::env::var("MODULESTORE").is_ok();
    if usage_page || store_page || std::env::var("DEVREQUESTS").is_ok() {
        use workspace::Item;
        let mut page: Box<dyn Item> = if usage_page {
            Box::new(agent_usage_ui::preview_page())
        } else if store_page {
            Box::new({
                let mut page = module_store_ui::preview_page();
                if std::env::var("OPTIMIZE").is_ok() {
                    page.click(5);
                }
                page
            })
        } else {
            Box::new(dev_services_ui::preview_page())
        };
        let (width, height) = (1000.0_f32, 1500.0_f32);
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = page.paint_body(body, true).unwrap_or_default();
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("ONBOARD") {
        use workspace::Item;
        let mut page = onboarding_ui::preview_page(&which);
        let (width, height) = (1000.0_f32, 900.0_f32);
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = page.paint_body(body, true).unwrap_or_default();
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("AGENTPOP") {
        let node = if which == "history" {
            workspace::agent_popover::history(&[], None)
        } else {
            workspace::agent_popover::new_agent(
                "feat-login",
                workspace::SideAgentRole::Ask,
                workspace::SideAgentStart::Auto,
                [Some(9_300), Some(62_000), Some(9_300), Some(5_200)],
                Some(62_000),
                Some("codex"),
                None,
            )
        };
        let (width, height) = (420.0_f32, 760.0_f32);
        let painted = ui::render(
            &ui::div().p(20.0).child(node).into(),
            ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
        );
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var("SERVICES").is_ok() {
        use workspace::SidePanelView;
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let stopped = std::env::var("STOPPED").ok();
        let mut scratch = None;
        // REMOTE=1: the same panel over a project on another machine, fed the facts its server would send.
        let mut panel = if std::env::var("REMOTE").is_ok() {
            remote_fixture::services_panel(SERVICES_YML, stopped.as_deref())?
        } else {
            let dir = std::env::temp_dir().join(format!("pom-snapshot-svc-{}", std::process::id()));
            std::fs::create_dir_all(dir.join("workspace--feat-login"))?;
            scratch = Some(dir.clone());
            std::fs::write(dir.join("pom.yml"), SERVICES_YML)?;
            let config = pom_config::Config::load(&dir.join("pom.yml"))?;
            let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
                pom_services::RunnerOptions {
                    project_root: dir.clone(),
                    session: "myproject".into(),
                    state: pom_paths::StateDir::new(dir.join("state")),
                    holders: pom_ptyhost::SocketDir::new(dir.join("s")),
                    binary: "/nonexistent".into(),
                    docker: "/nonexistent".into(),
                },
            ));
            let mut panel = services_ui::ServicesPanel::new(
                services_ui::ServicesContext {
                    runner: runner.clone(),
                    config: std::sync::Arc::new(std::sync::RwLock::new(Some(std::sync::Arc::new(
                        config,
                    )))),
                    branch: "feat-login".into(),
                    ticket: "PROJ-101".into(),
                    is_main: false,
                    waker: std::sync::Arc::new(|| {}),
                },
                dir.join("workspace--feat-login"),
            );
            // The facts the boards show: leased ports behind the proxy, and api/server's holder up for 12 minutes.
            for (repo, service, port) in [("api", "server", 3000), ("web", "app", 5173)] {
                let key = pom_ports::service_key(
                    &pom_env::port_ws_key("feat-login"),
                    &format!("{repo}~{service}"),
                );
                runner.ports().acquire_preferred(&key, port, 1);
            }
            let server = runner.holder_name(&pom_services::ServiceTarget {
                branch: "feat-login".into(),
                is_main: false,
                repo: "api".into(),
                service: "server".into(),
            });
            let pidfile = runner.holders().pidfile(&server);
            if let Some(parent) = pidfile.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&pidfile, "48213\n")?;
            std::fs::File::options()
                .write(true)
                .open(&pidfile)?
                .set_modified(
                    std::time::SystemTime::now() - std::time::Duration::from_secs(12 * 60 + 5),
                )?;
            use services_ui::Status;
            panel.show_status("_ws", "gateway", Status::Running, None, None);
            panel.show_status("api", "server", Status::Running, None, None);
            panel.show_status(
                "api",
                "jobs",
                Status::Crashed,
                None,
                Some(services_ui::Crash {
                    line: Some("KeyError: key not found: \"REDIS_URL\"".into()),
                    exit: "exit 1".into(),
                    at: Some(std::time::SystemTime::now() - std::time::Duration::from_secs(130)),
                }),
            );
            panel.show_status("web", "app", Status::Running, None, None);
            panel.show_status(
                "web",
                "storybook",
                Status::Stopped,
                Some("port 6006 is already in use by node (pid 48213)".into()),
                None,
            );
            for name in ["postgres", "redis", "files"] {
                panel.show_shared_running(name);
            }
            if let Some((repo, service)) = std::env::var("STOPPED")
                .ok()
                .as_deref()
                .and_then(|which| which.split_once('/'))
            {
                panel.show_status(repo, service, Status::Stopped, None, None);
            }
            panel
        };
        let tab = std::env::var("SERVICES").ok().filter(|which| which != "1");
        let (width, height) = if tab.is_some() {
            (1000.0_f32, 520.0_f32)
        } else {
            (320.0_f32, 900.0_f32)
        };
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let node = match tab.as_deref().and_then(|which| which.split_once('/')) {
            Some(("shared", name)) => panel.shared_tab_preview(
                name,
                &[
                    "LOG:  database system is ready to accept connections",
                    "LOG:  checkpoint starting: time",
                    "ERROR:  relation \"accounts\" does not exist at character 15",
                    "LOG:  checkpoint complete: wrote 3 buffers",
                ],
                width,
                height,
            ),
            Some((repo, service)) => panel.tab_preview(
                repo,
                service,
                &[
                    "[vite] hmr update /src/pages/Login.tsx",
                    "[vite] page reload src/main.tsx",
                    "Warning: something is deprecated",
                    "[vite] page reload src/main.tsx",
                    "Error: could not resolve 'vite-plugin-svgr'",
                    "[vite] hmr update /src/components/Button.tsx",
                ],
                &std::env::var("FILTER").unwrap_or_default(),
                width,
                height,
            ),
            None => panel.render(width, height),
        };
        // FACTS=<w>: only the facts card at that width, moved to the origin, as tall as it lays out.
        let facts = std::env::var("FACTS")
            .ok()
            .and_then(|w| w.parse::<f32>().ok());
        let (painted, width, height, scale) = match facts {
            Some(card_w) => {
                let wide = card_w + 32.0;
                let node = match tab.as_deref().and_then(|which| which.split_once('/')) {
                    Some((repo, service)) => {
                        panel.tab_preview(repo, service, &[], "", wide, height)
                    }
                    None => node,
                };
                let probe = ui::render(
                    &node,
                    ui::Rect::new(0.0, 0.0, wide, height, ui::Rgba::TRANSPARENT),
                );
                let card = probe
                    .debug_bounds
                    .iter()
                    .find(|(name, _)| *name == "fact-card")
                    .map(|(_, rect)| *rect)
                    .ok_or_else(|| anyhow::anyhow!("no facts card"))?;
                let painted = ui::render(
                    &node,
                    ui::Rect::new(-card.x, -card.y, wide, height, ui::Rgba::TRANSPARENT),
                );
                (painted, card_w, card.h.round(), 1.0)
            }
            None => (
                ui::render(
                    &ui::div()
                        .bg(ui::theme().panel_background)
                        .child(node)
                        .into(),
                    body,
                ),
                width,
                height,
                2.0,
            ),
        };
        let mut r =
            ui::UiRenderer::new_headless((width * scale) as u32, (height * scale) as u32, scale)?;
        r.render_frame(
            if facts.is_some() {
                ui::theme().editor_background
            } else {
                ui::theme().panel_background
            },
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if let Some(dir) = scratch {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!("remove {}: {error}", dir.display());
            }
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("DATABASE") {
        use workspace::{Item, SidePanelView};
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let dir = std::env::temp_dir().join(format!("pom-snapshot-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let yaml = if which.starts_with("board-") {
            "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  storage:\n    image: minio/minio\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n    env:\n      REDIS_URL: \"{{shared.redis.url}}\"\n  web: {}\n  worker:\n    env:\n      REDIS_URL: \"{{shared.redis.url}}\"\n      S3_ENDPOINT: \"http://{{shared.storage.host}}\"\n"
        } else {
            "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\n  queue:\n    image: rabbitmq:3\nrepos:\n  api:\n    databases:\n      dev: \"{{branch.safe}}\"\n      dev_tx: \"transaction_{{branch.safe}}\"\n      test: \"{{branch.safe}}_test\"\n    env:\n      REDIS_URL: \"{{shared.redis.url}}\"\n      S3_ENDPOINT: \"http://{{shared.files.host}}\"\n  web:\n    shared_services:\n      - redis\n    databases:\n      main: \"web_{{branch.safe}}\"\n"
        };
        std::fs::write(dir.join("pom.yml"), yaml)?;
        let config = std::sync::Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
        let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
            pom_services::RunnerOptions {
                project_root: dir.clone(),
                session: "myproject".into(),
                state: pom_paths::StateDir::new(dir.join("state")),
                holders: pom_ptyhost::SocketDir::new(dir.join("s")),
                binary: "/nonexistent".into(),
                docker: "/nonexistent".into(),
            },
        ));
        let context = database_ui::DatabaseContext {
            runner,
            state: pom_paths::StateDir::new(dir.join("state")),
            config: std::sync::Arc::new(move || Some(config.clone())),
            branch: "feat-login".into(),
            workspace_root: dir.join("workspace--feat-login"),
            config_path: dir.join("pom.yml"),
            waker: std::sync::Arc::new(|| {}),
            objects: std::sync::Arc::new(SnapshotStorage),
            choose_files: std::sync::Arc::new(Vec::new),
        };
        let table = |schema: &str, name: &str, kind: pom_db::TableKind, count: Option<usize>| {
            pom_db::Table {
                schema: schema.into(),
                name: name.into(),
                kind,
                count,
            }
        };
        let scene_size = std::env::var("SCENESIZE").ok().and_then(|size| {
            let (width, height) = size.split_once('x')?;
            Some((width.parse::<f32>().ok()?, height.parse::<f32>().ok()?))
        });
        let (width, height) = if let (true, Some(size)) = (which.starts_with("board-"), scene_size)
        {
            size
        } else if which == "failures" {
            (320.0_f32, 620.0_f32)
        } else if which == "panel" || which == "menu" {
            (300.0_f32, 1180.0_f32)
        } else if which == "object" {
            (760.0_f32, 460.0_f32)
        } else if which == "bucket" {
            (1100.0_f32, 480.0_f32)
        } else if which == "console" {
            (900.0_f32, 280.0_f32)
        } else if which == "table" {
            (
                std::env::var("SNAPW")
                    .ok()
                    .and_then(|width| width.parse().ok())
                    .unwrap_or(1240.0_f32),
                620.0_f32,
            )
        } else {
            (900.0_f32, 460.0_f32)
        };
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let databases = pom_db::list_databases(
            &pom_config::Config::load(&dir.join("pom.yml"))?,
            "feat-login",
        );
        let first = databases
            .first()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
        let mut consoles = vec![database_ui::new_console(&[], first)];
        consoles.push(database_ui::new_console(&consoles, first));
        consoles[1].id.push('b');
        context.save_consoles(&consoles);
        let mut overlay: Option<ui::Painted> = None;
        let painted = if which == "console" {
            use workspace::ItemFooter;
            let mut footer = database_ui::ConsoleFooter::new(context, consoles[0].clone());
            footer.show_result(Ok(pom_db::QueryResult {
                columns: ["id", "email", "role"].map(str::to_string).to_vec(),
                rows: (1..=12)
                    .map(|row| {
                        vec![
                            Some(row.to_string()),
                            Some(format!("user{row}@example.com")),
                            Some(if row % 3 == 0 { "admin" } else { "member" }.to_string()),
                        ]
                    })
                    .collect(),
                ..pom_db::QueryResult::default()
            }));
            footer.height(height);
            footer
                .paint(body)
                .ok_or_else(|| anyhow::anyhow!("no footer"))?
        } else if which == "failures" {
            let mut panel = database_ui::DatabasePanel::new(context);
            let names: Vec<String> = databases.iter().map(|db| db.name.clone()).collect();
            let missing = format!("{}-with-escalated-inbox-and-a-long-name", names[0]);
            panel.show_failure(
                &names[0],
                pom_db::ConnectError::new(
                    pom_db::Engine::Postgres,
                    &missing,
                    "localhost",
                    5434,
                    "postgres",
                    format!("db error: FATAL: database \"{missing}\" does not exist"),
                ),
                Some("myproject_main".into()),
            );
            panel.toggle_full_error(&names[0]);
            panel.show_failure(
                &names[3],
                pom_db::ConnectError::new(
                    pom_db::Engine::Postgres,
                    &names[3],
                    "localhost",
                    5434,
                    "postgres",
                    "error connecting to server: Connection refused (os error 61)".into(),
                ),
                None,
            );
            panel.fold(&names[3]);
            let node = panel.render(width, height);
            ui::render(
                &ui::div()
                    .bg(ui::theme().panel_background)
                    .child(node)
                    .into(),
                body,
            )
        } else if which == "panel" || which == "menu" {
            let mut panel = database_ui::DatabasePanel::new(context);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let names: Vec<String> = databases.iter().map(|db| db.name.clone()).collect();
            let column = |table: &str,
                          name: &str,
                          data_type: &str,
                          primary_key: bool,
                          references: Option<&str>| {
                pom_db::Column {
                    schema: "public".into(),
                    table: table.into(),
                    name: name.into(),
                    data_type: data_type.into(),
                    primary_key,
                    references: references.map(str::to_string),
                }
            };
            let users = table("public", "users", pom_db::TableKind::Table, Some(1284));
            panel.show_schema(
                &names[0],
                pom_db::Schema {
                    tables: vec![
                        users.clone(),
                        table("public", "login_tokens", pom_db::TableKind::Table, Some(37)),
                        table("public", "orders", pom_db::TableKind::Table, Some(5120)),
                        table(
                            "public",
                            "schema_migrations",
                            pom_db::TableKind::Table,
                            Some(42),
                        ),
                        table("public", "active_users", pom_db::TableKind::View, None),
                    ],
                    columns: vec![
                        column("users", "id", "bigint", true, None),
                        column("users", "email", "varchar(255)", false, None),
                        column("users", "name", "varchar(120)", false, None),
                        column("users", "created_at", "timestamptz", false, None),
                        column("login_tokens", "user_id", "bigint", false, Some("users")),
                    ],
                },
            );
            panel.expand_table(&names[0], &users);
            panel.show_failure(
                &names[2],
                pom_db::ConnectError::new(
                    pom_db::Engine::Postgres,
                    &names[2],
                    "localhost",
                    5434,
                    "postgres",
                    format!("db error: FATAL: database \"{}\" does not exist", names[2]),
                ),
                None,
            );
            panel.show_tables(
                "redis",
                vec![
                    table("", "session", pom_db::TableKind::Keyspace, Some(214)),
                    table("", "cache", pom_db::TableKind::Keyspace, Some(1873)),
                ],
            );
            panel.fold("redis");
            panel.show_buckets("files", vec!["uploads".into()]);
            let object = |key: &str, size: u64| pom_db::object_storage::ObjectEntry {
                key: key.into(),
                size,
                ..pom_db::object_storage::ObjectEntry::default()
            };
            panel.show_folder(
                "files",
                "uploads",
                "",
                pom_db::object_storage::Listing {
                    prefixes: vec!["avatars/".into(), "exports/".into()],
                    objects: vec![
                        object("invoice-4412.pdf", 98_304),
                        object("import-2026-09.csv", 3_565_158),
                    ],
                    next: Some("more".into()),
                },
            );
            panel.show_prefix_stats(
                "files",
                "uploads",
                "avatars/",
                pom_db::object_storage::PrefixStats {
                    objects: 2940,
                    bytes: 1_288_490_188,
                    capped: false,
                },
            );
            let node = panel.render(width, height);
            if which == "menu" {
                if let Some(id) = panel.row_named("users") {
                    panel.open_menu(id);
                    let items = panel.menu_items();
                    overlay = Some(workspace::context_menu(
                        120.0, 250.0, 250.0, width, height, &items, None, None,
                    ));
                }
            }
            ui::render(
                &ui::div()
                    .bg(ui::theme().panel_background)
                    .child(node)
                    .into(),
                body,
            )
        } else if which == "object" {
            let files = databases
                .iter()
                .find(|db| db.name == "files")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no object storage"))?;
            let mut item = database_ui::ObjectItem::new(
                context,
                files,
                "uploads".into(),
                pom_db::object_storage::ObjectEntry {
                    key: "exports/orders-2026-09-27.json".into(),
                    size: 5_347_737,
                    modified: "2026-09-27T18:42:00.000Z".into(),
                    etag: "9b2cf5e1a4d0".into(),
                },
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while item.is_busy() && std::time::Instant::now() < deadline {
                item.tick(&|| None);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            item.tick(&|| None);
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        } else if which == "bucket" {
            let files = databases
                .iter()
                .find(|db| db.name == "files")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no object storage"))?;
            let object =
                |key: &str, size: u64, modified: &str| pom_db::object_storage::ObjectEntry {
                    key: key.into(),
                    size,
                    modified: modified.into(),
                    etag: "9b2cf5e1a4d0".into(),
                };
            let mut item = database_ui::BucketItem::new(
                context,
                files,
                "uploads".into(),
                "exports/",
                Some("exports/orders-2026-09-27.json".into()),
            );
            let wait = |item: &mut database_ui::BucketItem| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while item.is_busy() && std::time::Instant::now() < deadline {
                    item.tick(&|| None);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                item.tick(&|| None);
            };
            wait(&mut item);
            item.show(
                pom_db::object_storage::Listing {
                    prefixes: vec!["exports/2025/".into(), "exports/archive/".into()],
                    objects: vec![
                        object(
                            "exports/customers.csv",
                            3_565_158,
                            "2026-09-28T09:15:00.000Z",
                        ),
                        object(
                            "exports/invoice-4412.pdf",
                            98_304,
                            "2026-09-29T14:02:00.000Z",
                        ),
                        object(
                            "exports/orders-2026-09-27.json",
                            5_347_737,
                            "2026-09-27T18:42:00.000Z",
                        ),
                        object("exports/report.xlsx", 61_440, "2026-09-26T08:00:00.000Z"),
                    ],
                    next: None,
                },
                vec![
                    (
                        "exports/2025/".into(),
                        pom_db::object_storage::PrefixStats {
                            objects: 214,
                            bytes: 63_963_136,
                            capped: false,
                        },
                    ),
                    (
                        "exports/archive/".into(),
                        pom_db::object_storage::PrefixStats {
                            objects: 14,
                            bytes: 1_288_490,
                            capped: false,
                        },
                    ),
                ],
            );
            wait(&mut item);
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        } else if which == "redis" {
            let database = pom_db::list_databases(
                &pom_config::Config::load(&dir.join("pom.yml"))?,
                "feat-login",
            )
            .into_iter()
            .find(|database| database.engine == pom_db::Engine::Redis)
            .ok_or_else(|| anyhow::anyhow!("no redis"))?;
            let mut item = database_ui::KeyspaceItem::new(
                context,
                database,
                table("", "bull", pom_db::TableKind::Keyspace, None),
            );
            let key = |key: &str, kind: &str, ttl: Option<i64>| pom_db::RedisKey {
                key: key.into(),
                kind: kind.into(),
                ttl,
            };
            let mut keys = vec![
                key("bull:mail:id", "string", None),
                key("bull:mail:meta", "hash", None),
                key("bull:mail:wait", "list", None),
                key("bull:mail:delayed", "zset", None),
            ];
            keys.extend(
                (1..6).map(|job| key(&format!("bull:mail:{job}"), "hash", Some(3600 * job))),
            );
            keys.push(key("bull:sms:wait", "list", None));
            keys.push(key("bull:sms:workers", "set", Some(95)));
            let (selected, value) = match std::env::var("KEY").as_deref() {
                Ok("hash") => (
                    1,
                    pom_db::RedisValue::Hash(vec![
                        ("opts".into(), r#"{"maxLenEvents":10000}"#.into()),
                        ("version".into(), "bullmq:5.12.0".into()),
                        ("concurrency".into(), "4".into()),
                    ]),
                ),
                Ok("list") => (
                    2,
                    pom_db::RedisValue::List((41..47).map(|job| job.to_string()).collect()),
                ),
                Ok("zset") => (
                    3,
                    pom_db::RedisValue::Sorted(
                        (7..11)
                            .map(|job| (job.to_string(), format!("{}", 1_727_700_000_000_i64 + job * 60_000)))
                            .collect(),
                    ),
                ),
                Ok("set") => (
                    keys.len() - 1,
                    pom_db::RedisValue::Set(
                        ["worker-a1", "worker-b7", "worker-c3", "worker-d9"].map(str::to_string).to_vec(),
                    ),
                ),
                _ => (
                    0,
                    pom_db::RedisValue::Text(
                        r#"{"name":"welcome","data":{"to":"ann@example.com","template":"welcome","vars":{"first":"Ann"}},"opts":{"attempts":3,"backoff":{"type":"exponential","delay":1000}},"timestamp":1727700000000}"#.into(),
                    ),
                ),
            };
            item.show(keys, selected, value);
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        } else if which == "board-bucket" {
            let storage = databases
                .iter()
                .find(|db| db.engine == pom_db::Engine::Minio)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no object storage"))?;
            let object =
                |name: &str, size: u64, modified: &str| pom_db::object_storage::ObjectEntry {
                    key: format!("avatars/{name}"),
                    size,
                    modified: modified.into(),
                    etag: "9b2cf5e1a4d0".into(),
                };
            let mut item = database_ui::BucketItem::new(
                database_ui::DatabaseContext {
                    objects: std::sync::Arc::new(BoardAvatar),
                    ..context
                },
                storage,
                "uploads".into(),
                "avatars/",
                Some("avatars/b77e.png".into()),
            );
            let wait = |item: &mut database_ui::BucketItem| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while item.is_busy() && std::time::Instant::now() < deadline {
                    item.tick(&|| None);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                item.tick(&|| None);
            };
            wait(&mut item);
            item.show(
                pom_db::object_storage::Listing {
                    prefixes: Vec::new(),
                    objects: vec![
                        object("a1f3.png", 49_152, "2026-10-21T10:12:00.000Z"),
                        object("b77e.png", 52_224, "2026-10-21T10:04:00.000Z"),
                        object("c0d9.jpg", 217_088, "2026-10-20T16:40:00.000Z"),
                        object("d4aa.png", 39_936, "2026-10-18T09:21:00.000Z"),
                        object("e912.webp", 18_432, "2026-10-18T08:02:00.000Z"),
                        object("f5c1.png", 45_056, "2026-10-16T14:55:00.000Z"),
                    ],
                    next: Some("more".into()),
                },
                Vec::new(),
            );
            wait(&mut item);
            item.paint_body(body, true);
            wait(&mut item);
            let painted = item
                .paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?;
            std::mem::forget(item);
            painted
        } else if which == "board-keyspace" {
            let database = databases
                .iter()
                .find(|database| database.engine == pom_db::Engine::Redis)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no redis"))?;
            let mut item = database_ui::KeyspaceItem::new(
                context,
                database,
                table("", "session", pom_db::TableKind::Keyspace, Some(214)),
            );
            let hour = 3600;
            let keys = [
                ("4f2a9c", Some(23 * hour)),
                ("8be310", Some(22 * hour)),
                ("91c07d", Some(6 * hour)),
                ("a3e55b", None),
                ("c71f04", Some(41 * 60)),
                ("d0b9e2", Some(18 * hour)),
                ("e6a1f8", Some(3 * hour)),
                ("f2c88a", Some(12 * hour)),
            ]
            .map(|(id, ttl)| pom_db::RedisKey {
                key: format!("session:{id}"),
                kind: "hash".into(),
                ttl,
            })
            .to_vec();
            item.show(
                keys,
                1,
                pom_db::RedisValue::Hash(vec![
                    ("user_id".into(), "2".into()),
                    ("csrf".into(), "Yk3...9Qa".into()),
                    ("return_to".into(), "/orders?page=2".into()),
                    ("flash".into(), r#"{"notice":"Signed in"}"#.into()),
                ]),
            );
            item.paint_body(body, true);
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        } else if which == "board-table" {
            let database = databases
                .iter()
                .find(|db| db.label == "main")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no main database"))?;
            let mut item = database_ui::TableItem::new(
                context,
                database,
                table("public", "users", pom_db::TableKind::Table, None),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while item.is_busy() && std::time::Instant::now() < deadline {
                item.tick(&|| None);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let people = [
                (
                    "dev",
                    "Alex Dev",
                    "admin",
                    Some("2026-09-02 10:14:03"),
                    r#"{"theme":"dark"}"#,
                ),
                (
                    "sam",
                    "Sam Lee",
                    "member",
                    Some("2026-09-03 08:41:55"),
                    r#"{"digest":true}"#,
                ),
                (
                    "kai",
                    "Kai Moss",
                    "member",
                    None,
                    r#"{"theme":"dark","digest":true,"beta":false}"#,
                ),
                (
                    "rio",
                    "Rio Park",
                    "member",
                    Some("2026-09-11 17:20:10"),
                    r#"{"theme":"light"}"#,
                ),
                (
                    "noa",
                    "Noa Ruiz",
                    "owner",
                    Some("2026-09-12 09:02:44"),
                    r#"{"digest":false}"#,
                ),
                (
                    "lee",
                    "Lee Chen",
                    "member",
                    Some("2026-09-14 13:37:21"),
                    "{}",
                ),
                ("ash", "Ash Kim", "member", None, r#"{"beta":true}"#),
                (
                    "max",
                    "Max Ode",
                    "member",
                    Some("2026-09-20 11:05:09"),
                    "{}",
                ),
                (
                    "ivy",
                    "Ivy Tan",
                    "member",
                    Some("2026-09-21 16:48:30"),
                    r#"{"theme":"dark"}"#,
                ),
                (
                    "joe",
                    "Joe Vale",
                    "member",
                    Some("2026-09-25 07:59:12"),
                    "{}",
                ),
                (
                    "uma",
                    "Uma Roy",
                    "admin",
                    Some("2026-10-01 12:00:00"),
                    r#"{"digest":true}"#,
                ),
                ("zed", "Zoe Hart", "member", None, "{}"),
            ];
            let rows = people
                .iter()
                .enumerate()
                .map(|(index, (mail, name, role, confirmed, settings))| {
                    vec![
                        Some((index + 1).to_string()),
                        Some(format!("{mail}@example.com")),
                        Some(name.to_string()),
                        Some(role.to_string()),
                        confirmed.map(str::to_string),
                        Some(settings.to_string()),
                    ]
                })
                .collect();
            let columns = ["id", "email", "name", "role", "confirmed_at", "settings"];
            item.show_page(
                pom_db::QueryResult {
                    columns: columns.map(str::to_string).to_vec(),
                    rows,
                    ..pom_db::QueryResult::default()
                },
                Some(1284),
            );
            let types = [
                "bigint",
                "varchar",
                "varchar",
                "varchar",
                "timestamp",
                "jsonb",
            ];
            item.show_structure(pom_db::TableStructure {
                columns: columns
                    .iter()
                    .zip(types)
                    .map(|(name, data_type)| pom_db::ColumnInfo {
                        name: name.to_string(),
                        data_type: data_type.into(),
                        nullable: *name != "id",
                        default: None,
                        primary_key: *name == "id",
                        references: None,
                    })
                    .collect(),
                indexes: Vec::new(),
                referenced_by: Vec::new(),
            });
            item.edit_cell(1, 3, Some("admin"));
            item.paint_body(body, true);
            item.select_cell(2, 5);
            item.paint_body(body, true);
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        } else {
            let database = pom_db::list_databases(
                &pom_config::Config::load(&dir.join("pom.yml"))?,
                "feat-login",
            )
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
            let mut item = database_ui::TableItem::new(
                context,
                database,
                table("public", "users", pom_db::TableKind::Table, None),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while item.is_busy() && std::time::Instant::now() < deadline {
                item.tick(&|| None);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // TABLEVIEW=structure|ddl, SIDE=row, EDITS=1, CELL=<column>: the table tab's other states.
            let names = ["Ann", "Bob", "Cy", "Di", "Ed", "Flo", "Gus", "Hal"];
            let big_settings = format!(
                "{{\"theme\":\"dark\",\"beta\":true,\"digest\":\"weekly\",\"locale\":\"en-US\",\"notifications\":{{\"email\":{{\"enabled\":true}},\"push\":{{\"enabled\":false}}}},\"saved_views\":[{}]}}",
                (0..180)
                    .map(|index| format!("{{\"name\":\"View {index}\",\"sort\":\"updated_at\"}}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let rows = (0..40)
                .map(|row| {
                    vec![
                        Some((row + 101).to_string()),
                        Some(format!(
                            "{}@example.com",
                            names[row % names.len()].to_lowercase()
                        )),
                        Some(names[row % names.len()].to_string()),
                        Some((row % 3 + 1).to_string()),
                        Some(if row % 3 == 0 { "admin" } else { "member" }.to_string()),
                        match row % 5 {
                            1 => Some(big_settings.clone()),
                            2 => None,
                            _ => Some(format!(
                                "{{\"theme\":\"{}\",\"beta\":{}}}",
                                if row % 2 == 0 { "light" } else { "dark" },
                                row % 3 == 0
                            )),
                        },
                        (row % 4 != 0)
                            .then(|| format!("2026-09-{:02} 10:{:02}:00", row % 28 + 1, row % 60)),
                    ]
                })
                .collect();
            item.show_page(
                pom_db::QueryResult {
                    columns: [
                        "id",
                        "email",
                        "name",
                        "org_id",
                        "role",
                        "settings",
                        "last_seen_at",
                    ]
                    .map(str::to_string)
                    .to_vec(),
                    rows,
                    ..pom_db::QueryResult::default()
                },
                Some(1234),
            );
            let info = |name: &str,
                        data_type: &str,
                        primary_key: bool,
                        references: Option<(&str, &str)>| {
                pom_db::ColumnInfo {
                    name: name.into(),
                    data_type: data_type.into(),
                    nullable: !primary_key && name != "email",
                    default: primary_key.then(|| "nextval('users_id_seq'::regclass)".to_string()),
                    primary_key,
                    references: references
                        .map(|(table, column)| (table.to_string(), column.to_string())),
                }
            };
            item.show_structure(pom_db::TableStructure {
                columns: vec![
                    info("id", "bigint", true, None),
                    info("email", "varchar(255)", false, None),
                    info("name", "varchar(120)", false, None),
                    info("org_id", "bigint", false, Some(("orgs", "id"))),
                    info("role", "text", false, None),
                    info("settings", "jsonb", false, None),
                    info("last_seen_at", "timestamptz", false, None),
                ],
                indexes: vec![
                    pom_db::Index {
                        name: "users_pkey".into(),
                        columns: "id".into(),
                        unique: true,
                        primary: true,
                    },
                    pom_db::Index {
                        name: "users_email_key".into(),
                        columns: "email".into(),
                        unique: true,
                        primary: false,
                    },
                    pom_db::Index {
                        name: "index_users_on_org_id".into(),
                        columns: "org_id".into(),
                        unique: false,
                        primary: false,
                    },
                ],
                referenced_by: vec![
                    pom_db::Reference {
                        schema: "public".into(),
                        table: "orders".into(),
                        column: "user_id".into(),
                        on_delete: "cascade".into(),
                    },
                    pom_db::Reference {
                        schema: "public".into(),
                        table: "login_tokens".into(),
                        column: "user_id".into(),
                        on_delete: "cascade".into(),
                    },
                ],
            });
            item.show_labels(3, &[("1", "Acme"), ("2", "Globex"), ("3", "Initech")]);
            if std::env::var_os("EDITS").is_some() {
                item.edit_cell(3, 4, Some("admin"));
                item.edit_cell(6, 2, Some("Gus QA"));
            }
            let column: usize = std::env::var("CELL")
                .ok()
                .and_then(|c| c.parse().ok())
                .unwrap_or(5);
            item.paint_body(body, true);
            item.select_cell(1, column);
            if std::env::var("SIDE").as_deref() == Ok("row") {
                item.show_side_row();
            }
            match std::env::var("TABLEVIEW").as_deref() {
                Ok("structure") => item.show_view_structure(),
                Ok("ddl") => item.show_ddl("CREATE TABLE \"public\".\"users\" (\n    \"id\" bigint DEFAULT nextval('users_id_seq'::regclass) NOT NULL,\n    \"email\" character varying(255) NOT NULL,\n    \"org_id\" bigint,\n    \"settings\" jsonb,\n    CONSTRAINT \"users_pkey\" PRIMARY KEY (id),\n    CONSTRAINT \"users_org_id_fkey\" FOREIGN KEY (org_id) REFERENCES orgs(id)\n);\nCREATE INDEX index_users_on_org_id ON public.users USING btree (org_id);\n"),
                _ => {}
            }
            item.paint_body(body, true);
            item.pointer_move(
                200.0,
                37.0 + 27.0 + 27.0 + 22.0 * 5.5,
                terminal::Modifiers::default(),
                true,
            );
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        };
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let mut layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        if let Some(overlay) = &overlay {
            layers.push((
                overlay.rects.as_slice(),
                overlay.tris.as_slice(),
                overlay.texts.as_slice(),
                overlay.icons.as_slice(),
                None,
            ));
        }
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!("remove {}: {error}", dir.display());
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("ENVTAB") {
        use workspace::Item;
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let dir = std::env::temp_dir().join(format!("pom-snapshot-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("pom.yml"),
            "session: myproject\npresets:\n  rails:\n    env:\n      RAILS_ENV: development\n      RAILS_LOG_TO_STDOUT: \"1\"\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    preset: [rails]\n    databases:\n      main: \"api_{{branch.safe}}\"\n      cache: \"api_cache_{{branch.safe}}\"\n    env:\n      DATABASE_URL: \"postgresql://{{shared.postgres.url}}/{{db.main}}\"\n      STRIPE_KEY: \"{{secret.STRIPE_KEY}}\"\n      APP_HOST: \"{{api.web.host}}\"\n    services:\n      web:\n        cmd: rails s\n",
        )?;
        let config = std::sync::Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
        let state = pom_paths::StateDir::new(dir.join("state"));
        let store = pom_secrets::SecretStore::new(state.clone(), "myproject");
        let secrets: &[(&str, &str)] = if which == "board-secrets" {
            &[
                ("STRIPE_SECRET_KEY", "sk_test_123"),
                ("SMTP_PASSWORD", "mailpit-dev"),
                ("GITHUB_TOKEN", "ghp_example"),
                ("SENTRY_DSN", "https://example.invalid/1"),
                ("OPENAI_API_KEY", "sk-example"),
            ]
        } else {
            &[
                ("STRIPE_KEY", "sk_test_123"),
                ("GITHUB_TOKEN", "ghp_example"),
                ("SENTRY_DSN", "https://example.invalid/1"),
            ]
        };
        for (name, value) in secrets {
            store
                .set(name, value)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
            pom_services::RunnerOptions {
                project_root: dir.clone(),
                session: "myproject".into(),
                state: state.clone(),
                holders: pom_ptyhost::SocketDir::new(dir.join("s")),
                binary: "/nonexistent".into(),
                docker: "/nonexistent".into(),
            },
        ));
        let context = environment_ui::EnvironmentContext {
            state,
            runner,
            config: std::sync::Arc::new(move || Some(config.clone())),
            workspaces: vec![("main".into(), true), ("feat-login".into(), false)],
            branch: "feat-login".into(),
        };
        let (width, height) = match scene_size() {
            Some(size) if which.starts_with("board-") => size,
            _ => (820.0_f32, 420.0_f32),
        };
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let mut item: Box<dyn Item> = if which == "board-env" {
            let line = |key: &str, value: &str, source: &str, secret: bool| pom_services::EnvLine {
                connection: false,
                key: key.into(),
                value: value.into(),
                source: source.into(),
                secret,
            };
            let mut item = environment_ui::EnvItem::new(context);
            item.show(
                pom_services::ServiceExplain {
                    repo: "api".into(),
                    alias: "api".into(),
                    service: "server".into(),
                    cmd: "rails s".into(),
                    databases: [("main".to_string(), "myproject_api_feat-login".to_string())]
                        .into_iter()
                        .collect(),
                    env: vec![
                        line(
                            "DATABASE_URL",
                            "postgres://postgres@localhost:5432/myproject_api_feat-login",
                            "own",
                            false,
                        ),
                        line("REDIS_URL", "redis://localhost:6379/3", "own", false),
                        line("RAILS_ENV", "development", "preset:rails", false),
                        line("RAILS_LOG_TO_STDOUT", "1", "preset:rails", false),
                        line(
                            "WEB_URL",
                            "http://app.web.feat-login.localhost:8767",
                            "resolved",
                            false,
                        ),
                        line("STRIPE_SECRET_KEY", "sk_test_51Hx...", "own", true),
                        line("SMTP_PASSWORD", "mailpit-dev", "own", true),
                        line("PORT", "3000", "resolved", false),
                    ],
                },
                Some(3000),
                &["SMTP_PASSWORD"],
            );
            Box::new(item)
        } else if which == "board-secrets" {
            let mut item = environment_ui::SecretsItem::new(context);
            item.type_new("POSTHOG_KEY", "phc_example1");
            item.show_states(("SMTP_PASSWORD", "mailpit-dev"), "SENTRY_DSN");
            item.set_focused(true);
            // The pointer rests on the armed row, as the board draws it.
            let row = item.paint_body(body, true).and_then(|painted| {
                painted
                    .hits
                    .iter()
                    .find(|(_, id)| *id == 111)
                    .map(|(rect, _)| *rect)
            });
            if let Some(rect) = row {
                item.pointer_move(
                    rect.x + 4.0,
                    rect.y + 4.0,
                    terminal::Modifiers::default(),
                    true,
                );
            }
            Box::new(item)
        } else if which == "secrets" {
            Box::new(environment_ui::SecretsItem::new(context))
        } else {
            Box::new(environment_ui::EnvItem::new(context))
        };
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!("remove {}: {error}", dir.display());
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(query) = std::env::var("SEARCH") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let board = std::env::var_os("SEARCHBOARD").is_some();
        let (width, height) = match (board, scene_size()) {
            (true, Some(size)) => size,
            _ => (900.0_f32, 560.0_f32),
        };
        let mut item = if board {
            let root =
                std::env::temp_dir().join(format!("pom-snapshot-search-{}", std::process::id()));
            search_fixture(&root)?;
            let mut item = if query.is_empty() {
                files_ui::project_search_preview(root.clone(), "", false)
            } else {
                files_ui::project_search_scoped(
                    root.clone(),
                    &query,
                    "User.active",
                    "api/**/*.rb, web/src/**",
                )
            };
            item.set_focused(false);
            if let Err(error) = std::fs::remove_dir_all(&root) {
                eprintln!("remove {}: {error}", root.display());
            }
            item
        } else {
            files_ui::project_search_preview(
                std::env::current_dir()?,
                &query,
                std::env::var("FILTERS").is_ok(),
            )
        };
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var_os("DIAGBOARD").is_some() {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let (width, height) = scene_size().unwrap_or((1092.0, 798.0));
        let users_ts = editor_fixture::API_TS.to_string();
        let mut item = files_ui::project_diagnostics_preview(vec![
            (
                "web/src/api/users.ts".into(),
                users_ts,
                vec![
                    (12, 41, 56, true, "A function whose declared type is neither 'undefined', 'void', nor 'any' must return a value.", "ts", "2355"),
                    (13, 44, 52, true, "Cannot find name 'pageSize'.", "ts", "2304"),
                ],
            ),
            (
                "api/app/controllers/users_controller.rb".into(),
                editor_fixture::CONTROLLER.to_string(),
                vec![(
                    14,
                    4,
                    6,
                    false,
                    "Style/GuardClause: Use a guard clause instead of wrapping the code inside a conditional expression.",
                    "rubocop",
                    "",
                )],
            ),
        ]);
        item.set_focused(true);
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless(width as u32, height as u32, 1.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(query) = std::env::var("FINDER") {
        let root = std::env::current_dir()?;
        let paths = files::project_files(&root, false);
        let recent = vec![
            "crates/files_ui/src/files_ui.rs".to_string(),
            "crates/workspace/src/workspace_view.rs".to_string(),
        ];
        let node = files_ui::file_finder_preview(paths, recent, &query);
        let (width, height) = (560.0_f32, 460.0_f32);
        let painted = ui::render(
            &ui::div().p(8.0).child(node).into(),
            ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
        );
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("TICKETTAB") {
        use workspace::Item;
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let (width, height) = match scene_size() {
            Some(size) if which.starts_with("board") => size,
            _ => (720.0_f32, 620.0_f32),
        };
        let dir = std::env::temp_dir().join(format!("pom-snapshot-ticket-{}", std::process::id()));
        let mut item = jira_ui::TicketItem::new(
            pom_paths::StateDir::new(dir.join("state")),
            "myproject".into(),
            "PROJ-101".into(),
            std::sync::Arc::new(|| {}),
        );
        let comment = |id: &str, author: &str, created: &str, body: &str| pom_jira::Comment {
            id: id.into(),
            author: author.into(),
            avatar: String::new(),
            created: created.into(),
            body: body.into(),
        };
        let link = |title: &str| pom_jira::WebLink {
            title: title.into(),
            url: "https://example.com".into(),
            icon: String::new(),
        };
        if which == "board-unset" {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while item.is_busy() && std::time::Instant::now() < deadline {
                item.tick(&|| None);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        } else if which == "board" {
            item.show(
                pom_jira::IssueDetail {
                    key: "PROJ-101".into(),
                    summary: "Login with Google".into(),
                    status: "In Progress".into(),
                    url: "https://example.atlassian.net/browse/PROJ-101".into(),
                    description: "Let people sign in with their Google account next to email and password.\n\n**Acceptance**\n\n- A \"Continue with Google\" button on the login page\n- First sign-in creates the user; a matching `email` links to the existing one\n- Errors come back to the login page with a message".into(),
                    comments: vec![
                        comment("1", "Sam Lee", "2026-10-21T16:20:00.000+0700", "Use the existing session cookie; no new token storage."),
                        comment("2", "Alex Dev", "2026-10-22T10:05:00.000+0700", "Callback route is in, spec next."),
                    ],
                    web_links: vec![link("Design doc"), link("OAuth consent screen")],
                },
                "indeterminate",
            );
        } else {
            item.show(
            pom_jira::IssueDetail {
                key: "PROJ-101".into(),
                summary: "Flag escalated conversations in the inbox".into(),
                status: "In Progress".into(),
                url: "https://example.atlassian.net/browse/PROJ-101".into(),
                description: "## Background\n\nNothing marks a message where the sender **asks for a person**. See https://example.com/spec.\n\n## Acceptance criteria\n\n- [x] Detect a direct ask\n- [ ] Show a banner in the conversation\n- [ ] Email the team".into(),
                comments: vec![
                    pom_jira::Comment {
                        id: "1".into(),
                        author: "Ann".into(),
                        avatar: String::new(),
                        created: "2026-09-17T03:53:12.000+0700".into(),
                        body: "Should this cover *email* too?".into(),
                    },
                    pom_jira::Comment {
                        id: "2".into(),
                        author: "Bea".into(),
                        avatar: String::new(),
                        created: "2026-09-18T10:05:00.000+0700".into(),
                        body: "Yes, every channel. Use `inbox_flag`.".into(),
                    },
                ],
                web_links: vec![pom_jira::WebLink {
                    title: "Design doc".into(),
                    url: "https://example.com/design".into(),
                    icon: String::new(),
                }],
            },
            "indeterminate",
        );
        }
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!("remove {}: {error}", dir.display());
            }
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }
    if std::env::var("PRTAB").is_ok() {
        use workspace::Item;
        let (width, height) = (720.0_f32, 520.0_f32);
        let dir = std::env::temp_dir().join(format!("pom-snapshot-pr-{}", std::process::id()));
        let target = pom_forge::PrTarget {
            repo: "web".into(),
            owner: "acme".into(),
            name: "web".into(),
            head: "feat-login".into(),
        };
        let check = |name: &str, workflow: &str, result: &str| pom_forge::Check {
            name: name.into(),
            workflow_name: workflow.into(),
            details_url: "https://example.com".into(),
            result: result.into(),
            ..pom_forge::Check::default()
        };
        let mut pr = pom_forge::PullRequest {
            number: 42,
            title: "Add the login page with remember-me and rate limiting".into(),
            state: "OPEN".into(),
            head_ref_name: "feat-login".into(),
            base_ref_name: "main".into(),
            author: Some(pom_forge::Actor { login: "dev".into(), avatar_url: String::new() }),
            additions: 184,
            deletions: 23,
            body: "### Related ticket\n[PROJ-101](https://example.atlassian.net/browse/PROJ-101)\n\n### Description\nAdds the **login form** and `session` handling. See https://example.com/docs.\n\n- Remember me for *30 days*\n- Five attempts per minute per IP\n- [x] tests\n\n| Case | Result |\n|---|---|\n| valid | 200 |\n| locked | 423 |\n\n```rust\nfn login() -> bool { true }\n```".into(),
            labels: vec![
                pom_forge::Label { name: "feature".into(), color: "a2eeef".into() },
                pom_forge::Label { name: "needs-review".into(), color: "fbca04".into() },
            ],
            status_check_rollup: vec![
                check("test", "CI", "pass"),
                check("lint", "CI", "fail"),
                check("deploy-preview", "", "pending"),
            ],
            reviewers: vec![
                pom_forge::Reviewer { name: "ann".into(), state: "pending".into() },
                pom_forge::Reviewer { name: "bea".into(), state: "changes".into() },
                pom_forge::Reviewer { name: "cy".into(), state: "approved".into() },
            ],
            ..pom_forge::PullRequest::default()
        };
        pr.checks = "fail".into();
        pr.timeline = vec![
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Review,
                author: "bea".into(),
                body: "The rate limit should key on the **account**, not only the IP.".into(),
                at: "2026-09-01T09:00:00Z".into(),
                state: "CHANGES_REQUESTED".into(),
                threads: vec![pom_forge::ReviewThread {
                    id: "PRRT_1".into(),
                    path: "src/auth/limit.rs".into(),
                    line: Some(42),
                    resolved: true,
                    comments: vec![
                        pom_forge::ThreadComment {
                            author: "bea".into(),
                            body: "This counter never resets.".into(),
                            at: "2026-09-01T09:00:00Z".into(),
                        },
                        pom_forge::ThreadComment {
                            author: "dev".into(),
                            body: "Fixed with a sliding window, see `Window::tick`.".into(),
                            at: "2026-09-01T12:00:00Z".into(),
                        },
                    ],
                }],
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Comment,
                author: "ann".into(),
                body: "QA notes are in https://example.com/qa.".into(),
                at: "2026-09-02T10:00:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
        ];
        let mut item = pull_request_ui::PrItem::new(
            pom_paths::StateDir::new(dir.join("state")),
            "myproject".into(),
            std::sync::Arc::new(|| {}),
            target,
            None,
        );
        item.show(Some(pr));
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        item.paint_body(body, true);
        if std::env::var("CHECKS").is_ok() {
            item.pointer_down(110.0, 119.0, 1, terminal::Modifiers::default());
        }
        if let Some(scroll) = std::env::var("SCROLL")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
        {
            item.pointer_scroll(0.0, 0.0, -scroll, terminal::Modifiers::default());
        }
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!("remove {}: {error}", dir.display());
            }
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    // With GITPANEL=<repo dir>, render the Git panel for that repository (against `main`).
    if let Ok(repo) = std::env::var("GITPANEL") {
        use workspace::SidePanelView;
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let scene_size = std::env::var("SCENESIZE").ok().and_then(|size| {
            let (w, h) = size.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        });
        let (width, height) = scene_size.unwrap_or((330.0_f32, 820.0_f32));
        let scale = if scene_size.is_some() {
            1.0_f32
        } else {
            2.0_f32
        };
        // GITPANEL=<repo>[,<repo>...]; WSBRANCH names the workspace branch.
        let sources: Vec<git_ui::RepoSource> = repo
            .split(',')
            .map(|path| git_ui::RepoSource {
                name: std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                root: path.into(),
                default_branch: "main".into(),
                expected_branch: std::env::var("WSBRANCH").unwrap_or_else(|_| "feat-login".into()),
                kept: false,
                origin: None,
            })
            .collect();
        let roots: Vec<std::path::PathBuf> =
            sources.iter().map(|source| source.root.clone()).collect();
        let mut panel = git_ui::GitPanel::new(sources, None, std::sync::Arc::new(|| {}));
        if let Ok(reason) = std::env::var("GITUNSERVED") {
            panel = panel.with_access(std::sync::Arc::new(git::access::UnservedGit::new(reason)));
        } else if std::env::var("GITWIRE").is_ok() {
            panel = panel.with_access(std::sync::Arc::new(WireGit));
        }
        if std::env::var("GITPRS").is_ok() {
            let branch = std::env::var("WSBRANCH").unwrap_or_else(|_| "feat-login".into());
            panel = panel.with_pull_requests(board_pull_requests(&roots, &branch)?);
        }
        panel.render(width, height);
        panel.wait_for_scan();
        // TAB=remote|history picks the tab; HOVER=<offset> hovers a panel id (a row is row * 8 + control).
        match std::env::var("TAB").as_deref() {
            Ok("remote") => panel.set_tab(git_ui::Tab::Remote),
            Ok("history") => panel.set_tab(git_ui::Tab::History),
            _ => {}
        }
        if std::env::var("GITTIMELINE").is_ok() {
            panel.set_timeline(true);
        }
        if let Ok(marks) = std::env::var("GITREVIEWED") {
            for mark in marks.split(',') {
                if let Some((repo, path)) = mark.split_once(':') {
                    panel.mark_reviewed(repo.parse().unwrap_or_default(), path);
                }
            }
        }
        if let Some((repo, index)) = std::env::var("GITOPEN").ok().and_then(|open| {
            let (repo, index) = open.split_once(':')?;
            Some((repo.parse::<usize>().ok()?, index.parse::<usize>().ok()?))
        }) {
            panel.open_history_commit(repo, index);
        }
        match std::env::var("GITVIEW").as_deref() {
            Ok("tree") => panel.set_view(true, true),
            Ok("ungrouped") => panel.set_view(false, false),
            Ok(_) => panel.set_view(true, true),
            Err(_) if scene_size.is_some() => panel.set_view(false, true),
            Err(_) => {}
        }
        if let Some(steps) = std::env::var("GITSELECT")
            .ok()
            .and_then(|steps| steps.parse::<usize>().ok())
        {
            panel.render(width, height);
            panel.key(workspace::EditKey::Tab, false);
            for _ in 0..steps {
                panel.list_key(workspace::EditKey::Down, false);
            }
        }
        if std::env::var("GITAMEND").is_ok() {
            panel.set_signoff(true);
            if !panel.key(workspace::EditKey::ReplaceAll, true) {
                anyhow::bail!("the panel did not take the amend key");
            }
        }
        panel.render(width, height);
        if let Ok(hover) = std::env::var("HOVER") {
            if let Ok(offset) = hover.parse::<u64>() {
                panel.set_hover(Some(
                    workspace::side_panel_base(workspace::PaneKind::Git) + offset,
                ));
            }
        }
        let node = panel.render(width, height);
        let mut r =
            ui::UiRenderer::new_headless((width * scale) as u32, (height * scale) as u32, scale)?;
        let frame = ui::paint_frame(
            &ui::div()
                .bg(ui::theme().panel_background)
                .child(node)
                .into(),
            ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
        );
        let layers: Vec<ui::Layer> = std::iter::once((&frame.base, None))
            .chain(
                frame
                    .overlays
                    .iter()
                    .map(|overlay| (&overlay.painted, overlay.clip.map(|r| (r.x, r.y, r.w, r.h)))),
            )
            .map(|(painted, clip)| {
                (
                    painted.rects.as_slice(),
                    painted.tris.as_slice(),
                    painted.texts.as_slice(),
                    painted.icons.as_slice(),
                    clip,
                )
            })
            .collect();
        r.render_frame(ui::theme().panel_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    let scene_size = std::env::var("SCENESIZE").ok().and_then(|size| {
        let (w, h) = size.split_once('x')?;
        Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
    });
    // A scene is compared against a board drawn at 1x, so it renders at 1x too.
    let scale = if scene_size.is_some() {
        1.0_f32
    } else {
        2.0_f32
    };
    let crop = window_crop();
    let (lw, lh) = match (scene_size, std::env::var("MAINVIEW")) {
        (Some(_), Ok(_)) if crop.is_some() => (1440.0, 900.0),
        (Some(_), _) if crop.is_some() && std::env::var("REMOTEVIEW").is_ok() => (1440.0, 900.0),
        (Some(size), _) => size,
        (None, Ok(_)) => (
            std::env::var("SNAPW")
                .ok()
                .and_then(|width| width.parse().ok())
                .unwrap_or(1200.0_f32),
            780.0_f32,
        ),
        (None, Err(_)) => (920.0_f32, 760.0_f32),
    }; // logical
    let mut r = ui::UiRenderer::new_headless((lw * scale) as u32, (lh * scale) as u32, scale)?;

    if std::env::var("MODAL").is_ok()
        || std::env::var("PROMPT").is_ok()
        || std::env::var("MERGECONFIRM").is_ok()
    {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let node = if std::env::var("MERGECONFIRM").is_ok() {
            // The board frames the section with 8px of the tab around it, where its edge and shadow fall.
            let section = pr_fixture("merge-confirm")
                .confirming_merge_box(lw - 16.0)
                .ok_or_else(|| anyhow::anyhow!("the fixture pull request cannot merge"))?;
            ui::div()
                .col()
                .p(8.0)
                .bg(ui::theme().editor_background)
                .child(section)
                .into()
        } else if let Ok(name) = std::env::var("MODAL") {
            let mut modal = modal_fixture::modal(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown modal {name}"))?;
            modal.render()
        } else {
            let name = std::env::var("PROMPT")?;
            let prompt = modal_fixture::prompt(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown prompt {name}"))?;
            workspace::prompt_card(&prompt, 0, 0)
        };
        // Laid out in a window taller than the crop, so a popover drops below its trigger as it would there.
        let frame = ui::paint_frame(
            &ui::div().col().child(node).into(),
            ui::Rect::new(0.0, 0.0, lw, lh + 400.0, ui::Rgba::TRANSPARENT),
        );
        let mut layers: Vec<ui::Layer> = vec![(
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        )];
        for overlay in &frame.overlays {
            layers.push((
                overlay.painted.rects.as_slice(),
                overlay.painted.tris.as_slice(),
                overlay.painted.texts.as_slice(),
                overlay.painted.icons.as_slice(),
                overlay.clip.map(|c| (c.x, c.y, c.w, c.h)),
            ));
        }
        let backdrop = if std::env::var("MERGECONFIRM").is_ok() {
            ui::theme().editor_background
        } else {
            ui::theme().background
        };
        r.render_frame(backdrop, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("ONBOARDSCENE").or_else(|_| std::env::var("PAGESCENE")) {
        use workspace::Item;
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let mut page: Box<dyn Item> = match which.as_str() {
            "store" => Box::new(module_store_ui::board_page("store")),
            "store-measuring" => Box::new(module_store_ui::board_page("measuring")),
            "store-optimize" => {
                let mut page = module_store_ui::board_page("store");
                page.paint_body(ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT), true);
                page.press_optimize();
                Box::new(page)
            }
            "requests" => Box::new(dev_services_ui::board_page(true)),
            "requests-empty" => Box::new(dev_services_ui::board_page(false)),
            "ports" => Box::new(remote_ui::ports_board_page("forwarded")),
            "ports-moved" => Box::new(remote_ui::ports_board_page("moved")),
            "ports-error" => Box::new(remote_ui::ports_board_page("error")),
            "ports-empty" => Box::new(remote_ui::ports_board_page("empty")),
            _ => Box::new(onboarding_ui::preview_page(&which)),
        };
        let body = ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT);
        let painted = page.paint_body(body, true).unwrap_or_default();
        let footer = page.paint_popover(body).unwrap_or_default();
        r.render_frame(
            ui::theme().editor_background,
            &[
                (
                    painted.rects.as_slice(),
                    painted.tris.as_slice(),
                    painted.texts.as_slice(),
                    painted.icons.as_slice(),
                    None,
                ),
                (
                    footer.rects.as_slice(),
                    footer.tris.as_slice(),
                    footer.texts.as_slice(),
                    footer.icons.as_slice(),
                    None,
                ),
            ],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(view) = std::env::var("PRFIXTURE") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let mut item = pr_fixture(&view);
        let painted = {
            use workspace::Item;
            item.paint_body(ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT), true)
                .unwrap_or_default()
        };
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("TABBAR") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let mut pane = workspace::pane::Pane::new(1);
        let mut buttons = Vec::new();
        if which == "agent" {
            pane.add_item(Box::new(SampleTab {
                title: "Main",
                icon: Some(ui::IconKind::Sparkle),
                material: None,
                dirty: false,
            }));
            pane.toggle_pin(0);
            for (index, icon) in [
                ui::IconKind::Plus,
                ui::IconKind::Clock,
                ui::IconKind::Maximize,
            ]
            .into_iter()
            .enumerate()
            {
                buttons.push(workspace::pane::TabBarButton {
                    icon,
                    id: 900 + index as u64,
                    selected: false,
                    disabled: false,
                });
            }
        } else {
            for (title, icon, material, dirty) in [
                (
                    "users_controller.rb",
                    None,
                    Some(ui::MaterialIcon::Ruby),
                    true,
                ),
                ("routes.rb", None, Some(ui::MaterialIcon::Ruby), false),
                ("api > server", Some(ui::IconKind::Server), None, false),
            ] {
                pane.add_item(Box::new(SampleTab {
                    title,
                    icon,
                    material,
                    dirty,
                }));
                pane.keep_preview();
            }
            pane.activate_user(0);
        }
        let config = workspace::pane::TabBarConfig {
            show_nav: false,
            buttons,
            toolbar: Vec::new(),
        };
        let ids = workspace::pane::PaneClickIds {
            tab_activate: 100,
            tab_close: 200,
            nav_back: 300,
            nav_forward: 301,
            search: 302,
        };
        let (nav, _) = workspace::pane::tab_bar_edges(&config);
        let bar = ui::render(
            &workspace::pane::render_pane(&pane, &config, ids, lw),
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        let strip = ui::render(
            &workspace::pane::render_tab_strip(&pane, ids, None),
            ui::Rect::new(nav, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().panel_background,
            &[
                (
                    bar.rects.as_slice(),
                    bar.tris.as_slice(),
                    bar.texts.as_slice(),
                    bar.icons.as_slice(),
                    None,
                ),
                (
                    strip.rects.as_slice(),
                    strip.tris.as_slice(),
                    strip.texts.as_slice(),
                    strip.icons.as_slice(),
                    None,
                ),
            ],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("ROWMENU") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let main = which == "main";
        let items = workspace::workspace_row_menu(workspace::RowMenuFacts {
            sleeping: false,
            ticket: !main,
            running: true,
            main,
            missing: main,
        });
        let (card, _, _) = workspace::context_menu_card(&items, items.first().map(|item| item.id));
        let painted = ui::render(
            &card,
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().panel_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var("HOSTSTATES").is_ok() {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let colors = ui::theme();
        let states = [
            (workspace::HostLink::Connecting, "Connecting to: dev"),
            (workspace::HostLink::Connected, "Connected to: dev"),
            (
                workspace::HostLink::Reconnecting,
                "Lost connection to dev. Reconnecting...",
            ),
            (workspace::HostLink::Disconnected, "Disconnected from dev"),
        ];
        let mut sheet = ui::div()
            .col()
            .w_px(lw)
            .px(12.0)
            .py(8.0)
            .bg(colors.title_bar_background);
        for (link, meta) in states {
            sheet = sheet.child(
                ui::div()
                    .row()
                    .items_center()
                    .gap(2.0)
                    .h_px(40.0)
                    .child(workspace::host_indicator(
                        "dev",
                        link,
                        false,
                        colors.title_bar_background,
                    ))
                    .child(
                        ui::div()
                            .row()
                            .items_center()
                            .h_px(24.0)
                            .px(8.0)
                            .child(ui::label("myproject").size(13.0).color(colors.text)),
                    )
                    .child(ui::div().row().flex(1.0))
                    .child(ui::label(meta).size(11.0).color(colors.text_placeholder)),
            );
        }
        let painted = ui::render(
            &sheet.into(),
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            colors.title_bar_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("WELCOME") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        workspace::keymap::set_key_hints(&[(
            workspace::keymap::Action::OpenProject,
            "cmd-o".into(),
        )]);
        let home = std::env::var("HOME").unwrap_or_default();
        let mut sessions: Vec<workspace::Session> = ["myproject", "storefront", "billing"]
            .into_iter()
            .map(|name| workspace::Session {
                name: name.into(),
                path: std::path::PathBuf::from(format!("{home}/code/{name}")),
                running: false,
                missing: false,
                host: None,
            })
            .collect();
        let remote = which == "remote";
        if remote {
            sessions.truncate(2);
            sessions[0].host = Some("dev".into());
            sessions[0].path = "/home/dev/pom/myproject".into();
        }
        let docker = which != "docker";
        let checks = vec![
            workspace::MachineCheck {
                name: "Docker".into(),
                ok: docker,
                detail: if docker {
                    "Running".into()
                } else {
                    "Docker is not running. Shared services and databases start once it is.".into()
                },
                fix: (!docker).then(|| "Start Docker".to_string()),
            },
            workspace::MachineCheck {
                name: "git".into(),
                ok: true,
                detail: "git version 2.50.1".into(),
                fix: None,
            },
        ];
        let page = workspace::welcome_page(
            &sessions,
            &checks,
            lw,
            Some(if remote {
                workspace::WELCOME_CONNECT_HOST
            } else {
                workspace::WELCOME_NEW_PROJECT_CARD
            }),
        );
        let painted = ui::render(
            &page,
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().editor_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var_os("DBFAILURE").is_some() {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let node = ui::div()
            .p(8.0)
            .child(database_ui::missing_database_preview(
                lw - 16.0,
                "myproject_api_feat-login",
                "api",
            ))
            .into();
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().panel_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("NOTICE") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let colors = ui::theme();
        let update = |label: &str, tone, progress: Option<f32>| workspace::UpdateButton {
            label: label.into(),
            icon: if progress.is_some() {
                ui::IconKind::Download
            } else {
                ui::IconKind::RotateCw
            },
            progress,
            tooltip: None,
            tone,
            dismissable: progress.is_none(),
            click: None,
        };
        let (node, backdrop) = match which.as_str() {
            "toast" => (
                workspace::toast_preview("Start the agent first to add the selection to it", None)
                    .0,
                colors.editor_background,
            ),
            "port-ready" => (
                workspace::toast_preview(
                    &pom_remote::reachable_notice("dev", &[("postgres".to_string(), 5432)])
                        .unwrap_or_default(),
                    None,
                )
                .0,
                colors.editor_background,
            ),
            "update-ready-button" => (
                workspace::update_button(
                    &update("Restart to Update", workspace::UpdateTone::Ready, None),
                    None,
                ),
                colors.title_bar_background,
            ),
            "update-states" => {
                let states = [
                    (
                        "Checking for updates...",
                        ui::IconKind::LoadCircle,
                        workspace::UpdateTone::Normal,
                        None,
                        false,
                    ),
                    (
                        "Downloading 0.7.4",
                        ui::IconKind::Download,
                        workspace::UpdateTone::Normal,
                        Some(0.45),
                        false,
                    ),
                    (
                        "Verifying 0.7.4...",
                        ui::IconKind::LoadCircle,
                        workspace::UpdateTone::Normal,
                        None,
                        false,
                    ),
                    (
                        "Restart to Update",
                        ui::IconKind::RotateCw,
                        workspace::UpdateTone::Ready,
                        None,
                        true,
                    ),
                    (
                        "Update failed",
                        ui::IconKind::Warning,
                        workspace::UpdateTone::Warning,
                        None,
                        true,
                    ),
                ];
                let mut sheet = ui::div().col().gap(8.0);
                for (label, icon, tone, progress, dismissable) in states {
                    let button = workspace::UpdateButton {
                        label: label.into(),
                        icon,
                        progress,
                        tooltip: None,
                        tone,
                        dismissable,
                        click: None,
                    };
                    sheet = sheet.child(
                        ui::div()
                            .row()
                            .child(workspace::update_button(&button, None)),
                    );
                }
                (sheet.into(), colors.title_bar_background)
            }
            name => (
                workspace::notification_preview(name)
                    .ok_or_else(|| anyhow::anyhow!("unknown notice {name}"))?
                    .0,
                colors.editor_background,
            ),
        };
        // The board frames a toast or the update button with 8px of what lies under it.
        let frame = if matches!(
            which.as_str(),
            "toast" | "port-ready" | "update-ready-button" | "update-states"
        ) {
            8.0
        } else {
            0.0
        };
        let content = ui::render(
            &ui::div().row().p(frame).child(node).into(),
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        let mut painted = ui::Painted::default();
        if !which.starts_with("update") {
            let (right, bottom) = content
                .rects
                .iter()
                .fold((0.0_f32, 0.0_f32), |(r, b), rect| {
                    (r.max(rect.x + rect.w), b.max(rect.y + rect.h))
                });
            let card = ui::Rect::new(
                frame,
                frame,
                right - frame,
                bottom - frame,
                ui::Rgba::TRANSPARENT,
            );
            painted.rects.extend(workspace::elevation_shadow(
                card,
                workspace::Elevation::Modal,
            ));
        }
        painted.rects.extend(content.rects);
        painted.tris.extend(content.tris);
        painted.texts.extend(content.texts);
        painted.icons.extend(content.icons);
        r.render_frame(
            backdrop,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(picker) = std::env::var("PICKER") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let (kind, query) = picker.split_once(':').unwrap_or((picker.as_str(), ""));
        let popovers = match kind {
            "hover" => Some(files_ui::hover_preview(
                "web/src/api/users.ts",
                editor_fixture::API_TS,
                (13, 44),
                vec![(13, 44, 52, true, "Cannot find name 'pageSize'.", "ts", "2304")],
                "```typescript\nfunction fetchUsers({ page, per }: Page): Promise<User[]>\n```\n\nThe server caps per at 100; ask for less and page instead.",
                (1092.0, 770.0),
            )),
            "completion" => Some(files_ui::completion_preview(
                vec![
                    ("filter", "(predicate) => User[]", false),
                    ("find", "(predicate) => User", false),
                    ("findIndex", "(predicate) => number", false),
                    ("findLast", "(predicate) => User", false),
                    ("fill", "(value, start?, end?)", false),
                    ("flat", "(depth?) => User[]", false),
                    ("for in", "snippet", true),
                ],
                query,
                "```typescript\nfind(predicate): User | undefined\n```\n\nReturns the value of the first element in the array where predicate is true, and undefined otherwise.",
            )),
            _ => None,
        };
        let painted = match popovers {
            Some(popovers) => {
                let mut painted = ui::Painted::default();
                for (node, rect) in popovers {
                    let rect =
                        ui::Rect::new(rect.x + 24.0, rect.y + 24.0, rect.w, rect.h, rect.color);
                    painted.rects.extend(workspace::elevation_shadow(
                        rect,
                        workspace::Elevation::Elevated,
                    ));
                    let part = ui::render(&ui::div().col().child(node).into(), rect);
                    painted.rects.extend(part.rects);
                    painted.tris.extend(part.tris);
                    painted.texts.extend(part.texts);
                    painted.icons.extend(part.icons);
                }
                painted
            }
            None => {
                let node = match kind {
                    "finder" => files_ui::file_finder_preview(finder_fixture(), Vec::new(), query),
                    "goto-states" => ui::div()
                        .col()
                        .gap(8.0)
                        .children(
                            ["", "26:5", "-2"]
                                .map(|query| files_ui::go_to_line_preview(5, 4, 31, query)),
                        )
                        .into(),
                    _ => files_ui::command_palette_preview(&palette_fixture(), query),
                };
                ui::render(
                    &node,
                    ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
                )
            }
        };
        let under = if kind == "hover" || kind == "completion" {
            ui::theme().editor_background
        } else {
            ui::theme().background
        };
        r.render_frame(
            under,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(query) = std::env::var("SWITCHER") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        let node = files_ui::workspace_switcher_preview(switcher_fixture(), &query);
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var("AGENTSTRIP").is_ok() {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        workspace::keymap::set_key_hints(&[
            (workspace::keymap::Action::AgentAllow, "cmd-k y".into()),
            (workspace::keymap::Action::AgentDeny, "cmd-k n".into()),
        ]);
        let pending = pom_agent::PendingApproval {
            request: "1".into(),
            tool: "Bash".into(),
            summary: "bundle exec rspec spec/requests/users_spec.rb".into(),
        };
        let node = terminal_ui::lease_strip(0, Some("an orchestrator"), Some(&pending), lw)
            .unwrap_or_else(|| ui::div().into());
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, lw, lh, ui::Rgba::TRANSPARENT),
        );
        r.render_frame(
            ui::theme().panel_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(mode) = std::env::var("REMOTEVIEW") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        return remote_fixture::render(&mode, &out, (lw, lh), scale, &mut r, crop);
    }
    if let Ok(mode) = std::env::var("MAINVIEW") {
        if let Ok(name) = std::env::var("THEME") {
            ui::set_theme(ui::by_name(&name));
        }
        if let Ok(size) = std::env::var("FONTSIZE").unwrap_or_default().parse::<f32>() {
            ui::set_ui_text_scale(size / ui::UI_FONT_BASE);
        }
        let sessions: Vec<workspace::Session> = ["myproject", "api", "web", "old"]
            .iter()
            .map(|name| workspace::Session {
                name: name.to_string(),
                path: format!("/projects/{name}").into(),
                running: false,
                missing: *name == "old",
                host: None,
            })
            .collect();
        let project = (mode != "welcome" && mode != "onboard").then(|| workspace::ProjectInfo {
            states: Vec::new(),
            name: "myproject".into(),
            branch: "main".into(),
            config_path: "/projects/myproject/pom.yml".into(),
            host: None,
            workspaces: vec![
                "main".into(),
                "feat-login".into(),
                "proj-101-a-very-long-branch-name-for-the-workspace-panel".into(),
            ],
            active: "feat-login".into(),
            labels: vec![String::new(), "Login page".into(), String::new()],
            running: vec![0, 2, 0],
            tickets: vec![String::new(), String::new(), "In Progress".into()],
            ticket_categories: vec![String::new(), String::new(), "indeterminate".into()],
            prs: vec![
                None,
                Some(workspace::PrSummary {
                    count: 2,
                    severity: workspace::PrSeverity::Warn,
                    trouble: Some(workspace::PrTrouble::Pending),
                }),
                Some(workspace::PrSummary {
                    count: 1,
                    severity: workspace::PrSeverity::Danger,
                    trouble: Some(workspace::PrTrouble::ChecksFailed),
                }),
            ],
            missing: Vec::new(),
            repo_branches: Vec::new(),
        });
        // MAINVIEW=sidebar: a project in every state the WORKSPACES list shows (RAIL=1 folds it to the rail).
        let sidebar = mode == "sidebar";
        let project = if sidebar {
            let pr = |count: usize,
                      severity: workspace::PrSeverity,
                      trouble: Option<workspace::PrTrouble>| {
                Some(workspace::PrSummary {
                    count,
                    severity,
                    trouble,
                })
            };
            // branch, label, ticket status, its category, running services, PRs.
            type Sample = (
                &'static str,
                &'static str,
                &'static str,
                &'static str,
                usize,
                Option<workspace::PrSummary>,
            );
            let rows: [Sample; 8] = [
                (
                    "main",
                    "",
                    "",
                    "",
                    2,
                    pr(2, workspace::PrSeverity::Merged, None),
                ),
                (
                    "proj-101-email-open-tracking",
                    "PROJ-101 Email open tracking",
                    "QA In Progress",
                    "indeterminate",
                    0,
                    pr(
                        3,
                        workspace::PrSeverity::Danger,
                        Some(workspace::PrTrouble::ChecksFailed),
                    ),
                ),
                (
                    "proj-102-ai-lead-creation",
                    "PROJ-102 AI lead creation from forms",
                    "To QA",
                    "indeterminate",
                    0,
                    pr(1, workspace::PrSeverity::Ok, None),
                ),
                (
                    "proj-103-flag-escalated",
                    "PROJ-103 Flag escalated conversations",
                    "Code review",
                    "indeterminate",
                    0,
                    pr(
                        4,
                        workspace::PrSeverity::Danger,
                        Some(workspace::PrTrouble::Conflict),
                    ),
                ),
                (
                    "proj-10432-migrate-billing-webhooks",
                    "PROJ-10432 Migrate billing webhooks to the new provider and backfill",
                    "Waiting for customer feedback",
                    "indeterminate",
                    12,
                    pr(
                        12,
                        workspace::PrSeverity::Danger,
                        Some(workspace::PrTrouble::ChecksFailed),
                    ),
                ),
                (
                    "proj-106-redesign-listing",
                    "PROJ-106 Redesign listing page",
                    "Backlog",
                    "new",
                    0,
                    None,
                ),
                (
                    "proj-108-inbound-call-routing",
                    "PROJ-108 Inbound call routing",
                    "Done",
                    "done",
                    0,
                    pr(4, workspace::PrSeverity::Merged, None),
                ),
                (
                    "investigate-0917-5fwg",
                    "",
                    "",
                    "",
                    0,
                    pr(
                        1,
                        workspace::PrSeverity::Warn,
                        Some(workspace::PrTrouble::Pending),
                    ),
                ),
            ];
            Some(workspace::ProjectInfo {
                states: Vec::new(),
                name: "myproject".into(),
                branch: "main".into(),
                config_path: "/projects/myproject/pom.yml".into(),
                host: None,
                workspaces: rows.iter().map(|row| row.0.to_string()).collect(),
                active: "main".into(),
                labels: rows.iter().map(|row| row.1.to_string()).collect(),
                running: rows.iter().map(|row| row.4).collect(),
                tickets: rows.iter().map(|row| row.2.to_string()).collect(),
                ticket_categories: rows.iter().map(|row| row.3.to_string()).collect(),
                prs: rows.iter().map(|row| row.5).collect(),
                missing: Vec::new(),
                repo_branches: Vec::new(),
            })
        } else {
            project
        };
        let window = mode == "window";
        let project = if window {
            Some(window_project())
        } else {
            project
        };
        let current = project.as_ref().map(|_| 0);
        let mut app = ui::Application::new();
        let (handle, entity) = app.open_raw_window(
            ui::WindowOptions {
                width: lw,
                height: lh,
                scale,
                ..Default::default()
            },
            move |_| {
                // With DIFFFILE=<file> (and DIFFBASE=<file with its old text>), show that file's diff;
                // SPLITDIFF=1 opens it side by side.
                if std::env::var("SPLITDIFF").is_ok() {
                    let font_changed = files_ui::set_editor_defaults(
                        files_ui::default_buffer_font_size(),
                        false,
                        true,
                    );
                    if font_changed {
                        eprintln!("snapshot: buffer font reset to its default");
                    }
                }
                if std::env::var("EDITORBOARD").is_ok_and(|board| board.starts_with("vim")) {
                    files_ui::set_vim_mode(true);
                }
                let files_view = std::env::var("DIFFFILE").ok().map(|file| {
                    let path = std::path::PathBuf::from(&file);
                    let root = path
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_default();
                    let mut files = files_ui::FilesView::local(root);
                    let base = std::env::var("DIFFBASE")
                        .ok()
                        .and_then(|base| std::fs::read_to_string(base).ok());
                    workspace::FunctionView::open_diff(&mut files, &path, base);
                    Box::new(files) as Box<dyn workspace::FunctionView>
                });
                // OPENFILES=<a,b,...>: open each file as a tab (the tab strip scrolls once they overflow).
                let files_view = files_view.or_else(|| {
                    let list = std::env::var("OPENFILES").ok()?;
                    let paths: Vec<std::path::PathBuf> =
                        list.split(',').map(std::path::PathBuf::from).collect();
                    let root = match std::env::var("FILESROOT") {
                        Ok(root) => std::path::PathBuf::from(root),
                        Err(_) => paths.first()?.parent()?.to_path_buf(),
                    };
                    let mut files = files_ui::FilesView::local(root)
                        .read_only(std::env::var("READONLY").is_ok());
                    for path in &paths {
                        workspace::FunctionView::open_file_at(&mut files, path, None, None);
                    }
                    // The window scenes show the tree opened down to the file being edited.
                    if std::env::var("FILESROOT").is_ok() {
                        if let Some(last) = paths.last() {
                            workspace::FunctionView::reveal_in_tree(&mut files, last);
                        }
                    }
                    if let Some((line, column, lines)) =
                        std::env::var("SELECT").ok().and_then(|spec| {
                            let parts: Vec<usize> = spec
                                .split(',')
                                .filter_map(|part| part.parse().ok())
                                .collect();
                            Some((*parts.first()?, *parts.get(1)?, *parts.get(2)?))
                        })
                    {
                        use workspace::ItemInput;
                        for _ in 0..line {
                            files.editor_key(workspace::EditKey::Down, false);
                        }
                        for _ in 0..column {
                            files.editor_key(workspace::EditKey::Right, false);
                        }
                        for _ in 0..lines {
                            files.editor_key(workspace::EditKey::Down, true);
                        }
                    }
                    // MINIMAP=1: turn the minimap on through the first editor's Editor Controls menu.
                    if std::env::var("MINIMAP").is_ok() {
                        use workspace::FunctionView;
                        files.on_click((1 << 51) + 3);
                        if let Some(menu) = files.take_menu_request() {
                            if let Some(entry) = menu.first() {
                                files.menu_pick(entry.id);
                            }
                        }
                    }
                    // MDPREVIEW=1: preview the last (markdown) file beside its editor.
                    if std::env::var("MDPREVIEW").is_ok() {
                        workspace::ItemInput::editor_key(
                            &mut files,
                            workspace::EditKey::OpenMarkdownPreviewToTheSide,
                            false,
                        );
                    }
                    Some(Box::new(files) as Box<dyn workspace::FunctionView>)
                });
                let mut layout = workspace::Layout {
                    project,
                    files_view,
                    ..Default::default()
                };
                if window {
                    layout.left.width = 256.0;
                    layout.right.collapsed = std::env::var_os("EDITORBOARD").is_some();
                    layout.right.width = 380.0;
                    layout.bottom.collapsed = true;
                    layout.usage = window_usage();
                }
                if let Ok(shell) = std::env::var("TERMSHELL") {
                    terminal_ui::set_terminal_defaults(15.0, &shell, 10_000);
                    layout.terminal_view = Some(Box::new(terminal_ui::TerminalPanel::new(
                        fixture_root(),
                        std::sync::Arc::new(ui::wake),
                    )));
                    layout.bottom.width = 360.0;
                }
                let mut view = workspace::WorkspaceView::new(layout);
                view.set_sessions(sessions, current);
                if window {
                    view.set_bindings(vec![(
                        workspace::keymap::Action::NextWorkspaceNeedingAttention,
                        "cmd-alt-a".into(),
                    )]);
                    view.set_agent_states(
                        [
                            ("proj-101-login-google", workspace::AgentDot::ToolUse),
                            ("api-102-rate-limit", workspace::AgentDot::AwaitingInput),
                            ("investigate-0917-5fwg", workspace::AgentDot::Thinking),
                        ]
                        .into_iter()
                        .map(|(branch, dot)| (branch.to_string(), dot))
                        .collect(),
                    );
                }
                if sidebar {
                    view.set_agent_states(
                        [
                            ("main", workspace::AgentDot::ToolUse),
                            (
                                "proj-101-email-open-tracking",
                                workspace::AgentDot::Thinking,
                            ),
                            (
                                "proj-103-flag-escalated",
                                workspace::AgentDot::AwaitingInput,
                            ),
                            (
                                "proj-10432-migrate-billing-webhooks",
                                workspace::AgentDot::AwaitingInput,
                            ),
                        ]
                        .into_iter()
                        .map(|(branch, dot)| (branch.to_string(), dot))
                        .collect(),
                    );
                }
                view
            },
        );
        if mode == "problem" {
            entity.update(app.app_mut(), |view, _| {
                view.set_config_problem(
                    Some((
                        "/projects/myproject/pom.yml: unmarshal errors:\n  line 4: cannot unmarshal !!str `npm i` into []string".into(),
                        Some(4),
                    )),
                    std::path::Path::new("/projects/myproject/pom.yml"),
                )
            });
        }
        // wscreate / wsrename: the workspace forms; wsops: creation cards (one running, one failed).
        let namer: workspaces_ui::Namer = std::sync::Arc::new(|seed: &str, _: &str| {
            Ok(pom_agent::NameSuggestion {
                name: seed.to_string(),
                slug: seed.to_string(),
            })
        });
        if mode == "wscreate" || mode == "wsticket" {
            let mut modal = workspaces_ui::CreateWorkspaceModal::new(
                vec![
                    "api".into(),
                    "web".into(),
                    "notifications-service-worker".into(),
                ],
                vec!["main".into(), "feat-login".into()],
                namer.clone(),
            )
            // ENVS=n: that many environment profiles.
            .with_options(
                match std::env::var("ENVS")
                    .ok()
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    Some(count) => (1..=count)
                        .map(|index| format!("staging-{index}"))
                        .collect(),
                    None => vec!["staging".into()],
                },
                vec!["api".into()],
            )
            .with_modules(std::sync::Arc::new(|repo: &str| match repo {
                "api" => module_store::Outlook::FromStore,
                "web" => module_store::Outlook::InstallsOnce,
                _ => module_store::Outlook::SelfManaged("pnpm"),
            }));
            if mode == "wsticket" {
                let issue =
                    |key: &str, summary: &str, status: &str, mine: bool| pom_jira::SprintIssue {
                        key: key.into(),
                        summary: summary.into(),
                        status: status.into(),
                        category: if status == "In Progress" {
                            "indeterminate".into()
                        } else {
                            "new".into()
                        },
                        mine,
                        ..pom_jira::SprintIssue::default()
                    };
                let issues = vec![
                    issue(
                        "PROJ-101",
                        "Payments page crashes on submit",
                        "In Progress",
                        true,
                    ),
                    issue("PROJ-104", "Add CSV export to reports", "To Do", false),
                    issue(
                        "PROJ-97",
                        "Login redirect loses the query string",
                        "To Do",
                        false,
                    ),
                ];
                modal = modal.with_tickets(workspaces_ui::TicketSource {
                    boards: std::sync::Arc::new(|| {
                        Ok(vec![
                            pom_jira::Board {
                                id: 1,
                                name: "Team board".into(),
                            },
                            pom_jira::Board {
                                id: 2,
                                name: "Platform board".into(),
                            },
                        ])
                    }),
                    // TICKETSLOW=1: Jira never answers, so the list shows its loading rows.
                    issues: std::sync::Arc::new(move |_| {
                        if std::env::var_os("TICKETSLOW").is_some() {
                            std::thread::sleep(std::time::Duration::from_secs(60));
                        }
                        Ok(issues.clone())
                    }),
                    board: None,
                    start: None,
                    remember: std::sync::Arc::new(|_| {}),
                    only_mine: false,
                });
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while workspace::WindowModal::busy(&modal) && std::time::Instant::now() < deadline {
                    workspace::WindowModal::tick(&mut modal);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if std::env::var_os("BOARDMENU").is_some() {
                    workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 8);
                }
            } else {
                workspace::WindowModal::text(
                    &mut modal,
                    &std::env::var("SNAPNAME").unwrap_or_else(|_| "Fix checkout page".into()),
                );
                workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 101);
            }
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        // wsbranches: the create form with web on a taken-over branch and the api picker open (PICKROW=2 opens
        // the last row's, which flips above near the bottom).
        if mode == "wsbranches" {
            let info = |name: &str, remote: bool, author: &str, when: &str, subject: &str| {
                pom_workspace::BranchInfo {
                    name: name.into(),
                    remote,
                    author: author.into(),
                    relative_time: when.into(),
                    subject: subject.into(),
                }
            };
            let before = vec![
                info("main", false, "you", "1 day ago", "Merge PROJ-98"),
                info(
                    "ana/checkout-api",
                    true,
                    "Ana Lima",
                    "2 hours ago",
                    "Validate cart totals",
                ),
                info(
                    "ana/checkout-ui",
                    true,
                    "Ana Lima",
                    "40 minutes ago",
                    "New checkout summary card",
                ),
                info(
                    "ben/payments",
                    true,
                    "Ben Okafor",
                    "3 days ago",
                    "Stripe webhook retries with a much longer subject line",
                ),
            ];
            let mut after = before.clone();
            after.insert(
                1,
                info(
                    "ben/checkout-copy",
                    true,
                    "Ben Okafor",
                    "10 minutes ago",
                    "Checkout copy tweaks",
                ),
            );
            let fetched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let listed = fetched.clone();
            let source = workspaces_ui::BranchSource {
                list: std::sync::Arc::new(move |_| {
                    let branches = if listed.load(std::sync::atomic::Ordering::SeqCst) {
                        after.clone()
                    } else {
                        before.clone()
                    };
                    Ok(workspaces_ui::RepoBranches {
                        base: "main".into(),
                        branches,
                    })
                }),
                fetch: std::sync::Arc::new(move |_| {
                    fetched.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }),
            };
            let mut modal = workspaces_ui::CreateWorkspaceModal::new(
                vec!["api".into(), "web".into(), "mobile".into()],
                vec!["main".into()],
                namer.clone(),
            )
            .with_branches(source);
            let settle = |modal: &mut workspaces_ui::CreateWorkspaceModal| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while workspace::WindowModal::busy(modal) && std::time::Instant::now() < deadline {
                    workspace::WindowModal::tick(modal);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            };
            workspace::WindowModal::text(
                &mut modal,
                &std::env::var("SNAPNAME").unwrap_or_else(|_| "Fix checkout page".into()),
            );
            settle(&mut modal);
            let branch_box = workspace::WINDOW_MODAL_BASE + 300;
            workspace::WindowModal::click(&mut modal, branch_box + 1);
            workspace::WindowModal::text(&mut modal, "checkout-ui");
            workspace::WindowModal::key(&mut modal, workspace::EditKey::Enter, false);
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 102);
            settle(&mut modal);
            let row: u64 = std::env::var("PICKROW")
                .ok()
                .and_then(|row| row.parse().ok())
                .unwrap_or(0);
            workspace::WindowModal::click(&mut modal, branch_box + row);
            settle(&mut modal);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        // usebranch: an existing workspace's worker picking another branch, after a fetch brought one in.
        if mode == "usebranch" {
            let info = |name: &str, remote: bool, author: &str, when: &str, subject: &str| {
                pom_workspace::BranchInfo {
                    name: name.into(),
                    remote,
                    author: author.into(),
                    relative_time: when.into(),
                    subject: subject.into(),
                }
            };
            let before = vec![
                info(
                    "fix-checkout-page",
                    false,
                    "you",
                    "5 minutes ago",
                    "Checkout page layout",
                ),
                info("main", false, "you", "1 day ago", "Merge PROJ-98"),
                info(
                    "ana/mail-retry",
                    true,
                    "Ana Lima",
                    "2 hours ago",
                    "Retry failed mail with a much longer subject line than fits",
                ),
            ];
            let mut after = before.clone();
            after.push(info(
                "ben/queue-metrics",
                true,
                "Ben Okafor",
                "10 minutes ago",
                "Queue depth metrics",
            ));
            let fetched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let listed = fetched.clone();
            let source = workspaces_ui::BranchSource {
                list: std::sync::Arc::new(move |_| {
                    let branches = if listed.load(std::sync::atomic::Ordering::SeqCst) {
                        after.clone()
                    } else {
                        before.clone()
                    };
                    Ok(workspaces_ui::RepoBranches {
                        base: "fix-checkout-page".into(),
                        branches,
                    })
                }),
                fetch: std::sync::Arc::new(move |_| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    fetched.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }),
            };
            let mut modal = workspaces_ui::UseBranchModal::new(
                "worker",
                "fix-checkout-page",
                Some("fix-checkout-page"),
                source,
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while workspace::WindowModal::busy(&modal) && std::time::Instant::now() < deadline {
                workspace::WindowModal::tick(&mut modal);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            workspace::WindowModal::key(&mut modal, workspace::EditKey::Down, false);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode.starts_with("usage") {
            let usage = workspace::UsageInfo {
                account: Some(workspace::UsageAccount {
                    name: "dev".into(),
                    email: "dev@example.com".into(),
                    plan: "team".into(),
                    organization: "Example".into(),
                }),
                session: Some(workspace::UsageWindow {
                    used: 23.0,
                    resets: "in 2h 31m".into(),
                }),
                weekly: Some(workspace::UsageWindow {
                    used: 74.0,
                    resets: "Sat 17:00".into(),
                }),
                note: "Updated 12s ago - checks every minute".into(),
                today: Some(workspace::UsageToday {
                    total: "$17.25".into(),
                    sessions: 4,
                    by_workspace: vec![
                        ("Login page".into(), "$11.02".into()),
                        ("main".into(), "$6.23".into()),
                    ],
                }),
                hidden: false,
            };
            let (w, h) = (lw * 2.0, lh * 2.0);
            entity.update(app.app_mut(), |view, _| {
                view.set_usage(usage);
                match mode.as_str() {
                    "usagecard" => view.show_usage_card(
                        false,
                        ui::Rect::new(w - 300.0, 14.0, 250.0, 48.0, ui::Rgba::TRANSPARENT),
                    ),
                    "usagetoday" => view.show_usage_card(
                        true,
                        ui::Rect::new(w - 420.0, h - 44.0, 200.0, 40.0, ui::Rgba::TRANSPARENT),
                    ),
                    "usagemenu" => view.show_app_menu(ui::Rect::new(
                        w - 68.0,
                        14.0,
                        48.0,
                        48.0,
                        ui::Rgba::TRANSPARENT,
                    )),
                    _ => {}
                }
            });
        }
        if mode == "newproject" || mode == "onboard" {
            let which = std::env::var("ONBOARDPAGE").unwrap_or_else(|_| "repos".into());
            let page = onboarding_ui::preview_page(&which);
            entity.update(app.app_mut(), |view, _| view.open_page(Box::new(page)));
        }
        if mode == "exportconfig" {
            let mut modal = workspaces_ui::ExportConfigModal::new(3);
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 4);
            workspace::WindowModal::text(&mut modal, "secret");
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode == "importconfig" {
            let file = std::env::temp_dir().join("pom-snapshot-import.yml");
            let yaml = "session: myproject\ndefault_branch: main\nrepos:\n  api:\n    alias: be\n    services:\n      server:\n        type: backend\n        cmd: bundle exec rails s -p $PORT\n        env:\n          DATABASE_URL: \"{{db.main.url}}\"\n          API_KEY: \"{{secret.API_KEY}}\"\n  web:\n    services:\n      app:\n        cmd: pnpm dev --port $PORT\n        env:\n          API_URL: \"{{be.server.url}}\"\n";
            if let Err(error) = std::fs::write(&file, yaml) {
                eprintln!("snapshot: {error}");
            }
            let mut modal =
                workspaces_ui::ImportConfigModal::new(Box::new(move || Some(file.clone())));
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 6);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        // RAIL=1: the WORKSPACES panel folded to its rail.
        if std::env::var("RAIL").is_ok() {
            entity.update(app.app_mut(), |view, _| {
                view.run_action(workspace::keymap::Action::ToggleLeftDock)
            });
        }
        if mode == "prompt" {
            entity.update(app.app_mut(), |view, _| {
                view.ask(workspace::Prompt {
                    token: 1,
                    message: "Stop all shared services?".into(),
                    detail: Some(
                        "Shared services are used by all workspaces; 12 services in other workspaces are still running."
                            .into(),
                    ),
                    buttons: vec!["Stop".into(), "Cancel".into()],
                    warning: None,
                })
            });
        }
        if mode == "wsrename" {
            let modal = workspaces_ui::RenameWorkspaceModal::new("feat-login", "Login page", namer);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        // The window boards group the list by ticket status and show one workspace being created.
        if window {
            use workspace::{OpStatus, StageState, WorkspaceOp};
            let stages = [
                "Validating config and hosts",
                "Provisioning workspace",
                "Creating git worktrees",
                "Configuring repos",
                "Running setup commands",
                "Seeding databases",
                "Starting services",
                "Saving the baseline snapshot",
            ];
            let op = WorkspaceOp {
                id: 1,
                branch: "proj-118-export-audit-log".into(),
                title: "PROJ-118 Export audit log".into(),
                status: OpStatus::Running,
                stages: stages
                    .iter()
                    .enumerate()
                    .map(|(at, name)| {
                        let state = match at {
                            0..=3 => StageState::Done,
                            4 => StageState::Running,
                            _ => StageState::Pending,
                        };
                        (name.to_string(), state)
                    })
                    .collect(),
                retryable: true,
                ..Default::default()
            };
            let folded: Vec<String> = std::env::var("FOLDED")
                .map(|keys| keys.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            entity.update(app.app_mut(), |view, _| {
                view.set_workspace_grouping(Some(workspace::Grouping::from_keys(&[], &folded)));
                view.set_workspace_ops(vec![op]);
            });
        }
        // MAINVIEW=sidebar GROUPED=1: the list grouped by ticket status, Done folded.
        if sidebar && std::env::var("GROUPED").is_ok() {
            entity.update(app.app_mut(), |view, _| {
                view.set_workspace_grouping(Some(workspace::Grouping::from_keys(
                    &[],
                    &["done".to_string()],
                )))
            });
        }
        // MAINVIEW=sidebar OPS=1: main's update failed (opened), one creation running, one queued, one failed.
        if sidebar && std::env::var("OPS").is_ok() {
            use workspace::{OpStatus, StageState, WorkspaceOp};
            let op = |id: u64,
                      branch: &str,
                      title: &str,
                      status: OpStatus,
                      stages: Vec<(&str, StageState)>,
                      error: &str,
                      quiet: bool| WorkspaceOp {
                id,
                branch: branch.into(),
                title: title.into(),
                status,
                stages: stages
                    .into_iter()
                    .map(|(name, state)| (name.to_string(), state))
                    .collect(),
                detail: String::new(),
                error: error.into(),
                retryable: true,
                quiet,
                ..Default::default()
            };
            use StageState::{Done, Failed, Pending, Running, Skipped};
            let ops = vec![
                op(
                    1,
                    "",
                    "Updating main",
                    OpStatus::Failed,
                    vec![("api", Failed), ("web", Done), ("worker", Skipped)],
                    "api: Your local changes to app/models/user.rb would be overwritten by merge",
                    true,
                ),
                op(
                    2,
                    "dark-mode",
                    "Dark mode",
                    OpStatus::Running,
                    vec![
                        ("worktrees", Done),
                        ("ports", Done),
                        ("databases", Running),
                        ("env files", Pending),
                        ("setup", Pending),
                    ],
                    "",
                    false,
                ),
                op(
                    3,
                    "proj-111-export-orders",
                    "PROJ-111 Export orders to CSV",
                    OpStatus::Queued,
                    Vec::new(),
                    "",
                    false,
                ),
                op(
                    4,
                    "proj-112-checkout-redesign",
                    "PROJ-112 Checkout redesign",
                    OpStatus::Failed,
                    vec![
                        ("validate", Done),
                        ("provision", Done),
                        ("databases", Failed),
                        ("worktrees", Pending),
                        ("configure", Pending),
                        ("setup", Pending),
                    ],
                    "shared services: docker: Cannot connect to the Docker daemon",
                    false,
                ),
            ];
            let mut ops = ops;
            ops[0].log = "api: Your local changes to app/models/user.rb would be overwritten by merge\n\
                          error: Your local changes to the following files would be overwritten by merge:\n\
                          \tapp/models/user.rb\n\
                          Please commit your changes or stash them before you merge."
                .into();
            ops[0].fix_dir = "/Users/me/myproject/main/api".into();
            ops[3].log = "shared services: docker: Cannot connect to the Docker daemon at \
                          unix:///var/run/docker.sock. Is the docker daemon running?"
                .into();
            ops[3].skip = "no shared services or databases until they start".into();
            entity.update(app.app_mut(), |view, _| {
                view.set_workspace_ops(ops);
                view.toggle_workspace_op(4);
                if std::env::var("MAINOPEN").is_ok() {
                    view.toggle_workspace_op(1);
                }
                if std::env::var("MANUAL").is_ok() {
                    view.toggle_manual_fixes(4);
                }
                if std::env::var("POPOVER").is_ok() {
                    view.toggle_failure_popover(1);
                    view.toggle_manual_fixes(1);
                }
            });
        }
        if mode == "wsops" {
            use workspace::{OpStatus, StageState, WorkspaceOp};
            let stages = |states: [StageState; 7]| -> Vec<(String, StageState)> {
                [
                    "Validating config and hosts",
                    "Provisioning workspace",
                    "Starting shared services and databases",
                    "Creating git worktrees (parallel)",
                    "Configuring repos (parallel)",
                    "Running setup commands (parallel)",
                    "Seeding databases (parallel)",
                ]
                .iter()
                .zip(states)
                .map(|(label, state)| (label.to_string(), state))
                .collect()
            };
            use StageState::{Done, Failed, Pending, Running};
            let ops = vec![
                WorkspaceOp {
                    id: 1,
                    branch: "fix-checkout".into(),
                    title: "Fix checkout page".into(),
                    status: OpStatus::Running,
                    stages: stages([Done, Done, Done, Running, Pending, Pending, Pending]),
                    detail: "worktree: web".into(),
                    error: String::new(),
                    retryable: true,
                    quiet: false,
                    ..Default::default()
                },
                WorkspaceOp {
                    id: 2,
                    branch: "proj-101".into(),
                    title: "PROJ-101 Payments".into(),
                    status: OpStatus::Failed,
                    stages: stages([Done, Done, Failed, Pending, Pending, Pending, Pending]),
                    detail: String::new(),
                    error: "shared services: docker: No such file or directory".into(),
                    retryable: true,
                    quiet: false,
                    ..Default::default()
                },
            ];
            entity.update(app.app_mut(), |view, _| {
                view.set_workspace_ops(ops);
                view.toggle_workspace_op(1);
            });
        }
        if let Ok(which) = std::env::var("UPDATESTATE") {
            let snapshot = update_snapshot(&which);
            entity.update(app.app_mut(), |view, _| {
                view.set_update(auto_update_ui::update_info(&snapshot));
                if which == "updated" {
                    let (title, message, button) = auto_update_ui::updated_notification("0.7.4");
                    view.notify_release_notes(title, message, button);
                }
            });
            if std::env::var("UPDATEHOVER").is_ok() {
                app.draw(handle).expect("frame");
                if let Some((x, y)) = app
                    .window(handle)
                    .and_then(|window| window.center_of(workspace::UPDATE_BUTTON))
                {
                    entity.update(app.app_mut(), |view, _| view.mouse_move(x, y));
                }
            }
        }
        if let Ok(board) = std::env::var("EDITORBOARD") {
            for step in editor_board_steps(&board) {
                app.draw(handle).expect("frame");
                entity.update(app.app_mut(), |view, _| step(view));
            }
        }
        // Background work (the diff's hunks, a large file's load) settles over a few frames; SETTLE=<frames>.
        let settle = std::env::var("SETTLE")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(40);
        for _ in 0..settle {
            app.draw(handle).expect("frame");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // UNDOBENCH=<n>: type n characters, then undo them one frame each, timing the undo frames.
        if let Some(count) = std::env::var("UNDOBENCH")
            .ok()
            .and_then(|n| n.parse::<u32>().ok())
        {
            for _ in 0..count {
                entity.update(app.app_mut(), |view, _| view.editor_text("x"));
                app.draw(handle).expect("frame");
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            let (mut layout_total, mut render_total, mut worst) = (
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            );
            for _ in 0..count {
                let started = std::time::Instant::now();
                entity.update(app.app_mut(), |view, _| {
                    view.editor_key(workspace::EditKey::Undo, false)
                });
                let frame = app.draw(handle).expect("frame");
                let laid_out = started.elapsed();
                let mut layers: Vec<ui::Layer> = vec![(
                    frame.base.rects.as_slice(),
                    frame.base.tris.as_slice(),
                    frame.base.texts.as_slice(),
                    frame.base.icons.as_slice(),
                    None,
                )];
                for overlay in &frame.overlays {
                    layers.push((
                        overlay.painted.rects.as_slice(),
                        overlay.painted.tris.as_slice(),
                        overlay.painted.texts.as_slice(),
                        overlay.painted.icons.as_slice(),
                        overlay.clip.map(|c| (c.x, c.y, c.w, c.h)),
                    ));
                }
                let rendering = std::time::Instant::now();
                r.render_frame(ui::theme().background, &layers)?;
                layout_total += laid_out;
                render_total += rendering.elapsed();
                worst = worst.max(started.elapsed());
                if std::env::var("BENCHEACH").is_ok() {
                    println!("  undo {laid_out:?} + {:?}", rendering.elapsed());
                }
            }
            println!(
                "undo frames: layout {:?}/frame, render {:?}/frame, worst {worst:?}",
                layout_total / count,
                render_total / count
            );
        }
        // DIAGNOSTICS=1: open the project diagnostics tab once the servers had time to report.
        if std::env::var("DIAGNOSTICS").is_ok() {
            entity.update(app.app_mut(), |view, _| {
                view.deploy_project_diagnostics(true)
            });
            for _ in 0..20 {
                app.draw(handle).expect("frame");
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        // DRAGBENCH=<x>: press the editor at x (a scrollbar or the minimap) and drag it down, timing each frame.
        if let Some(bench_x) = std::env::var("DRAGBENCH")
            .ok()
            .and_then(|x| x.parse::<f32>().ok())
        {
            entity.update(app.app_mut(), |view, _| view.mouse_down(bench_x, 200.0));
            let (mut layout_total, mut render_total) =
                (std::time::Duration::ZERO, std::time::Duration::ZERO);
            let frames = 60;
            for step in 0..frames {
                let still = std::env::var("BENCHSTILL").is_ok();
                entity.update(app.app_mut(), |view, _| {
                    view.mouse_move(bench_x, 200.0 + if still { 0.0 } else { step as f32 * 8.0 });
                });
                let started = std::time::Instant::now();
                let frame = app.draw(handle).expect("frame");
                let laid_out = started.elapsed();
                let mut layers: Vec<ui::Layer> = vec![(
                    frame.base.rects.as_slice(),
                    frame.base.tris.as_slice(),
                    frame.base.texts.as_slice(),
                    frame.base.icons.as_slice(),
                    None,
                )];
                for overlay in &frame.overlays {
                    layers.push((
                        overlay.painted.rects.as_slice(),
                        overlay.painted.tris.as_slice(),
                        overlay.painted.texts.as_slice(),
                        overlay.painted.icons.as_slice(),
                        overlay.clip.map(|c| (c.x, c.y, c.w, c.h)),
                    ));
                }
                let started = std::time::Instant::now();
                r.render_frame(ui::theme().background, &layers)?;
                layout_total += laid_out;
                render_total += started.elapsed();
            }
            entity.update(app.app_mut(), |view, _| view.mouse_up());
            println!(
                "drag frames: layout {:?}/frame, render {:?}/frame",
                layout_total / frames,
                render_total / frames
            );
        }
        let frame = app.draw(handle).expect("frame");
        // The OS draws the window buttons; a scene stands in for them where the app pins them.
        let lights: Vec<ui::Rect> = if scene_size.is_some() {
            scene_lights()
        } else {
            Vec::new()
        };
        let mut layers: Vec<ui::Layer> = vec![(
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        )];
        for overlay in &frame.overlays {
            layers.push((
                overlay.painted.rects.as_slice(),
                overlay.painted.tris.as_slice(),
                overlay.painted.texts.as_slice(),
                overlay.painted.icons.as_slice(),
                overlay.clip.map(|c| (c.x, c.y, c.w, c.h)),
            ));
        }
        layers.push((lights.as_slice(), &[], &[], &[], None));
        r.render_frame(ui::theme().background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let (w, h, rgba) = match crop {
            Some(rect) => crop_rgba(w, &rgba, rect),
            None => (w, h, rgba),
        };
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    // With MAINMENU=1, render the main window with the header session switcher menu open.
    if std::env::var("MAINMENU").is_ok() {
        let mut layout = workspace::Layout {
            session_menu: true,
            ..Default::default()
        };
        if let Ok(s) = std::env::var("MENUSCROLL")
            .unwrap_or_default()
            .parse::<f32>()
        {
            layout.session_scroll = s;
        }
        let (mut rects, mut texts) = layout.build(lw, lh);
        let mut tris: Vec<ui::Tri> = Vec::new();
        let hover = std::env::var("MENUHOVER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let header = layout.header(lw, None);
        rects.extend(header.rects);
        tris.extend(header.tris);
        texts.extend(header.texts);
        let menu = layout.session_menu("", hover, true);
        let tip = hover.and_then(|hv| {
            let text = workspace::session_action_tooltip(hv)?;
            let rect = menu
                .list
                .hits
                .iter()
                .find(|(_, id)| *id == hv)
                .map(|(r, _)| *r)?;
            Some(workspace::tooltip(rect, text, lw))
        });
        let mut layers: Vec<ui::Layer> = vec![
            (
                rects.as_slice(),
                tris.as_slice(),
                texts.as_slice(),
                &[],
                None,
            ),
            (
                menu.fixed.rects.as_slice(),
                menu.fixed.tris.as_slice(),
                menu.fixed.texts.as_slice(),
                &[],
                None,
            ),
            (
                menu.list.rects.as_slice(),
                menu.list.tris.as_slice(),
                menu.list.texts.as_slice(),
                &[],
                Some(menu.clip),
            ),
        ];
        if let Some(t) = &tip {
            layers.push((
                t.rects.as_slice(),
                t.tris.as_slice(),
                t.texts.as_slice(),
                &[],
                None,
            ));
        }
        r.render_frame(ui::Rgba::new(0.0, 0.0, 0.0, 1.0), &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    let mut settings = settings::Settings::default();
    if let Ok(fs) = std::env::var("FONTSIZE").unwrap_or_default().parse::<f32>() {
        settings.ui_font_size = fs;
        ui::set_ui_text_scale(fs / ui::UI_FONT_BASE);
    }
    // THEMEFILE=path: a user theme family file, loaded before THEME picks a name from it.
    if let Ok(path) = std::env::var("THEMEFILE") {
        let text = std::fs::read_to_string(&path)?;
        let (themes, problems) = ui::theme_file::parse_family(&text, &path);
        ui::theme_file::set_user_themes(themes);
        ui::theme_file::set_theme_problems(problems);
    }
    if let Ok(mode) = std::env::var("THEMESELECTION") {
        settings.theme_selection = mode;
    }
    if let Ok(t) = std::env::var("THEME") {
        if t == "light" {
            ui::set_theme(ui::one_light());
        } else {
            settings.theme = t.clone();
            let colors = ui::by_name(&t);
            // Dynamic mode reads the slot for the theme's own appearance.
            match colors.appearance {
                ui::Appearance::Dark => settings.theme_dark = t.clone(),
                ui::Appearance::Light => settings.theme_light = t.clone(),
            }
            ui::set_theme(colors);
        }
    }
    if let Ok(f) = std::env::var("UIFONT") {
        settings.ui_font = f;
    }
    if let Ok(w) = std::env::var("UIWEIGHT").unwrap_or_default().parse::<u16>() {
        settings.ui_font_weight = w as f32;
        ui::set_ui_font_weight(w);
    }
    r.set_ui_font(&settings.ui_font);
    // EXPANDED=<index>: only that category open in the sidebar, as a fresh window shows it.
    let expanded: Vec<bool> = match std::env::var("EXPANDED")
        .ok()
        .and_then(|index| index.parse::<usize>().ok())
    {
        Some(open) => (0..settings_ui::CATEGORY_COUNT)
            .map(|index| index == open)
            .collect(),
        None => vec![true; settings_ui::CATEGORY_COUNT],
    };
    // JIRA=1: the Integrations page as a configured project sees it; REMOTE=1: opened from the window of a
    // project on the host `dev`, whose settings are kept there.
    let remote = std::env::var("REMOTE").is_ok();
    let jira = if std::env::var("JIRA").is_ok() {
        settings_ui::IntegrationsPage {
            session: "myproject".into(),
            kept_on: remote.then(|| {
                (
                    "dev".to_string(),
                    "Jira is read on dev with these settings; the token never leaves it."
                        .to_string(),
                )
            }),
            keep_main_fresh: true,
            refresh_minutes: 30,
            site: "https://acme.atlassian.net".into(),
            email: "you@example.com".into(),
            token: settings_ui::TokenSource::Secret,
            status: settings_ui::ConnectionStatus::SignedIn("Sam (you@example.com)".into()),
        }
    } else {
        settings_ui::IntegrationsPage::default()
    };
    // LIVE=1: the Agent and Network pages with registration done and some proxy traffic.
    let live = std::env::var("LIVE").is_ok();
    let state = settings_ui::PageState {
        jira,
        project: settings_ui::ProjectPage {
            session: "myproject".into(),
            repos: vec![
                settings_ui::RepoSummary {
                    name: "api".into(),
                    alias: "be".into(),
                    services: 2,
                    cloned: true,
                    present: 3,
                    workspaces: 4,
                },
                settings_ui::RepoSummary {
                    name: "web".into(),
                    alias: "web".into(),
                    services: 1,
                    cloned: false,
                    present: 0,
                    workspaces: 4,
                },
            ],
            host: remote.then(|| "dev".to_string()),
        },
        agent: if live {
            settings_ui::AgentPage {
                mcp: settings_ui::Registration::Done,
                hooks: settings_ui::Registration::Done,
            }
        } else {
            settings_ui::AgentPage::default()
        },
        dev_services: settings_ui::DevServicesPage {
            proxy_running: live,
            webhook_running: live,
            proxy_port: 8767,
            webhook_port: 8766,
            service_url: "http://<service>.<repo>.<ticket or branch>.localhost:8767".into(),
            ..settings_ui::DevServicesPage::default()
        },
        general: {
            let row = std::env::var("UPDATESTATE").ok().map(|which| {
                auto_update_ui::settings_row(
                    &update_snapshot(&which),
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12 * 60),
                )
            });
            settings_ui::GeneralPage {
                start_at_login: false,
                version: env!("CARGO_PKG_VERSION").into(),
                updates_apply: live || row.is_some(),
                update_note: row
                    .as_ref()
                    .map(|row| row.note.clone())
                    .or_else(|| std::env::var("UPDATENOTE").ok()),
                update_button: row.map(|row| (row.button, row.enabled)),
            }
        },
        keymap: settings_ui::KeymapPage {
            rows: workspace::keymap::Action::ALL
                .iter()
                .map(|action| {
                    (
                        action.label().to_string(),
                        action.name().to_string(),
                        workspace::keymap::Keymap::defaults()
                            .binding_for(*action)
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            problems: Vec::new(),
        },
        // LANGUAGE=<index>: that language's page over Languages & Tools.
        language: std::env::var("LANGUAGE")
            .ok()
            .and_then(|index| index.parse().ok()),
    };
    let fs_edit = std::env::var("FSEDIT").ok();
    let editing = fs_edit
        .as_deref()
        .map(|b| (settings_ui::CTRL_FONT_SIZE_EDIT, b));
    let search = std::env::var("SETTINGSSEARCH")
        .or_else(|_| std::env::var("SEARCH"))
        .unwrap_or_default();

    // With SCROLL=<px>, exercise the scissor-clipped scrolling page path.
    if let Ok(scroll) = std::env::var("SCROLL").unwrap_or_default().parse::<f32>() {
        let clip = settings_ui::content_region(lw, lh);
        let (page, total_h) =
            settings_ui::page(category, &settings, &state, None, &search, lw, lh, scroll);
        let parts = settings_ui::chrome(
            category,
            None,
            Some(0),
            &expanded,
            lw,
            lh,
            &search,
            false,
            0.0,
            &state.project,
            state.language,
        );
        let mut chrome = parts.fixed;
        chrome.rects.extend(parts.nav.rects);
        chrome.texts.extend(parts.nav.texts);
        if let Some(bar) = settings_ui::content_scrollbar(clip, total_h, scroll) {
            chrome.rects.push(bar);
        }
        r.render_layered_clip(
            (&page.rects, &page.tris, &page.texts),
            (&chrome.rects, &chrome.tris, &chrome.texts),
            Some(clip),
            None,
            ui::theme().editor_background,
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }
    let mut base = settings_ui::panel(
        category,
        None,
        &expanded,
        &settings,
        &state,
        lw,
        lh,
        editing,
        &search,
        !search.is_empty(),
    );
    if scene_size.is_some() {
        for (index, (red, green, blue)) in [
            (0.996, 0.373, 0.341),
            (0.996, 0.737, 0.18),
            (0.157, 0.784, 0.251),
        ]
        .iter()
        .enumerate()
        {
            base.rects.push(ui::Rect {
                radius: 6.0,
                ..ui::Rect::new(
                    20.0 + index as f32 * 20.0,
                    14.0,
                    12.0,
                    12.0,
                    ui::Rgba::new(*red, *green, *blue, 1.0),
                )
            });
        }
    }

    // With POPOVER=1, render an open Theme dropdown to verify the floating overlay.
    // With FONTPOP=1, render the (long, scrolled) font-family dropdown to verify the scrollbar.
    if std::env::var("POPOVER").is_ok() || std::env::var("FONTPOP").is_ok() {
        let font_pop = std::env::var("FONTPOP").is_ok();
        let cid = if font_pop {
            settings_ui::CTRL_FONT_FAMILY
        } else {
            settings_ui::CTRL_THEME
        };
        let anchor = base
            .hits
            .iter()
            .find(|(_, id)| *id == cid)
            .map(|(r, _)| *r)
            .expect("control anchor");
        let fonts = if font_pop { r.font_families() } else { vec![] };
        let items = settings_ui::control_items(cid, &fonts);
        let query = std::env::var("QUERY").unwrap_or_default();
        let (current, scroll) = if font_pop {
            (
                settings.ui_font.clone(),
                if query.is_empty() { 8 } else { 0 },
            )
        } else {
            (settings.theme.clone(), 0)
        };
        let hover = std::env::var("HOVER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|i| settings_ui::POPOVER_BASE + i);
        let pop = settings_ui::popover(anchor, &current, &items, scroll, &query, hover, lw, lh);
        r.render_layered(
            (&base.rects, &base.tris, &base.texts),
            (&pop.rects, &pop.tris, &pop.texts),
        )?;
    } else {
        let layers: Vec<ui::Layer> = vec![(
            base.rects.as_slice(),
            base.tris.as_slice(),
            base.texts.as_slice(),
            base.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().background, &layers)?;
    }

    let (w, h, rgba) = r.read_rgba()?;
    let file = std::fs::File::create(&out)?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}

/// A tab with nothing behind it, for the tab bar scenes.
struct SampleTab {
    title: &'static str,
    icon: Option<ui::IconKind>,
    material: Option<ui::MaterialIcon>,
    dirty: bool,
}

impl workspace::Item for SampleTab {
    fn id(&self) -> Option<String> {
        Some(self.title.to_string())
    }
    fn title(&self) -> String {
        self.title.to_string()
    }
    fn icon(&self) -> Option<ui::MaterialIcon> {
        self.material
    }
    fn tab_icon(&self) -> Option<ui::IconKind> {
        self.icon
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn render(&mut self) -> ui::Node {
        ui::div().into()
    }
}

/// Canned object storage answers for the Database snapshots.
fn scene_size() -> Option<(f32, f32)> {
    let size = std::env::var("SCENESIZE").ok()?;
    let (width, height) = size.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// The three files the project search boards find `User.where(active: true)` in.
fn search_fixture(root: &std::path::Path) -> anyhow::Result<()> {
    let files = [
        (
            "api/app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  before_action :set_user, only: :show\n\n  def index\n    @users = User.where(active: true).order(:name)\n    render json: @users\n  end\n\n  def show\n    render json: @user\n  end\n\n  private\n\n  def set_user\n    @user = User.where(active: true).find(params[:id])\n  end\nend\n",
        ),
        (
            "api/spec/requests/users_spec.rb",
            "require \"rails_helper\"\n\nRSpec.describe \"Users\", type: :request do\n  describe \"GET /users\" do\n    before { create_list(:user, 30) }\n\n    it \"lists active users\" do\n      get \"/users\"\n      expect(User.where(active: true).count).to eq(30)\n    end\n  end\nend\n",
        ),
        (
            "web/src/api/users.ts",
            "import { client } from \"./client\";\n\nexport async function activeUsers() {\n  // mirrors User.where(active: true) on the server\n  return client.get(\"/users\");\n}\n",
        ),
    ];
    for (path, text) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}

struct SnapshotStorage;

struct BoardAvatar;

impl pom_db::object_storage::HttpTransport for BoardAvatar {
    fn send(
        &self,
        _request: &pom_db::object_storage::HttpRequest,
    ) -> Result<pom_db::object_storage::HttpResponse, String> {
        let side = 256_u32;
        let accent = ui::theme().accent_fill;
        let mut pixels = Vec::with_capacity((side * side * 4) as usize);
        let center = side as f32 / 2.0;
        for y in 0..side {
            for x in 0..side {
                let distance =
                    ((x as f32 + 0.5 - center).powi(2) + (y as f32 + 0.5 - center).powi(2)).sqrt();
                let coverage = (center - distance).clamp(0.0, 1.0);
                pixels.extend_from_slice(&[
                    (accent.r * 255.0) as u8,
                    (accent.g * 255.0) as u8,
                    (accent.b * 255.0) as u8,
                    (coverage * 255.0) as u8,
                ]);
            }
        }
        let mut body = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut body, side, side);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
            writer
                .write_image_data(&pixels)
                .map_err(|error| error.to_string())?;
        }
        body.resize(body.len().max(52_224), 0);
        Ok(pom_db::object_storage::HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "image/png".into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            body,
        })
    }
}

impl pom_db::object_storage::HttpTransport for SnapshotStorage {
    fn send(
        &self,
        _request: &pom_db::object_storage::HttpRequest,
    ) -> Result<pom_db::object_storage::HttpResponse, String> {
        Ok(pom_db::object_storage::HttpResponse {
            status: 206,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Content-Range".into(), "bytes 0-65535/5347737".into()),
            ],
            body: b"[\n  { \"id\": 5120, \"status\": \"paid\", \"total_cents\": 4900 },\n  { \"id\": 5119, \"status\": \"refunded\", \"total_cents\": 1200 }\n]\n".to_vec(),
        })
    }
}

fn update_snapshot(which: &str) -> auto_update::Snapshot {
    use auto_update::Status;
    let version = || "0.7.4".to_string();
    let status = match which {
        "checking" => Status::Checking,
        "uptodate" => Status::UpToDate,
        "downloading" => Status::Downloading {
            version: version(),
            progress: Some(0.45),
        },
        "verifying" => Status::Verifying { version: version() },
        "ready" | "dismissed" => Status::Ready { version: version() },
        "failed" => Status::Failed("the download's signature did not verify".into()),
        _ => Status::Idle,
    };
    auto_update::Snapshot {
        status,
        manual: true,
        dismissed: which == "dismissed",
        supported: true,
        last_checked: Some(std::time::SystemTime::UNIX_EPOCH),
    }
}

/// The window boards' workspaces as cmd-alt-o lists them, with their agents, the login one current.
/// The window commands the palette board lists, with the keys they are bound to.
fn palette_fixture() -> Vec<workspace::ExtraCommand> {
    [
        ("workspace: switch workspace", "cmd-alt-o"),
        ("workspace: new workspace", "cmd-n"),
        (
            "workspace: activate next workspace needing attention",
            "cmd-alt-a",
        ),
        ("workspace: activate previous workspace", "cmd-alt-up"),
        ("workspace: activate next workspace", "cmd-alt-down"),
        ("workspace: open ticket", ""),
        ("workspace: export config", ""),
        ("workspace: import config", ""),
        ("workspace: toggle left dock", "cmd-b"),
        ("terminal: new terminal", "ctrl-`"),
        ("git: commit", "cmd-enter"),
    ]
    .into_iter()
    .enumerate()
    .map(|(id, (name, keys))| workspace::ExtraCommand {
        name: name.into(),
        keys: (!keys.is_empty())
            .then(|| keys.to_string())
            .into_iter()
            .collect(),
        id: id as u64,
    })
    .collect()
}

/// The project files the Go to file board searches.
fn finder_fixture() -> Vec<String> {
    [
        "api/app/controllers/users_controller.rb",
        "api/app/controllers/sessions_controller.rb",
        "api/app/models/user.rb",
        "api/config/routes.rb",
        "api/spec/requests/users_spec.rb",
        "web/src/components/UsersTable.tsx",
        "web/src/hooks/useUsers.ts",
        "web/src/main.tsx",
        "worker/src/jobs.ts",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn switcher_fixture() -> workspace::WorkspaceSwitcher {
    let project = window_project();
    let agent = |branch: &str| match branch {
        "proj-101-login-google" => Some(workspace::AgentDot::ToolUse),
        "api-102-rate-limit" => Some(workspace::AgentDot::AwaitingInput),
        "investigate-0917-5fwg" => Some(workspace::AgentDot::Thinking),
        _ => None,
    };
    let rows: Vec<workspace::WorkspaceRow> = project
        .workspaces
        .iter()
        .enumerate()
        .map(|(index, branch)| workspace::WorkspaceRow {
            index,
            label: project.label(index).to_string(),
            branch: branch.clone(),
            agent: agent(branch),
            ticket: project.tickets[index].clone(),
            ticket_category: project.ticket_categories[index].clone(),
            running: project.running[index],
            state: String::new(),
            pr: project.prs[index],
            missing: Vec::new(),
        })
        .collect();
    workspace::WorkspaceSwitcher {
        entries: workspace::switcher_order(&rows, None)
            .into_iter()
            .map(|(section, row)| workspace::SwitcherEntry {
                row: row.clone(),
                section,
                id: row.index as u64,
            })
            .collect(),
        current: 1,
        new_workspace: workspace::ExtraCommand {
            name: "New workspace".into(),
            keys: vec!["cmd-n".into()],
            id: 0,
        },
    }
}

/// The workspaces the window boards list, the active one first after main.
fn window_crop() -> Option<(u32, u32, u32, u32)> {
    let crop = std::env::var("CROP").ok()?;
    if std::env::var_os("NOCROP").is_some() {
        return Some((0, 0, 1440, 900));
    }
    let parts: Vec<u32> = crop
        .split(',')
        .filter_map(|part| part.parse().ok())
        .collect();
    match parts[..] {
        [x, y, w, h] => Some((x, y, w, h)),
        _ => None,
    }
}

fn crop_rgba(width: u32, rgba: &[u8], (x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, Vec<u8>) {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = ((row * width + x) * 4) as usize;
        out.extend_from_slice(rgba.get(start..start + (w * 4) as usize).unwrap_or(&[]));
    }
    (w, h, out)
}

fn window_project() -> workspace::ProjectInfo {
    let pr =
        |count: usize, severity: workspace::PrSeverity, trouble: Option<workspace::PrTrouble>| {
            Some(workspace::PrSummary {
                count,
                severity,
                trouble,
            })
        };
    // branch, label, ticket status, its category, running services, PRs.
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        usize,
        Option<workspace::PrSummary>,
    );
    let rows: [Row; 6] = [
        ("main", "", "", "", 2, None),
        (
            "proj-101-login-google",
            "Login with Google",
            "In Progress",
            "indeterminate",
            3,
            pr(2, workspace::PrSeverity::Ok, None),
        ),
        (
            "api-102-rate-limit",
            "Rate limit the public API",
            "In Progress",
            "indeterminate",
            0,
            None,
        ),
        (
            "web-107-invoice-pdf",
            "Invoice PDF export",
            "In Review",
            "indeterminate",
            1,
            pr(
                1,
                workspace::PrSeverity::Danger,
                Some(workspace::PrTrouble::ChecksFailed),
            ),
        ),
        (
            "ops-112-queue-upgrade",
            "Upgrade the queue worker",
            "In Review",
            "indeterminate",
            0,
            pr(
                2,
                workspace::PrSeverity::Warn,
                Some(workspace::PrTrouble::Pending),
            ),
        ),
        (
            "investigate-0917-5fwg",
            "investigate flaky spec",
            "",
            "",
            0,
            None,
        ),
    ];
    workspace::ProjectInfo {
        states: Vec::new(),
        name: "myproject".into(),
        branch: "main".into(),
        config_path: fixture_root().join("pom.yml"),
        host: None,
        workspaces: rows.iter().map(|row| row.0.to_string()).collect(),
        active: "proj-101-login-google".into(),
        labels: rows.iter().map(|row| row.1.to_string()).collect(),
        running: rows.iter().map(|row| row.4).collect(),
        tickets: rows.iter().map(|row| row.2.to_string()).collect(),
        ticket_categories: rows.iter().map(|row| row.3.to_string()).collect(),
        prs: rows.iter().map(|row| row.5).collect(),
        missing: Vec::new(),
        repo_branches: Vec::new(),
    }
}

/// The window buttons the OS draws, which a scene stands in for where the app pins them.
fn scene_lights() -> Vec<ui::Rect> {
    [
        (0.996, 0.373, 0.341),
        (0.996, 0.737, 0.18),
        (0.157, 0.784, 0.251),
    ]
    .iter()
    .enumerate()
    .map(|(index, (r, g, b))| ui::Rect {
        radius: 6.0,
        ..ui::Rect::new(
            20.0 + index as f32 * 20.0,
            (workspace::TOP_BAR_H - 12.0) / 2.0,
            12.0,
            12.0,
            ui::Rgba::new(*r, *g, *b, 1.0),
        )
    })
    .collect()
}

fn window_usage() -> workspace::UsageInfo {
    workspace::UsageInfo {
        account: Some(workspace::UsageAccount {
            name: "Alex".into(),
            email: "alex@example.com".into(),
            plan: "team".into(),
            organization: "myproject".into(),
        }),
        session: Some(workspace::UsageWindow {
            used: 8.0,
            resets: "in 2h 56m".into(),
        }),
        weekly: Some(workspace::UsageWindow {
            used: 26.0,
            resets: "Sat 17:00".into(),
        }),
        note: "Updated 2m ago".into(),
        today: Some(workspace::UsageToday {
            total: "$128".into(),
            sessions: 6,
            by_workspace: Vec::new(),
        }),
        hidden: false,
    }
}

/// The pull requests the Remote boards draw: api's with CI failing and changes requested, worker's passing.
fn board_pull_requests(
    roots: &[std::path::PathBuf],
    branch: &str,
) -> anyhow::Result<pull_request_ui::PullRequests> {
    let state = fixture_dir().join("pr-state");
    let state = state.as_path();
    let _ = std::fs::remove_dir_all(state);
    let prs = pull_request_ui::PullRequests::new(
        pom_paths::StateDir::new(state.to_path_buf()),
        "myproject",
        std::sync::Arc::new(|| {}),
    );
    let pr = |repo: &str, number: u64, title: &str, checks: &str, review: &str| {
        let target = pom_forge::PrTarget {
            repo: repo.into(),
            owner: "myorg".into(),
            name: repo.into(),
            head: branch.into(),
        };
        let pull = pom_forge::PullRequest {
            number,
            title: title.into(),
            state: "OPEN".into(),
            head_ref_name: branch.into(),
            base_ref_name: "main".into(),
            checks: checks.into(),
            review: review.into(),
            ..Default::default()
        };
        (target, pull)
    };
    let mut shown = Vec::new();
    if branch != "feat-login" {
        let (target, mut pull) = pr(
            "myproject-api-service",
            512,
            "Paginate the users index and cache every page of results",
            "fail",
            "approved",
        );
        pull.conflict = true;
        shown.push((roots[0].clone(), target, pull));
        prs.show(branch, shown);
        return Ok(prs);
    }
    for (root, (target, pull)) in roots
        .iter()
        .zip([
            Some(pr(
                "api",
                482,
                "Paginate the users index",
                "fail",
                "changes",
            )),
            None,
            Some(pr("worker", 318, "Retry failed mail", "pass", "approved")),
        ])
        .filter_map(|(root, pr)| pr.map(|pr| (root, pr)))
    {
        shown.push((root.clone(), target, pull));
    }
    prs.show("feat-login", shown);
    Ok(prs)
}

#[cfg(test)]
mod tests {
    use workspace::SidePanelView;

    /// Every text and icon of the panel at `width`, as (x, y, w, h, what).
    fn boxes(node: &ui::Node, width: f32) -> Vec<(f32, f32, f32, f32, String)> {
        let painted = ui::render(
            node,
            ui::Rect::new(0.0, 0.0, width, 832.0, ui::Rgba::TRANSPARENT),
        );
        let scale = ui::ui_text_scale();
        let mut out: Vec<_> = painted
            .texts
            .iter()
            .map(|text| {
                let w =
                    ui::measure_text_width(&text.text, text.size, text.font, text.weight) / scale;
                // A line box, not just the em: glyphs sit lower than `y` by about the leading.
                (text.x, text.y, w, text.size * 1.4, text.text.clone())
            })
            .collect();
        out.extend(
            painted
                .icons
                .iter()
                .map(|icon| (icon.x, icon.y, icon.w, icon.h, format!("{:?}", icon.kind))),
        );
        out
    }

    #[test]
    fn a_narrow_git_panel_truncates_instead_of_overlapping() {
        let roots = crate::git_fixture::create_remote(crate::git_fixture::RemoteState::Narrow)
            .expect("fixture");
        let sources: Vec<git_ui::RepoSource> = roots
            .iter()
            .map(|root| git_ui::RepoSource {
                name: root.file_name().unwrap().to_string_lossy().into_owned(),
                root: root.clone(),
                default_branch: "main".into(),
                expected_branch: crate::git_fixture::NARROW_BRANCH.into(),
                kept: false,
                origin: None,
            })
            .collect();
        let mut panel = git_ui::GitPanel::new(sources, None, std::sync::Arc::new(|| {}))
            .with_pull_requests(
                crate::board_pull_requests(&roots, crate::git_fixture::NARROW_BRANCH)
                    .expect("pull requests"),
            );
        let width = 300.0;
        panel.render(width, 832.0);
        panel.wait_for_scan();
        panel.set_view(true, true);
        for tab in [
            git_ui::Tab::Changes,
            git_ui::Tab::Remote,
            git_ui::Tab::History,
        ] {
            panel.set_tab(tab);
            let found = boxes(&panel.render(width, 832.0), width);
            for (x, _, w, _, what) in &found {
                assert!(x + w <= width + 0.5, "{what} runs past the panel ({tab:?})");
            }
            for (i, a) in found.iter().enumerate() {
                for b in &found[i + 1..] {
                    let rows_meet = a.1 < b.1 + b.3 && b.1 < a.1 + a.3;
                    let columns_meet = a.0 + 0.5 < b.0 + b.2 && b.0 + 0.5 < a.0 + a.2;
                    assert!(
                        !(rows_meet && columns_meet),
                        "{} overlaps {} ({tab:?})",
                        a.4,
                        b.4
                    );
                }
            }
        }
    }
}
