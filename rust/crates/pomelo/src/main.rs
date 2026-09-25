//! Pomelo app shell (cross-platform, winit + wgpu). This stage builds the LAYOUT: a top bar, a resizable left dock
//! (the workspace-list panel — our addition) that collapses to an icon rail instead of vanishing, and the
//! content area (colored placeholders for now). Panels/tabs, file tree, terminal and the ported core come next.
//! This crate is the composition root only: the framework lives in `ui`, the
//! layout/dock system in `workspace`, self-update in `auto_update`.

#[cfg(target_os = "macos")]
mod add_repo;
mod config_bundle;
mod notifications;
mod workspaces;

use std::sync::Arc;
use std::time::{Duration, Instant};

use settings::Settings;
use ui::UiRenderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};
use workspace::pane::PaneCommand;
use workspace::pane_group::SplitDirection;
use workspace::{DockPosition, EditKey, Layout};

enum EditorInput {
    Key(EditKey),
    Text(String),
}

/// Apply the persisted dock layout (widths, collapsed state, and the side/hidden of every button) onto a fresh
/// `Layout`, so the user's arrangement is restored on launch and for windows opened later.
fn apply_dock_settings(s: &Settings, layout: &mut Layout) {
    layout.left.width = s.left_dock_width;
    layout.left.collapsed = s.left_dock_collapsed;
    layout.right.width = s.right_dock_width;
    layout.right.collapsed = s.right_dock_collapsed;
    layout.bottom.collapsed = s.bottom_dock_collapsed;
    layout.sidebar_side = DockPosition::from_side(&s.sidebar_side);
    layout.agent_side = DockPosition::from_side(&s.agent_side);
    layout.terminal_side = DockPosition::from_side(&s.terminal_side);
    layout.agent_hidden = s.agent_hidden;
    layout.terminal_hidden = s.terminal_hidden;
    for (i, side) in s.func_sides.iter().enumerate() {
        if let Some(slot) = layout.func_side.get_mut(i) {
            *slot = DockPosition::from_side(side);
        }
    }
    for (i, hidden) in s.func_hidden.iter().enumerate() {
        if let Some(slot) = layout.func_hidden.get_mut(i) {
            *slot = *hidden;
        }
    }
}

fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn terminal_view(
    root: std::path::PathBuf,
    workspace_key: &str,
) -> Box<dyn workspace::TerminalPanelView> {
    let waker = std::sync::Arc::new(ui::wake);
    match std::env::current_exe() {
        Ok(binary) => Box::new(terminal_ui::TerminalPanel::with_holders(
            root,
            waker,
            terminal_ui::HolderScope {
                dir: pom_ptyhost::SocketDir::from_env(),
                binary,
                prefix: format!("term-{workspace_key}"),
            },
        )),
        Err(error) => {
            eprintln!("terminals will not survive a restart: {error}");
            Box::new(terminal_ui::TerminalPanel::new(root, waker))
        }
    }
}

/// The Git panel for the active workspace: every repo checked out there, against its default branch.
fn git_panel(
    project: &pom_core::Project,
    pull_requests: Option<&pull_request_ui::PullRequests>,
) -> Box<dyn workspace::SidePanelView> {
    let sources = project
        .active_workspace()
        .map(|workspace| {
            workspace
                .repos
                .iter()
                .map(|repo| git_ui::RepoSource {
                    name: repo.name.clone(),
                    root: repo.path.clone(),
                    default_branch: project.config.as_ref().map_or_else(
                        || "main".to_string(),
                        |config| config.default_branch_for(&repo.name).to_string(),
                    ),
                })
                .collect()
        })
        .unwrap_or_default();
    let reviews = pom_paths::StateDir::from_env().path(format!(
        "reviews/{}-{}.json",
        pom_env::branch_safe(&project.session),
        pom_env::branch_safe(project.active_branch())
    ));
    let panel = git_ui::GitPanel::new(sources, Some(reviews), Arc::new(ui::wake));
    Box::new(match pull_requests {
        Some(prs) => panel.with_pull_requests(prs.clone()),
        None => panel,
    })
}

/// The window's view of the project; `runner` (when services run) counts each workspace's running services.
fn project_info(
    project: &pom_core::Project,
    runner: Option<&pom_services::ServiceRunner>,
    tickets: Option<&workspaces_ui::TicketStatuses>,
    pull_requests: Option<&pull_request_ui::PullRequests>,
) -> workspace::ProjectInfo {
    let running_holders = runner
        .map(|runner| runner.running_holders())
        .unwrap_or_default();
    let running = project
        .workspaces
        .iter()
        .map(|workspace| {
            let (Some(runner), Some(config)) = (runner, project.config.as_ref()) else {
                return 0;
            };
            let target = |repo: &str, service: &str| pom_services::ServiceTarget {
                branch: workspace.branch.clone(),
                is_main: workspace.is_main,
                repo: repo.to_string(),
                service: service.to_string(),
            };
            let mut holders = Vec::new();
            for (repo, dir) in &config.repos {
                for service in dir.services.keys() {
                    holders.push(runner.holder_name(&target(repo, service)));
                }
            }
            for service in config.workspace_services.keys() {
                holders.push(runner.holder_name(&target("", service)));
            }
            holders
                .iter()
                .filter(|holder| running_holders.contains(holder))
                .count()
        })
        .collect();
    workspace::ProjectInfo {
        name: project.session.clone(),
        branch: project.branch().to_string(),
        config_path: project.config_path.clone(),
        workspaces: project
            .workspaces
            .iter()
            .map(|workspace| workspace.branch.clone())
            .collect(),
        active: project.active_branch().to_string(),
        labels: project
            .workspaces
            .iter()
            .map(|workspace| pom_layout::WorkspaceState::load(&workspace.path).display_name)
            .collect(),
        running,
        tickets: project
            .workspaces
            .iter()
            .map(|workspace| {
                tickets
                    .and_then(|tickets| tickets.status(&workspace.branch))
                    .unwrap_or_default()
            })
            .collect(),
        ticket_categories: project
            .workspaces
            .iter()
            .map(|workspace| {
                tickets
                    .and_then(|tickets| tickets.category(&workspace.branch))
                    .unwrap_or_default()
            })
            .collect(),
        prs: project
            .workspaces
            .iter()
            .map(|workspace| pull_requests.and_then(|prs| prs.summary(&workspace.branch)))
            .collect(),
        missing: project
            .workspaces
            .iter()
            .map(|workspace| project.missing_repos(workspace))
            .collect(),
    }
}

fn setup_summary(findings: &[pom_doctor::Finding]) -> Option<String> {
    let shown: Vec<&pom_doctor::Finding> = findings
        .iter()
        .filter(|finding| finding.severity != pom_doctor::Severity::Ok)
        .filter(|finding| !matches!(finding.id.as_str(), "config.validate" | "config.load"))
        .collect();
    if shown.is_empty() {
        return None;
    }
    let errors = shown
        .iter()
        .filter(|finding| finding.severity == pom_doctor::Severity::Error)
        .count();
    let warnings = shown.len() - errors;
    let mut counts = Vec::new();
    match errors {
        0 => {}
        1 => counts.push("1 error".to_string()),
        errors => counts.push(format!("{errors} errors")),
    }
    match warnings {
        0 => {}
        1 => counts.push("1 warning".to_string()),
        warnings => counts.push(format!("{warnings} warnings")),
    }
    let mut titles: Vec<&str> = shown
        .iter()
        .take(3)
        .map(|finding| finding.title.as_str())
        .collect();
    if shown.len() > 3 {
        titles.push("...");
    }
    Some(format!("{}: {}", counts.join(", "), titles.join("; ")))
}

fn config_problem(project: &pom_core::Project) -> Option<(String, Option<u32>)> {
    project
        .error
        .as_ref()
        .map(|problem| (problem.message.clone(), problem.line))
}

fn session_rows(
    entries: &[pom_core::SessionEntry],
    current: Option<&str>,
) -> (Vec<workspace::Session>, Option<usize>) {
    let rows: Vec<workspace::Session> = entries
        .iter()
        .map(|entry| workspace::Session {
            name: entry.name.clone(),
            path: entry.path.clone(),
            running: false,
            missing: entry.missing,
        })
        .collect();
    let index = current.and_then(|name| rows.iter().position(|row| row.name == name));
    (rows, index)
}

/// Which pane group focus the key arrives in: the terminal claims ctrl-alt-arrows and cmd-d for splitting and
/// leaves the cmd-k chord to its own clear, the editor uses the cmd-k chord and cmd-backslash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneKeyContext {
    Editor { after_cmd_k: bool },
    Terminal,
}

fn pane_key(
    event: &winit::event::KeyEvent,
    modifiers: terminal::Modifiers,
    context: PaneKeyContext,
) -> Option<PaneCommand> {
    let terminal::Modifiers {
        shift,
        alt,
        ctrl,
        cmd,
    } = modifiers;
    if event.logical_key == Key::Named(NamedKey::Escape) && shift && !cmd && !alt && !ctrl {
        return Some(PaneCommand::ToggleZoom);
    }
    let arrow = match &event.logical_key {
        Key::Named(NamedKey::ArrowLeft) => Some(SplitDirection::Left),
        Key::Named(NamedKey::ArrowRight) => Some(SplitDirection::Right),
        Key::Named(NamedKey::ArrowUp) => Some(SplitDirection::Up),
        Key::Named(NamedKey::ArrowDown) => Some(SplitDirection::Down),
        _ => None,
    };
    let plain = match event.key_without_modifiers() {
        Key::Character(c) => Some(c.to_string()),
        _ => None,
    };
    let chord = context == PaneKeyContext::Editor { after_cmd_k: true };
    if chord && event.logical_key == Key::Named(NamedKey::Enter) && shift && !cmd && !alt && !ctrl {
        return Some(PaneCommand::TogglePinTab);
    }
    if let (PaneKeyContext::Editor { after_cmd_k: true }, Some(direction)) = (context, arrow) {
        return match (cmd, shift, alt, ctrl) {
            (false, false, false, false) => Some(PaneCommand::Split(direction)),
            (true, false, false, false) => Some(PaneCommand::ActivatePane(direction)),
            (false, true, false, false) => Some(PaneCommand::SwapPane(direction)),
            _ => None,
        };
    }
    if cmd && alt && !ctrl && !shift {
        match arrow {
            Some(SplitDirection::Left) => return Some(PaneCommand::ActivatePreviousItem),
            Some(SplitDirection::Right) => return Some(PaneCommand::ActivateNextItem),
            _ => {}
        }
    }
    if context == PaneKeyContext::Terminal && ctrl && alt && !cmd && !shift {
        if let Some(direction) = arrow {
            return Some(PaneCommand::Split(direction));
        }
    }
    let plain = plain?;
    if ctrl && !cmd && !alt && !shift {
        return match plain.as_str() {
            "0" => Some(PaneCommand::ActivateLastItem),
            digit => digit
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .map(|n| PaneCommand::ActivateItem(n - 1)),
        };
    }
    if cmd && shift && !alt && !ctrl {
        return match plain.as_str() {
            "[" => Some(PaneCommand::ActivatePreviousItem),
            "]" => Some(PaneCommand::ActivateNextItem),
            _ => None,
        };
    }
    if cmd && !shift && !alt && !ctrl {
        return match (context, plain.as_str()) {
            (PaneKeyContext::Editor { .. }, "\\") => {
                Some(PaneCommand::Split(SplitDirection::Right))
            }
            (PaneKeyContext::Terminal, "d") => Some(PaneCommand::Split(SplitDirection::Right)),
            _ => None,
        };
    }
    None
}

/// A key press in keymap terms: the typed character (shift applied, so `?` not `/`) or the named key.
fn keymap_keystroke(
    event: &winit::event::KeyEvent,
    modifiers: terminal::Modifiers,
) -> Option<workspace::keymap::Keystroke> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    let key = match (&event.logical_key, event.key_without_modifiers()) {
        (Key::Character(typed), _) if typed.chars().all(|c| !c.is_control()) => {
            typed.to_lowercase()
        }
        (_, Key::Character(base)) => base.to_lowercase(),
        (Key::Named(_), _) => terminal_keystroke(event, modifiers)?.key,
        _ => return None,
    };
    Some(workspace::keymap::Keystroke {
        cmd: modifiers.cmd,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        key,
    })
}

/// A key press in the terminal's terms: named keys by name, character keys as typed without modifiers (so
/// ctrl/alt bindings see the base letter rather than a composed character).
fn terminal_keystroke(
    event: &winit::event::KeyEvent,
    modifiers: terminal::Modifiers,
) -> Option<terminal::Keystroke> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    let key = match event.key_without_modifiers() {
        Key::Character(c) => c.to_string(),
        Key::Named(named) => match named {
            NamedKey::Enter => "enter".into(),
            NamedKey::Tab => "tab".into(),
            NamedKey::Escape => "escape".into(),
            NamedKey::Backspace => "backspace".into(),
            NamedKey::Delete => "delete".into(),
            NamedKey::Insert => "insert".into(),
            NamedKey::Home => "home".into(),
            NamedKey::End => "end".into(),
            NamedKey::PageUp => "pageup".into(),
            NamedKey::PageDown => "pagedown".into(),
            NamedKey::ArrowUp => "up".into(),
            NamedKey::ArrowDown => "down".into(),
            NamedKey::ArrowLeft => "left".into(),
            NamedKey::ArrowRight => "right".into(),
            NamedKey::Space => "space".into(),
            NamedKey::F1 => "f1".into(),
            NamedKey::F2 => "f2".into(),
            NamedKey::F3 => "f3".into(),
            NamedKey::F4 => "f4".into(),
            NamedKey::F5 => "f5".into(),
            NamedKey::F6 => "f6".into(),
            NamedKey::F7 => "f7".into(),
            NamedKey::F8 => "f8".into(),
            NamedKey::F9 => "f9".into(),
            NamedKey::F10 => "f10".into(),
            NamedKey::F11 => "f11".into(),
            NamedKey::F12 => "f12".into(),
            NamedKey::F13 => "f13".into(),
            NamedKey::F14 => "f14".into(),
            NamedKey::F15 => "f15".into(),
            NamedKey::F16 => "f16".into(),
            NamedKey::F17 => "f17".into(),
            NamedKey::F18 => "f18".into(),
            NamedKey::F19 => "f19".into(),
            NamedKey::F20 => "f20".into(),
            _ => return None,
        },
        _ => return None,
    };
    Some(terminal::Keystroke::new(key, modifiers))
}

