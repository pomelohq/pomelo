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
    /// The workspace whose files the window shows, once the host's server reads them.
    files_for: Option<String>,
    /// The files shown are a stand-in, for a host whose pom does not read them.
    files_unserved: bool,
    /// The Services panel's source, and the workspace it shows.
    services: Option<(String, Arc<services_ui::RemoteServices>)>,
    /// The agent tabs open when last saved, for the shown workspace.
    agent_tabs: Vec<services_ui::AgentTab>,
    /// The shown workspace's saved agent tabs still have to be put back, once its panes are in.
    restore_agent_tabs: bool,
    /// Server restarts already told about.
    restarts_told: u64,
    /// Updating pom on the host, written by the thread that runs it.
    pom_update: Arc<std::sync::Mutex<remote_ui::PomUpdate>>,
    update_shown: remote_ui::PomUpdate,
    /// What of the host is open here: its dev proxy, the shown workspace's shared services, ports asked for.
    pub ports: Option<Arc<pom_remote::RemotePorts>>,
    ports_seen: u64,
    /// The Claude account and cost of the host's agents, for the chip and the status bar.
    pub usage: crate::usage::RemoteUsage,
    /// Jira with the host's settings, else this Mac's for the project.
    pub jira: Arc<pom_remote::RemoteJira>,
    /// Each workspace's ticket status, kept here under `<project>@<host>`.
    tickets: workspaces_ui::TicketStatuses,
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

/// A Ports tab may still hold the forwards; they stop with the window's project all the same.
impl Drop for RemoteWindow {
    fn drop(&mut self) {
        if let Some(ports) = self.ports.take() {
            ports.close();
        }
    }
}

impl RemoteWindow {
    fn matches(&self, host: &str, project: &str) -> bool {
        self.host == host && self.project == project
    }

    /// What Settings edits for this project's Jira: the host's own settings when its pom keeps them (or is not
    /// reached yet), else this Mac's under `<project>@<host>`.
    pub fn settings_jira(&self) -> settings_ui::RemoteJira {
        let older = self
            .watch
            .snapshot()
            .transport
            .is_some_and(|transport| !transport.jira);
        settings_ui::RemoteJira {
            host: self.host.clone(),
            session: self.jira_session(),
            state: pom_paths::StateDir::from_env(),
            store: (!older).then(|| self.jira.host_store()),
        }
    }

    /// The Project page: the repositories main has on the host and how many workspaces have each.
    pub fn project_page(&self) -> settings_ui::ProjectPage {
        let workspaces = self
            .watch
            .snapshot()
            .workspaces
            .map(|list| list.workspaces)
            .unwrap_or_default();
        let main = workspaces.iter().find(|workspace| workspace.is_main);
        let others: Vec<_> = workspaces
            .iter()
            .filter(|workspace| !workspace.is_main)
            .collect();
        let mut names: Vec<String> = main.map(|main| main.repos.clone()).unwrap_or_default();
        for workspace in &others {
            for repo in &workspace.repos {
                if !names.contains(repo) {
                    names.push(repo.clone());
                }
            }
        }
        settings_ui::ProjectPage {
            session: self.project.clone(),
            repos: names
                .into_iter()
                .map(|name| settings_ui::RepoSummary {
                    cloned: main.is_some_and(|main| main.repos.contains(&name)),
                    present: others
                        .iter()
                        .filter(|workspace| workspace.repos.contains(&name))
                        .count(),
                    workspaces: others.len(),
                    name,
                    ..settings_ui::RepoSummary::default()
                })
                .collect(),
            host: Some(self.host.clone()),
        }
    }

