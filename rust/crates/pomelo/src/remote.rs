//! A window on a project that runs on another machine: what it reads comes from `pom` there over ssh, and the
//! features that need the project's files here stay hidden.

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
        let source =
            pom_remote::RemoteSource::new(host, picked.config.clone(), pom_remote::Ssh::system());
        let watch = pom_remote::Watch::start(Arc::new(source), "", Arc::new(ui::wake));
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
        if generation == remote.seen {
            return;
        }
        remote.seen = generation;
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
        );
        remote.notice_actions = actions;
        let title = remote_title(remote);
        main.window.set_title(&title);
        main.dirty = true;
        self.with_workspace_view(id, |view, _| {
            view.update_project(info);
            view.set_agent_states(dots);
            view.set_center_notice(Some(notice));
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
            None => {}
        }
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