/// Read the current dock layout back into settings for persistence.
fn read_dock_settings(s: &mut Settings, layout: &Layout) {
    s.left_dock_width = layout.left.width;
    s.right_dock_width = layout.right.width;
    s.left_dock_collapsed = layout.left.collapsed;
    s.right_dock_collapsed = layout.right.collapsed;
    s.bottom_dock_collapsed = layout.bottom.collapsed;
    s.sidebar_side = layout.sidebar_side.as_str().into();
    s.agent_side = layout.agent_side.as_str().into();
    s.terminal_side = layout.terminal_side.as_str().into();
    s.agent_hidden = layout.agent_hidden;
    s.terminal_hidden = layout.terminal_hidden;
    s.func_sides = layout
        .func_side
        .iter()
        .map(|d| d.as_str().to_string())
        .collect();
    s.func_hidden = layout.func_hidden.clone();
}

/// One on-screen main window: its winit handle + GPU renderer, plus the framework `WindowHandle`/`WorkspaceView`
/// entity it drives. Many can coexist (one per session opened in a new window).
struct MainWindow {
    window: Arc<Window>,
    ui: UiRenderer,
    handle: ui::WindowHandle,
    entity: ui::Entity<workspace::WorkspaceView>,
    cursor: (f64, f64),
    dirty: bool,
    last_drawn: Instant,
    project: Option<pom_core::Project>,
    watcher: Option<pom_core::ConfigWatcher>,
    /// The project's other workspaces, keyed by folder, kept alive while this one has the window.
    parked: std::collections::HashMap<std::path::PathBuf, workspace::ParkedWorkspace>,
    services: Option<ProjectServices>,
    /// Running services that still have the config from before the last reload.
    stale: Vec<pom_services::ServiceTarget>,
    /// Workspace creations and deletions started from this window.
    ops: workspaces_ui::OpQueue,
    /// The project's Jira ticket status per workspace.
    tickets: Option<workspaces_ui::TicketStatuses>,
    pull_requests: Option<pull_request_ui::PullRequests>,
    doctor: Option<std::sync::mpsc::Receiver<Vec<pom_doctor::Finding>>>,
    doctor_findings: Vec<pom_doctor::Finding>,
    /// When the Services badge last checked the active workspace's services.
    services_checked: Option<Instant>,
    /// Refresh-main, auto-push and the port reaper, when this process holds the session's primary lock.
    background: Option<workspaces_ui::BackgroundSync>,
}

struct ProjectServices {
    runner: Arc<pom_services::ServiceRunner>,
    config: services_ui::SharedConfig,
}

impl ProjectServices {
    fn new(project: &pom_core::Project, state: &pom_paths::StateDir) -> Option<ProjectServices> {
        let binary = std::env::current_exe()
            .map_err(|error| eprintln!("services cannot start without the app binary: {error}"))
            .ok()?;
        let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
            project_root: project.root.clone(),
            session: project.session.clone(),
            state: state.clone(),
            holders: pom_ptyhost::SocketDir::from_env(),
            binary,
            docker: "docker".into(),
        });
        Some(ProjectServices {
            runner: Arc::new(runner),
            config: Arc::new(std::sync::RwLock::new(project.config.clone().map(Arc::new))),
        })
    }

    fn update_config(&self, project: &pom_core::Project) {
        if let Ok(mut config) = self.config.write() {
            *config = project.config.clone().map(Arc::new);
        }
    }

    fn database_panel(&self, project: &pom_core::Project) -> Box<dyn workspace::SidePanelView> {
        let config = self.config.clone();
        Box::new(database_ui::DatabasePanel::new(
            database_ui::DatabaseContext {
                runner: self.runner.clone(),
                state: pom_paths::StateDir::from_env(),
                config: Arc::new(move || config.read().ok().and_then(|config| config.clone())),
                branch: project.active_branch().to_string(),
                waker: Arc::new(ui::wake),
            },
        ))
    }

    fn panel(&self, project: &pom_core::Project) -> Box<dyn workspace::SidePanelView> {
        let is_main = project
            .active_workspace()
            .is_none_or(|workspace| workspace.is_main);
        let config = self.config.clone();
        let environment = environment_ui::EnvironmentContext {
            state: pom_paths::StateDir::from_env(),
            runner: self.runner.clone(),
            config: Arc::new(move || config.read().ok().and_then(|config| config.clone())),
            workspaces: project
                .workspaces
                .iter()
                .map(|workspace| (workspace.branch.clone(), workspace.is_main))
                .collect(),
            branch: project.active_branch().to_string(),
        };
        let for_secrets = environment.clone();
        let tabs = vec![
            services_ui::TabButton {
                icon: ui::IconKind::Key,
                id: "secrets".into(),
                open: Arc::new(move || {
                    Box::new(environment_ui::SecretsItem::new(for_secrets.clone()))
                }),
            },
            services_ui::TabButton {
                icon: ui::IconKind::Server,
                id: "environment".into(),
                open: Arc::new(move || Box::new(environment_ui::EnvItem::new(environment.clone()))),
            },
        ];
        Box::new(
            services_ui::ServicesPanel::new(
                services_ui::ServicesContext {
                    runner: self.runner.clone(),
                    config: self.config.clone(),
                    branch: project.active_branch().to_string(),
                    is_main,
                    waker: Arc::new(ui::wake),
                },
                project.active_root(),
            )
            .with_tab_buttons(tabs),
        )
    }
}

/// A new project being cloned and detected off the main thread, and the window that asked for it.
struct Scaffolding {
    window: WindowId,
    name: String,
    use_ai: bool,
    receiver: std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>,
}

#[derive(Default)]
struct App {
    // All main windows share one reactive `Application` (the framework's single App, many windows); each `MainWindow`
    // drives a `WorkspaceView` entity. The binary owns the winit windows + GPU renderers.
    main_app: Option<ui::Application>,
    mains: std::collections::HashMap<WindowId, MainWindow>,
    settings: Settings,
    // Settings is its own OS window (opened on Cmd+,), not an overlay on the main window. Its state + input
    // live in a reactive `SettingsView` driven by a `ui::Application`; the binary only owns the winit window +
    // GPU renderer and pumps draw/input into the framework.
    settings_window: Option<Arc<Window>>,
    settings_ui: Option<UiRenderer>,
    settings_app: Option<ui::Application>,
    settings_handle: Option<ui::WindowHandle>,
    settings_entity: Option<ui::Entity<settings_ui::SettingsView>>,
    caret_last_toggle: Option<Instant>,
    /// Cmd+K was pressed; the next key completes a two-stroke editor binding.
    pending_cmd_k: bool,
    settings_dirty: bool,
    settings_cursor: (f64, f64),
    super_down: bool,
    shift_down: bool,
    alt_down: bool,
    ctrl_down: bool,
    agents: AgentTracker,
    next_agent_item: u64,
    scaffolding: Option<Scaffolding>,
    adding_repo: Option<add_repo::AddingRepo>,
    cloning_repos: Option<add_repo::CloningRepos>,
    keymap: workspace::keymap::Keymap,
    /// The first keys of a longer binding typed so far (`cmd-k` of `cmd-k cmd-s`).
    pending_keys: Vec<workspace::keymap::Keystroke>,
    keymap_problems: Vec<String>,
    /// When the user's keymap file was last read, to pick up edits.
    keymap_read: Option<std::time::SystemTime>,
    keymap_checked: Option<Instant>,
    dev_proxy: Option<pom_proxy::DevProxy>,
    agent_registration: Arc<std::sync::Mutex<settings_ui::AgentPage>>,
    settings_pages_at: Option<Instant>,
    /// The main window last focused: Settings edits its project's Jira settings.
    focused_main: Option<WindowId>,
}

/// What each workspace's coding agent last reported, and the watcher that says when it changes.
#[derive(Default)]
struct AgentTracker {
    watcher: Option<pom_agent::AgentWatcher>,
    /// Keyed by (session, branch): two projects can have a workspace on the same branch.
    states: std::collections::HashMap<(String, String), pom_agent::AgentState>,
    read_at: Option<Instant>,
    /// The first read only learns the current states; notifications start with the next change.
    primed: bool,
}

const AGENT_RECHECK: Duration = Duration::from_secs(5);

/// After a config change: rewrites every workspace's env files, then lists running services whose command or
/// env no longer matches what they were started with.
fn refresh_env_and_find_stale(
    runner: &pom_services::ServiceRunner,
    project: &pom_core::Project,
) -> Vec<pom_services::ServiceTarget> {
    let Some(config) = project.config.as_ref().filter(|_| project.error.is_none()) else {
        return Vec::new();
    };
    let mut stale = Vec::new();
    for workspace in &project.workspaces {
        if let Err(error) = runner.refresh_workspace_env(config, &workspace.branch) {
            eprintln!("env for {}: {error}", workspace.branch);
        }
        let targets = pom_services::ServiceRunner::service_targets(
            config,
            &workspace.branch,
            workspace.is_main,
        );
        stale.extend(runner.stale_services(config, &targets));
    }
    stale
}

fn stale_name(target: &pom_services::ServiceTarget, project: &pom_core::Project) -> String {
    let service = if target.repo.is_empty() {
        target.service.clone()
    } else {
        format!("{}/{}", target.repo, target.service)
    };
    if target.branch == project.active_branch() {
        service
    } else {
        format!("{service} ({})", target.branch)
    }
}

/// Claude Code is installed where the agent launcher looks for it.
fn claude_installed() -> bool {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    pom_agent::claude_available(&pom_agent::resolve_claude(&home, pom_services::tool_path()))
}

fn agent_dot(state: pom_agent::AgentState) -> workspace::AgentDot {
    match state {
        pom_agent::AgentState::Idle => workspace::AgentDot::Idle,
        pom_agent::AgentState::Thinking => workspace::AgentDot::Thinking,
        pom_agent::AgentState::ToolUse => workspace::AgentDot::ToolUse,
        pom_agent::AgentState::Compacting => workspace::AgentDot::Compacting,
        pom_agent::AgentState::AwaitingInput => workspace::AgentDot::AwaitingInput,
    }
}

