//! A project on another machine in the app: the dialog that picks a host and one of its projects, and the page
//! that stands in for the editor, whose files are not on this machine.

mod ports;

pub use ports::{
    board_page as ports_board_page, board_ports, parse_port, BoardPorts, PortsAccess, PortsPage,
    PORTS_TAB_ID,
};

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use pom_remote::{Connection, Host, ProjectEntry, Snapshot, SourceError};
use ui::{div, icon, label, theme, IconKind, Node};
use workspace::{
    modal_button, modal_footer, modal_frame, modal_header, modal_note, modal_section, ButtonKind,
    CenterNotice, EditKey, ModalResult, NoticeBanner, NoticeButton, NoticeFact, NoticeTone,
    WindowModal, WINDOW_MODAL_BASE,
};

const WIDTH: f32 = 480.0;
const ROW_H: f32 = 44.0;
const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const BACK: u64 = WINDOW_MODAL_BASE + 2;
const RETRY: u64 = WINDOW_MODAL_BASE + 3;
const CANCEL: u64 = WINDOW_MODAL_BASE + 4;
const ROW_BASE: u64 = WINDOW_MODAL_BASE + 100;
const ROW_LIMIT: usize = 200;

/// The project picked in the dialog: where it lives and its pom.yml there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectProject {
    pub host: String,
    pub name: String,
    pub path: String,
    pub config: String,
}

pub type ListProjects = Arc<dyn Fn(&Host) -> Result<Vec<ProjectEntry>, SourceError> + Send + Sync>;

enum Step {
    Hosts,
    Projects {
        host: usize,
        listing: Option<Receiver<Result<Vec<ProjectEntry>, SourceError>>>,
        found: Option<Result<Vec<ProjectEntry>, SourceError>>,
    },
}

/// Connect to Host: the machines of hosts.json first, then the projects the chosen one knows.
pub struct ConnectHostModal {
    hosts: Vec<Host>,
    /// hosts.json could not be read.
    problem: Option<String>,
    list: ListProjects,
    step: Step,
    highlighted: usize,
    hover: Option<u64>,
    result: Option<ModalResult>,
}

impl ConnectHostModal {
    pub fn new(hosts: Result<Vec<Host>, String>, list: ListProjects) -> ConnectHostModal {
        let (hosts, problem) = match hosts {
            Ok(hosts) => (hosts, None),
            Err(problem) => (Vec::new(), Some(problem)),
        };
        ConnectHostModal {
            hosts,
            problem,
            list,
            step: Step::Hosts,
            highlighted: 0,
            hover: None,
            result: None,
        }
    }

    /// Opens straight on `host`'s projects (Reconnect, or a host named on the command line).
    pub fn on_host(mut self, name: &str) -> ConnectHostModal {
        if let Some(index) = self.hosts.iter().position(|host| host.name == name) {
            self.pick_host(index);
        }
        self
    }

    fn pick_host(&mut self, index: usize) {
        let Some(host) = self.hosts.get(index).cloned() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let list = self.list.clone();
        std::thread::spawn(move || {
            if sender.send(list(&host)).is_err() {
                eprintln!("connect: the dialog closed before {} answered", host.name);
            }
        });
        self.step = Step::Projects {
            host: index,
            listing: Some(receiver),
            found: None,
        };
        self.highlighted = 0;
    }

    fn projects(&self) -> &[ProjectEntry] {
        match &self.step {
            Step::Projects {
                found: Some(Ok(projects)),
                ..
            } => projects,
            _ => &[],
        }
    }

    fn row_count(&self) -> usize {
        match &self.step {
            Step::Hosts => self.hosts.len(),
            Step::Projects { .. } => self.projects().len(),
        }
    }

    fn choose(&mut self, index: usize) {
        match &self.step {
            Step::Hosts => self.pick_host(index),
            Step::Projects { host, .. } => {
                let (Some(host), Some(project)) =
                    (self.hosts.get(*host), self.projects().get(index))
                else {
                    return;
                };
                if project.missing {
                    return;
                }
                self.result = Some(ModalResult::Submitted(Box::new(ConnectProject {
                    host: host.name.clone(),
                    name: project.name.clone(),
                    path: project.path.to_string_lossy().into_owned(),
                    config: project.config.to_string_lossy().into_owned(),
                })));
            }
        }
    }

