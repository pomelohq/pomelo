use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use workspace::Layout;

use crate::{apply_dock_settings, now_seconds, App};

pub(crate) struct RemoteWindow {
    pub host: String,
    pub project: String,
    /// Its pom.yml there.
    pub config: String,
    pub watch: pom_remote::Watch,
    /// The workspace shown; empty until the first look says which one is main.
    pub active: String,
    seen: u64,
    notice_actions: Vec<remote_ui::NoticeAction>,
    /// The workspace whose terminals the window shows, once the host's server opens them.
    terminals_for: Option<String>,
    /// The agent roles whose tabs were open when last saved, for the shown workspace.
    agent_tabs: Vec<String>,
    /// Server restarts already told about.
    restarts_told: u64,
    /// Updating pom on the host, written by the thread that runs it.
    pom_update: Arc<std::sync::Mutex<remote_ui::PomUpdate>>,
    update_shown: remote_ui::PomUpdate,
}

fn read_update(update: &std::sync::Mutex<remote_ui::PomUpdate>) -> remote_ui::PomUpdate {
    update.lock().map(|state| state.clone()).unwrap_or_default()
}

fn write_update(
    update: &std::sync::Mutex<remote_ui::PomUpdate>,
    edit: impl FnOnce(&mut remote_ui::PomUpdate),
) {
    match update.lock() {
        Ok(mut state) => edit(&mut state),
        Err(poisoned) => edit(&mut poisoned.into_inner()),
    }
    ui::wake();
}

impl RemoteWindow {
    fn matches(&self, host: &str, project: &str) -> bool {
        self.host == host && self.project == project
    }
}

fn project_info(remote: &RemoteWindow, snapshot: &pom_remote::Snapshot) -> workspace::ProjectInfo {
    let workspaces = snapshot
        .workspaces
        .as_ref()
        .map(|list| list.workspaces.clone())
        .unwrap_or_default();
    let count = workspaces.len();
    let main = workspaces
        .iter()
        .find(|workspace| workspace.is_main)
        .map(|workspace| workspace.branch.clone())
        .unwrap_or_default();
    workspace::ProjectInfo {
        states: workspaces
            .iter()
            .map(|ws| match ws.state.as_str() {
                "sleeping" | "queued" => ws.state.clone(),
                _ => String::new(),
            })
            .collect(),
        name: remote.project.clone(),
        branch: main,
        config_path: std::path::PathBuf::from(&remote.config),
        host: Some(remote.host.clone()),
        workspaces: workspaces.iter().map(|ws| ws.branch.clone()).collect(),
        active: remote.active.clone(),
        labels: workspaces.iter().map(|ws| ws.name.clone()).collect(),
        running: workspaces.iter().map(|ws| ws.running).collect(),
        tickets: vec![String::new(); count],
        ticket_categories: vec![String::new(); count],
        prs: vec![None; count],
        missing: vec![Vec::new(); count],
        repo_branches: vec![Vec::new(); count],
    }
}

fn hosts() -> Result<Vec<pom_remote::Host>, String> {
    let path = pom_remote::Hosts::default_path().ok_or("no home folder")?;
    Ok(pom_remote::Hosts::load(&path)?.all())
}

fn host(name: &str) -> Result<pom_remote::Host, String> {
    let path = pom_remote::Hosts::default_path().ok_or("no home folder")?;
    pom_remote::Hosts::load(&path)?.get(name)
}

impl App {
    pub(crate) fn open_connect_host(&mut self, id: WindowId) {
        let list: remote_ui::ListProjects = Arc::new(|host: &pom_remote::Host| {
            pom_remote::remote_projects(&pom_remote::Ssh::system(), host)
        });
        let modal = remote_ui::ConnectHostModal::new(hosts(), list);
        self.focus_main_with_modal(id, Box::new(modal));
    }

    fn window_with_remote(&self, host: &str, project: &str) -> Option<WindowId> {
        self.mains.iter().find_map(|(window, main)| {
            main.remote
                .as_ref()
                .is_some_and(|remote| remote.matches(host, project))
                .then_some(*window)
        })
    }