impl App {
    /// Re-reads agent states after a hook wrote one: updates every window's workspace dots and tells
    /// the user about transitions in workspaces they are not looking at.
    fn refresh_agents(&mut self) {
        if self.agents.watcher.is_none() {
            match pom_agent::AgentWatcher::new(&pom_paths::StateDir::from_env(), Arc::new(ui::wake))
            {
                Ok(watcher) => {
                    self.agents.watcher = Some(watcher);
                    // An agent that exits writes nothing, and a stale working state only decays with time.
                    std::thread::spawn(|| loop {
                        std::thread::sleep(AGENT_RECHECK);
                        ui::wake();
                    });
                }
                Err(error) => {
                    eprintln!("agent states will not update: {error}");
                    return;
                }
            }
        }
        let changed = self
            .agents
            .watcher
            .as_ref()
            .is_some_and(pom_agent::AgentWatcher::take_changed);
        let due = self
            .agents
            .read_at
            .is_none_or(|at| at.elapsed() >= AGENT_RECHECK);
        if !changed && !due {
            return;
        }
        self.agents.read_at = Some(Instant::now());
        let reported: std::collections::HashMap<String, pom_agent::AgentState> =
            pom_agent::read_states(&pom_paths::StateDir::from_env())
                .into_iter()
                .map(|status| (status.branch, status.state))
                .collect();
        let holders: Vec<String> = pom_ptyhost::SocketDir::from_env()
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let mut fresh = std::collections::HashMap::new();
        let mut notices = Vec::new();
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            let Some(main) = self.mains.get(&id) else {
                continue;
            };
            let Some(project) = main.project.as_ref() else {
                continue;
            };
            let focused = main.window.has_focus();
            let active = project.active_branch().to_string();
            let session = project.session.clone();
            let mut dots = std::collections::HashMap::new();
            for workspace in &project.workspaces {
                let running = holders
                    .iter()
                    .any(|name| pom_agent::is_agent_holder(name, &session, &workspace.branch));
                if !running {
                    continue;
                }
                let state = reported
                    .get(&workspace.branch)
                    .copied()
                    .unwrap_or(pom_agent::AgentState::Idle);
                let key = (session.clone(), workspace.branch.clone());
                dots.insert(workspace.branch.clone(), agent_dot(state));
                let before = self.agents.states.get(&key).copied();
                fresh.insert(key, state);
                if !self.agents.primed || before == Some(state) {
                    continue;
                }
                let Some((title, event)) = pom_agent::notification_for(before, state) else {
                    continue;
                };
                let viewing = focused && workspace.branch == active;
                if !self.settings.announces(viewing) {
                    continue;
                }
                notices.push((
                    title,
                    event,
                    format!("{session} - {}", workspace.branch),
                    workspace.branch.clone(),
                ));
            }
            self.with_workspace_view(id, |view, _| view.set_agent_states(dots));
        }
        #[cfg(target_os = "macos")]
        for (title, event, body, branch) in notices {
            if event != "working" {
                notifications::post(title, &body, &branch);
            }
            // The dev build shares the installed app's agent state; only one of them should chime.
            let sound = self.settings.sound_for(event);
            if !sound.is_empty() && !is_dev_build() {
                notifications::play_sound(sound);
            }
        }
        self.agents.states = fresh;
        self.agents.primed = true;
    }

    /// Restarts the services that still ran the previous config, off the UI thread.
    fn restart_stale(&mut self, id: WindowId) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let stale = std::mem::take(&mut main.stale);
        let (Some(services), Some(config)) = (
            main.services.as_ref(),
            main.project
                .as_ref()
                .and_then(|project| project.config.clone()),
        ) else {
            return;
        };
        let runner = services.runner.clone();
        std::thread::spawn(move || {
            for target in &stale {
                if let Err(error) = runner.restart(&config, target) {
                    eprintln!("restart {}/{}: {error}", target.repo, target.service);
                }
            }
            ui::wake();
        });
        self.with_workspace_view(id, |view, _| view.set_stale_services(&[]));
    }

    /// The manual path after a new project: show its services and say what was drafted and what to do next.
    fn review_drafted_config(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let (repos, services) = project.config.as_ref().map_or((0, 0), |config| {
            let services: usize = config.repos.values().map(|repo| repo.services.len()).sum();
            (config.repos.len(), services)
        });
        let plural = |count: usize, noun: &str| {
            if count == 1 {
                format!("1 {noun}")
            } else {
                format!("{count} {noun}s")
            }
        };
        let message = format!(
            "Drafted from what was detected ({}, {}). Review it, then start services.",
            plural(repos, "repo"),
            plural(services, "service")
        );
        let path = project.config_path.clone();
        self.with_workspace_view(id, |view, _| {
            view.run_action(workspace::keymap::Action::FocusServices);
            view.notify_with_file(
                "pom.yml is ready to review",
                message,
                "Open pom.yml",
                path,
                None,
            );
        });
    }

    /// Opens the project's `pom.yml` for editing in the active pane; it stays editable even from main.
    pub(crate) fn open_project_config(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let root = project.root.clone();
        let mut files = vec![project.config_path.clone()];
        files.extend(pom_config::fragment_files(&root).unwrap_or_default());
        self.with_workspace_view(id, |view, _| view.pick_config_file(&root, files));
    }

    /// Opens the workspace's coding agent in the terminal panel, or focuses its tab. The agent runs in
    /// its own holder, so it survives the app and the tab reattaches to it.
    fn open_agent(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let (Ok(binary), Some(home)) = (std::env::current_exe(), std::env::var_os("HOME")) else {
            return;
        };
        let home = std::path::PathBuf::from(home);
        let state = pom_paths::StateDir::from_env();
        let cwd = project.active_root();
        let branch = project.active_branch().to_string();
        let is_main = project
            .active_workspace()
            .is_none_or(|workspace| workspace.is_main);
        let command = self.settings.agent_command.trim().to_string();
        let mut words = command.split_whitespace();
        let program = words.next().unwrap_or("claude");
        let is_claude = std::path::Path::new(program)
            .file_name()
            .is_some_and(|name| name == "claude");
        let launch = if is_claude {
            let mut launch = pom_agent::claude_launch(&pom_agent::LaunchContext {
                state: &state,
                home: &home,
                binary: &binary,
                tool_path: pom_services::tool_path(),
                session: &project.session,
                branch: &branch,
                is_main,
                cwd: &cwd,
            });
            let extra: Vec<&str> = words.collect();
            if let (false, Some(script)) = (extra.is_empty(), launch.argv.last_mut()) {
                script.push(' ');
                script.push_str(&extra.join(" "));
            }
            launch
        } else {
            let name = std::path::Path::new(program).file_name().map_or_else(
                || "agent".to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            pom_agent::AgentLaunch {
                holder: format!(
                    "ws-{}-{}-agent-{name}",
                    pom_env::branch_safe(&project.session),
                    pom_env::branch_safe(&branch)
                ),
                cwd: cwd.clone(),
                argv: vec![
                    "zsh".into(),
                    "-lc".into(),
                    format!(
                        "export PATH='{}'; exec {command}",
                        pom_services::tool_path().replace('\'', r"'\''")
                    ),
                ],
                title: name,
            }
        };
        self.open_agent_item(id, launch);
    }

    fn open_fixer(&mut self, id: WindowId) {
        let prompt = match self.mains.get(&id) {
            Some(main) => pom_doctor::fix_prompt(&main.doctor_findings),
            None => return,
        };
        self.open_task_agent(id, |context| {
            pom_agent::claude_task_launch(context, "fixer", &prompt)
        });
    }

    fn open_onboarder(&mut self, id: WindowId) {
        self.open_task_agent(id, pom_agent::onboard_launch);
    }

    fn open_task_agent(
        &mut self,
        id: WindowId,
        launch: impl FnOnce(&pom_agent::LaunchContext<'_>) -> pom_agent::AgentLaunch,
    ) {
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let Some(project) = main.project.as_ref() else {
            return;
        };
        let (Ok(binary), Some(home)) = (std::env::current_exe(), std::env::var_os("HOME")) else {
            return;
        };
        let home = std::path::PathBuf::from(home);
        let state = pom_paths::StateDir::from_env();
        let cwd = project.active_root();
        let branch = project.active_branch().to_string();
        let is_main = project
            .active_workspace()
            .is_none_or(|workspace| workspace.is_main);
        let mut launch = launch(&pom_agent::LaunchContext {
            state: &state,
            home: &home,
            binary: &binary,
            tool_path: pom_services::tool_path(),
            session: &project.session,
            branch: &branch,
            is_main,
            cwd: &cwd,
        });
        launch.holder = format!("{}-{}", launch.holder, self.next_agent_item);
        self.open_agent_item(id, launch);
    }

    fn start_doctor(&mut self, id: WindowId) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let Some(project) = main.project.as_ref() else {
            main.doctor = None;
            return;
        };
        let config = project.config.clone();
        let config_path = project.config_path.clone();
        let root = project.root.clone();
        let session = project.session.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pom-doctor".into())
            .spawn(move || {
                let names =
                    pom_secrets::SecretStore::new(pom_paths::StateDir::from_env(), &session)
                        .names()
                        .unwrap_or_default();
                let path = pom_services::tool_path();
                let has_tool = |tool: &str| pom_doctor::on_path(path, tool);
                let docker_running = || pom_doctor::docker_answers(path);
                let findings = pom_doctor::diagnose(
                    config.as_ref(),
                    &config_path,
                    &root,
                    &names,
                    &pom_doctor::Machine {
                        has_tool: &has_tool,
                        docker_running: &docker_running,
                    },
                );
                if sender.send(findings).is_ok() {
                    ui::wake();
                }
            });
        match spawned {
            Ok(_) => main.doctor = Some(receiver),
            Err(error) => eprintln!("doctor: {error}"),
        }
    }

    pub(crate) fn poll_doctor(&mut self, id: WindowId) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let Some(findings) = main
            .doctor
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        else {
            return;
        };
        main.doctor = None;
        main.doctor_findings = findings;
        let summary = setup_summary(&main.doctor_findings);
        main.dirty = true;
        self.with_workspace_view(id, |view, _| view.set_setup_problems(summary));
    }

    fn open_agent_item(&mut self, id: WindowId, launch: pom_agent::AgentLaunch) {
        let Ok(binary) = std::env::current_exe() else {
            return;
        };
        let item_id = format!("agent:{}", launch.holder);
        let item_number = self.next_agent_item;
        self.next_agent_item += 1;
        self.with_workspace_view(id, |view, _| {
            view.open_agent_item(&item_id, || {
                let options = terminal::HolderOptions {
                    dir: pom_ptyhost::SocketDir::from_env(),
                    name: launch.holder.clone(),
                    binary: binary.clone(),
                    attach_only: false,
                };
                match terminal_ui::TerminalItem::agent(
                    item_number,
                    launch.cwd.clone(),
                    item_id.clone(),
                    launch.title.clone(),
                    options,
                    launch.argv.clone(),
                    Arc::new(ui::wake),
                ) {
                    Ok(item) => Some(Box::new(item) as Box<dyn workspace::Item>),
                    Err(error) => {
                        eprintln!("could not open the agent: {error}");
                        None
                    }
                }
            });
        });
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    /// A clicked agent notification brings its workspace forward.
    fn open_clicked_notification(&mut self) {
        #[cfg(target_os = "macos")]
        let Some(branch) = notifications::take_clicked() else {
            return;
        };
        #[cfg(not(target_os = "macos"))]
        let branch = String::new();
        let found = self.mains.iter().find_map(|(id, main)| {
            let index = main
                .project
                .as_ref()?
                .workspaces
                .iter()
                .position(|workspace| workspace.branch == branch)?;
            Some((*id, index))
        });
        if let Some((id, index)) = found {
            if let Some(main) = self.mains.get(&id) {
                main.window.focus_window();
            }
            self.activate_workspace(id, index);
        }
    }

    /// Create a real OS window hosting a `WorkspaceView` for `layout`, wire its renderer + macOS chrome, and
    /// register it. Returns its `WindowId`. Used both for the first window and for "Open in new window".
    fn new_main_window(&mut self, event_loop: &ActiveEventLoop, layout: Layout) -> WindowId {
        let mut attrs = Window::default_attributes()
            .with_title("Pomelo")
            .with_inner_size(LogicalSize::new(
                self.settings.window_width,
                self.settings.window_height,
            ));
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        // Input methods (e.g. Vietnamese Telex) compose through Ime events; plain keys still arrive as input.
        window.set_ime_allowed(true);
        let id = window.id();
        let mut renderer = UiRenderer::new(window.clone()).expect("ui");
        renderer.set_ui_font(&self.settings.ui_font);
        let (w, h) = renderer.size();
        let scale = window.scale_factor() as f32;
        #[cfg(target_os = "macos")]
        {
            center_traffic_lights(&window);
            pin_layer_top_left(&window);
        }
        let app = self.main_app.get_or_insert_with(ui::Application::new);
        let (handle, entity) = app.open_raw_window::<workspace::WorkspaceView>(
            ui::WindowOptions {
                title: "Pomelo".into(),
                width: w,
                height: h,
                scale,
            },
            move |_| workspace::WorkspaceView::new(layout),
        );
        self.mains.insert(
            id,
            MainWindow {
                window,
                ui: renderer,
                handle,
                entity,
                cursor: (0.0, 0.0),
                dirty: false,
                last_drawn: Instant::now(),
                project: None,
                watcher: None,
                parked: std::collections::HashMap::new(),
                services: None,
                stale: Vec::new(),
                ops: workspaces_ui::OpQueue::new(Arc::new(ui::wake)),
                tickets: None,
                pull_requests: None,
                doctor: None,
                doctor_findings: Vec::new(),
                services_checked: None,
                background: None,
            },
        );
        let bindings = self.keymap_bindings();
        self.with_workspace_view(id, |view, _| view.set_bindings(bindings));
        id
    }

    /// What each window action is bound to now, for the command palette.
    fn keymap_bindings(&self) -> Vec<(workspace::keymap::Action, String)> {
        workspace::keymap::Action::ALL
            .iter()
            .filter_map(|action| Some((*action, self.keymap.binding_for(*action)?)))
            .collect()
    }

    fn open_project_in(&mut self, id: WindowId, config: Option<std::path::PathBuf>) {
        let state = pom_paths::StateDir::from_env();
        let project = config.map(|config| pom_core::Project::open(&config, &state));
        if let Some(project) = &project {
            if let Err(error) = project.record_open(&state, now_seconds()) {
                eprintln!("failed to record the opened project: {error}");
            }
        }
        let watcher = project.as_ref().and_then(|project| {
            pom_core::ConfigWatcher::new(&project.root, Arc::new(ui::wake))
                .map_err(|error| eprintln!("config watch failed: {error}"))
                .ok()
        });
        let services = project
            .as_ref()
            .and_then(|project| ProjectServices::new(project, &state));
        let tickets = project.as_ref().map(|project| {
            workspaces_ui::TicketStatuses::new(state.clone(), &project.session, Arc::new(ui::wake))
        });
        let pull_requests = project.as_ref().map(|project| {
            pull_request_ui::PullRequests::new(state.clone(), &project.session, Arc::new(ui::wake))
        });
        let background = services
            .as_ref()
            .zip(self.mains.get(&id))
            .map(|(services, main)| {
                let config = services.config.clone();
                workspaces_ui::BackgroundSync::start(workspaces_ui::BackgroundContext {
                    runner: services.runner.clone(),
                    state: state.clone(),
                    config: Arc::new(move || config.read().ok().and_then(|config| config.clone())),
                    ops: main.ops.clone(),
                })
            });
        if let Some(main) = self.mains.get_mut(&id) {
            main.background = background;
            main.tickets = tickets;
            main.pull_requests = pull_requests;
            main.project = project;
            main.watcher = watcher;
            main.parked.clear();
            main.services = services;
        }
        self.install_project_views(id);
        self.refresh_sessions();
        self.sync_dev_proxy();
    }

    /// Pushes the editor and terminal settings to their crates; open files lay out again when the font changed.
    fn apply_editor_defaults(&mut self) {
        let font_changed = files_ui::set_editor_defaults(
            self.settings.buffer_font_size,
            self.settings.soft_wrap,
            self.settings.split_diff,
        );
        terminal_ui::set_terminal_defaults(
            self.settings.terminal_font_size,
            &self.settings.terminal_shell,
            self.settings.terminal_scrollback as usize,
        );
        if font_changed {
            let windows: Vec<WindowId> = self.mains.keys().copied().collect();
            for id in windows {
                self.with_workspace_view(id, |view, _| view.editor_metrics_changed());
            }
        }
        self.mark_all_mains_dirty();
    }

    /// Opens the user's keymap file in the editor, starting it from a commented example when missing.
    fn edit_keymap(&mut self) {
        let Some(path) = workspace::keymap::Keymap::user_file() else {
            return;
        };
        if !path.exists() {
            let example = "[\n  {\n    \"context\": \"Workspace\",\n    \"bindings\": {}\n  }\n]\n";
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, example));
            if let Err(error) = written {
                eprintln!("keymap: create {}: {error}", path.display());
                return;
            }
        }
        let Some(id) = self
            .focused_main
            .or_else(|| self.mains.keys().next().copied())
        else {
            return;
        };
        self.with_workspace_view(id, |view, _| view.open_file(&path));
        if let Some(main) = self.mains.get(&id) {
            main.window.focus_window();
        }
        self.mark_all_mains_dirty();
    }

    /// Reads the keymap file again when it changed since the last read (it is edited while the app runs).
    fn reload_keymap_if_changed(&mut self) {
        if self
            .keymap_checked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.keymap_checked = Some(Instant::now());
        let modified = workspace::keymap::Keymap::user_file()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok());
        if modified == self.keymap_read {
            return;
        }
        self.keymap_read = modified;
        let (keymap, problems) = workspace::keymap::Keymap::load();
        self.keymap = keymap;
        self.keymap_problems = problems;
        self.pending_keys.clear();
        let bindings = self.keymap_bindings();
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            let bindings = bindings.clone();
            self.with_workspace_view(id, |view, _| view.set_bindings(bindings));
        }
    }

    /// Opens the active file (else the workspace) in the external editor from the settings, or the first
    /// known one installed.
    fn open_in_external_editor(&mut self, id: WindowId) {
        let Some(target) = self
            .with_workspace_view(id, |view, _| view.external_target())
            .flatten()
        else {
            return;
        };
        let installed = |name: &str| {
            std::path::Path::new(&format!("/Applications/{name}.app")).exists()
                || std::env::var_os("HOME").is_some_and(|home| {
                    std::path::Path::new(&home)
                        .join(format!("Applications/{name}.app"))
                        .exists()
                })
        };
        let editor = if self.settings.external_editor.is_empty() {
            settings_ui::EXTERNAL_EDITORS
                .into_iter()
                .find(|name| installed(name))
                .map(str::to_string)
        } else {
            Some(self.settings.external_editor.clone())
        };
        let Some(editor) = editor else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast(
                    "No external editor found; pick one in Settings > Editor",
                    None,
                )
            });
            return;
        };
        if let Err(error) = std::process::Command::new("open")
            .arg("-a")
            .arg(&editor)
            .arg(&target)
            .spawn()
        {
            let message = format!("Failed to open {editor}: {error}");
            self.with_workspace_view(id, |view, _| view.show_toast(message, None));
        }
    }

    /// Binds again after a port another instance held was freed.
    fn restart_dev_proxy(&mut self) {
        self.dev_proxy = None;
        self.sync_dev_proxy();
    }

    /// Hands the settings window what it shows from outside the settings file: agent registration and the
    /// servers' state and traffic. Throttled, since it runs on every loop turn.
    fn refresh_settings_pages(&mut self) {
        const INTERVAL: Duration = Duration::from_millis(500);
        const REQUESTS_SHOWN: usize = 50;
        if self.settings_entity.is_none()
            || self
                .settings_pages_at
                .is_some_and(|at| at.elapsed() < INTERVAL)
        {
            return;
        }
        self.settings_pages_at = Some(Instant::now());
        let agent = self
            .agent_registration
            .lock()
            .map(|page| page.clone())
            .unwrap_or_default();
        let ports = self
            .dev_proxy
            .as_ref()
            .map_or_else(pom_proxy::Ports::from_env, pom_proxy::DevProxy::ports);
        let network = settings_ui::NetworkPage {
            proxy_running: self
                .dev_proxy
                .as_ref()
                .is_some_and(pom_proxy::DevProxy::proxy_running),
            webhook_running: self
                .dev_proxy
                .as_ref()
                .is_some_and(pom_proxy::DevProxy::webhook_running),
            proxy_port: ports.proxy,
            webhook_port: ports.webhook,
            served_elsewhere: !self
                .dev_proxy
                .as_ref()
                .is_some_and(pom_proxy::DevProxy::proxy_running)
                && pom_proxy::listening(ports.proxy),
            requests: self
                .dev_proxy
                .as_ref()
                .map(|proxy| proxy.log(REQUESTS_SHOWN))
                .unwrap_or_default()
                .into_iter()
                .map(|entry| settings_ui::RequestRow {
                    time: entry.time,
                    method: entry.method,
                    path: entry.path,
                    profile: entry.profile,
                    target: entry.target,
                    status: entry.status,
                    ms: entry.ms,
                })
                .collect(),
        };
        let general = settings_ui::GeneralPage {
            start_at_login: start_at_login(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            updates_apply: installed_app().is_some(),
        };
        let keymap = settings_ui::KeymapPage {
            rows: workspace::keymap::Action::ALL
                .iter()
                .map(|action| {
                    (
                        action.label().to_string(),
                        action.name().to_string(),
                        self.keymap.binding_for(*action).unwrap_or_default(),
                    )
                })
                .collect(),
            problems: self.keymap_problems.clone(),
        };
        let changed = self.with_settings_view(|view, _| {
            let agent_changed = view.set_agent_page(agent);
            let general_changed = view.set_general_page(general);
            let keymap_changed = view.set_keymap_page(keymap);
            view.set_network_page(network) || agent_changed || general_changed || keymap_changed
        });
        if changed == Some(true) {
            self.settings_dirty = true;
        }
    }

    /// Points the dev proxy and webhook relay at the projects the windows have open, starting them with the
    /// first project.
    fn sync_dev_proxy(&mut self) {
        let projects: Vec<pom_proxy::ProjectRoute> = self
            .mains
            .values()
            .filter_map(|main| {
                Some(pom_proxy::ProjectRoute {
                    root: main.project.as_ref()?.root.clone(),
                    config: main.services.as_ref()?.config.clone(),
                })
            })
            .collect();
        if self.dev_proxy.is_none() && !projects.is_empty() {
            let machine = pom_proxy::SystemMachine {
                state: pom_paths::StateDir::from_env(),
                holders: pom_ptyhost::SocketDir::from_env(),
            };
            match pom_proxy::DevProxy::start(Box::new(machine), pom_proxy::Ports::from_env()) {
                Ok(proxy) => self.dev_proxy = Some(proxy),
                Err(error) => eprintln!("dev proxy failed to start: {error}"),
            }
        }
        if let Some(proxy) = &self.dev_proxy {
            proxy.set_projects(projects);
        }
    }

    /// Root the window's file tree and terminal in its active workspace (or show the welcome page).
    fn install_project_views(&mut self, id: WindowId) {
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let project = main.project.as_ref();
        let runner = main
            .services
            .as_ref()
            .map(|services| services.runner.as_ref());
        let info = project.map(|project| {
            project_info(
                project,
                runner,
                main.tickets.as_ref(),
                main.pull_requests.as_ref(),
            )
        });
        let problem = project.and_then(config_problem);
        let config_path = project
            .map(|project| project.config_path.clone())
            .unwrap_or_default();
        let workspace_root = project.map(pom_core::Project::active_root);
        let is_main = project.is_some_and(|project| {
            project
                .active_workspace()
                .is_none_or(|workspace| workspace.is_main)
        });
        let files: Option<Box<dyn workspace::FunctionView>> = workspace_root.clone().map(|root| {
            let project_dir = config_path.parent().map(std::path::Path::to_path_buf);
            let writable = project_dir
                .map(|dir| vec![config_path.clone(), dir.join("pom.d")])
                .unwrap_or_default();
            let checked_config = config_path.clone();
            let check: files_ui::SaveCheck = Arc::new(move |path, text| {
                if !pom_config::edit::config_files(&checked_config)
                    .iter()
                    .any(|file| file == path)
                {
                    return Ok(());
                }
                pom_config::edit::check_file_edit(&checked_config, path, text).map(|_| ())
            });
            Box::new(
                files_ui::FilesView::new(root)
                    .read_only(is_main)
                    .writable(writable)
                    .save_check(check),
            ) as Box<dyn workspace::FunctionView>
        });
        let mut side_panels: Vec<Box<dyn workspace::SidePanelView>> = Vec::new();
        if let (Some(services), Some(project)) = (&main.services, project) {
            side_panels.push(services.panel(project));
            side_panels.push(services.database_panel(project));
        }
        if let Some(project) = project {
            side_panels.push(git_panel(project, main.pull_requests.as_ref()));
        }
        let terminal_root = workspace_root.unwrap_or_else(home_dir);
        let workspace_key = project.map_or_else(
            || "home".to_string(),
            |project| {
                format!(
                    "{}-{}",
                    pom_env::branch_safe(&project.session),
                    pom_env::branch_safe(project.active_branch())
                )
            },
        );
        let title = project.map_or_else(
            || "Pomelo".to_string(),
            |project| format!("{} - {} - Pomelo", project.session, project.active_branch()),
        );
        self.with_workspace_view(id, |view, _| {
            view.set_project(
                info,
                files,
                Some(terminal_view(terminal_root.clone(), &workspace_key)),
                Some(Box::new(terminal_ui::TerminalPanel::agent_dock(
                    terminal_root,
                    Arc::new(ui::wake),
                ))),
            );
            view.set_side_panels(side_panels);
            view.set_ai_available(claude_installed());
            view.set_config_problem(problem, &config_path);
        });
        if let Some(main) = self.mains.get_mut(&id) {
            main.window.set_title(&title);
            main.dirty = true;
        }
        self.start_doctor(id);
    }

    fn activate_workspace(&mut self, id: WindowId, index: usize) {
        let state = pom_paths::StateDir::from_env();
        let Some(project) = self.mains.get_mut(&id).and_then(|m| m.project.as_mut()) else {
            return;
        };
        let Some(branch) = project.workspaces.get(index).map(|w| w.branch.clone()) else {
            return;
        };
        let leaving = project.active_root();
        match project.set_active(&branch, &state) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast(format!("Failed to switch workspace: {error}"), None)
                });
                return;
            }
        }
        let Some(parked) = self.with_workspace_view(id, |view, _| view.park()) else {
            return;
        };
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        main.parked.insert(leaving, parked);
        let Some(project) = main.project.as_ref() else {
            return;
        };
        let entering = project.active_root();
        let Some(parked) = main.parked.remove(&entering) else {
            self.install_project_views(id);
            return;
        };
        let runner = main
            .services
            .as_ref()
            .map(|services| services.runner.as_ref());
        let info = project_info(
            project,
            runner,
            main.tickets.as_ref(),
            main.pull_requests.as_ref(),
        );
        let problem = config_problem(project);
        let config_path = project.config_path.clone();
        let title = format!("{} - {} - Pomelo", project.session, project.active_branch());
        main.window.set_title(&title);
        main.dirty = true;
        self.with_workspace_view(id, |view, _| {
            view.resume(info, parked);
            view.set_config_problem(problem, &config_path);
        });
    }

    fn refresh_sessions(&mut self) {
        let entries = pom_core::session_entries(&pom_paths::StateDir::from_env());
        let windows: Vec<(WindowId, Option<String>)> = self
            .mains
            .iter()
            .map(|(id, main)| (*id, main.project.as_ref().map(|p| p.session.clone())))
            .collect();
        for (id, current) in windows {
            let (rows, index) = session_rows(&entries, current.as_deref());
            self.with_workspace_view(id, |view, _| view.set_sessions(rows, index));
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        }
    }

    fn reload_changed_projects(&mut self) {
        let state = pom_paths::StateDir::from_env();
        let mut updates = Vec::new();
        let mut rerooted = Vec::new();
        let mut reloaded = Vec::new();
        let mut stale_notices = Vec::new();
        for (id, main) in self.mains.iter_mut() {
            let changed = main.watcher.as_ref().is_some_and(|w| w.take_changed());
            let Some(project) = main.project.as_mut().filter(|_| changed) else {
                continue;
            };
            let root_before = project.active_root();
            let shown_changed = project.reload(&state);
            // Env or service edits change nothing the list shows, but services must still run the new config.
            if let Some(services) = &main.services {
                services.update_config(project);
                main.stale = refresh_env_and_find_stale(&services.runner, project);
                let names: Vec<String> = main
                    .stale
                    .iter()
                    .map(|target| stale_name(target, project))
                    .collect();
                stale_notices.push((*id, names));
            }
            if project.error.is_none() {
                reloaded.push(*id);
            }
            if !shown_changed {
                continue;
            }
            main.dirty = true;
            // The active workspace was removed (or main moved into its folder): follow it.
            main.parked.retain(|root, _| root.is_dir());
            if project.active_root() != root_before {
                rerooted.push(*id);
                continue;
            }
            let runner = main
                .services
                .as_ref()
                .map(|services| services.runner.as_ref());
            updates.push((
                *id,
                project_info(
                    project,
                    runner,
                    main.tickets.as_ref(),
                    main.pull_requests.as_ref(),
                ),
                config_problem(project),
                project.config_path.clone(),
            ));
        }
        for (id, info, problem, config_path) in updates {
            self.with_workspace_view(id, |view, _| {
                view.update_project(info);
                view.set_config_problem(problem, &config_path);
            });
            self.start_doctor(id);
        }
        for id in rerooted {
            self.install_project_views(id);
        }
        for id in reloaded {
            self.with_workspace_view(id, |view, _| view.show_toast("pom.yml reloaded", None));
        }
        for (id, names) in stale_notices {
            self.with_workspace_view(id, |view, _| view.set_stale_services(&names));
        }
    }

    fn session_path(&self, id: WindowId, index: usize) -> Option<(String, std::path::PathBuf)> {
        let app = self.main_app.as_ref()?;
        let main = self.mains.get(&id)?;
        let view = main.entity.read(app.app());
        let session = view.layout().sessions.get(index)?;
        Some((session.name.clone(), session.path.clone()))
    }

    fn handle_session_request(&mut self, id: WindowId, request: workspace::SessionRequest) {
        match request {
            workspace::SessionRequest::Switch(index) => {
                if let Some((_, path)) = self.session_path(id, index) {
                    self.open_folder_in(id, &path);
                }
            }
            workspace::SessionRequest::Forget(index) => {
                if let Some((name, _)) = self.session_path(id, index) {
                    let state = pom_paths::StateDir::from_env();
                    if let Err(error) = pom_core::forget_session(&state, &name) {
                        self.with_workspace_view(id, |view, _| {
                            view.show_toast(format!("Failed to remove {name}: {error}"), None)
                        });
                    }
                    self.refresh_sessions();
                }
            }
            workspace::SessionRequest::Reveal(index) => {
                if let Some((_, path)) = self.session_path(id, index) {
                    if let Err(error) = std::process::Command::new("open").arg(&path).spawn() {
                        self.with_workspace_view(id, |view, _| {
                            view.show_toast(
                                format!("Failed to open {}: {error}", path.display()),
                                None,
                            )
                        });
                    }
                }
            }
            workspace::SessionRequest::ChooseFolder => {
                if let Some(folder) = choose_folders(false).into_iter().next() {
                    self.open_folder_in(id, &folder);
                }
            }
            workspace::SessionRequest::NewProject => {
                let modal = workspaces_ui::NewProjectModal::new(
                    pom_paths::sessions_root(),
                    Box::new(|| choose_folders(true)),
                )
                .with_ai(claude_installed(), self.settings.onboard_with_ai);
                self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
            }
        }
    }

    pub(crate) fn start_scaffold(&mut self, id: WindowId, project: &workspaces_ui::NewProject) {
        if self.scaffolding.is_some() {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Another project is still being created", None)
            });
            return;
        }
        let request = pom_core::ScaffoldRequest {
            name: project.name.clone(),
            root: String::new(),
            default_branch: project.default_branch.clone(),
            repos: project
                .repos
                .iter()
                .map(|(path, alias)| pom_core::RepoSpec {
                    path: path.clone(),
                    alias: alias.clone(),
                })
                .collect(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = pom_core::scaffold_session(&request, &pom_paths::StateDir::from_env());
            if sender.send(result).is_err() {
                eprintln!("scaffold: the app stopped waiting");
            }
            ui::wake();
        });
        let message = format!("Creating {}: cloning repositories...", project.name);
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
        self.scaffolding = Some(Scaffolding {
            window: id,
            name: project.name.clone(),
            use_ai: project.use_ai,
            receiver,
        });
        if self.settings.onboard_with_ai != project.use_ai {
            self.settings.onboard_with_ai = project.use_ai;
            let use_ai = project.use_ai;
            self.with_settings_view(|view, _| view.remember_onboard_with_ai(use_ai));
            if let Err(error) = self.settings.save() {
                eprintln!("could not save settings: {error}");
            }
        }
    }

    fn poll_scaffold(&mut self) {
        let Some(scaffolding) = self.scaffolding.as_ref() else {
            return;
        };
        let result = match scaffolding.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("stopped unexpectedly".into()),
        };
        let Some(Scaffolding {
            window,
            name,
            use_ai,
            ..
        }) = self.scaffolding.take()
        else {
            return;
        };
        match result {
            Ok(session_dir) => {
                self.refresh_sessions();
                self.open_folder_in(window, &session_dir);
                // Either way the drafted config is in front: the agent's edits show up in it as it works.
                self.open_project_config(window);
                if use_ai {
                    self.open_onboarder(window);
                } else {
                    self.review_drafted_config(window);
                }
            }
            Err(error) => {
                let message = format!("Failed to create {name}: {error}");
                self.with_workspace_view(window, |view, _| view.show_toast(message, None));
            }
        }
        if let Some(main) = self.mains.get_mut(&window) {
            main.dirty = true;
        }
    }

    fn open_folder_in(&mut self, id: WindowId, folder: &std::path::Path) {
        match pom_core::config_in(folder) {
            Some(config) => self.open_project_in(id, Some(config)),
            None => {
                let message = format!("No pom.yml in {}", folder.display());
                self.with_workspace_view(id, |view, _| view.show_toast(message, None));
                if let Some(main) = self.mains.get_mut(&id) {
                    main.dirty = true;
                }
            }
        }
    }

    fn draw_main(&mut self, id: WindowId) {
        let Some(app) = self.main_app.as_mut() else {
            return;
        };
        let Some(m) = self.mains.get_mut(&id) else {
            return;
        };
        m.last_drawn = Instant::now();
        let (w, h) = m.ui.size();
        let scale = m.window.scale_factor() as f32;
        app.resize(m.handle, w, h, scale);
        let Some(frame) = app.draw(m.handle) else {
            return;
        };
        let black = ui::Rgba::new(0.0, 0.0, 0.0, 1.0);
        let mut layers: Vec<ui::Layer> = Vec::with_capacity(frame.overlays.len() + 1);
        layers.push((
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        ));
        for o in &frame.overlays {
            let clip = o.clip.map(|r| (r.x, r.y, r.w, r.h));
            layers.push((
                o.painted.rects.as_slice(),
                o.painted.tris.as_slice(),
                o.painted.texts.as_slice(),
                o.painted.icons.as_slice(),
                clip,
            ));
        }
        if let Err(e) = m.ui.render_frame(black, &layers) {
            eprintln!("ui render error: {e}");
        }
    }

    fn draw_settings(&mut self) {
        // Sync the winit window's logical size + scale into the view, draw its Frame, then composite: layer 0
        // clears, each overlay (clipped page, fixed chrome, popover) draws above it.
        let (w, h) = match self.settings_ui.as_ref() {
            Some(ui) => ui.size(),
            None => return,
        };
        let scale = self
            .settings_window
            .as_ref()
            .map(|win| win.scale_factor() as f32)
            .unwrap_or(2.0);
        let (Some(app), Some(handle)) = (self.settings_app.as_mut(), self.settings_handle) else {
            return;
        };
        app.resize(handle, w, h, scale);
        let Some(frame) = app.draw(handle) else {
            return;
        };
        let clear = ui::theme().editor_background;
        let mut layers: Vec<ui::Layer> = Vec::with_capacity(frame.overlays.len() + 1);
        layers.push((
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        ));
        for o in &frame.overlays {
            let clip = o.clip.map(|r| (r.x, r.y, r.w, r.h));
            layers.push((
                o.painted.rects.as_slice(),
                o.painted.tris.as_slice(),
                o.painted.texts.as_slice(),
                o.painted.icons.as_slice(),
                clip,
            ));
        }
        if let Some(ui) = self.settings_ui.as_mut() {
            if let Err(e) = ui.render_frame(clear, &layers) {
                eprintln!("settings render error: {e}");
            }
        }
    }

    /// A key press through the keymap: runs a bound action (returns true), or remembers the first key of a
    /// longer binding and lets the press go on to the editor as usual.
    fn dispatch_keys(
        &mut self,
        id: WindowId,
        stroke: workspace::keymap::Keystroke,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        use workspace::keymap::KeyMatch;
        let in_settings = self.settings_window.as_ref().map(|w| w.id()) == Some(id);
        if !in_settings && !self.mains.contains_key(&id) {
            return false;
        }
        match self.keymap.match_keys(&self.pending_keys, &stroke) {
            KeyMatch::Action(action) => {
                self.pending_keys.clear();
                if in_settings {
                    if action == workspace::keymap::Action::OpenSettings {
                        self.toggle_settings(event_loop);
                        return true;
                    }
                    return false;
                }
                self.run_app_action(id, action, event_loop)
            }
            KeyMatch::Pending => {
                self.pending_keys.push(stroke);
                false
            }
            KeyMatch::None => {
                self.pending_keys.clear();
                false
            }
        }
    }

    /// Runs a keymap or palette action for the window `id`; false when it does not apply right now.
    fn run_app_action(
        &mut self,
        id: WindowId,
        action: workspace::keymap::Action,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        use workspace::keymap::Action;
        let blocked = self.with_workspace_view(id, |v, _| v.window_modal_open() || v.menu_open())
            == Some(true);
        if blocked && action != Action::OpenSettings {
            return false;
        }
        match action {
            Action::OpenSettings => self.toggle_settings(event_loop),
            Action::OpenKeymap => {
                if self.settings_window.is_none() {
                    self.toggle_settings(event_loop);
                }
                self.settings_pages_at = None;
                self.refresh_settings_pages();
                self.with_settings_view(|view, _| view.select_category(settings_ui::KEYMAP));
                self.settings_dirty = true;
            }
            Action::OpenInExternalEditor => self.open_in_external_editor(id),
            Action::OpenProjectConfig => self.open_project_config(id),
            Action::AddRepository => self.open_add_repo(id),
            Action::ApplyConfig => self.apply_config(id),
            Action::SetUpProjectWithAi => {
                if claude_installed() {
                    self.open_project_config(id);
                    self.open_onboarder(id);
                } else {
                    self.with_workspace_view(id, |view, _| {
                        view.show_toast("Install Claude Code to set up with AI", None)
                    });
                }
            }
            Action::ExportConfig => self.open_export_config(id),
            Action::ImportConfig => self.open_import_config(id),
            Action::OpenTicket => {
                let branch = self
                    .mains
                    .get(&id)
                    .and_then(|main| main.project.as_ref())
                    .map(|project| project.active_branch().to_string());
                if let Some(branch) = branch {
                    self.open_ticket(id, &branch);
                }
            }
            Action::OpenProject => {
                self.handle_session_request(id, workspace::SessionRequest::ChooseFolder)
            }
            Action::NewProject => {
                self.handle_session_request(id, workspace::SessionRequest::NewProject)
            }
            Action::NewWorkspace => self.open_create_workspace(id),
            Action::CycleTheme => {
                self.settings.theme = settings_ui::next_theme(&self.settings.theme).to_string();
                self.apply_theme();
                if let Err(error) = self.settings.save() {
                    eprintln!("settings: save: {error}");
                }
                let theme = self.settings.theme.clone();
                if self
                    .with_settings_view(|view, _| view.set_theme(&theme))
                    .is_some()
                {
                    self.settings_dirty = true;
                }
                self.mark_all_mains_dirty();
            }
            Action::CommandPalette => {
                self.with_workspace_view(id, |v, _| {
                    v.editor_key(workspace::EditKey::ToggleCommandPalette, false)
                });
            }
            action => {
                if self.with_workspace_view(id, |v, _| v.run_action(action)) != Some(true) {
                    return false;
                }
                self.sync_workspace_effects(id, event_loop);
            }
        }
        if let Some(m) = self.mains.get_mut(&id) {
            m.dirty = true;
        }
        true
    }

    /// Toggle the Settings window: open a real second window, or close it if already open.
    fn toggle_settings(&mut self, event_loop: &ActiveEventLoop) {
        if self.settings_window.is_some() {
            self.settings_ui = None;
            self.settings_window = None;
            self.settings_app = None;
            self.settings_handle = None;
            self.settings_entity = None;
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("Settings")
            .with_inner_size(LogicalSize::new(920.0, 760.0));
        // Self-managed title bar like the main window: content fills under transparent titlebar, traffic
        // lights stay top-left, no native title strip. The settings page insets its top for them.
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let win = Arc::new(event_loop.create_window(attrs).expect("settings window"));
        let renderer = UiRenderer::new(win.clone()).expect("settings ui");
        #[cfg(target_os = "macos")]
        pin_layer_top_left(&win);
        let (w, h) = renderer.size();
        let scale = win.scale_factor() as f32;
        let fonts = renderer.font_families();
        self.settings_ui = Some(renderer);

        // The settings window is a real framework window: its state + input live in a `SettingsView` entity
        // driven by a `ui::Application`; the binary only pumps draw/input into it.
        let mut app = ui::Application::new();
        let settings = self.settings.clone();
        let jira_session = self
            .focused_main
            .and_then(|id| self.mains.get(&id))
            .or_else(|| self.mains.values().find(|main| main.project.is_some()))
            .and_then(|main| main.project.as_ref())
            .map(|project| (pom_paths::StateDir::from_env(), project.session.clone()));
        let project_config = self
            .focused_main
            .and_then(|id| self.mains.get(&id))
            .or_else(|| self.mains.values().find(|main| main.project.is_some()))
            .and_then(|main| main.project.as_ref())
            .and_then(|project| project.config.clone())
            .map(Arc::new);
        let (handle, entity) = app.open_raw_window::<settings_ui::SettingsView>(
            ui::WindowOptions {
                title: "Settings".into(),
                width: w,
                height: h,
                scale,
            },
            move |_| {
                let mut view = settings_ui::SettingsView::new(settings);
                view.set_fonts(fonts);
                view.set_jira_session(jira_session);
                view.set_project_config(project_config);
                view
            },
        );
        self.settings_app = Some(app);
        self.settings_handle = Some(handle);
        self.settings_entity = Some(entity);
        self.settings_window = Some(win);
        self.apply_ui_font();
        self.draw_settings();
    }

    /// Apply the configured UI font (".PomeloSans"/".PomeloMono"/a family name) to every window's renderer.
    fn apply_ui_font(&mut self) {
        let font = self.settings.ui_font.clone();
        for m in self.mains.values_mut() {
            m.ui.set_ui_font(&font);
        }
        if let Some(u) = self.settings_ui.as_mut() {
            u.set_ui_font(&font);
        }
    }

    /// Apply the configured theme to the global palette the whole UI reads (selected by name).
    fn apply_theme(&self) {
        ui::set_theme(ui::by_name(&self.settings.theme));
    }

    /// Force the caret visible and restart its blink cycle (called on keystrokes so it doesn't blink off mid-type).
    fn reset_caret(&mut self) {
        ui::set_caret_phase(true);
        self.caret_last_toggle = Some(Instant::now());
    }

    /// Push the configured UI font size into the global text scale so all interface text zooms with it.
    fn apply_font_scale(&self) {
        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
    }

    /// Push the configured UI font weight into the global so all interface text is shaped at it.
    fn apply_font_weight(&self) {
        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
    }

    /// After routing an input to the settings view, apply the cross-window side-effects it flagged: re-apply
    /// the UI font to every renderer, and/or repaint the main window because a shared global changed. Also
    /// mirror the view's settings back so the binary persists them and repaints the settings window.
    fn sync_settings_side_effects(&mut self) {
        let Some(entity) = self.settings_entity.clone() else {
            return;
        };
        let Some(app) = self.settings_app.as_mut() else {
            return;
        };
        let effects = entity.update(app.app_mut(), |v, _| v.take_side_effects());
        self.settings = entity.read(app.app()).settings().clone();
        self.apply_editor_defaults();
        if effects.toggle_login_item {
            if let Err(error) = set_start_at_login(!start_at_login()) {
                eprintln!("start at login: {error}");
            }
            self.settings_pages_at = None;
        }
        if effects.check_updates {
            auto_update::spawn_background_check();
        }
        if effects.edit_keymap {
            self.edit_keymap();
        }
        if effects.edit_project_config {
            if let Some(id) = self.bundle_window() {
                self.open_project_config(id);
                if let Some(main) = self.mains.get(&id) {
                    main.window.focus_window();
                }
            }
        }
        if effects.export_config || effects.import_config {
            if let Some(id) = self.bundle_window() {
                if effects.export_config {
                    self.open_export_config(id);
                } else {
                    self.open_import_config(id);
                }
            }
        }
        if effects.reapply_font {
            self.apply_ui_font();
        }
        if effects.reapply_font || effects.redraw_others {
            self.mark_all_mains_dirty();
        }
        if effects.reinstall_agents {
            register_with_agents(self.agent_registration.clone());
        }
        #[cfg(target_os = "macos")]
        {
            if effects.test_notification {
                notifications::post_test();
            }
            if let Some(sound) = &effects.play_sound {
                notifications::play_sound(sound);
            }
        }
        if effects.start_servers {
            self.restart_dev_proxy();
        }
        if effects.reinstall_agents || effects.start_servers {
            self.settings_pages_at = None;
            self.refresh_settings_pages();
        }
        // Any view mutation (click/scroll/key) may have changed the frame; repaint the settings window.
        self.settings_dirty = true;
    }

    fn mark_all_mains_dirty(&mut self) {
        for m in self.mains.values_mut() {
            m.dirty = true;
        }
    }

    /// Run `f` against the settings view entity, scoping the app+entity borrow so callers can then touch other
    /// fields (draw, side-effects). Returns None if the settings window is closed.
    fn with_settings_view<R>(
        &mut self,
        f: impl FnOnce(&mut settings_ui::SettingsView, &mut ui::Context<settings_ui::SettingsView>) -> R,
    ) -> Option<R> {
        let entity = self.settings_entity.clone()?;
        let app = self.settings_app.as_mut()?;
        Some(entity.update(app.app_mut(), f))
    }

    fn settings_simulate_move(&mut self, x: f32, y: f32) -> Option<u64> {
        let handle = self.settings_handle?;
        self.settings_app
            .as_mut()
            .and_then(|a| a.simulate_move(handle, x, y))
    }

    fn settings_simulate_click(&mut self, x: f32, y: f32) {
        if let (Some(app), Some(handle)) = (self.settings_app.as_mut(), self.settings_handle) {
            app.simulate_click(handle, x, y);
        }
    }

    /// Run `f` against a specific main window's `WorkspaceView`, scoping the app+entity borrow. None if gone.
    fn with_workspace_view<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut workspace::WorkspaceView, &mut ui::Context<workspace::WorkspaceView>) -> R,
    ) -> Option<R> {
        let entity = self.mains.get(&id)?.entity.clone();
        let app = self.main_app.as_mut()?;
        Some(entity.update(app.app_mut(), f))
    }

    /// After routing an input to a main view, apply the effects it flagged: persist dock geometry, and open a
    /// new window for a session ("Open in new window") as a real second OS window.
    fn sync_workspace_effects(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        let Some(entity) = self.mains.get(&id).map(|m| m.entity.clone()) else {
            return;
        };
        let Some(app) = self.main_app.as_mut() else {
            return;
        };
        let effects = entity.update(app.app_mut(), |v, _| v.take_effects());
        if effects.persist {
            let view = entity.read(app.app());
            read_dock_settings(&mut self.settings, view.layout());
            drop(view);
            let _ = self.settings.save();
        }
        if let Some(index) = effects.open_new_window {
            if let Some((_, path)) = self.session_path(id, index) {
                let mut layout = Layout::default();
                apply_dock_settings(&self.settings, &mut layout);
                let new_id = self.new_main_window(event_loop, layout);
                self.open_folder_in(new_id, &path);
            }
        }
        if let Some(request) = effects.session {
            self.handle_session_request(id, request);
        }
        if effects.open_settings {
            self.toggle_settings(event_loop);
        }
        if let Some(index) = effects.activate_workspace {
            self.activate_workspace(id, index);
        }
        if effects.open_agent {
            self.open_agent(id);
        }
        if effects.fix_setup {
            self.open_fixer(id);
        }
        if effects.restart_stale {
            self.restart_stale(id);
        }
        if let Some(action) = effects.action {
            self.run_app_action(id, action, event_loop);
        }
        self.handle_workspace_requests(id);
        if let Some(m) = self.mains.get_mut(&id) {
            m.dirty = true;
        }
    }

    /// Whether a given main window's session menu is open (drives caret blink + wheel routing).
    fn main_menu_open(&self, id: WindowId) -> bool {
        match (self.mains.get(&id), self.main_app.as_ref()) {
            (Some(m), Some(a)) => m.entity.read(a.app()).menu_open(),
            _ => false,
        }
    }

    /// Whether any main window has its session menu open (for the caret-blink wake condition).
    fn any_menu_open(&self) -> bool {
        let Some(app) = self.main_app.as_ref() else {
            return false;
        };
        self.mains
            .values()
            .any(|m| m.entity.read(app.app()).menu_open())
    }

    /// Persist dock geometry read from any one main view (on quit / dock change).
    fn persist_settings(&mut self) {
        if let (Some(app), Some(m)) = (self.main_app.as_ref(), self.mains.values().next()) {
            let view = m.entity.read(app.app());
            read_dock_settings(&mut self.settings, view.layout());
        }
        let _ = self.settings.save();
    }
}