    fn row(&self, index: usize, kind: IconKind, title: &str, detail: &str, enabled: bool) -> Node {
        let colors = theme();
        let id = ROW_BASE + index as u64;
        let mut row = div()
            .row()
            .items_center()
            .gap(12.0)
            .h_px(ROW_H)
            .px(10.0)
            .rounded(6.0)
            .child(icon(kind).size(16.0).color(colors.icon_muted))
            .child(
                div()
                    .col()
                    .flex(1.0)
                    .child(
                        label(title.to_string())
                            .size(13.0)
                            .medium()
                            .color(if enabled {
                                colors.text
                            } else {
                                colors.text_disabled
                            })
                            .truncate(),
                    )
                    .child(
                        label(detail.to_string())
                            .size(12.0)
                            .color(colors.text_placeholder)
                            .truncate_start(),
                    ),
            );
        if enabled {
            row = row.on_click(id);
            if self.highlighted == index {
                row = row.bg(colors.overlay_selected);
            } else if self.hover == Some(id) {
                row = row.bg(colors.ghost_element_hover);
            }
        }
        row.into()
    }

    fn line(text: &str) -> Node {
        label(text.to_string())
            .size(12.5)
            .color(theme().text_muted)
            .wrap(WIDTH - 40.0)
            .into()
    }

    fn status(kind: IconKind, tint: ui::Rgba, text: &str) -> Node {
        div()
            .row()
            .items_center()
            .gap(8.0)
            .child(icon(kind).size(14.0).color(tint))
            .child(
                label(text.to_string())
                    .size(12.5)
                    .color(theme().text_muted)
                    .wrap(WIDTH - 62.0),
            )
            .into()
    }

    fn command(text: &str) -> Node {
        let colors = theme();
        div()
            .row()
            .px(10.0)
            .py(8.0)
            .rounded(6.0)
            .bg(colors.element_background)
            .child(label(text.to_string()).size(12.0).mono().color(colors.text))
            .into()
    }
}