    /// Opens the remote project in window `id`, or brings forward the window that already has it.
    pub(crate) fn open_remote_in(&mut self, id: WindowId, picked: &remote_ui::ConnectProject) {
        if let Some(window) = self.window_with_remote(&picked.host, &picked.name) {
            if let Some(main) = self.mains.get(&window) {
                main.window.set_minimized(false);
                main.window.focus_window();
            }
            return;
        }
        let host = match host(&picked.host) {
            Ok(host) => host,
            Err(error) => {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast(format!("Cannot connect: {error}"), None)
                });
                return;
            }
        };
        let state = pom_paths::StateDir::from_env();
        if let Err(error) = pom_core::record_remote_open(
            &state,
            &picked.name,
            &picked.host,
            &picked.path,
            now_seconds(),
        ) {
            eprintln!("failed to record the remote project: {error}");
        }
        let can_install = !crate::is_dev_build() && host.pom_path.is_none();
        let source =
            pom_remote::project_source(host, picked.config.clone(), pom_remote::Ssh::system());
        let watch = pom_remote::Watch::start(source, "", Arc::new(ui::wake));
        if let Some(main) = self.mains.get_mut(&id) {
            main.project = None;
            main.watcher = None;
            main.services = None;
            main.background = None;
            main.tickets = None;
            main.pull_requests = None;
            main.parked.clear();
            main.stale.clear();
            watch.set_visible(!main.hidden);
            main.remote = Some(RemoteWindow {
                host: picked.host.clone(),
                project: picked.name.clone(),
                config: picked.config.clone(),
                watch,
                active: String::new(),
                seen: u64::MAX,
                notice_actions: Vec::new(),
                terminals_for: None,
                agent_tabs: Vec::new(),
                restarts_told: 0,
                pom_update: Arc::new(std::sync::Mutex::new(remote_ui::PomUpdate {
                    can_install,
                    ..remote_ui::PomUpdate::default()
                })),
                update_shown: remote_ui::PomUpdate::default(),
            });
        }
        self.install_remote_views(id);
        self.refresh_sessions();
    }

    /// The session row at `index` of window `id` is a remote project: open it here, or in a new window.
    pub(crate) fn open_remote_session(
        &mut self,
        id: WindowId,
        index: usize,
        event_loop: Option<&ActiveEventLoop>,
    ) -> bool {
        let Some(session) = self.session_row(id, index) else {
            return false;
        };
        let Some(host) = session.host else {
            return false;
        };
        let path = session.path.to_string_lossy().into_owned();
        let picked = remote_ui::ConnectProject {
            config: format!(
                "{}/{}",
                path.trim_end_matches('/'),
                pom_config::CONFIG_FILE_NAME
            ),
            host,
            name: session.name,
            path,
        };
        let target = match event_loop {
            Some(event_loop)
                if self
                    .window_with_remote(&picked.host, &picked.name)
                    .is_none() =>
            {
                let mut layout = Layout::default();
                apply_dock_settings(&self.settings, &mut layout);
                self.new_main_window(event_loop, layout, None)
            }
            _ => id,
        };
        self.open_remote_in(target, &picked);
        true
    }

    pub(crate) fn install_remote_views(&mut self, id: WindowId) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let snapshot = remote.watch.snapshot();
        let info = project_info(remote, &snapshot);
        let panels: Vec<Box<dyn workspace::SidePanelView>> = vec![Box::new(
            services_ui::RemoteServicesPanel::new(remote.watch.clone()),
        )];
        let title = remote_title(remote);
        self.with_workspace_view(id, |view, _| {
            view.set_project(Some(info), None, None, None);
            view.set_side_panels(panels);
            view.set_ai_available(false);
            view.set_config_problem(None, std::path::Path::new(""));
        });
        if let Some(main) = self.mains.get_mut(&id) {
            main.window.set_title(&title);
            #[cfg(target_os = "macos")]
            crate::platform::center_traffic_lights(&main.window);
            if let Some(remote) = main.remote.as_mut() {
                remote.seen = u64::MAX;
                remote.terminals_for = None;
            }
            main.dirty = true;
        }
        self.poll_remote(id);
    }

    /// Picks up what the last look found: the workspace list, the page in place of the editor, the title.
    pub(crate) fn poll_remote(&mut self, id: WindowId) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let hidden = main.hidden;
        let Some(remote) = main.remote.as_mut() else {
            return;
        };
        remote.watch.set_visible(!hidden);
        let generation = remote.watch.generation();
        let update = read_update(&remote.pom_update);
        if generation == remote.seen && update == remote.update_shown {
            return;
        }
        remote.seen = generation;
        remote.update_shown = update.clone();
        let snapshot = remote.watch.snapshot();
        if remote.active.is_empty() {
            let main_branch = snapshot.workspaces.as_ref().and_then(|list| {
                list.workspaces
                    .iter()
                    .find(|workspace| workspace.is_main)
                    .map(|workspace| workspace.branch.clone())
            });
            if let Some(branch) = main_branch {
                remote.active = branch;
                remote.watch.set_branch(&remote.active);
            }
        }
        let info = project_info(remote, &snapshot);
        let dots: std::collections::HashMap<String, workspace::AgentDot> =
            agent_dot(&snapshot.agents)
                .map(|dot| (remote.active.clone(), dot))
                .into_iter()
                .collect();
        let (notice, actions) = remote_ui::remote_notice(
            &snapshot,
            remote.watch.location(),
            &remote.host,
            &remote.project,
            &update,
        );
        remote.notice_actions = actions;
        let restarted = snapshot.server_restarts > remote.restarts_told;
        remote.restarts_told = snapshot.server_restarts;
        let restart_notice = restarted.then(|| remote_ui::restart_notice(&remote.host));
        let title = remote_title(remote);
        main.window.set_title(&title);
        main.dirty = true;
        self.with_workspace_view(id, |view, _| {
            view.update_project(info);
            view.set_agent_states(dots);
            view.set_center_notice(Some(notice));
            if let Some(message) = restart_notice {
                view.show_toast(message, None);
            }
        });
        self.install_remote_terminals(id, &snapshot);
        self.open_started_agents(id);
        self.save_remote_agent_tabs(id, &snapshot);
    }

    /// Agents this window started on the host, each opened in the dock once it runs.
    fn open_started_agents(&mut self, id: WindowId) {
        let Some(watch) = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .map(|remote| remote.watch.clone())
        else {
            return;
        };
        for started in watch.take_started() {
            self.open_remote_agent(id, &watch, &started.role);
        }
    }

    fn open_remote_agent(&mut self, id: WindowId, watch: &pom_remote::Watch, role: &str) {
        let item_id = services_ui::agent_item_id(&watch.snapshot().branch, role);
        let number = self.next_agent_item;
        self.next_agent_item += 1;
        self.with_workspace_view(id, |view, _| {
            view.open_agent_item(&item_id, false, || {
                services_ui::remote_agent_item(watch, role, number)
            })
        });
    }

    /// Keeps which agent tabs of the shown workspace are open, so they come back next time.
    fn save_remote_agent_tabs(&mut self, id: WindowId, snapshot: &pom_remote::Snapshot) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        if remote.terminals_for.as_deref() != Some(remote.active.as_str())
            || snapshot.agents.is_empty()
        {
            return;
        }
        let branch = remote.active.clone();
        let roles: Vec<String> = snapshot
            .agents
            .iter()
            .map(|agent| agent.role.clone())
            .collect();
        let open: Vec<String> = self
            .with_workspace_view(id, |view, _| {
                roles
                    .into_iter()
                    .filter(|role| view.agent_item_open(&services_ui::agent_item_id(&branch, role)))
                    .collect()
            })
            .unwrap_or_default();
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        if open == remote.agent_tabs {
            return;
        }
        remote.agent_tabs = open;
        let Some(tabs) = services_ui::RemoteAgentTabs::default_location() else {
            return;
        };
        if let Err(error) = tabs.save(&remote.host, &remote.project, &branch, &remote.agent_tabs) {
            eprintln!("failed to save the remote agent tabs: {error}");
        }
    }

    /// Terminal tabs and the agent dock of the shown workspace, run on the host through its server; the editor,
    /// files and Git stay there.
    fn install_remote_terminals(&mut self, id: WindowId, snapshot: &pom_remote::Snapshot) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        if remote.active.is_empty() || remote.terminals_for.as_deref() == Some(&remote.active) {
            return;
        }
        let Some(provider) = remote.watch.terminals() else {
            return;
        };
        let root = snapshot
            .workspaces
            .as_ref()
            .and_then(|list| {
                list.workspaces
                    .iter()
                    .find(|workspace| workspace.branch == remote.active)
            })
            .map(|workspace| workspace.path.clone())
            .unwrap_or_default();
        remote.terminals_for = Some(remote.active.clone());
        let prefix = format!(
            "term-{}-{}",
            pom_env::branch_safe(&remote.project),
            pom_env::branch_safe(&remote.active)
        );
        let waker = Arc::new(ui::wake);
        let terminals = terminal_ui::TerminalPanel::with_holders(
            root.clone(),
            waker.clone(),
            terminal_ui::HolderScope {
                place: terminal_ui::HolderPlace::Remote(provider),
                binary: std::path::PathBuf::new(),
                prefix,
                group: None,
            },
        );
        let starts_agents = snapshot
            .transport
            .as_ref()
            .is_some_and(|transport| transport.agents);
        let watch = remote.watch.clone();
        let start_agent = starts_agents.then(|| {
            let watch = watch.clone();
            Box::new(move || {
                let role = services_ui::next_agent_role(&watch.snapshot().agents);
                watch.start_agent(&role);
            }) as Box<dyn FnMut()>
        });
        let (host, project) = (remote.host.clone(), remote.project.clone());
        remote.agent_tabs = Vec::new();
        let agents = terminal_ui::TerminalPanel::remote_agent_dock(root, waker, start_agent);
        self.with_workspace_view(id, |view, _| {
            view.set_terminal_views(Some(Box::new(terminals)), Some(Box::new(agents)));
        });
        let Some(tabs) = services_ui::RemoteAgentTabs::default_location() else {
            return;
        };
        let mut number = self.next_agent_item;
        let restored = services_ui::restore_agent_tabs(&watch, &tabs, &host, &project, &mut number);
        self.next_agent_item = number;
        self.with_workspace_view(id, |view, _| {
            for (item_id, item) in restored {
                view.open_agent_item(&item_id, false, move || Some(item));
            }
        });
    }

    pub(crate) fn activate_remote_workspace(&mut self, id: WindowId, index: usize) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        let snapshot = remote.watch.snapshot();
        let Some(branch) = snapshot
            .workspaces
            .as_ref()
            .and_then(|list| list.workspaces.get(index))
            .map(|workspace| workspace.branch.clone())
        else {
            return;
        };
        if branch == remote.active {
            return;
        }
        remote.active = branch;
        remote.watch.set_branch(&remote.active);
        remote.seen = u64::MAX;
        self.poll_remote(id);
    }

    pub(crate) fn remote_notice_action(&mut self, id: WindowId, index: usize) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        match remote.notice_actions.get(index) {
            Some(remote_ui::NoticeAction::Reconnect) => remote.watch.refresh_now(),
            Some(remote_ui::NoticeAction::OpenInEditor) => self.open_remote_editor(id, None),
            Some(remote_ui::NoticeAction::UpdatePom) => self.update_remote_pom(id),
            None => {}
        }
    }

    /// Installs this app's version of pom on the host, off the UI thread; the page shows how it goes.
    fn update_remote_pom(&mut self, id: WindowId) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let state = remote.pom_update.clone();
        if read_update(&state).running.is_some() {
            return;
        }
        write_update(&state, |update| {
            update.error = None;
            update.running = Some(pom_remote::update::UpdateStep::Machine);
        });
        let source = remote.watch.source();
        let thread_state = state.clone();
        let spawned = std::thread::Builder::new()
            .name("pom-update".into())
            .spawn(move || {
                let release = pom_remote::update::Release::published(pom_remote::VERSION);
                let result = source.update_pom(&release, &|step| {
                    write_update(&thread_state, |update| update.running = Some(step));
                });
                write_update(&thread_state, |update| {
                    update.running = None;
                    update.error = result.err();
                });
            });
        if let Err(error) = spawned {
            write_update(&state, |update| {
                update.running = None;
                update.error = Some(error.to_string());
            });
        }
    }

    /// Opens the worktree of the workspace at `index` (the shown one with `None`) in the editor that the
    /// `remote_editor` setting names, over SSH to the project's host.
    pub(crate) fn open_remote_editor(&mut self, id: WindowId, index: Option<usize>) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let snapshot = remote.watch.snapshot();
        let workspaces = snapshot
            .workspaces
            .map(|list| list.workspaces)
            .unwrap_or_default();
        let workspace = match index {
            Some(index) => workspaces.get(index),
            None => workspaces
                .iter()
                .find(|workspace| workspace.branch == remote.active),
        };
        let message = match (workspace, host(&remote.host)) {
            (None, _) => format!("{} has not listed its workspaces yet", remote.host),
            (_, Err(error)) => format!("Cannot open an editor: {error}"),
            (Some(workspace), Ok(host)) => {
                let path = workspace.path.to_string_lossy();
                let find =
                    |program: &str| pom_remote::find_program(program, pom_services::tool_path());
                match pom_remote::editor_argv(
                    &self.settings.remote_editor,
                    &host.url.alias(),
                    &path,
                    &find,
                ) {
                    Ok(argv) => match launch(&argv) {
                        Ok(()) => format!(
                            "Opening {} on {} in {}",
                            workspace.branch,
                            host.name,
                            program_name(&argv)
                        ),
                        Err(error) => format!("Could not run {}: {error}", program_name(&argv)),
                    },
                    Err(error) => format!("Cannot open an editor: {error}"),
                }
            }
        };
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    /// Creating a workspace on the host: the create form, with the repos main has there.
    pub(crate) fn open_remote_create(&mut self, id: WindowId) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let snapshot = remote.watch.snapshot();
        if !runs_jobs(&snapshot) {
            self.remote_only_on_host(id, "Creating a workspace");
            return;
        }
        let workspaces = snapshot
            .workspaces
            .map(|list| list.workspaces)
            .unwrap_or_default();
        let repos = workspaces
            .iter()
            .find(|workspace| workspace.is_main)
            .map(|workspace| workspace.repos.clone())
            .unwrap_or_default();
        let existing = workspaces
            .iter()
            .map(|workspace| workspace.branch.clone())
            .collect();
        let modal =
            workspaces_ui::CreateWorkspaceModal::new(repos, existing, crate::workspaces::namer());
        self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
    }

    /// A remote workspace's menu: sleep, wake and pull run on the host as jobs; the rest stays there.
    pub(crate) fn remote_row_action(
        &mut self,
        id: WindowId,
        index: usize,
        action: workspace::RowAction,
    ) {
        use pom_remote::server_api::{BranchArgs, JobRequest};
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let snapshot = remote.watch.snapshot();
        let Some(workspace) = snapshot
            .workspaces
            .as_ref()
            .and_then(|list| list.workspaces.get(index))
            .cloned()
        else {
            return;
        };
        let label = if workspace.name.is_empty() {
            workspace.branch.clone()
        } else {
            workspace.name.clone()
        };
        let branch = BranchArgs {
            branch: workspace.branch.clone(),
            force: false,
        };
        let (request, title) = match action {
            workspace::RowAction::OpenInEditor => return self.open_remote_editor(id, Some(index)),
            workspace::RowAction::Sleep => (JobRequest::Sleep(branch), format!("Sleeping {label}")),
            workspace::RowAction::Wake => (JobRequest::Wake(branch), format!("Waking {label}")),
            workspace::RowAction::Pull => (JobRequest::Sync(branch), format!("Pulling {label}")),
            _ => {
                self.remote_only_on_host(id, "Changing a workspace");
                return;
            }
        };
        self.queue_remote_job(id, request, title);
    }

    /// Runs `request` on the host as a job whose card shows its stages; a host whose pom cannot says how.
    pub(crate) fn queue_remote_job(
        &mut self,
        id: WindowId,
        request: pom_remote::server_api::JobRequest,
        title: String,
    ) {
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let Some(remote) = main.remote.as_ref() else {
            return;
        };
        if !runs_jobs(&remote.watch.snapshot()) {
            let host = remote.host.clone();
            let message = format!(
                "{title} needs a newer pom on {host}: update it there, or use pom -H {host}"
            );
            self.with_workspace_view(id, |view, _| view.show_toast(message, None));
            return;
        }
        main.ops
            .enqueue_remote(request, title, remote.watch.source());
    }

    /// After jobs on the host: look again now, and show a workspace just created there.
    pub(crate) fn finish_remote_jobs(
        &mut self,
        id: WindowId,
        finished: &[workspaces_ui::Finished],
    ) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        if let Some(created) = finished.iter().rev().find(|done| done.ok && done.created) {
            remote.active = created.branch.clone();
            remote.watch.set_branch(&remote.active);
            remote.seen = u64::MAX;
        }
        remote.watch.refresh_now();
    }

    /// Workspace changes run on the host itself for now; say how.
    pub(crate) fn remote_only_on_host(&mut self, id: WindowId, what: &str) -> bool {
        let Some(host) = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .map(|remote| remote.host.clone())
        else {
            return false;
        };
        let message = format!("{what} runs on {host}: use pom -H {host} there for now");
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
        true
    }
}

