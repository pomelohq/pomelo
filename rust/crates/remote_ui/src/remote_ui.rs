//! A project on another machine in the app: the dialog that picks a host and one of its projects, and the page
//! that stands in for the editor, whose files are not on this machine.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use pom_remote::{Connection, Host, ProjectEntry, Snapshot, SourceError};
use ui::{div, icon, label, theme, IconKind, Node};
use workspace::{
    modal_button, modal_footer, modal_frame, modal_header, modal_note, modal_section, ButtonKind,
    CenterNotice, EditKey, ModalResult, NoticeTone, WindowModal, WINDOW_MODAL_BASE,
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
        let lit = self.hover == Some(id) || self.highlighted == index;
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
            if lit {
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
                            "No hosts yet. Name a machine you reach over SSH (your own keys and ~/.ssh/config are used), then check it:",
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
                        section = section.child(Self::line(&format!(
                            "Connecting to {name} and asking for its projects..."
                        )));
                    }
                    Some(Err(error)) => {
                        let lead = match error {
                            SourceError::Unreachable(_) => "Could not connect",
                            SourceError::Failed(_) => "It answered with an error",
                        };
                        section = section
                            .gap(10.0)
                            .child(Self::line(&format!("{lead}: {error}")))
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

/// Why a remote project shows no editor, file tree, Git or terminals yet.
pub fn local_only_reason(host: &str) -> String {
    format!(
        "The editor, files, Git and terminals of a project on {host} come with the Pomelo server. Until then, open its worktree in an editor that works over SSH."
    )
}

/// The buttons of the page in place of the editor, in the order `CenterNotice::actions` lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    OpenInEditor,
    Reconnect,
}

/// The page in place of the editor for a remote project: the connection, why the editor is not here, and what
/// to do instead.
pub fn remote_notice(
    snapshot: &Snapshot,
    location: &str,
    host: &str,
    project: &str,
) -> (CenterNotice, Vec<NoticeAction>) {
    let (_, status, retry) = services_ui::connection_line(snapshot, location);
    let tone = match snapshot.connection {
        Connection::Connected => NoticeTone::Good,
        Connection::Connecting => NoticeTone::Info,
        Connection::Disconnected { .. } => NoticeTone::Warning,
    };
    let mut lines = vec![local_only_reason(host)];
    if let Some(list) = &snapshot.workspaces {
        let running: usize = list.workspaces.iter().map(|ws| ws.running).sum();
        lines.push(format!(
            "{} workspaces, {running} services running.",
            list.workspaces.len()
        ));
    }
    let mut actions = vec![NoticeAction::OpenInEditor];
    if retry {
        actions.insert(0, NoticeAction::Reconnect);
    }
    let labels = actions
        .iter()
        .map(|action| match action {
            NoticeAction::OpenInEditor => "Open in Editor".to_string(),
            NoticeAction::Reconnect => "Reconnect".to_string(),
        })
        .collect();
    (
        CenterNotice {
            status,
            tone,
            title: format!("{project} on {host}"),
            lines,
            actions: labels,
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
}