impl WindowModal for ConnectHostModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let mut section = modal_section(4.0);
        let (title, note, back) = match &self.step {
            Step::Hosts => {
                if let Some(problem) = &self.problem {
                    section = section.child(Self::line(&format!(
                        "hosts.json could not be read: {problem}"
                    )));
                } else if self.hosts.is_empty() {
                    section = section
                        .gap(10.0)
                        .child(Self::line(
                            "No hosts yet. Add a machine you reach over SSH, then check it:",
                        ))
                        .child(Self::command("pom host add dev ssh://dev@dev.example.net"))
                        .child(Self::command("pom host test dev"));
                }
                for (index, host) in self.hosts.iter().enumerate().take(ROW_LIMIT) {
                    section = section.child(self.row(
                        index,
                        IconKind::Server,
                        &host.name,
                        &host.url.alias(),
                        true,
                    ));
                }
                ("Connect to Host".to_string(), "Pick a machine", false)
            }
            Step::Projects { host, found, .. } => {
                let host = self.hosts.get(*host);
                let name = host.map(|host| host.name.clone()).unwrap_or_default();
                match found {
                    None => {
                        section = section.child(Self::status(
                            IconKind::LoadCircle,
                            theme().text_accent,
                            &format!("Connecting to {name} and asking for its projects..."),
                        ));
                    }
                    Some(Err(error)) => {
                        let lead = match error {
                            SourceError::Unreachable(_) => "Could not connect",
                            SourceError::Failed(_) | SourceError::Refused(_) => {
                                "It answered with an error"
                            }
                        };
                        section = section
                            .gap(10.0)
                            .child(Self::status(
                                IconKind::XCircle,
                                theme().error,
                                &format!("{lead}: {error}"),
                            ))
                            .child(Self::command(&format!("pom host test {name}")));
                    }
                    Some(Ok(projects)) if projects.is_empty() => {
                        section = section.child(Self::line(&format!(
                            "{name} knows no projects yet. Create or open one there first (pom init, or pom onboard)."
                        )));
                    }
                    Some(Ok(projects)) => {
                        for (index, project) in projects.iter().enumerate().take(ROW_LIMIT) {
                            let detail = if project.missing {
                                format!("{} (missing)", project.path.display())
                            } else {
                                project.path.display().to_string()
                            };
                            section = section.child(self.row(
                                index,
                                IconKind::Folder,
                                &project.name,
                                &detail,
                                !project.missing,
                            ));
                        }
                    }
                }
                (format!("Projects on {name}"), "Pick a project", true)
            }
        };
        let mut buttons = div().row().items_center().gap(8.0);
        if back {
            buttons = buttons.child(modal_button(BACK, "Back", None, ButtonKind::Ghost, true));
            if matches!(
                &self.step,
                Step::Projects {
                    found: Some(Err(_)),
                    ..
                }
            ) {
                buttons = buttons.child(modal_button(
                    RETRY,
                    "Retry",
                    None,
                    ButtonKind::Secondary,
                    true,
                ));
            }
        }
        buttons = buttons.child(modal_button(
            CANCEL,
            "Cancel",
            Some("escape"),
            ButtonKind::Secondary,
            true,
        ));
        modal_frame(WIDTH)
            .child(modal_header(&title, Some(CLOSE)))
            .child(section)
            .child(modal_footer(Some(modal_note(note)), buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            BACK => {
                let host = match &self.step {
                    Step::Projects { host, .. } => *host,
                    Step::Hosts => 0,
                };
                self.step = Step::Hosts;
                self.highlighted = host;
            }
            RETRY => {
                if let Step::Projects { host, .. } = self.step {
                    self.pick_host(host);
                }
            }
            id if (ROW_BASE..ROW_BASE + ROW_LIMIT as u64).contains(&id) => {
                self.choose((id - ROW_BASE) as usize)
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, _shift: bool) -> bool {
        let count = self.row_count();
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Up => self.highlighted = self.highlighted.saturating_sub(1),
            EditKey::Down if count > 0 => self.highlighted = (self.highlighted + 1).min(count - 1),
            EditKey::Enter if self.highlighted < count => self.choose(self.highlighted),
            EditKey::Backspace if matches!(self.step, Step::Projects { .. }) => self.click(BACK),
            _ => {}
        }
        true
    }

    fn text(&mut self, _text: &str) -> bool {
        false
    }

    fn tick(&mut self) -> bool {
        let Step::Projects { listing, found, .. } = &mut self.step else {
            return false;
        };
        let Some(receiver) = listing else {
            return false;
        };
        let answer = match receiver.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err(SourceError::Failed("the listing stopped".into()))
            }
        };
        *listing = None;
        *found = Some(answer);
        true
    }

    fn busy(&self) -> bool {
        matches!(
            self.step,
            Step::Projects {
                listing: Some(_),
                ..
            }
        )
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        if self.hover == id {
            return false;
        }
        self.hover = id;
        true
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

/// What the page says runs here and what stays on the host, in place of the editor, files and Git.
pub fn local_only_facts(host: &str, terminals: bool, path: &str) -> Vec<NoticeFact> {
    let here = if terminals {
        NoticeFact {
            label: "Here".into(),
            value: "Terminals, services and agents".into(),
            detail: String::new(),
        }
    } else {
        NoticeFact {
            label: "Here".into(),
            value: "Services and agents".into(),
            detail: format!("terminals open once {host} runs pom server"),
        }
    };
    let mut on_host = NoticeFact {
        label: format!("On {host}"),
        value: "Editor, files and Git".into(),
        detail: "Open in Editor reaches them over SSH".into(),
    };
    if path.is_empty() {
        on_host.detail.clear();
    }
    vec![here, on_host]
}

/// Updating pom on the host from the app, as the page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PomUpdate {
    /// The app installs pom there itself: a release build, and pom in its default place on the host.
    pub can_install: bool,
    pub running: Option<pom_remote::update::UpdateStep>,
    pub error: Option<String>,
}

/// A server older than file access: the files stay on the host until pom there is updated.
fn lacks_files(transport: &pom_remote::TransportInfo) -> bool {
    transport.via == pom_remote::Via::Server && !transport.files
}