fn runs_jobs(snapshot: &pom_remote::Snapshot) -> bool {
    snapshot
        .transport
        .as_ref()
        .is_some_and(|transport| transport.jobs)
}

fn program_name(argv: &[String]) -> String {
    argv.first()
        .and_then(|program| std::path::Path::new(program).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Starts the editor on its own and reaps it when it exits, so it never lingers as a zombie of the app.
fn launch(argv: &[String]) -> std::io::Result<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("no editor command"))?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        if let Err(error) = child.wait() {
            eprintln!("editor: {error}");
        }
    });
    Ok(())
}

fn remote_title(remote: &RemoteWindow) -> String {
    if remote.active.is_empty() {
        format!("{} on {} - Pomelo", remote.project, remote.host)
    } else {
        format!(
            "{} on {} - {} - Pomelo",
            remote.project, remote.host, remote.active
        )
    }
}

/// The shown workspace's dot: the agent that most needs a look (waiting first, then working).
fn agent_dot(agents: &[pom_agent::SessionView]) -> Option<workspace::AgentDot> {
    let dot = |state: &str| match state {
        "awaiting_input" => Some((0, workspace::AgentDot::AwaitingInput)),
        "tool_use" => Some((1, workspace::AgentDot::ToolUse)),
        "thinking" => Some((2, workspace::AgentDot::Thinking)),
        "compacting" => Some((3, workspace::AgentDot::Compacting)),
        "idle" => Some((4, workspace::AgentDot::Idle)),
        _ => None,
    };
    agents
        .iter()
        .filter_map(|agent| dot(&agent.state))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, dot)| dot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dot_shows_the_agent_that_most_needs_a_look() {
        let agent = |state: &str| pom_agent::SessionView {
            state: state.into(),
            ..pom_agent::SessionView::default()
        };
        assert_eq!(agent_dot(&[]), None);
        assert_eq!(
            agent_dot(&[agent("idle"), agent("thinking")]),
            Some(workspace::AgentDot::Thinking)
        );
        assert_eq!(
            agent_dot(&[agent("tool_use"), agent("awaiting_input"), agent("died")]),
            Some(workspace::AgentDot::AwaitingInput)
        );
    }
}