const CARET_BLINK: Duration = Duration::from_millis(500);
const FRAME_INTERVAL: Duration = Duration::from_micros(8_333);

impl ApplicationHandler for App {
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            self.with_workspace_view(id, |v, _| v.persist_panes(true));
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        self.reload_changed_projects();
        self.refresh_agents();
        self.open_clicked_notification();
        // Background work (language servers, terminals, git) can wake us hundreds of times a second: mark the
        // windows and let `about_to_wait` draw each at most once per display frame.
        for main in self.mains.values_mut() {
            main.dirty = true;
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_scaffold();
        self.poll_add_repo();
        self.poll_clone_repos();
        self.reload_keymap_if_changed();
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            self.poll_workspaces(id);
        }
        self.refresh_settings_pages();
        if self.with_settings_view(|view, _| view.tick()) == Some(true) {
            self.draw_settings();
        }
        // Coalesce per-window redraws (scroll/hover bursts) into one per iteration.
        let now = Instant::now();
        let mut next_frame: Option<Instant> = None;
        let dirty: Vec<WindowId> = self
            .mains
            .iter()
            .filter(|(_, m)| m.dirty)
            .filter_map(|(id, m)| {
                let due = m.last_drawn + FRAME_INTERVAL;
                if now >= due {
                    return Some(*id);
                }
                next_frame = Some(next_frame.map_or(due, |at| at.min(due)));
                None
            })
            .collect();
        for id in dirty {
            if let Some(m) = self.mains.get_mut(&id) {
                m.dirty = false;
            }
            self.draw_main(id);
        }
        let ticking: Vec<WindowId> = match self.main_app.as_ref() {
            Some(a) => self
                .mains
                .iter()
                .filter(|(_, m)| m.entity.read(a.app()).ticking())
                .map(|(id, _)| *id)
                .collect(),
            None => Vec::new(),
        };
        for id in &ticking {
            self.draw_main(*id);
        }
        let editor_windows: Vec<WindowId> = match self.main_app.as_ref() {
            Some(a) => self
                .mains
                .iter()
                .filter(|(_, m)| {
                    let view = m.entity.read(a.app());
                    view.editor_focused() || view.terminal_focused()
                })
                .map(|(id, _)| *id)
                .collect(),
            None => Vec::new(),
        };
        let needs_caret =
            self.settings_window.is_some() || self.any_menu_open() || !editor_windows.is_empty();
        if !needs_caret && ticking.is_empty() {
            event_loop.set_control_flow(match next_frame {
                Some(at) => ControlFlow::WaitUntil(at),
                None => ControlFlow::Wait,
            });
            return;
        }
        // Coalesce hot-path updates (scroll, hover) into one redraw per loop iteration so a burst of trackpad
        // wheel events doesn't trigger a redraw storm.
        if self.settings_dirty {
            self.settings_dirty = false;
            self.draw_settings();
        }
        let now = Instant::now();
        let mut wake = if ticking.is_empty() {
            None
        } else {
            Some(now + Duration::from_millis(33))
        };
        if needs_caret {
            let last = *self.caret_last_toggle.get_or_insert(now);
            let caret_wake = if now.duration_since(last) >= CARET_BLINK {
                self.caret_last_toggle = Some(now);
                ui::set_caret_phase(!ui::caret_phase());
                if self.settings_window.is_some() {
                    self.draw_settings();
                }
                let menu_windows: Vec<WindowId> = self
                    .mains
                    .keys()
                    .copied()
                    .filter(|id| self.main_menu_open(*id))
                    .collect();
                for id in menu_windows {
                    self.draw_main(id);
                }
                for id in &editor_windows {
                    self.draw_main(*id);
                }
                now + CARET_BLINK
            } else {
                last + CARET_BLINK
            };
            wake = Some(wake.map_or(caret_wake, |w| w.min(caret_wake)));
        }
        if let Some(at) = next_frame {
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }
        match wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.mains.is_empty() {
            return;
        }
        self.settings = Settings::load();
        self.apply_theme();
        self.apply_font_scale();
        self.apply_font_weight();
        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
        self.apply_editor_defaults();
        #[cfg(target_os = "macos")]
        set_dock_icon();
        if self.settings.auto_update {
            auto_update::spawn_background_check();
        }