/// The banners of a host read over ssh, or whose pom is on another minor version than the app; `update_action`
/// is the index of the page's Update button.
pub fn transport_banners(
    snapshot: &Snapshot,
    host: &str,
    update: &PomUpdate,
    update_action: Option<usize>,
) -> Vec<NoticeBanner> {
    let Some(transport) = &snapshot.transport else {
        return Vec::new();
    };
    let mut banners = Vec::new();
    if let Some(reason) = &transport.fallback_reason {
        banners.push(NoticeBanner {
            tone: NoticeTone::Info,
            text: format!("Reading over ssh: {reason}."),
            ..NoticeBanner::default()
        });
    }
    if transport.version_mismatch && transport.host_newer {
        let version = match &transport.remote_version {
            Some(remote) => version_line(remote, host),
            None => format!(
                "pom on {host} is newer than the app ({}).",
                pom_remote::VERSION
            ),
        };
        banners.push(NoticeBanner {
            tone: NoticeTone::Warning,
            text: format!("{version} Update this app to use everything pom there offers."),
            ..NoticeBanner::default()
        });
    } else if transport.version_mismatch {
        let version = match &transport.remote_version {
            Some(remote) => version_line(remote, host),
            None => format!(
                "pom on {host} is older than the app ({}).",
                pom_remote::VERSION
            ),
        };
        let banner = match (&update.running, &update.error) {
            (Some(step), _) => NoticeBanner {
                tone: NoticeTone::Info,
                busy: true,
                text: format!("Updating pom on {host}: {}...", step.label()),
                ..NoticeBanner::default()
            },
            (None, Some(error)) => NoticeBanner {
                tone: NoticeTone::Error,
                text: format!("Could not update pom on {host}: {error}"),
                action: update_action,
                ..NoticeBanner::default()
            },
            (None, None) if !update.can_install => NoticeBanner {
                tone: NoticeTone::Warning,
                text: format!("{version} To update it, run on {host}:"),
                commands: pom_remote::update::manual_commands(pom_remote::VERSION),
                ..NoticeBanner::default()
            },
            (None, None) => NoticeBanner {
                tone: NoticeTone::Warning,
                text: version,
                action: update_action,
                ..NoticeBanner::default()
            },
        };
        banners.push(banner);
    } else if lacks_files(transport) {
        let text = match &transport.remote_version {
            Some(remote) => format!("pom {remote} on {host} cannot show its files here."),
            None => format!("pom on {host} cannot show its files here."),
        };
        let banner = match (&update.running, &update.error) {
            (Some(step), _) => NoticeBanner {
                tone: NoticeTone::Info,
                busy: true,
                text: format!("Updating pom on {host}: {}...", step.label()),
                ..NoticeBanner::default()
            },
            (None, Some(error)) => NoticeBanner {
                tone: NoticeTone::Error,
                text: format!("Could not update pom on {host}: {error}"),
                action: update_action,
                ..NoticeBanner::default()
            },
            (None, None) if !update.can_install => NoticeBanner {
                tone: NoticeTone::Warning,
                text: format!("{text} To update it, run on {host}:"),
                commands: pom_remote::update::manual_commands(pom_remote::VERSION),
                ..NoticeBanner::default()
            },
            (None, None) => NoticeBanner {
                tone: NoticeTone::Warning,
                text: format!("{text} Update it to open files in this window."),
                action: update_action,
                ..NoticeBanner::default()
            },
        };
        banners.push(banner);
    }
    banners
}

/// The banner of a host whose pom is on another minor version than the app.
pub fn version_line(remote: &str, host: &str) -> String {
    format!(
        "pom {remote} on {host}, the app is {}.",
        pom_remote::VERSION
    )
}

/// Said once each time the session comes back to a server that restarted meanwhile.
pub fn restart_notice(host: &str) -> String {
    format!("pom server on {host} restarted; terminals and services kept running")
}

/// The buttons of the page in place of the editor, in the order `CenterNotice::actions` lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    OpenInEditor,
    Reconnect,
    UpdatePom,
}

/// The session as the title bar's host shows it.
pub fn host_link(connection: &Connection) -> workspace::HostLink {
    match connection {
        Connection::Connecting => workspace::HostLink::Connecting,
        Connection::Connected => workspace::HostLink::Connected,
        Connection::Reconnecting { .. } => workspace::HostLink::Reconnecting,
        Connection::Disconnected { .. } => workspace::HostLink::Disconnected,
    }
}