    /// The key this Mac keeps the project's Jira cache under.
    pub fn jira_session(&self) -> String {
        pom_remote::remote_jira_session(&self.project, &self.host)
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
        tickets: workspaces
            .iter()
            .map(|ws| remote.tickets.status(&ws.branch).unwrap_or_default())
            .collect(),
        ticket_categories: workspaces
            .iter()
            .map(|ws| remote.tickets.category(&ws.branch).unwrap_or_default())
            .collect(),
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

    /// The title bar's host: the projects on the host this window's project runs on.
    pub(crate) fn open_host_projects(&mut self, id: WindowId) {
        let Some(host) = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .map(|remote| remote.host.clone())
        else {
            return self.open_connect_host(id);
        };
        let list: remote_ui::ListProjects = Arc::new(|host: &pom_remote::Host| {
            pom_remote::remote_projects(&pom_remote::Ssh::system(), host)
        });
        let modal = remote_ui::ConnectHostModal::new(hosts(), list).on_host(&host);
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
        let unconfirmed_input_limit = usize::try_from(
            self.settings
                .server
                .unconfirmed_input_kib
                .saturating_mul(1024),
        )
        .unwrap_or(usize::MAX);
        let host_entry = host.clone();
        let source = pom_remote::project_source(
            host,
            picked.config.clone(),
            pom_remote::Ssh::system(),
            unconfirmed_input_limit,
        );
        let state = pom_paths::StateDir::from_env();
        let ports = Arc::new(pom_remote::RemotePorts::start(pom_remote::PortsSetup {
            source: source.clone(),
            ssh: pom_remote::Ssh::system(),
            host: host_entry,
            project: picked.name.clone(),
            record_dir: Some(pom_remote::ports_dir(&state, &picked.host)),
            proxy_port_here: self.proxy_port_here(),
            waker: Arc::new(ui::wake),
        }));
        let usage = crate::usage::RemoteUsage::start(source.clone());
        let jira = Arc::new(pom_remote::RemoteJira::new(
            source.clone(),
            state.clone(),
            &picked.name,
            &picked.host,
        ));
        let tickets = workspaces_ui::TicketStatuses::new(
            state.clone(),
            &pom_remote::remote_jira_session(&picked.name, &picked.host),
            Arc::new(ui::wake),
        )
        .with_jira(jira.connect());
        {
            let jira = jira.clone();
            let looked = std::thread::Builder::new()
                .name("remote-jira".into())
                .spawn(move || jira.place());
            if let Err(error) = looked {
                eprintln!("remote jira: {error}");
            }
        }
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
                files_for: None,
                files_unserved: false,
                services: None,
                agent_tabs: Vec::new(),
                restore_agent_tabs: false,
                restarts_told: 0,
                pom_update: Arc::new(std::sync::Mutex::new(remote_ui::PomUpdate {
                    can_install,
                    ..remote_ui::PomUpdate::default()
                })),
                update_shown: remote_ui::PomUpdate::default(),
                ports: Some(ports),
                ports_seen: 0,
                usage,
                jira,
                tickets,
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
        let panels = vec![remote_services_panel(remote, &snapshot).0];
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
                remote.files_for = None;
                remote.services = None;
            }
            main.dirty = true;
        }
        self.poll_remote(id);
    }

    /// Picks up what the last look found: the workspace list, the page in place of the editor, the title.
    pub(crate) fn poll_remote(&mut self, id: WindowId) {
        self.start_remote_tasks(id);
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let hidden = main.hidden;
        let Some(remote) = main.remote.as_mut() else {
            return;
        };
        remote.watch.set_visible(!hidden);
        let branches: Vec<String> = remote
            .watch
            .snapshot()
            .workspaces
            .map(|list| list.workspaces.into_iter().map(|ws| ws.branch).collect())
            .unwrap_or_default();
        if !branches.is_empty() {
            remote.tickets.refresh_if_due(&branches);
        }
        let tickets_changed = remote.tickets.take_changed();
        let generation = remote.watch.generation();
        let update = read_update(&remote.pom_update);
        let ports_version = remote.ports.as_ref().map_or(0, |ports| ports.version());
        if generation == remote.seen
            && update == remote.update_shown
            && ports_version == remote.ports_seen
            && !tickets_changed
        {
            return;
        }
        let ports_moved = ports_version != remote.ports_seen;
        remote.seen = generation;
        remote.update_shown = update.clone();
        remote.ports_seen = ports_version;
        let snapshot = remote.watch.snapshot();
        let port_notices = remote
            .ports
            .as_ref()
            .map(|ports| {
                ports.sync(&snapshot);
                ports.take_notices()
            })
            .unwrap_or_default();
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
        let link = remote_ui::host_link(&snapshot.connection);
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
            view.set_host_link(link);
            if let Some(message) = restart_notice {
                view.show_toast(message, None);
            }
            for message in port_notices {
                view.show_toast(message, None);
            }
        });
        if ports_moved {
            self.sync_dev_proxy();
        }
        self.install_remote_terminals(id, &snapshot);
        self.install_remote_files(id, &snapshot);
        self.install_remote_services(id, &snapshot);
        self.restore_remote_agent_tabs(id);
        self.open_started_agents(id);
        self.save_remote_agent_tabs(id, &snapshot);
    }

    /// This machine's dev proxy port, which every workspace address here uses.
    pub(crate) fn proxy_port_here(&self) -> u16 {
        self.dev_proxy
            .as_ref()
            .map_or_else(pom_proxy::Ports::from_env, pom_proxy::DevProxy::ports)
            .proxy
    }

    /// Where the dev proxy sends each remote window's workspace addresses.
    pub(crate) fn remote_gateways(&self) -> Vec<pom_proxy::RemoteGateway> {
        self.mains
            .values()
            .filter_map(|main| main.remote.as_ref()?.ports.as_ref()?.gateway_route())
            .map(|route| pom_proxy::RemoteGateway {
                host: route.host,
                session: route.session,
                branches: route.branches,
                port: route.port,
            })
            .collect()
    }

    /// The Ports tab of a remote window: what of the host is forwarded here.
    pub(crate) fn open_remote_ports(&mut self, id: WindowId) {
        let Some(ports) = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .and_then(|remote| remote.ports.clone())
        else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Ports are for a project on another machine", None)
            });
            return;
        };
        let open_url: Arc<dyn Fn(&str)> = Arc::new(|url: &str| {
            if let Err(error) = desktop::open(url) {
                eprintln!("open {url}: {error}");
            }
        });
        let page = remote_ui::PortsPage::new(ports, open_url);
        self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    /// Stops every window's forwards before the app goes, so none outlives it on a shared ssh connection.
    pub(crate) fn close_remote_ports(&mut self) {
        let closing: Vec<Arc<pom_remote::RemotePorts>> = self
            .mains
            .values_mut()
            .filter_map(|main| main.remote.as_mut()?.ports.take())
            .collect();
        let threads: Vec<_> = closing
            .into_iter()
            .map(|ports| std::thread::spawn(move || ports.close()))
            .collect();
        for thread in threads {
            if thread.join().is_err() {
                eprintln!("ports: closing panicked");
            }
        }
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

    /// The host's main agent of the shown workspace, started there when none runs.
    pub(crate) fn open_remote_main_agent(&mut self, id: WindowId) {
        let Some(watch) = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .map(|remote| remote.watch.clone())
        else {
            return;
        };
        let snapshot = watch.snapshot();
        let role = pom_agent::CLAUDE_DRIVER;
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.alive && agent.role == role)
        {
            self.open_remote_agent(id, &watch, role);
            return;
        }
        self.with_workspace_view(id, |view, _| view.reveal_agent_dock());
        if snapshot
            .transport
            .as_ref()
            .is_some_and(|transport| transport.agents)
        {
            watch.start_agent(role);
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
        let open: Vec<services_ui::AgentTab> = self
            .with_workspace_view(id, |view, _| {
                roles
                    .into_iter()
                    .filter_map(|role| {
                        let item_id = services_ui::agent_item_id(&branch, &role);
                        view.agent_item_open(&item_id)
                            .then(|| services_ui::AgentTab {
                                center: view.agent_item_in_center(&item_id),
                                role,
                            })
                    })
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
        remote.agent_tabs = Vec::new();
        remote.restore_agent_tabs = true;
        let agents = terminal_ui::TerminalPanel::remote_agent_dock(root, waker, start_agent);
        self.with_workspace_view(id, |view, _| {
            view.set_terminal_views(Some(Box::new(terminals)), Some(Box::new(agents)));
        });
    }

    /// Puts the shown workspace's saved agent tabs back where they were, after its file panes are in so a tab
    /// moved next to the code returns there.
    fn restore_remote_agent_tabs(&mut self, id: WindowId) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        if !std::mem::take(&mut remote.restore_agent_tabs) {
            return;
        }
        let Some(tabs) = services_ui::RemoteAgentTabs::default_location() else {
            return;
        };
        let mut number = self.next_agent_item;
        let restored = services_ui::restore_agent_tabs(
            &remote.watch,
            &tabs,
            &remote.host,
            &remote.project,
            &mut number,
        );
        self.next_agent_item = number;
        self.with_workspace_view(id, |view, _| {
            for tab in restored {
                let item = tab.item;
                view.open_agent_item(&tab.id, tab.center, move || Some(item));
            }
        });
    }

    /// The file tree and editor of the shown workspace, read on the host through its server; an older server
    /// keeps the page, whose banner asks for a newer pom there.
    fn install_remote_files(&mut self, id: WindowId, snapshot: &pom_remote::Snapshot) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        let shown = remote.files_for.as_deref() == Some(&remote.active);
        if remote.active.is_empty() || (shown && !remote.files_unserved) {
            return;
        }
        // A host whose pom does not read files still gets the panes, so the window works as a local one.
        let (fs, unserved): (Arc<dyn files::ProjectFs>, bool) = match remote.watch.files() {
            Some(fs) => (fs, false),
            None if shown || snapshot.transport.is_none() => return,
            None => (
                Arc::new(files::UnservedFs::new(format!(
                    "Files need a newer pom on {}; update it there",
                    remote.host
                ))),
                true,
            ),
        };
        let shown = snapshot.workspaces.as_ref().and_then(|list| {
            list.workspaces
                .iter()
                .find(|workspace| workspace.branch == remote.active)
        });
        let root = shown
            .map(|workspace| workspace.path.clone())
            .unwrap_or_default();
        // Main changes only its pom.yml, and a server that cannot save keeps every file read only.
        let saves = snapshot
            .transport
            .as_ref()
            .is_some_and(|transport| transport.files_write);
        let main = shown.is_none_or(|workspace| workspace.is_main);
        remote.files_for = Some(remote.active.clone());
        remote.files_unserved = unserved;
        let files = files_ui::FilesView::new(fs, root)
            .read_only(main || !saves)
            .writable(if saves {
                vec![std::path::PathBuf::from(&remote.config)]
            } else {
                Vec::new()
            });
        self.with_workspace_view(id, |view, _| {
            view.set_files_view(Some(Box::new(files)));
        });
    }

    /// The Services panel of the shown workspace, the same as a local one's, over the host's server.
    fn install_remote_services(&mut self, id: WindowId, snapshot: &pom_remote::Snapshot) {
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        let shown = remote
            .services
            .as_ref()
            .is_some_and(|(branch, _)| *branch == remote.active);
        if remote.active.is_empty() || shown {
            return;
        }
        let (panel, source) = remote_services_panel(remote, snapshot);
        remote.services = Some((remote.active.clone(), source));
        self.with_workspace_view(id, |view, _| view.set_side_panels(vec![panel]));
    }

    /// Repo commands the Services panel asked for, run on the host as jobs.
    fn start_remote_tasks(&mut self, id: WindowId) {
        let tasks = self
            .mains
            .get(&id)
            .and_then(|main| main.remote.as_ref())
            .and_then(|remote| remote.services.as_ref())
            .map(|(_, source)| source.take_tasks())
            .unwrap_or_default();
        for (request, title) in tasks {
            self.queue_remote_job(id, request, title);
        }
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
        self.save_remote_agent_tabs(id, &snapshot);
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
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
        let mut modal =
            workspaces_ui::CreateWorkspaceModal::new(repos, existing, crate::workspaces::namer());
        if matches!(remote.jira.known(), Some(Some(_))) {
            let board = (self.settings.jira_board != 0).then_some(self.settings.jira_board);
            modal = modal.with_tickets(workspaces_ui::TicketSource::reaching(
                &pom_paths::StateDir::from_env(),
                &remote.jira_session(),
                remote.jira.connect(),
                board,
                self.settings.jira_only_mine,
            ));
        }
        self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
    }

    /// The ticket a remote workspace's branch names, read with the host's Jira settings when it keeps them.
    fn open_remote_ticket(&mut self, id: WindowId, branch: &str) {
        let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) else {
            return;
        };
        let Some(key) = pom_jira::key_for_branch(branch) else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast(format!("{branch} names no Jira ticket"), None)
            });
            return;
        };
        let (session, connect) = (remote.jira_session(), remote.jira.connect());
        let item_id = jira_ui::item_id(&key);
        self.with_workspace_view(id, |view, _| {
            view.reveal_center_item(&item_id, || {
                Some(Box::new(jira_ui::TicketItem::reaching(
                    pom_paths::StateDir::from_env(),
                    session,
                    key,
                    Arc::new(ui::wake),
                    connect,
                )))
            })
        });
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
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
            workspace::RowAction::OpenTicket => {
                // A ticket opens in its own workspace, as it does here.
                if workspace.branch != remote.active {
                    self.activate_remote_workspace(id, index);
                }
                return self.open_remote_ticket(id, &workspace.branch);
            }
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
        let created = finished.iter().rev().find(|done| done.ok && done.created);
        if created.is_some() {
            if let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) {
                let snapshot = remote.watch.snapshot();
                self.save_remote_agent_tabs(id, &snapshot);
            }
        }
        let Some(remote) = self
            .mains
            .get_mut(&id)
            .and_then(|main| main.remote.as_mut())
        else {
            return;
        };
        if let Some(created) = created {
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

/// The Services panel over the host's server for the shown workspace, and its source; its Ports button opens
/// what of the host is forwarded here.
fn remote_services_panel(
    remote: &RemoteWindow,
    snapshot: &pom_remote::Snapshot,
) -> (
    Box<dyn workspace::SidePanelView>,
    Arc<services_ui::RemoteServices>,
) {
    let workspace = snapshot.workspaces.as_ref().and_then(|list| {
        list.workspaces
            .iter()
            .find(|workspace| workspace.branch == remote.active)
    });
    let mut source =
        services_ui::RemoteServices::new(remote.watch.clone(), &remote.active, &remote.host);
    let mut buttons = Vec::new();
    if let Some(ports) = remote.ports.clone() {
        source = source.with_ports(ports.clone());
        buttons.push(services_ui::TabButton {
            icon: ui::IconKind::ArrowUpRight,
            id: remote_ui::PORTS_TAB_ID.to_string(),
            open: Arc::new(move || {
                let open_url: Arc<dyn Fn(&str)> = Arc::new(|url: &str| {
                    if let Err(error) = desktop::open(url) {
                        eprintln!("open {url}: {error}");
                    }
                });
                Box::new(remote_ui::PortsPage::new(ports.clone(), open_url))
            }),
        });
    }
    let source = Arc::new(source);
    let panel = services_ui::ServicesPanel::with_access(
        source.clone(),
        services_ui::WorkspaceInfo {
            branch: remote.active.clone(),
            ticket: pom_jira::key_for_branch(&remote.active).unwrap_or_default(),
            is_main: workspace.is_none_or(|workspace| workspace.is_main),
        },
        Arc::new(ui::wake),
        workspace
            .map(|workspace| workspace.path.clone())
            .unwrap_or_default(),
    )
    .with_tab_buttons(buttons);
    (Box::new(panel), source)
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