        let mut layout = Layout::default();
        apply_dock_settings(&self.settings, &mut layout);
        let id = self.new_main_window(event_loop, layout);
        let explicit = std::env::var_os("POM_CONFIG").map(std::path::PathBuf::from);
        let config =
            pom_core::startup_config(&pom_paths::StateDir::from_env(), explicit.as_deref());
        self.open_project_in(id, config);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.mains.is_empty() {
            return;
        }
        // Cmd+, toggles the Settings window from anywhere; Esc closes it if focused.
        if let WindowEvent::ModifiersChanged(mods) = &event {
            self.super_down = mods.state().super_key();
            self.shift_down = mods.state().shift_key();
            self.alt_down = mods.state().alt_key();
            self.ctrl_down = mods.state().control_key();
            ui::set_modifiers(ui::Modifiers {
                cmd: self.super_down,
                shift: self.shift_down,
                alt: self.alt_down,
                ctrl: self.ctrl_down,
            });
            if let Some((x, y)) = self.mains.get(&id).map(|m| m.cursor) {
                let scale = self
                    .mains
                    .get(&id)
                    .map_or(1.0, |m| m.window.scale_factor() as f32);
                if self.with_workspace_view(id, |v, _| {
                    v.mouse_move(x as f32 / scale, y as f32 / scale)
                }) == Some(true)
                {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
            }
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed {
                let esc_on_settings = ke.logical_key == Key::Named(NamedKey::Escape)
                    && self.settings_window.as_ref().map(|w| w.id()) == Some(id);
                let modifiers = terminal::Modifiers {
                    shift: self.shift_down,
                    alt: self.alt_down,
                    ctrl: self.ctrl_down,
                    cmd: self.super_down,
                };
                if let Some(stroke) = keymap_keystroke(ke, modifiers) {
                    if self.dispatch_keys(id, stroke, event_loop) {
                        return;
                    }
                }
                if esc_on_settings {
                    self.settings_ui = None;
                    self.settings_window = None;
                    return;
                }
            }
        }
        // Type into a main window's open session-menu search field.
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed
                && self.mains.contains_key(&id)
                && self.main_menu_open(id)
            {
                self.reset_caret();
                let changed = match &ke.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.with_workspace_view(id, |v, _| v.key_escape())
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.with_workspace_view(id, |v, _| v.key_backspace())
                    }
                    _ => {
                        let text = ke.text.as_ref().map(|t| t.to_string());
                        match text {
                            Some(t) => Some(
                                self.with_workspace_view(id, |v, _| v.key_text(&t)) == Some(true),
                            ),
                            None => Some(false),
                        }
                    }
                };
                if changed == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                return;
            }
        }
        if let WindowEvent::Ime(Ime::Commit(text)) = &event {
            if self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.terminal_focused()) == Some(true)
            {
                self.with_workspace_view(id, |v, _| v.terminal_text(text));
                if let Some(m) = self.mains.get_mut(&id) {
                    m.dirty = true;
                }
                return;
            }
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed
                && self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.terminal_focused()) == Some(true)
            {
                let modifiers = terminal::Modifiers {
                    shift: self.shift_down,
                    alt: self.alt_down,
                    ctrl: self.ctrl_down,
                    cmd: self.super_down,
                };
                self.reset_caret();
                if let Some(command) = pane_key(ke, modifiers, PaneKeyContext::Terminal) {
                    if self.with_workspace_view(id, |v, _| v.pane_command(command)) == Some(true) {
                        if let Some(m) = self.mains.get_mut(&id) {
                            m.dirty = true;
                        }
                        return;
                    }
                }
                let consumed = terminal_keystroke(ke, modifiers).is_some_and(|keystroke| {
                    self.with_workspace_view(id, |v, _| v.terminal_key(&keystroke)) == Some(true)
                });
                if !consumed && !modifiers.cmd && !modifiers.ctrl {
                    if let Some(text) = ke.text.as_ref().map(|text| text.to_string()) {
                        self.with_workspace_view(id, |v, _| v.terminal_text(&text));
                    }
                }
                if let Some(m) = self.mains.get_mut(&id) {
                    m.dirty = true;
                }
                return;
            }
        }
        if let WindowEvent::Ime(ime) = &event {
            if self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.editor_focused()) == Some(true)
            {
                self.reset_caret();
                let changed = match ime {
                    Ime::Preedit(text, cursor) => {
                        let selected = cursor.map(|(start, end)| {
                            text[..start.min(text.len())].chars().count()
                                ..text[..end.min(text.len())].chars().count()
                        });
                        self.with_workspace_view(id, |v, _| v.editor_ime_preedit(text, selected))
                    }
                    Ime::Commit(text) => {
                        self.with_workspace_view(id, |v, _| v.editor_ime_commit(text))
                    }
                    Ime::Enabled | Ime::Disabled => None,
                };
                if changed == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                return;
            }
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed
                && self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.editor_focused()) == Some(true)
            {
                let (shift, cmd, alt, ctrl) = (
                    self.shift_down,
                    self.super_down,
                    self.alt_down,
                    self.ctrl_down,
                );
                if cmd {
                    if let Key::Character(c) = &ke.logical_key {
                        match c.as_str() {
                            "s" => {
                                self.with_workspace_view(id, |v, _| {
                                    if let Some(Err(reason)) = v.editor_save() {
                                        v.show_toast(reason, None);
                                    }
                                });
                                if let Some(m) = self.mains.get_mut(&id) {
                                    m.dirty = true;
                                }
                                return;
                            }
                            "c" => {
                                self.with_workspace_view(id, |v, _| v.editor_copy_to_clipboard());
                                return;
                            }
                            "x" | "v" => {
                                self.reset_caret();
                                let changed = self.with_workspace_view(id, |v, _| {
                                    if c.as_str() == "x" {
                                        v.editor_cut_to_clipboard()
                                    } else {
                                        v.editor_paste_from_clipboard()
                                    }
                                });
                                if changed == Some(true) {
                                    if let Some(m) = self.mains.get_mut(&id) {
                                        m.dirty = true;
                                    }
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                let after_cmd_k = std::mem::take(&mut self.pending_cmd_k);
                if cmd
                    && !shift
                    && matches!(&ke.logical_key, Key::Character(c) if c.as_str() == "k")
                {
                    self.pending_cmd_k = true;
                    return;
                }
                let modifiers = terminal::Modifiers {
                    shift,
                    alt,
                    ctrl,
                    cmd,
                };
                if let Some(command) =
                    pane_key(ke, modifiers, PaneKeyContext::Editor { after_cmd_k })
                {
                    if self.with_workspace_view(id, |v, _| v.pane_command(command)) == Some(true) {
                        self.reset_caret();
                        if let Some(m) = self.mains.get_mut(&id) {
                            m.dirty = true;
                        }
                        return;
                    }
                }
                let key = |k| Some(EditorInput::Key(k));
                let input: Option<EditorInput> = match &ke.logical_key {
                    Key::Character(c) if after_cmd_k && !cmd => match c.as_str() {
                        "z" => key(EditKey::ToggleSoftWrap),
                        _ => None,
                    },
                    Key::Named(NamedKey::ArrowRight) if cmd && alt => {
                        key(EditKey::SetPickerPreviewRight)
                    }
                    Key::Named(NamedKey::ArrowLeft) if cmd => key(EditKey::Home),
                    Key::Named(NamedKey::ArrowRight) if cmd => key(EditKey::End),
                    Key::Named(NamedKey::ArrowUp) if cmd && alt => key(EditKey::AddCursorAbove),
                    Key::Named(NamedKey::ArrowDown) if cmd && alt => key(EditKey::AddCursorBelow),
                    Key::Named(NamedKey::ArrowRight) if cmd && ctrl => {
                        key(EditKey::SelectLargerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowLeft) if cmd && ctrl => {
                        key(EditKey::SelectSmallerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowRight) if ctrl && shift => {
                        key(EditKey::SelectLargerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowLeft) if ctrl && shift => {
                        key(EditKey::SelectSmallerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowUp) if alt && shift => key(EditKey::DuplicateLineUp),
                    Key::Named(NamedKey::ArrowDown) if alt && shift => {
                        key(EditKey::DuplicateLineDown)
                    }
                    Key::Named(NamedKey::ArrowUp) if alt => key(EditKey::MoveLineUp),
                    Key::Named(NamedKey::ArrowDown) if alt => key(EditKey::MoveLineDown),
                    Key::Named(NamedKey::ArrowUp) if cmd => key(EditKey::DocumentStart),
                    Key::Named(NamedKey::ArrowDown) if cmd => key(EditKey::DocumentEnd),
                    Key::Named(NamedKey::Home) if cmd => key(EditKey::DocumentStart),
                    Key::Named(NamedKey::End) if cmd => key(EditKey::DocumentEnd),
                    Key::Named(NamedKey::ArrowLeft) if ctrl && alt => key(EditKey::SubwordLeft),
                    Key::Named(NamedKey::ArrowRight) if ctrl && alt => key(EditKey::SubwordRight),
                    Key::Named(NamedKey::ArrowLeft) if alt => key(EditKey::WordLeft),
                    Key::Named(NamedKey::ArrowRight) if alt => key(EditKey::WordRight),
                    Key::Named(NamedKey::ArrowLeft) => key(EditKey::Left),
                    Key::Named(NamedKey::ArrowRight) => key(EditKey::Right),
                    Key::Named(NamedKey::ArrowUp) => key(EditKey::Up),
                    Key::Named(NamedKey::ArrowDown) => key(EditKey::Down),
                    Key::Named(NamedKey::Home) => key(EditKey::Home),
                    Key::Named(NamedKey::End) => key(EditKey::End),
                    Key::Named(NamedKey::F12) if cmd => key(EditKey::GoToTypeDefinition),
                    Key::Named(NamedKey::F12) if shift => key(EditKey::GoToImplementation),
                    Key::Named(NamedKey::F12) if ctrl => key(EditKey::GoToDeclaration),
                    Key::Named(NamedKey::F12) => key(EditKey::GoToDefinition),
                    Key::Named(NamedKey::F8) if cmd && shift => key(EditKey::GoToPreviousHunk),
                    Key::Named(NamedKey::F8) if cmd => key(EditKey::GoToHunk),
                    Key::Named(NamedKey::F8) if shift => key(EditKey::GoToPreviousDiagnostic),
                    Key::Named(NamedKey::F8) => key(EditKey::GoToDiagnostic),
                    Key::Named(NamedKey::Space) if ctrl && !cmd => key(EditKey::ShowCompletions),
                    Key::Named(NamedKey::PageUp) => key(EditKey::PageUp),
                    Key::Named(NamedKey::PageDown) => key(EditKey::PageDown),
                    Key::Named(NamedKey::Backspace) if cmd => key(EditKey::DeleteToLineStart),
                    Key::Named(NamedKey::Backspace) if ctrl && alt => {
                        key(EditKey::DeleteSubwordLeft)
                    }
                    Key::Named(NamedKey::Backspace) if alt => key(EditKey::DeleteWordLeft),
                    Key::Named(NamedKey::Backspace) => key(EditKey::Backspace),
                    Key::Named(NamedKey::Delete) if cmd => key(EditKey::DeleteToLineEnd),
                    Key::Named(NamedKey::Delete) if ctrl && alt => key(EditKey::DeleteSubwordRight),
                    Key::Named(NamedKey::Delete) if alt => key(EditKey::DeleteWordRight),
                    Key::Named(NamedKey::Delete) => key(EditKey::Delete),
                    Key::Named(NamedKey::Enter) if alt => key(EditKey::SelectAllMatchesInSearch),
                    Key::Named(NamedKey::Enter) if cmd => key(EditKey::ReplaceAll),
                    Key::Named(NamedKey::Enter) => key(EditKey::Enter),
                    Key::Named(NamedKey::Escape) => key(EditKey::Escape),
                    // Emacs-style Control bindings macOS text fields share.
                    Key::Character(c) if ctrl && !cmd && !alt => match c.as_str() {
                        "a" => key(EditKey::LineStart),
                        "e" => key(EditKey::LineEnd),
                        "b" => key(EditKey::Left),
                        "f" => key(EditKey::Right),
                        "p" => key(EditKey::Up),
                        "n" => key(EditKey::Down),
                        "h" => key(EditKey::Backspace),
                        "d" => key(EditKey::Delete),
                        "w" => key(EditKey::DeleteWordLeft),
                        "t" => key(EditKey::Transpose),
                        "g" => key(EditKey::ToggleGoToLine),
                        "-" => key(EditKey::GoBack),
                        "_" => key(EditKey::GoForward),
                        "j" => key(EditKey::JoinLines),
                        "m" => key(EditKey::MoveToEnclosingBracket),
                        _ => None,
                    },
                    Key::Named(NamedKey::Tab) if shift => key(EditKey::Backtab),
                    Key::Named(NamedKey::Tab) => key(EditKey::Tab),
                    Key::Character(_) if cmd && alt => match ke.key_without_modifiers() {
                        Key::Character(c) => match c.as_str() {
                            "c" => key(EditKey::ToggleSearchCaseSensitive),
                            "p" => key(EditKey::TogglePickerPreview),
                            "z" => key(EditKey::GitRestore),
                            "y" => key(EditKey::ToggleStaged),
                            "w" => key(EditKey::ToggleSearchWholeWord),
                            "x" => key(EditKey::ToggleSearchRegex),
                            _ => None,
                        },
                        _ => None,
                    },
                    Key::Character(c) if cmd => match c.as_str() {
                        "f" => key(EditKey::DeploySearch),
                        "g" | "G" if shift => key(EditKey::SelectPreviousMatch),
                        "g" => key(EditKey::SelectNextMatch),
                        "h" | "H" if shift => key(EditKey::ToggleSearchReplace),
                        "e" => key(EditKey::UseSelectionForFind),
                        "z" | "Z" if shift => key(EditKey::Redo),
                        "y" | "Y" if shift => key(EditKey::UnstageAndNext),
                        "y" => key(EditKey::StageAndNext),
                        "\"" => key(EditKey::ExpandAllDiffHunks),
                        "'" => key(EditKey::ToggleSelectedDiffHunks),
                        "k" | "K" if shift => key(EditKey::DeleteLine),
                        "l" | "L" if shift => key(EditKey::SelectAllMatches),
                        "i" | "I" if shift => key(EditKey::ToggleIncludeIgnored),
                        "o" | "O" if shift => key(EditKey::ToggleOutline),
                        "p" if ctrl => key(EditKey::AddCursorAboveRow),
                        "n" if ctrl => key(EditKey::AddCursorBelowRow),
                        "d" if !ctrl => key(EditKey::SelectNext),
                        "z" => key(EditKey::Undo),
                        "a" => key(EditKey::SelectAll),
                        "[" => key(EditKey::Outdent),
                        "]" => key(EditKey::Indent),
                        "/" => key(EditKey::ToggleComments),
                        "|" | "\\" if shift => key(EditKey::MoveToEnclosingBracket),
                        _ => None, // leave copy/paste/other Cmd shortcuts to their own handlers
                    },
                    _ if cmd => None,
                    _ => ke.text.as_ref().map(|t| EditorInput::Text(t.to_string())),
                };
                if let Some(input) = input {
                    self.reset_caret();
                    let changed = match input {
                        EditorInput::Key(k) => {
                            self.with_workspace_view(id, |v, _| v.editor_key(k, shift))
                        }
                        EditorInput::Text(t) => {
                            self.with_workspace_view(id, |v, _| v.editor_text(&t))
                        }
                    };
                    if changed == Some(true) {
                        if let Some(m) = self.mains.get_mut(&id) {
                            m.dirty = true;
                        }
                    }
                    // A key can pick a palette command meant for the app (open a file, a modal...).
                    self.sync_workspace_effects(id, event_loop);
                    return;
                }
            }
        }

        // Route the rest to whichever window the event belongs to.
        if self.settings_window.as_ref().map(|w| w.id()) == Some(id) {
            let scale = self
                .settings_window
                .as_ref()
                .map(|w| w.scale_factor() as f32)
                .unwrap_or(2.0);
            match event {
                WindowEvent::CloseRequested => {
                    self.settings_ui = None;
                    self.settings_window = None;
                    self.settings_app = None;
                    self.settings_handle = None;
                    self.settings_entity = None;
                }
                WindowEvent::Resized(size) => {
                    if let Some(ui) = self.settings_ui.as_mut() {
                        ui.resize(size.width, size.height);
                    }
                    self.draw_settings();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.settings_cursor = (position.x, position.y);
                    let (lx, ly) = (position.x as f32 / scale, position.y as f32 / scale);
                    let hit = self.settings_simulate_move(lx, ly);
                    if let Some(win) = self.settings_window.as_ref() {
                        win.set_cursor(if hit.is_some() {
                            CursorIcon::Pointer
                        } else {
                            CursorIcon::Default
                        });
                    }
                    if self.with_settings_view(|v, _| v.hover(lx, ly)) == Some(true) {
                        self.settings_dirty = true;
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (cx, cy) = (
                        self.settings_cursor.0 as f32 / scale,
                        self.settings_cursor.1 as f32 / scale,
                    );
                    let dy = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y * 30.0,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    if self.with_settings_view(|v, _| v.scroll(dy, cx, cy)) == Some(true) {
                        self.settings_dirty = true;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    let (lx, ly) = (
                        self.settings_cursor.0 as f32 / scale,
                        self.settings_cursor.1 as f32 / scale,
                    );
                    self.settings_simulate_click(lx, ly);
                    self.sync_settings_side_effects();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state != ElementState::Pressed {
                        return;
                    }
                    self.reset_caret();
                    let paste = self.super_down
                        && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "v");
                    let changed = match &event.logical_key {
                        _ if paste => {
                            let text = arboard::Clipboard::new()
                                .ok()
                                .and_then(|mut clipboard| clipboard.get_text().ok());
                            text.and_then(|text| self.with_settings_view(|v, _| v.key_paste(&text)))
                        }
                        _ if self.super_down => Some(false),
                        Key::Named(NamedKey::Escape) => {
                            self.with_settings_view(|v, _| v.key_escape())
                        }
                        Key::Named(NamedKey::Enter) => {
                            self.with_settings_view(|v, _| v.key_enter())
                        }
                        Key::Named(NamedKey::Backspace) => {
                            self.with_settings_view(|v, _| v.key_backspace())
                        }
                        _ => {
                            let text = event.text.as_ref().map(|t| t.to_string());
                            text.map(|t| {
                                self.with_settings_view(|v, _| v.key_text(&t)) == Some(true)
                            })
                            .map(Some)
                            .unwrap_or(Some(false))
                        }
                    };
                    if changed == Some(true) {
                        self.sync_settings_side_effects();
                    }
                }
                WindowEvent::RedrawRequested => self.draw_settings(),
                _ => {}
            }
            return;
        }

        // A main window event.
        if !self.mains.contains_key(&id) {
            return;
        }
        let scale = self
            .mains
            .get(&id)
            .map(|m| m.window.scale_factor() as f32)
            .unwrap_or(2.0);
        match event {
            WindowEvent::CloseRequested => {
                self.with_workspace_view(id, |v, _| v.persist_panes(true));
                self.persist_settings();
                if let Some(m) = self.mains.remove(&id) {
                    if let Some(app) = self.main_app.as_mut() {
                        app.close_window(m.handle);
                    }
                }
                self.sync_dev_proxy();
                if self.mains.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(m) = self.mains.get_mut(&id) {
                    m.ui.resize(size.width, size.height);
                    #[cfg(target_os = "macos")]
                    {
                        pin_layer_top_left(&m.window);
                        center_traffic_lights(&m.window);
                    }
                }
                // Remember the window size (persisted on quit / dock change), not on every resize event.
                self.settings.window_width = size.width as f32 / scale;
                self.settings.window_height = size.height as f32 / scale;
                self.draw_main(id);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(m) = self.mains.get_mut(&id) {
                    m.cursor = (position.x, position.y);
                }
                let (lx, ly) = (position.x as f32 / scale, position.y as f32 / scale);
                let dragging = match (self.mains.get(&id), self.main_app.as_ref()) {
                    (Some(m), Some(a)) => m.entity.read(a.app()).dragging(),
                    _ => false,
                };
                if self.with_workspace_view(id, |v, _| v.mouse_move(lx, ly)) == Some(true) {
                    // A live divider drag repaints immediately for smoothness; hover is coalesced.
                    if dragging {
                        self.reset_caret();
                        self.draw_main(id);
                    } else if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                let resize = self
                    .with_workspace_view(id, |v, _| v.resize_cursor_at(lx, ly))
                    .flatten();
                let over = self
                    .with_workspace_view(id, |v, _| v.hit_at(lx, ly))
                    .flatten()
                    .is_some();
                let terminal_pointer = self
                    .with_workspace_view(id, |v, _| v.terminal_pointer_at(lx, ly))
                    .flatten();
                if let Some(m) = self.mains.get(&id) {
                    m.window.set_cursor(match resize {
                        Some(workspace::ResizeCursor::Horizontal) => CursorIcon::EwResize,
                        Some(workspace::ResizeCursor::Vertical) => CursorIcon::NsResize,
                        None if terminal_pointer == Some(true) => CursorIcon::Pointer,
                        None if terminal_pointer == Some(false) => CursorIcon::Text,
                        None if over => CursorIcon::Pointer,
                        None => CursorIcon::Default,
                    });
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                match state {
                    ElementState::Pressed => {
                        self.with_workspace_view(id, |v, _| v.mouse_down(lx, ly));
                        self.reset_caret();
                        self.sync_workspace_effects(id, event_loop);
                    }
                    ElementState::Released => {
                        self.with_workspace_view(id, |v, _| v.mouse_up());
                        self.sync_workspace_effects(id, event_loop);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                self.reset_caret();
                // Row menus offer what applies now (e.g. Stop All Services only while some run).
                self.refresh_project_info(id);
                if self.with_workspace_view(id, |v, _| v.right_click(lx, ly)) == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
            }
            WindowEvent::Focused(true) => {
                self.focused_main = Some(id);
                self.with_workspace_view(id, |v, _| v.refresh_disk_state());
                self.reset_caret();
                if let Some(m) = self.mains.get_mut(&id) {
                    m.dirty = true;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 30.0, y * 30.0),
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                if self.with_workspace_view(id, |v, _| v.scroll(lx, ly, dx, dy)) == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw_main(id),
            _ => {}
        }
    }
}

#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    const ICON: &[u8] = include_bytes!("../assets/AppIcon.icns");
    unsafe {
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICON.as_ptr() as *const std::ffi::c_void,
            length: ICON.len(),
        ];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            eprintln!(
                "[icon] NSImage init from icns FAILED (data_len={})",
                ICON.len()
            );
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        if std::env::var("POMELO_ICON_LOG").is_ok() {
            let valid: bool = msg_send![image, isValid];
            eprintln!("[icon] set applicationIconImage, image_valid={valid} app={app:p}");
        }
    }
}

#[cfg(target_os = "macos")]
fn pin_layer_top_left(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        let gravity = NSString::from_str("topLeft");
        let _: () = msg_send![layer, setContentsGravity: &*gravity];
    }
}

#[cfg(target_os = "macos")]
fn choose_folders(multiple: bool) -> Vec<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{MainThreadMarker, NSString};
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let panel = unsafe { NSOpenPanel::openPanel(mtm) };
    let (prompt, message) = if multiple {
        ("Add", "Choose the git repositories of the project")
    } else {
        ("Open", "Choose a project folder with a pom.yml")
    };
    unsafe {
        panel.setCanChooseFiles(false);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(multiple);
        panel.setPrompt(Some(&NSString::from_str(prompt)));
        panel.setMessage(Some(&NSString::from_str(message)));
    }
    if unsafe { panel.runModal() } != NSModalResponseOK {
        return Vec::new();
    }
    let urls = unsafe { panel.URLs() };
    urls.iter()
        .filter_map(|url| unsafe { url.path() })
        .map(|path| std::path::PathBuf::from(path.to_string()))
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn choose_folders(_multiple: bool) -> Vec<std::path::PathBuf> {
    Vec::new()
}

// Center the macOS traffic lights in our taller top bar: resize the titlebar container
// (two levels up) to a fixed height pinned to the window top, then place the buttons at a constant offset within it,
// so they don't drift when the window is resized.
#[cfg(target_os = "macos")]
fn center_traffic_lights(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{NSView, NSWindowButton};
    use objc2_foundation::NSPoint;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        let Some(ns_window) = view.window() else {
            return;
        };
        // Re-fetch the buttons every time — AppKit can recreate the standard buttons on layout passes.
        let Some(close) = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton)
        else {
            return;
        };
        let Some(minimize) =
            ns_window.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton)
        else {
            return;
        };
        let Some(zoom) = ns_window.standardWindowButton(NSWindowButton::NSWindowZoomButton) else {
            return;
        };
        // The titlebar container is TWO levels up (button -> widget container -> titlebar view). Resizing the wrong
        // one (the immediate superview) breaks/hides the buttons.
        let Some(button_container) = close.superview() else {
            return;
        };
        let Some(titlebar) = button_container.superview() else {
            return;
        };

        let catx = AnyClass::get("CATransaction");
        if let Some(catx) = catx {
            let _: () = msg_send![catx, begin];
            let _: () = msg_send![catx, setDisableActions: true];
        }

        let window_h = ns_window.frame().size.height;
        let close_f = close.frame();
        let btn_w = close_f.size.width;
        let btn_h = close_f.size.height;
        let btn_pad = (minimize.frame().origin.x - close_f.origin.x - btn_w).max(0.0);
        let pos_x = 19.0_f64;
        let pos_y = ((workspace::TOP_BAR_H as f64 - btn_h) / 2.0).max(0.0); // vertical padding -> centers in the top bar
        let container_h = btn_h + 2.0 * pos_y;

        // Pin the titlebar container to the top of the window at a fixed height (constant across resizes -> stable).
        let mut tf = titlebar.frame();
        tf.size.height = container_h;
        tf.origin.y = window_h - container_h;
        let _: () = msg_send![&*titlebar, setFrame: tf];

        let min_x = pos_x + btn_w + btn_pad;
        let zoom_x = min_x + btn_w + btn_pad;
        close.setFrameOrigin(NSPoint::new(pos_x, pos_y));
        minimize.setFrameOrigin(NSPoint::new(min_x, pos_y));
        zoom.setFrameOrigin(NSPoint::new(zoom_x, pos_y));
        let _: () = msg_send![&*titlebar, updateTrackingAreas];

        if std::env::var("POMELO_RESIZE_LOG").is_ok() {
            eprintln!(
                "[resize] window_h={window_h:.1} container_h={container_h:.1} titlebar.y={:.1} close=({pos_x:.1},{pos_y:.1}) btn={btn_w:.1}x{btn_h:.1}",
                tf.origin.y
            );
        }

        if let Some(catx) = catx {
            let _: () = msg_send![catx, commit];
        }
    }
}

/// The installed app bundle when this is it (only it replaces itself on update).
fn installed_app() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().nth(3)?;
    (app.file_name()? == "Pomelo.app").then(|| app.to_path_buf())
}

/// The LaunchAgent that opens the app at login.
fn login_item_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join("Library/LaunchAgents/app.pomelo.login.plist"))
}

fn start_at_login() -> bool {
    login_item_path().is_some_and(|path| path.exists())
}

/// Adds or removes the login LaunchAgent for the bundle this run came from.
fn set_start_at_login(enabled: bool) -> std::io::Result<()> {
    let Some(path) = login_item_path() else {
        return Ok(());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|app| app.extension().is_some_and(|extension| extension == "app"))
        .ok_or_else(|| std::io::Error::other("not running from an app bundle"))?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key><string>app.pomelo.login</string>\n\
  <key>ProgramArguments</key><array><string>/usr/bin/open</string><string>{}</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
</dict>\n</plist>\n",
        bundle.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, plist)
}

fn is_dev_build() -> bool {
    std::env::current_exe()
        .ok()
        .is_some_and(|exe| exe.to_string_lossy().contains("PomeloDev.app"))
}

/// Whether this run owns the user's real state: agent registration and notifications are skipped for a
/// run on a throwaway state dir (its wrappers would vanish) and when `POM_SKIP_GLOBAL_HOOK` is set.
fn uses_real_state() -> Option<(pom_agent::ClaudeHome, pom_paths::StateDir)> {
    if std::env::var_os("POM_SKIP_GLOBAL_HOOK").is_some() {
        return None;
    }
    let state = pom_paths::StateDir::from_env();
    let claude = pom_agent::ClaudeHome::from_env()?;
    (state.root() == claude.home.join(".local/state/pom")).then_some((claude, state))
}

/// Points coding agents (Claude Code) at this app's MCP server and hooks, reporting how it went.
fn register_with_agents(status: Arc<std::sync::Mutex<settings_ui::AgentPage>>) {
    let report = move |page: settings_ui::AgentPage| {
        if let Ok(mut current) = status.lock() {
            *current = page;
        }
        ui::wake();
    };
    let (Some((claude, state)), Ok(binary)) = (uses_real_state(), std::env::current_exe()) else {
        report(settings_ui::AgentPage {
            mcp: settings_ui::Registration::Skipped,
            hooks: settings_ui::Registration::Skipped,
        });
        return;
    };
    report(settings_ui::AgentPage::default());
    let spawned = std::thread::Builder::new()
        .name("agent-register".into())
        .spawn(move || {
            let outcome = |result: Result<bool, pom_agent::InstallError>, what: &str| match result {
                Ok(_) => settings_ui::Registration::Done,
                Err(error) => {
                    eprintln!("could not {what}: {error}");
                    settings_ui::Registration::Failed(error.to_string())
                }
            };
            let mcp = outcome(
                pom_agent::install_mcp(&claude, &state, &binary),
                "register the MCP server with Claude Code",
            );
            let hooks = outcome(
                pom_agent::install_hooks(&claude, &state, &binary),
                "install the Claude Code hooks",
            );
            report(settings_ui::AgentPage { mcp, hooks });
        });
    if let Err(error) = spawned {
        eprintln!("agent registration: {error}");
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = pom_ptyhost::cli::run(&args)
        .or_else(|| pom_mcp::run(&args))
        .or_else(|| pom_agent::run(&args))
    {
        std::process::exit(code);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let line = format!("panic at {loc}: {info}\n");
        if let Some(home) = std::env::var_os("HOME") {
            let path = std::path::Path::new(&home).join("Library/Logs/pomelo-panic.log");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        eprintln!("{line}");
        previous(info);
    }));

    let event_loop = EventLoop::new()?;
    let proxy = event_loop.create_proxy();
    ui::set_waker(move || {
        if let Err(error) = proxy.send_event(()) {
            eprintln!("wake after the event loop closed: {error}");
        }
    });
    #[cfg(target_os = "macos")]
    if uses_real_state().is_some() {
        notifications::start();
    }
    let mut app = App::default();
    let (keymap, problems) = workspace::keymap::Keymap::load();
    for problem in &problems {
        eprintln!("{problem}");
    }
    app.keymap = keymap;
    app.keymap_problems = problems;
    app.keymap_read = workspace::keymap::Keymap::user_file()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok());
    register_with_agents(app.agent_registration.clone());
    app.refresh_agents();
    event_loop.run_app(&mut app)?;
    Ok(())
}