/// The connection in one line; the host is named by the page's facts and the title bar.
pub fn status_line(snapshot: &Snapshot, host: &str) -> String {
    match &snapshot.connection {
        Connection::Connecting => format!("Connecting to {host}..."),
        Connection::Connected => {
            let via = match snapshot.transport.as_ref().map(|transport| transport.via) {
                Some(pom_remote::Via::Server) => "Connected via pom server",
                Some(pom_remote::Via::Ssh) => "Connected over ssh",
                None => "Connected",
            };
            match snapshot.answered_at {
                Some(at) => format!("{via} - updated {}", services_ui::ago(at)),
                None => via.to_string(),
            }
        }
        Connection::Reconnecting { attempt, next_in } => {
            let when = if next_in.is_zero() {
                String::new()
            } else {
                format!(
                    " (attempt {attempt} in {:.0}s)",
                    next_in.as_secs_f64().ceil()
                )
            };
            format!("Lost connection. Reconnecting...{when}")
        }
        Connection::Disconnected { reason } => format!("Disconnected: {reason}"),
    }
}

/// `dev@dev.example.net` out of a source's `dev (dev@dev.example.net)`.
fn address(location: &str, host: &str) -> String {
    location
        .strip_prefix(host)
        .map(str::trim)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(location)
        .to_string()
}

/// The page in place of the editor for a remote project: its connection, what needs doing about the host's
/// pom, where things are, and the buttons that act on it.
pub fn remote_notice(
    snapshot: &Snapshot,
    location: &str,
    host: &str,
    project: &str,
    update: &PomUpdate,
) -> (CenterNotice, Vec<NoticeAction>) {
    let tone = match snapshot.connection {
        Connection::Connected => NoticeTone::Good,
        Connection::Connecting => NoticeTone::Info,
        Connection::Reconnecting { .. } => NoticeTone::Warning,
        Connection::Disconnected { .. } => NoticeTone::Error,
    };
    let terminals = snapshot
        .transport
        .as_ref()
        .is_some_and(|transport| transport.via == pom_remote::Via::Server && transport.terminals);
    let mut actions = Vec::new();
    if matches!(snapshot.connection, Connection::Disconnected { .. }) {
        actions.push(NoticeAction::Reconnect);
    }
    actions.push(NoticeAction::OpenInEditor);
    let mismatch = snapshot.transport.as_ref().is_some_and(|transport| {
        (transport.version_mismatch && !transport.host_newer) || lacks_files(transport)
    });
    let update_action = (mismatch && update.can_install && update.running.is_none()).then(|| {
        actions.push(NoticeAction::UpdatePom);
        actions.len() - 1
    });
    let mut detail = address(location, host);
    let workspace = snapshot.workspaces.as_ref().and_then(|list| {
        let running: usize = list.workspaces.iter().map(|ws| ws.running).sum();
        detail = format!(
            "{detail} - {} workspaces, {running} services running",
            list.workspaces.len()
        );
        list.workspaces
            .iter()
            .find(|ws| ws.branch == snapshot.branch)
    });
    let mut facts = vec![NoticeFact {
        label: "Host".into(),
        value: host.to_string(),
        detail,
    }];
    let path = workspace
        .map(|ws| ws.path.display().to_string())
        .unwrap_or_default();
    if let Some(workspace) = workspace {
        facts.push(NoticeFact {
            label: "Workspace".into(),
            value: workspace.branch.clone(),
            detail: path.clone(),
        });
    }
    facts.extend(local_only_facts(host, terminals, &path));
    let buttons = actions
        .iter()
        .map(|action| match action {
            NoticeAction::OpenInEditor => NoticeButton {
                label: "Open in Editor".into(),
                primary: false,
            },
            NoticeAction::Reconnect => NoticeButton {
                label: "Reconnect".into(),
                primary: true,
            },
            NoticeAction::UpdatePom => NoticeButton {
                label: format!("Update pom on {host}"),
                primary: false,
            },
        })
        .collect();
    (
        CenterNotice {
            title: project.to_string(),
            status: status_line(snapshot, host),
            tone,
            actions: buttons,
            banners: transport_banners(snapshot, host, update, update_action),
            facts,
        },
        actions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<Host> {
        ["dev", "lab"]
            .iter()
            .map(|name| Host {
                name: name.to_string(),
                url: pom_remote::HostUrl::parse(&format!("ssh://me@{name}.example.net"))
                    .expect("url"),
                pom_path: None,
                transport: None,
            })
            .collect()
    }

    fn settle(modal: &mut ConnectHostModal) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while modal.busy() {
            modal.tick();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn picking_a_host_then_a_project_submits_where_it_lives() {
        let list: ListProjects = Arc::new(|host: &Host| {
            Ok(vec![
                ProjectEntry {
                    name: "gone".into(),
                    path: "/srv/gone".into(),
                    missing: true,
                    ..ProjectEntry::default()
                },
                ProjectEntry {
                    name: "myproject".into(),
                    path: format!("/home/{}/pom/myproject", host.name).into(),
                    config: format!("/home/{}/pom/myproject/pom.yml", host.name).into(),
                    ..ProjectEntry::default()
                },
            ])
        });
        let mut modal = ConnectHostModal::new(Ok(hosts()), list);
        modal.key(EditKey::Down, false);
        modal.key(EditKey::Enter, false);
        settle(&mut modal);
        modal.click(ROW_BASE);
        assert!(
            modal.take_result().is_none(),
            "a missing project is not picked"
        );
        modal.key(EditKey::Down, false);
        modal.key(EditKey::Enter, false);
        let Some(ModalResult::Submitted(value)) = modal.take_result() else {
            panic!("nothing submitted");
        };
        let picked = value.downcast_ref::<ConnectProject>().expect("a project");
        assert_eq!(picked.host, "lab");
        assert_eq!(picked.config, "/home/lab/pom/myproject/pom.yml");
    }

    #[test]
    fn an_unreachable_host_can_be_retried_or_left() {
        let list: ListProjects =
            Arc::new(|_: &Host| Err(SourceError::Unreachable("Connection refused".into())));
        let mut modal = ConnectHostModal::new(Ok(hosts()), list).on_host("dev");
        settle(&mut modal);
        assert!(matches!(
            modal.step,
            Step::Projects {
                found: Some(Err(_)),
                ..
            }
        ));
        modal.click(RETRY);
        assert!(modal.busy());
        settle(&mut modal);
        modal.click(BACK);
        assert!(matches!(modal.step, Step::Hosts));
        modal.key(EditKey::Escape, false);
        assert!(matches!(modal.take_result(), Some(ModalResult::Cancelled)));
    }

    #[test]
    fn a_server_without_files_keeps_the_page_and_offers_an_update() {
        let mut snapshot = Snapshot {
            connection: Connection::Connected,
            ..Snapshot::default()
        };
        snapshot.transport = Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Server,
            remote_version: Some("0.9.0".into()),
            terminals: true,
            ..pom_remote::TransportInfo::default()
        });
        let update = PomUpdate {
            can_install: true,
            ..PomUpdate::default()
        };
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &update);
        assert_eq!(
            actions,
            vec![NoticeAction::OpenInEditor, NoticeAction::UpdatePom]
        );
        assert_eq!(
            notice.banners[0].text,
            "pom 0.9.0 on dev cannot show its files here. Update it to open files in this window."
        );
        assert_eq!(notice.banners[0].action, Some(1));
        if let Some(transport) = snapshot.transport.as_mut() {
            transport.files = true;
        }
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &update);
        assert!(notice.banners.is_empty());
        assert_eq!(actions, vec![NoticeAction::OpenInEditor]);
    }

    #[test]
    fn a_newer_host_asks_to_update_the_app_and_an_older_protocol_offers_the_host_update() {
        let update = PomUpdate {
            can_install: true,
            ..PomUpdate::default()
        };
        let mut snapshot = Snapshot {
            connection: Connection::Connected,
            ..Snapshot::default()
        };
        snapshot.transport = Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Server,
            remote_version: Some("9.0.0".into()),
            version_mismatch: true,
            host_newer: true,
            files: true,
            ..pom_remote::TransportInfo::default()
        });
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &update);
        assert_eq!(actions, vec![NoticeAction::OpenInEditor], "never downgrade");
        assert!(notice.banners[0]
            .text
            .ends_with("Update this app to use everything pom there offers."));
        assert_eq!(notice.banners[0].action, None);

        snapshot.transport = Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Ssh,
            fallback_reason: Some(
                "pom server on dev speaks an older protocol than this app".into(),
            ),
            version_mismatch: true,
            ..pom_remote::TransportInfo::default()
        });
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &update);
        assert_eq!(
            actions,
            vec![NoticeAction::OpenInEditor, NoticeAction::UpdatePom]
        );
        assert_eq!(
            notice.banners[0].text,
            "Reading over ssh: pom server on dev speaks an older protocol than this app."
        );
        assert!(notice.banners[1]
            .text
            .starts_with("pom on dev is older than the app ("));
        assert_eq!(notice.banners[1].action, Some(1));
    }

    #[test]
    fn the_page_says_why_reads_go_over_ssh_and_when_versions_differ() {
        let mut snapshot = Snapshot::default();
        let update = PomUpdate {
            can_install: true,
            ..PomUpdate::default()
        };
        assert!(transport_banners(&snapshot, "dev", &update, None).is_empty());
        snapshot.transport = Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Server,
            remote_version: Some("0.1.0".into()),
            version_mismatch: true,
            ..pom_remote::TransportInfo::default()
        });
        let banners = transport_banners(&snapshot, "dev", &update, Some(1));
        assert_eq!(banners.len(), 1);
        assert!(banners[0].text.starts_with("pom 0.1.0 on dev, the app is "));
        assert_eq!(banners[0].action, Some(1));
        snapshot.connection = Connection::Connected;
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &update);
        assert_eq!(
            actions,
            vec![NoticeAction::OpenInEditor, NoticeAction::UpdatePom]
        );
        assert_eq!(notice.actions[1].label, "Update pom on dev");
        assert_eq!(notice.banners[0].action, Some(1), "the banner holds Update");

        let running = PomUpdate {
            running: Some(pom_remote::update::UpdateStep::Download),
            ..update.clone()
        };
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &running);
        assert!(
            !actions.contains(&NoticeAction::UpdatePom),
            "one update at a time"
        );
        assert_eq!(
            notice.banners[0].text,
            "Updating pom on dev: downloading..."
        );
        assert!(notice.banners[0].busy);
        let failed = PomUpdate {
            error: Some("pom-1.0.0-linux-x86_64.tar.gz does not match".into()),
            ..update.clone()
        };
        assert!(transport_banners(&snapshot, "dev", &failed, None)[0]
            .text
            .starts_with("Could not update pom on dev:"));

        let dev_build = PomUpdate::default();
        let (notice, actions) = remote_notice(&snapshot, "dev", "dev", "myproject", &dev_build);
        assert!(
            !actions.contains(&NoticeAction::UpdatePom),
            "a dev build shows the commands"
        );
        assert!(notice.banners[0]
            .commands
            .iter()
            .any(|line| line.contains("install.sh --cli-only")));
        snapshot.transport = Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Ssh,
            fallback_reason: Some(
                "its pom is older than pom server; update pom on the VM to get terminals".into(),
            ),
            ..pom_remote::TransportInfo::default()
        });
        let (notice, _) = remote_notice(
            &snapshot,
            "dev (dev@dev.example.net)",
            "dev",
            "myproject",
            &update,
        );
        assert_eq!(notice.status, "Connected over ssh");
        assert_eq!(notice.facts[0].detail, "dev@dev.example.net");
        assert_eq!(
            notice.banners[0].text,
            "Reading over ssh: its pom is older than pom server; update pom on the VM to get terminals."
        );
        assert_eq!(notice.facts[1].value, "Services and agents");
    }

    #[test]
    fn a_dropped_session_offers_reconnect_first() {
        let snapshot = Snapshot {
            connection: Connection::Disconnected {
                reason: "Connection refused".into(),
            },
            ..Snapshot::default()
        };
        let (notice, actions) =
            remote_notice(&snapshot, "dev", "dev", "myproject", &PomUpdate::default());
        assert_eq!(
            actions,
            vec![NoticeAction::Reconnect, NoticeAction::OpenInEditor]
        );
        assert!(notice.actions[0].primary);
        assert_eq!(notice.status, "Disconnected: Connection refused");
        assert_eq!(
            host_link(&snapshot.connection),
            workspace::HostLink::Disconnected
        );
    }
}
