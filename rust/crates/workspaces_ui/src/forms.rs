//! The create and rename forms. Both float over the window, take the keyboard (Tab between fields, Enter to
//! confirm, Escape to cancel) and can ask Claude for a name in the background.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use indexmap::IndexMap;
use module_store::Outlook;
use pom_agent::NameSuggestion;
use ui::{deferred, div, icon, label, theme, IconKind, LabelSize, Node, Rgba};
use workspace::{
    checkbox, modal_button, modal_footer, modal_frame, modal_header, modal_section,
    outlined_button, status_line, toggle_button_group, EditKey, InputField, ModalResult,
    WindowModal, WINDOW_MODAL_BASE,
};

use crate::repo_branch_picker::{
    BranchPicker, BranchSource, Entry, RepoBranches, ENTRY_BASE, ENTRY_END, PICKER_QUERY,
    PICKER_REFRESH, PICKER_SURFACE, TRIGGER_HEIGHT,
};
use crate::ticket_picker::{
    TicketPicker, SOURCE, SOURCE_MENU_SURFACE, SOURCE_OPTION_BASE, SOURCE_OPTION_END,
    SUGGESTIONS_SURFACE, SUGGESTION_BASE, TICKET_FIELD,
};
use crate::{humanize_branch, slugify, Namer, TicketSource};

/// The repo name column of the create form's repo rows.
const REPO_NAME_W: f32 = 168.0;
const MODULES_W: f32 = 130.0;
const WIDTH: f32 = 544.0;
/// The create form also lays out each repo's node_modules and the environment and data choices.
const CREATE_WIDTH: f32 = 720.0;
const SECTION_PADDING: f32 = 12.0;

const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const NAME_FIELD: u64 = WINDOW_MODAL_BASE + 2;
const BRANCH_FIELD: u64 = WINDOW_MODAL_BASE + 3;
const REFINE: u64 = WINDOW_MODAL_BASE + 4;
const CANCEL: u64 = WINDOW_MODAL_BASE + 5;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 6;
const FORM_SURFACE: u64 = WINDOW_MODAL_BASE + 12;
const REPO_BASE: u64 = WINDOW_MODAL_BASE + 100;
const BRANCH_BOX_BASE: u64 = WINDOW_MODAL_BASE + 300;
const RESET_BASE: u64 = WINDOW_MODAL_BASE + 400;
const ENVIRONMENT_BASE: u64 = WINDOW_MODAL_BASE + 20;
const ENVIRONMENT_END: u64 = WINDOW_MODAL_BASE + 40;
const DATA_FROM_MAIN: u64 = WINDOW_MODAL_BASE + 41;
const DATA_FRESH: u64 = WINDOW_MODAL_BASE + 42;
const ENVIRONMENT_SELECT: u64 = WINDOW_MODAL_BASE + 43;
const ENVIRONMENT_MENU_SURFACE: u64 = WINDOW_MODAL_BASE + 44;
const SELECT_HEIGHT: f32 = 28.0;
const PER_REPO: u64 = 100;

/// What the create form submits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateWorkspace {
    pub branch: String,
    pub display_name: String,
    /// Repos to check out; empty means all.
    pub repos: Vec<String>,
    /// The Jira board the ticket picker ended on, to open on next time.
    pub board: Option<i64>,
    /// Repo -> the branch it checks out instead of `branch`.
    pub repo_branches: IndexMap<String, String>,
    /// The environment profile; empty keeps `local`.
    pub environment: String,
    /// Start every database empty and run the seeds instead of copying main's.
    pub fresh_databases: bool,
}

/// What a new workspace of each repo will get for `node_modules`, asked off the UI thread.
pub type ModulesOutlook = Arc<dyn Fn(&str) -> Outlook + Send + Sync>;

/// What the rename form submits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameWorkspace {
    pub branch: String,
    pub display_name: String,
}

/// Claude's answer on its way back.
struct Refining {
    receiver: Receiver<Result<NameSuggestion, String>>,
}

impl Refining {
    fn start(namer: &Namer, seed: String, description: String) -> Refining {
        let (sender, receiver) = mpsc::channel();
        let namer = namer.clone();
        std::thread::spawn(move || {
            if sender.send(namer(&seed, &description)).is_err() {
                eprintln!("workspaces: the form closed before Claude answered");
            }
        });
        Refining { receiver }
    }

    /// The answer once it arrived.
    fn poll(&self) -> Option<Result<NameSuggestion, String>> {
        match self.receiver.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Claude stopped".into())),
        }
    }
}

/// `one` for a count of 1, else `many`.
fn verb(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

fn refine_status(refining: bool, error: Option<&str>) -> Option<Node> {
    if refining {
        return Some(status_line(
            IconKind::RotateCw,
            theme().icon_muted,
            "Refining with Claude...",
        ));
    }
    error.map(|error| status_line(IconKind::Warning, theme().warning, error))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreateFocus {
    Ticket,
    Name,
    Branch,
}

struct RepoRow {
    name: String,
    picked: bool,
    /// A branch other than the workspace's, picked for this repo.
    choice: Option<String>,
}

pub struct CreateWorkspaceModal {
    name: InputField,
    branch: InputField,
    /// The branch was typed or refined, so it no longer follows the name.
    branch_edited: bool,
    focus: CreateFocus,
    repos: Vec<RepoRow>,
    branch_source: Option<BranchSource>,
    known_branches: HashMap<String, RepoBranches>,
    listing: Option<Receiver<(String, Result<RepoBranches, String>)>>,
    picker: Option<BranchPicker>,
    existing: Vec<String>,
    namer: Namer,
    refining: Option<Refining>,
    refine_error: Option<String>,
    result: Option<ModalResult>,
    tickets: Option<TicketPicker>,
    /// The ticket summary last filled into the name, so a later pick may replace it.
    auto_name: Option<String>,
    outlooks: HashMap<String, Outlook>,
    outlook_receiver: Option<Receiver<(String, Outlook)>>,
    /// The project's environment profiles besides `local`.
    environments: Vec<String>,
    environment: usize,
    environment_menu_open: bool,
    /// Repos whose databases start as a copy of main's.
    seeded_from_main: Vec<String>,
    fresh_databases: bool,
}

impl CreateWorkspaceModal {
    /// `repos` are the project's repos; `existing` the branches that already have a workspace.
    pub fn new(repos: Vec<String>, existing: Vec<String>, namer: Namer) -> CreateWorkspaceModal {
        CreateWorkspaceModal {
            name: InputField::new("Name", "Display name"),
            branch: InputField::new("Branch", "feat-login").mono(),
            branch_edited: false,
            focus: CreateFocus::Name,
            repos: repos
                .into_iter()
                .map(|name| RepoRow {
                    name,
                    picked: true,
                    choice: None,
                })
                .collect(),
            branch_source: None,
            known_branches: HashMap::new(),
            listing: None,
            picker: None,
            existing,
            namer,
            refining: None,
            refine_error: None,
            result: None,
            tickets: None,
            auto_name: None,
            outlooks: HashMap::new(),
            outlook_receiver: None,
            environments: Vec::new(),
            environment: 0,
            environment_menu_open: false,
            seeded_from_main: Vec::new(),
            fresh_databases: false,
        }
    }

    /// Offers the project's environment profiles and, when some repos copy main's databases, the choice
    /// to start them empty.
    pub fn with_options(
        mut self,
        environments: Vec<String>,
        seeded_from_main: Vec<String>,
    ) -> CreateWorkspaceModal {
        self.environments = environments
            .into_iter()
            .filter(|name| name != "local")
            .collect();
        self.seeded_from_main = seeded_from_main;
        self
    }

    /// Shows on each repo row whether its node_modules come from the shared store.
    pub fn with_modules(mut self, outlook: ModulesOutlook) -> CreateWorkspaceModal {
        let (sender, receiver) = mpsc::channel();
        let repos: Vec<String> = self.repos.iter().map(|row| row.name.clone()).collect();
        std::thread::spawn(move || {
            for repo in repos {
                let found = outlook(&repo);
                if sender.send((repo, found)).is_err() {
                    return;
                }
            }
        });
        self.outlook_receiver = Some(receiver);
        self
    }

    fn poll_outlooks(&mut self) -> bool {
        let Some(receiver) = &self.outlook_receiver else {
            return false;
        };
        let mut changed = false;
        loop {
            match receiver.try_recv() {
                Ok((repo, outlook)) => {
                    self.outlooks.insert(repo, outlook);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.outlook_receiver = None;
                    return true;
                }
            }
        }
    }

    /// Adds the Ticket field, fed from the session's Jira.
    pub fn with_tickets(mut self, source: TicketSource) -> CreateWorkspaceModal {
        self.tickets = Some(TicketPicker::new(source, &self.existing));
        self.focus = CreateFocus::Ticket;
        self
    }

    /// Lets each repo row pick its own branch, listing every repo's branches in the background.
    pub fn with_branches(mut self, source: BranchSource) -> CreateWorkspaceModal {
        let (sender, receiver) = mpsc::channel();
        let repos: Vec<String> = self.repos.iter().map(|row| row.name.clone()).collect();
        let list = source.list.clone();
        std::thread::spawn(move || {
            for repo in repos {
                let branches = list(&repo);
                if sender.send((repo, branches)).is_err() {
                    return;
                }
            }
        });
        self.listing = Some(receiver);
        self.branch_source = Some(source);
        self
    }

    fn poll_listing(&mut self) -> bool {
        let Some(receiver) = &self.listing else {
            return false;
        };
        let mut changed = false;
        loop {
            match receiver.try_recv() {
                Ok((repo, Ok(branches))) => {
                    self.known_branches.entry(repo).or_insert(branches);
                    changed = true;
                }
                Ok((repo, Err(error))) => eprintln!("workspaces: branches of {repo}: {error}"),
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.listing = None;
                    return true;
                }
            }
        }
    }

    fn open_picker(&mut self, index: usize) {
        let Some(row) = self.repos.get(index) else {
            return;
        };
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.repo == row.name)
        {
            self.picker = None;
            return;
        }
        let mut picker = BranchPicker::open(&row.name, self.branch_source.as_ref());
        let current = row.choice.clone();
        let entries = picker.entries(&self.branch_text(), self.known_branches.get(&row.name));
        let at = entries.iter().position(|entry| match (entry, &current) {
            (Entry::WorkspaceBranch, None) => true,
            (Entry::Branch(info), Some(choice)) => &info.name == choice,
            _ => false,
        });
        picker.highlight(at.unwrap_or(0));
        self.picker = Some(picker);
    }

    fn picker_entries(&self) -> Vec<Entry> {
        self.picker
            .as_ref()
            .map(|picker| {
                picker.entries(&self.branch_text(), self.known_branches.get(&picker.repo))
            })
            .unwrap_or_default()
    }

    fn pick_branch(&mut self, index: usize) {
        let Some(entry) = self.picker_entries().into_iter().nth(index) else {
            return;
        };
        let choice = match entry {
            Entry::WorkspaceBranch => None,
            Entry::Branch(info) => Some(info.name),
            Entry::Create(name) => {
                if let Err(error) = pom_workspace::validate_branch_name(&name) {
                    if let Some(picker) = self.picker.as_mut() {
                        picker.error = Some(error);
                    }
                    return;
                }
                Some(name)
            }
        };
        let Some(picker) = self.picker.take() else {
            return;
        };
        if let Some(row) = self.repos.iter_mut().find(|row| row.name == picker.repo) {
            row.choice = choice.filter(|branch| *branch != self.branch.text().trim());
        }
    }

    fn picker_key(&mut self, key: EditKey, shift: bool) {
        let count = self.picker_entries().len();
        match key {
            EditKey::Escape => self.picker = None,
            EditKey::Up | EditKey::Down => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.move_highlight(key == EditKey::Down, count);
                }
            }
            EditKey::Enter => {
                let index = self.picker.as_ref().map(BranchPicker::highlighted);
                if let Some(index) = index {
                    self.pick_branch(index);
                }
            }
            EditKey::Tab | EditKey::Backtab => {
                self.picker = None;
                self.cycle_focus(key == EditKey::Backtab || shift);
            }
            key => {
                let changed = self
                    .picker
                    .as_mut()
                    .is_some_and(|picker| picker.query.key(key, shift));
                if changed {
                    self.picker_typed();
                }
            }
        }
    }

    fn picker_typed(&mut self) {
        let count = self.picker_entries().len();
        if let Some(picker) = self.picker.as_mut() {
            picker.typed(count);
        }
    }

    fn ticket_text(&self) -> String {
        self.tickets
            .as_ref()
            .map(|tickets| tickets.field.text().trim().to_string())
            .unwrap_or_default()
    }

    fn pick(&mut self, issue: pom_jira::SprintIssue) {
        let Some(tickets) = self.tickets.as_mut() else {
            return;
        };
        tickets.field.field.set_text(&issue.key);
        tickets.field.field.move_to_end();
        let name = self.name.text();
        if name.trim().is_empty() || self.auto_name.as_deref() == Some(name.as_str()) {
            self.name.field.set_text(&issue.summary);
            self.name.field.move_to_end();
            self.auto_name = Some(issue.summary);
        }
        self.follow();
        self.focus = CreateFocus::Name;
    }

    fn cycle_focus(&mut self, back: bool) {
        let order: &[CreateFocus] = if self.tickets.is_some() {
            &[CreateFocus::Ticket, CreateFocus::Name, CreateFocus::Branch]
        } else {
            &[CreateFocus::Name, CreateFocus::Branch]
        };
        let at = order
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(0);
        let next = if back {
            (at + order.len() - 1) % order.len()
        } else {
            (at + 1) % order.len()
        };
        self.focus = order[next];
    }

    fn branch_text(&self) -> String {
        self.branch.text().trim().to_string()
    }

    fn branch_error(&self) -> Option<String> {
        let branch = self.branch_text();
        if branch.is_empty() {
            return None;
        }
        if let Err(error) = pom_workspace::validate_branch_name(&branch) {
            return Some(error);
        }
        self.existing
            .contains(&branch)
            .then(|| format!("{branch} already has a workspace"))
    }

    fn can_create(&self) -> bool {
        !self.branch_text().is_empty()
            && self.branch_error().is_none()
            && (self.repos.is_empty() || self.repos.iter().any(|row| row.picked))
    }

    fn focused(&mut self) -> &mut InputField {
        match (self.focus, self.tickets.as_mut()) {
            (CreateFocus::Ticket, Some(tickets)) => &mut tickets.field,
            (CreateFocus::Branch, _) => &mut self.branch,
            _ => &mut self.name,
        }
    }

    /// An untouched branch comes from the ticket key when there is one, else from the name.
    fn follow(&mut self) {
        if self.branch_edited {
            return;
        }
        let source = match self.ticket_text() {
            ticket if ticket.is_empty() => self.name.text(),
            ticket => ticket,
        };
        self.branch.field.set_text(&slugify(&source));
        self.branch.field.move_to_end();
    }

    fn edited(&mut self) {
        match self.focus {
            CreateFocus::Ticket => {
                if let Some(tickets) = self.tickets.as_mut() {
                    tickets.typed();
                }
                self.follow();
            }
            CreateFocus::Name => self.follow(),
            CreateFocus::Branch => self.branch_edited = true,
        }
    }

    fn refine(&mut self) {
        let seed = match self.branch_text() {
            branch if branch.is_empty() => slugify(&self.name.text()),
            branch => branch,
        };
        if seed.is_empty() || self.refining.is_some() {
            return;
        }
        let description = self
            .tickets
            .as_ref()
            .and_then(TicketPicker::summary_of_typed)
            .unwrap_or_else(|| self.name.text());
        self.refine_error = None;
        self.refining = Some(Refining::start(&self.namer, seed, description));
    }

    fn submit(&mut self) {
        if !self.can_create() {
            return;
        }
        let branch = self.branch_text();
        let picked: Vec<&RepoRow> = self.repos.iter().filter(|row| row.picked).collect();
        let repos = if picked.len() == self.repos.len() {
            Vec::new()
        } else {
            picked.iter().map(|row| row.name.clone()).collect()
        };
        let repo_branches = picked
            .iter()
            .filter_map(|row| Some((row.name.clone(), row.choice.clone()?)))
            .filter(|(_, choice)| *choice != branch)
            .collect();
        self.result = Some(ModalResult::Submitted(Box::new(CreateWorkspace {
            display_name: self.name.text().trim().to_string(),
            repos,
            board: self.tickets.as_ref().and_then(TicketPicker::board),
            repo_branches,
            branch,
            environment: self.environment_name().unwrap_or_default().to_string(),
            fresh_databases: self.offers_data_choice() && self.fresh_databases,
        })));
    }

    /// The chosen profile, `None` for `local`.
    fn environment_name(&self) -> Option<&str> {
        self.environment
            .checked_sub(1)
            .and_then(|index| self.environments.get(index))
            .map(String::as_str)
    }

    fn offers_data_choice(&self) -> bool {
        self.repos
            .iter()
            .any(|row| row.picked && self.seeded_from_main.contains(&row.name))
    }

    /// The ticket a still-untouched field was filled from.
    fn filled_from(&self, untouched: bool) -> Option<String> {
        let ticket = self.ticket_text().to_uppercase();
        (untouched && !ticket.is_empty()).then(|| format!("from {ticket}"))
    }

    fn options_row(&self) -> Option<Node> {
        let colors = theme();
        let heading = |title: &str, hint: &str| {
            div()
                .row()
                .gap(6.0)
                .child(
                    label(title.to_string())
                        .label_size(LabelSize::Small)
                        .color(colors.text),
                )
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(hint.to_string())
                            .label_size(LabelSize::Small)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                )
        };
        let mut row = div().row().gap(12.0);
        let mut any = false;
        if !self.environments.is_empty() {
            let names: Vec<&str> = std::iter::once("local")
                .chain(self.environments.iter().map(String::as_str))
                .collect();
            let content = CREATE_WIDTH - 2.0 * SECTION_PADDING;
            let available = if self.offers_data_choice() {
                (content - 12.0) / 2.0
            } else {
                content
            };
            let control = self.environment_select(&names, available);
            row = row.child(
                div()
                    .col()
                    .flex(1.0)
                    .gap(4.0)
                    .child(heading("Environment", "what the services point at"))
                    .child(control),
            );
            any = true;
        }
        if self.offers_data_choice() {
            row = row.child(
                div()
                    .col()
                    .flex(1.0)
                    .gap(4.0)
                    .child(heading("Data", "the workspace's databases"))
                    .child(toggle_button_group(
                        &[
                            (DATA_FROM_MAIN, None, "Copy from main"),
                            (DATA_FRESH, None, "Empty, run seeds"),
                        ],
                        usize::from(self.fresh_databases),
                    )),
            );
            any = true;
        }
        any.then(|| row.into())
    }

    /// The profiles as a dropdown, so any number of them fits.
    fn environment_select(&self, names: &[&str], width: f32) -> Node {
        let colors = theme();
        let hairline = colors.border.alpha(0.6);
        let current = names.get(self.environment).copied().unwrap_or("local");
        let mut trigger = div()
            .row()
            .h_px(SELECT_HEIGHT)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .rounded(6.0)
            .border(1.0, hairline)
            .on_click(ENVIRONMENT_SELECT)
            .child(
                div().row().flex(1.0).items_center().child(
                    label(current.to_string())
                        .label_size(LabelSize::Small)
                        .color(colors.text)
                        .truncate(),
                ),
            )
            .child(
                icon(IconKind::ChevronDown)
                    .size(10.0)
                    .color(colors.icon_muted),
            );
        if self.environment_menu_open {
            trigger = trigger.bg(colors.ghost_element_hover);
        }
        let mut column = div().col().child(trigger);
        if self.environment_menu_open {
            let mut menu = div()
                .col()
                .w_px(width)
                .p(4.0)
                .rounded(8.0)
                .border(1.0, colors.border)
                .bg(colors.elevated_surface_background)
                .on_click(ENVIRONMENT_MENU_SURFACE);
            for (index, name) in names.iter().enumerate() {
                menu = menu.child(
                    div()
                        .row()
                        .h_px(26.0)
                        .px(8.0)
                        .items_center()
                        .rounded(4.0)
                        .on_click(ENVIRONMENT_BASE + index as u64)
                        .child(
                            label(name.to_string())
                                .label_size(LabelSize::Default)
                                .color(if index == self.environment {
                                    colors.text_accent
                                } else {
                                    colors.text
                                })
                                .truncate(),
                        ),
                );
            }
            column = column.child(
                deferred(menu)
                    .below_or_above(SELECT_HEIGHT, 4.0)
                    .snap_to_window()
                    .priority(1),
            );
        }
        column.into()
    }

    /// What Create is about to make, in one line: repos, installs, databases, environment.
    fn summary(&self) -> Node {
        let colors = theme();
        let picked: Vec<&RepoRow> = self.repos.iter().filter(|row| row.picked).collect();
        let mut parts = Vec::new();
        let outlook_of = |row: &&RepoRow| self.outlooks.get(&row.name).copied();
        let once = picked
            .iter()
            .filter(|row| outlook_of(row) == Some(Outlook::InstallsOnce))
            .count();
        let every_time = picked
            .iter()
            .filter(|row| outlook_of(row) == Some(Outlook::Installs))
            .count();
        let instant = picked
            .iter()
            .filter(|row| outlook_of(row) == Some(Outlook::FromStore))
            .count();
        if once > 0 {
            parts.push(format!("{once} {} once", verb(once, "installs", "install")));
        }
        if every_time > 0 {
            parts.push(format!(
                "{every_time} {}",
                verb(every_time, "installs", "install")
            ));
        }
        if once + every_time == 0 && instant > 0 && self.outlook_receiver.is_none() {
            parts.push("all instant".into());
        }
        if self.offers_data_choice() {
            parts.push(if self.fresh_databases {
                "databases seeded empty".into()
            } else {
                "databases copied from main".into()
            });
        }
        if !self.environments.is_empty() {
            parts.push(self.environment_name().unwrap_or("local").to_string());
        }
        let count = if self.repos.is_empty() {
            String::new()
        } else {
            format!("{} {}", picked.len(), verb(picked.len(), "repo", "repos"))
        };
        let rest = parts
            .iter()
            .map(|part| format!(" - {part}"))
            .collect::<String>();
        div()
            .row()
            .flex(1.0)
            .items_center()
            .px(4.0)
            .child(label(count).label_size(LabelSize::Small).color(colors.text))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(rest)
                        .label_size(LabelSize::Small)
                        .color(colors.text_muted)
                        .truncate(),
                ),
            )
            .into()
    }

    fn repos_list(&self) -> Node {
        let colors = theme();
        let mut list = div().col().rounded(6.0).border(1.0, colors.border_variant);
        for (index, row) in self.repos.iter().enumerate() {
            if index > 0 {
                list = list.child(div().h_px(1.0).bg(colors.border_variant));
            }
            list = list.child(self.repo_row(index, row));
        }
        div()
            .col()
            .gap(4.0)
            .child(
                div()
                    .row()
                    .gap(6.0)
                    .child(
                        label("Repos")
                            .label_size(LabelSize::Small)
                            .color(colors.text),
                    )
                    .child(
                        label("click a branch to use another one in that repo")
                            .label_size(LabelSize::Small)
                            .color(colors.text_muted),
                    ),
            )
            .child(list)
            .into()
    }

    fn repo_row(&self, index: usize, row: &RepoRow) -> Node {
        let colors = theme();
        let workspace_branch = self.branch_text();
        let open = self
            .picker
            .as_ref()
            .filter(|picker| picker.repo == row.name);
        let known = self.known_branches.get(&row.name);
        let (name, how) = match &row.choice {
            Some(choice) => (
                choice.clone(),
                known.map(|known| known.obtained(choice, true)),
            ),
            None => (
                workspace_branch.clone(),
                known.map(|known| known.obtained(&workspace_branch, false)),
            ),
        };
        let name_color = match (row.picked, row.choice.is_some()) {
            (false, _) => colors.text_disabled,
            (true, true) => colors.warning,
            (true, false) => colors.text,
        };
        let shown = if name.is_empty() {
            "workspace branch".to_string()
        } else {
            name
        };
        let mut branch_box = div()
            .row()
            .items_center()
            .gap(6.0)
            .h_px(TRIGGER_HEIGHT)
            .pl(8.0)
            .pr(6.0)
            .rounded(5.0)
            .on_click(BRANCH_BOX_BASE + index as u64)
            .child(icon(IconKind::Branch).size(12.0).color(colors.icon_muted))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(shown)
                        .label_size(LabelSize::Small)
                        .mono()
                        .color(name_color)
                        .truncate(),
                ),
            );
        branch_box = match open {
            Some(_) => branch_box
                .border(1.0, colors.border_variant)
                .bg(colors.ghost_element_hover),
            None => branch_box.border(1.0, Rgba::TRANSPARENT),
        };
        if let Some(how) = how.filter(|how| !how.is_empty()) {
            branch_box = branch_box.child(
                label(how)
                    .label_size(LabelSize::XSmall)
                    .color(colors.text_placeholder),
            );
        }
        branch_box = branch_box.child(
            icon(IconKind::ChevronDown)
                .size(10.0)
                .color(colors.icon_muted),
        );
        let mut anchor = div().col().flex(1.0).child(branch_box);
        if let Some(picker) = open {
            anchor = anchor.child(picker.render(&workspace_branch, known, row.choice.as_deref()));
        }
        let mut line = div()
            .row()
            .items_center()
            .gap(8.0)
            .h_px(36.0)
            .pl(6.0)
            .pr(8.0)
            .child(
                // A long repo name ends in an ellipsis instead of running under the branch box.
                div()
                    .row()
                    .w_px(REPO_NAME_W)
                    .items_center()
                    .on_click(REPO_BASE + index as u64)
                    .child(checkbox(REPO_BASE + index as u64, row.picked, ""))
                    .child(
                        div().row().flex(1.0).items_center().child(
                            label(row.name.clone())
                                .label_size(LabelSize::Default)
                                .color(if row.picked {
                                    colors.text
                                } else {
                                    colors.text_muted
                                })
                                .truncate(),
                        ),
                    ),
            )
            .child(anchor);
        if row.choice.is_some() && !workspace_branch.is_empty() {
            let text = format!("use {workspace_branch}");
            let width =
                ui::measure_text_width(&text, LabelSize::XSmall.px(), false, 400).min(140.0);
            line = line.child(
                div()
                    .row()
                    .items_center()
                    .w_px(width + 2.0)
                    .h_px(TRIGGER_HEIGHT)
                    .on_click(RESET_BASE + index as u64)
                    .child(
                        label(text)
                            .label_size(LabelSize::XSmall)
                            .color(colors.text_accent)
                            .truncate(),
                    ),
            );
        }
        line.child(self.modules_cell(row)).into()
    }

    fn modules_cell(&self, row: &RepoRow) -> Node {
        let colors = theme();
        let shown = match (row.picked, self.outlooks.get(&row.name)) {
            (false, _) | (_, None | Some(Outlook::NotNode)) => None,
            (_, Some(Outlook::FromStore)) => Some(("node_modules instant".into(), colors.success)),
            (_, Some(Outlook::InstallsOnce)) => Some(("installs once".into(), colors.warning)),
            (_, Some(Outlook::Installs)) => Some(("installs".into(), colors.text_muted)),
            (_, Some(Outlook::SelfManaged(name))) => Some((name.to_string(), colors.text_muted)),
        };
        let mut cell = div().row().w_px(MODULES_W).items_center().justify_end();
        if let Some((text, color)) = shown {
            cell = cell.child(
                label(text)
                    .label_size(LabelSize::XSmall)
                    .color(color)
                    .truncate(),
            );
        }
        cell.into()
    }
}

impl WindowModal for CreateWorkspaceModal {
    fn width(&self) -> f32 {
        CREATE_WIDTH
    }

    fn render(&mut self) -> Node {
        let branch_error = self.branch_error();
        let mut refine_row = div().row().items_center().gap(8.0).child(outlined_button(
            REFINE,
            Some(IconKind::Sparkle),
            "Refine name & branch with Claude",
            self.refining.is_none()
                && !(self.name.text().trim().is_empty() && self.branch_text().is_empty()),
        ));
        if let Some(status) = refine_status(self.refining.is_some(), self.refine_error.as_deref()) {
            refine_row = refine_row.child(status);
        }
        let mut section = modal_section(10.0);
        if let Some(tickets) = &self.tickets {
            section = section.child(tickets.render(
                self.focus == CreateFocus::Ticket,
                CREATE_WIDTH - 2.0 * SECTION_PADDING,
            ));
        }
        let name_from = self.filled_from(
            self.auto_name.is_some()
                && self.auto_name.as_deref() == Some(self.name.text().as_str()),
        );
        let branch_from = self.filled_from(!self.branch_edited);
        let names = div()
            .row()
            .gap(12.0)
            .child(div().col().flex(1.0).child(self.name.render(
                NAME_FIELD,
                self.focus == CreateFocus::Name,
                name_from.as_deref(),
                None,
            )))
            .child(
                div().col().flex(1.0).child(
                    self.branch.render(
                        BRANCH_FIELD,
                        self.focus == CreateFocus::Branch,
                        Some(
                            branch_from
                                .as_deref()
                                .unwrap_or("of every repo, unless set below"),
                        ),
                        branch_error.as_deref(),
                    ),
                ),
            );
        section = section.child(names).child(refine_row);
        if !self.repos.is_empty() {
            section = section.child(self.repos_list());
        }
        if let Some(options) = self.options_row() {
            section = section.child(options);
        }
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Create",
                Some("enter"),
                self.can_create(),
            ));
        modal_frame(CREATE_WIDTH)
            .on_click(FORM_SURFACE)
            .child(modal_header("Create Workspace", Some(CLOSE)))
            .child(section)
            .child(modal_footer(Some(self.summary()), buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            PICKER_QUERY | PICKER_SURFACE => return,
            PICKER_REFRESH => {
                if let (Some(picker), Some(source)) =
                    (self.picker.as_mut(), self.branch_source.as_ref())
                {
                    picker.fetch(source);
                }
                return;
            }
            id if (ENTRY_BASE..ENTRY_END).contains(&id) => {
                self.pick_branch((id - ENTRY_BASE) as usize);
                return;
            }
            id if (BRANCH_BOX_BASE..BRANCH_BOX_BASE + PER_REPO).contains(&id) => {
                self.open_picker((id - BRANCH_BOX_BASE) as usize);
                return;
            }
            _ => self.picker = None,
        }
        if id != ENVIRONMENT_SELECT && id != ENVIRONMENT_MENU_SURFACE {
            self.environment_menu_open = false;
        }
        let on_source_menu = id == SOURCE
            || id == SOURCE_MENU_SURFACE
            || (SOURCE_OPTION_BASE..SOURCE_OPTION_END).contains(&id);
        if !on_source_menu {
            if let Some(tickets) = self.tickets.as_mut() {
                tickets.close_menu();
            }
        }
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            NAME_FIELD => self.focus = CreateFocus::Name,
            BRANCH_FIELD => self.focus = CreateFocus::Branch,
            TICKET_FIELD => {
                self.focus = CreateFocus::Ticket;
                if let Some(tickets) = self.tickets.as_mut() {
                    tickets.open_list();
                }
            }
            SOURCE => {
                if let Some(tickets) = self.tickets.as_mut() {
                    tickets.toggle_menu();
                }
            }
            SOURCE_MENU_SURFACE | SUGGESTIONS_SURFACE => {}
            id if (SOURCE_OPTION_BASE..SOURCE_OPTION_END).contains(&id) => {
                if let Some(tickets) = self.tickets.as_mut() {
                    tickets.pick_source((id - SOURCE_OPTION_BASE) as usize);
                }
                self.focus = CreateFocus::Ticket;
            }
            id if (SUGGESTION_BASE..SUGGESTION_BASE + 100).contains(&id) => {
                let picked = self
                    .tickets
                    .as_ref()
                    .and_then(|tickets| tickets.suggestion((id - SUGGESTION_BASE) as usize));
                if let Some(issue) = picked {
                    self.pick(issue);
                }
            }
            REFINE => self.refine(),
            CONFIRM => self.submit(),
            id if (ENVIRONMENT_BASE..ENVIRONMENT_END).contains(&id) => {
                let index = (id - ENVIRONMENT_BASE) as usize;
                if index <= self.environments.len() {
                    self.environment = index;
                }
            }
            ENVIRONMENT_SELECT => self.environment_menu_open = !self.environment_menu_open,
            DATA_FROM_MAIN => self.fresh_databases = false,
            DATA_FRESH => self.fresh_databases = true,
            id if (REPO_BASE..REPO_BASE + PER_REPO).contains(&id) => {
                if let Some(row) = self.repos.get_mut((id - REPO_BASE) as usize) {
                    row.picked = !row.picked;
                }
            }
            id if (RESET_BASE..RESET_BASE + PER_REPO).contains(&id) => {
                if let Some(row) = self.repos.get_mut((id - RESET_BASE) as usize) {
                    row.choice = None;
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        if self.picker.is_some() {
            self.picker_key(key, shift);
            return true;
        }
        if key == EditKey::Escape && self.environment_menu_open {
            self.environment_menu_open = false;
            return true;
        }
        if key == EditKey::Escape {
            if let Some(tickets) = self.tickets.as_mut() {
                if tickets.menu_open() {
                    tickets.close_menu();
                    return true;
                }
                if self.focus == CreateFocus::Ticket && tickets.list_open() {
                    tickets.close_list();
                    return true;
                }
            }
        }
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Up | EditKey::Down if self.focus == CreateFocus::Ticket => {
                if let Some(tickets) = self.tickets.as_mut() {
                    tickets.move_highlight(key == EditKey::Down);
                }
            }
            EditKey::Enter if self.focus == CreateFocus::Ticket => {
                match self.tickets.as_ref().and_then(TicketPicker::highlighted) {
                    Some(issue) => self.pick(issue),
                    None => self.submit(),
                }
            }
            EditKey::Enter => self.submit(),
            EditKey::Tab | EditKey::Backtab => self.cycle_focus(key == EditKey::Backtab || shift),
            key => {
                if self.focused().field.key(key, shift) {
                    self.edited();
                }
            }
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        if let Some(picker) = self.picker.as_mut() {
            picker.query.insert(&typed);
            self.picker_typed();
            return true;
        }
        self.focused().field.insert(&typed);
        self.edited();
        true
    }

    fn copy(&self) -> Option<String> {
        if let Some(picker) = &self.picker {
            return picker.query.selected_text();
        }
        match (self.focus, self.tickets.as_ref()) {
            (CreateFocus::Ticket, Some(tickets)) => tickets.field.field.selected_text(),
            (CreateFocus::Branch, _) => self.branch.field.selected_text(),
            _ => self.name.field.selected_text(),
        }
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        if let Some(picker) = self.picker.as_mut() {
            if picker.query.key(EditKey::Backspace, false) {
                self.picker_typed();
            }
            return Some(text);
        }
        if self.focused().field.key(EditKey::Backspace, false) {
            self.edited();
        }
        Some(text)
    }

    fn tick(&mut self) -> bool {
        let mut loaded = self.tickets.as_mut().is_some_and(TicketPicker::poll);
        loaded |= self.poll_listing();
        loaded |= self.poll_outlooks();
        if let Some(picker) = self.picker.as_mut() {
            loaded |= picker.poll(&mut self.known_branches);
        }
        let Some(answer) = self.refining.as_ref().and_then(Refining::poll) else {
            return loaded;
        };
        self.refining = None;
        match answer {
            Ok(suggestion) => {
                if !suggestion.name.is_empty() {
                    self.name.field.set_text(&suggestion.name);
                    self.name.field.move_to_end();
                }
                if !suggestion.slug.is_empty() {
                    self.branch.field.set_text(&suggestion.slug);
                    self.branch.field.move_to_end();
                    self.branch_edited = true;
                }
            }
            Err(error) => self.refine_error = Some(error),
        }
        true
    }

    fn busy(&self) -> bool {
        self.refining.is_some()
            || self.listing.is_some()
            || self.outlook_receiver.is_some()
            || self.picker.as_ref().is_some_and(BranchPicker::busy)
            || self.tickets.as_ref().is_some_and(TicketPicker::busy)
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

pub struct RenameWorkspaceModal {
    branch: String,
    name: InputField,
    namer: Namer,
    refining: Option<Refining>,
    refine_error: Option<String>,
    result: Option<ModalResult>,
}

impl RenameWorkspaceModal {
    /// `current` is the display name today (empty when the workspace shows its branch).
    pub fn new(branch: &str, current: &str, namer: Namer) -> RenameWorkspaceModal {
        let mut name = InputField::new("Name", "Display name");
        let shown = if current.is_empty() {
            humanize_branch(branch)
        } else {
            current.to_string()
        };
        name.field.set_text(&shown);
        RenameWorkspaceModal {
            branch: branch.to_string(),
            name,
            namer,
            refining: None,
            refine_error: None,
            result: None,
        }
    }

    fn submit(&mut self) {
        self.result = Some(ModalResult::Submitted(Box::new(RenameWorkspace {
            branch: self.branch.clone(),
            display_name: self.name.text().trim().to_string(),
        })));
    }
}

impl WindowModal for RenameWorkspaceModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let note = div()
            .row()
            .gap(4.0)
            .child(
                label("Only the display name changes; the branch stays")
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted),
            )
            .child(
                label(self.branch.clone())
                    .label_size(LabelSize::Small)
                    .mono()
                    .color(colors.text),
            );
        let mut refine_row = div().row().items_center().gap(8.0).child(outlined_button(
            REFINE,
            Some(IconKind::Sparkle),
            "Refine with Claude",
            self.refining.is_none(),
        ));
        if let Some(status) = refine_status(self.refining.is_some(), self.refine_error.as_deref()) {
            refine_row = refine_row.child(status);
        }
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(CONFIRM, "Save", Some("enter"), true));
        modal_frame(WIDTH)
            .child(modal_header("Rename Workspace", Some(CLOSE)))
            .child(
                modal_section(10.0)
                    .child(note)
                    .child(self.name.render(NAME_FIELD, true, None, None))
                    .child(refine_row),
            )
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            REFINE if self.refining.is_none() => {
                self.refine_error = None;
                self.refining = Some(Refining::start(
                    &self.namer,
                    self.branch.clone(),
                    String::new(),
                ));
            }
            CONFIRM => self.submit(),
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            EditKey::Tab | EditKey::Backtab => {}
            key => {
                self.name.field.key(key, shift);
            }
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        self.name.field.insert(&typed);
        true
    }

    fn copy(&self) -> Option<String> {
        self.name.field.selected_text()
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.name.field.key(EditKey::Backspace, false);
        Some(text)
    }

    fn tick(&mut self) -> bool {
        let Some(answer) = self.refining.as_ref().and_then(Refining::poll) else {
            return false;
        };
        self.refining = None;
        match answer {
            Ok(suggestion) if !suggestion.name.is_empty() => {
                self.name.field.set_text(&suggestion.name);
                self.name.field.move_to_end();
            }
            Ok(_) => self.refine_error = Some("Claude's answer had no name".into()),
            Err(error) => self.refine_error = Some(error),
        }
        true
    }

    fn busy(&self) -> bool {
        self.refining.is_some()
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namer() -> Namer {
        Arc::new(|seed: &str, _: &str| {
            Ok(NameSuggestion {
                name: format!("Refined {seed}"),
                slug: format!("{seed}-refined"),
            })
        })
    }

    fn submitted<T: 'static + Clone>(result: Option<ModalResult>) -> Option<T> {
        match result {
            Some(ModalResult::Submitted(value)) => value.downcast_ref::<T>().cloned(),
            _ => None,
        }
    }

    fn painted_text(modal: &mut dyn WindowModal) -> String {
        let painted = ui::render(
            &modal.render(),
            ui::Rect::new(0.0, 0.0, WIDTH, 800.0, ui::Rgba::TRANSPARENT),
        );
        painted.texts.iter().map(|t| t.text.clone()).collect()
    }

    #[test]
    fn the_branch_follows_the_name_until_edited() {
        let mut modal = CreateWorkspaceModal::new(
            vec!["api".into(), "web".into()],
            vec!["taken".into()],
            namer(),
        );
        modal.text("Fix Login");
        assert_eq!(modal.branch_text(), "fix-login");
        modal.key(EditKey::Tab, false);
        modal.key(EditKey::Backspace, false);
        assert_eq!(modal.branch_text(), "fix-logi");
        modal.key(EditKey::Tab, false);
        modal.text(" page");
        assert_eq!(modal.branch_text(), "fix-logi", "an edited branch stays");

        modal.click(REPO_BASE);
        modal.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<CreateWorkspace>(modal.take_result()),
            Some(CreateWorkspace {
                branch: "fix-logi".into(),
                display_name: "Fix Login page".into(),
                repos: vec!["web".into()],
                board: None,
                repo_branches: IndexMap::new(),
                environment: String::new(),
                fresh_databases: false,
            })
        );
    }

    fn branch_source() -> BranchSource {
        let info = |name: &str, remote: bool| pom_workspace::BranchInfo {
            name: name.into(),
            remote,
            author: "Ana Lima".into(),
            relative_time: "40 minutes ago".into(),
            subject: "New checkout summary card".into(),
        };
        let branches = vec![info("main", false), info("ana/checkout-ui", true)];
        BranchSource {
            list: Arc::new(move |_| {
                Ok(RepoBranches {
                    base: "main".into(),
                    branches: branches.clone(),
                })
            }),
            fetch: Arc::new(|_| Ok(())),
        }
    }

    #[test]
    fn a_repo_takes_its_own_branch_from_the_picker() {
        let mut modal = CreateWorkspaceModal::new(
            vec!["api".into(), "web".into(), "mobile".into()],
            Vec::new(),
            namer(),
        )
        .with_branches(branch_source());
        modal.text("Fix checkout page");
        settle(&mut modal);
        assert!(painted_text(&mut modal).contains("new, from main"));

        modal.click(BRANCH_BOX_BASE + 1);
        settle(&mut modal);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("Branch for web") && text.contains("Fetched just now"),
            "{text}"
        );
        assert!(
            text.contains("Ana Lima - 40 minutes ago - New checkout summary card"),
            "{text}"
        );
        modal.text("checkout-ui");
        assert!(matches!(
            modal.picker_entries().first(),
            Some(Entry::Branch(info)) if info.name == "ana/checkout-ui"
        ));
        modal.key(EditKey::Enter, false);
        assert!(modal.picker.is_none(), "picking closes the picker");
        let text = painted_text(&mut modal);
        assert!(
            text.contains("taken over from origin") && text.contains("use fix-checkout-page"),
            "{text}"
        );

        modal.click(BRANCH_BOX_BASE);
        modal.text("ana/new-api");
        assert_eq!(
            modal.picker_entries(),
            [Entry::Create("ana/new-api".into())]
        );
        assert!(painted_text(&mut modal).contains("Create Branch: \"ana/new-api\""));
        modal.key(EditKey::Enter, false);
        modal.click(BRANCH_BOX_BASE + 2);
        modal.key(EditKey::Escape, false);
        assert!(
            modal.picker.is_none() && modal.take_result().is_none(),
            "escape only closes the picker"
        );
        modal.click(REPO_BASE + 2);
        modal.key(EditKey::Enter, false);
        let created = submitted::<CreateWorkspace>(modal.take_result()).expect("submitted");
        assert_eq!(created.branch, "fix-checkout-page");
        assert_eq!(created.repos, ["api", "web"]);
        assert_eq!(
            created.repo_branches.into_iter().collect::<Vec<_>>(),
            [
                ("api".to_string(), "ana/new-api".to_string()),
                ("web".to_string(), "ana/checkout-ui".to_string())
            ]
        );
    }

    #[test]
    fn use_the_workspace_branch_drops_the_override() {
        let mut modal = CreateWorkspaceModal::new(vec!["web".into()], Vec::new(), namer());
        modal.text("feat-login");
        modal.click(BRANCH_BOX_BASE);
        modal.text("ana/mail-retry");
        modal.key(EditKey::Enter, false);
        assert_eq!(modal.repos[0].choice.as_deref(), Some("ana/mail-retry"));
        modal.click(RESET_BASE);
        modal.key(EditKey::Enter, false);
        let created = submitted::<CreateWorkspace>(modal.take_result()).expect("submitted");
        assert!(created.repo_branches.is_empty() && created.repos.is_empty());
    }

    #[test]
    fn a_bad_or_taken_branch_blocks_create() {
        let mut modal = CreateWorkspaceModal::new(Vec::new(), vec!["taken".into()], namer());
        assert!(!modal.can_create(), "empty");
        modal.text("taken");
        assert!(painted_text(&mut modal).contains("taken already has a workspace"));
        modal.key(EditKey::Enter, false);
        assert!(modal.take_result().is_none());
        modal.key(EditKey::Tab, false);
        modal.key(EditKey::SelectAll, false);
        modal.text("a b");
        assert!(!modal.can_create());
        modal.key(EditKey::Escape, false);
        assert!(matches!(modal.take_result(), Some(ModalResult::Cancelled)));
    }

    #[test]
    fn refining_fills_name_and_branch_in_the_background() {
        let mut modal = CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer());
        modal.text("login");
        modal.click(REFINE);
        assert!(modal.busy());
        assert!(painted_text(&mut modal).contains("Refining with Claude..."));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !modal.tick() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!modal.busy());
        assert_eq!(modal.name.text(), "Refined login");
        assert_eq!(modal.branch_text(), "login-refined");
    }

    fn tickets(only_mine: bool) -> TicketSource {
        tickets_from(only_mine, None, Arc::new(|_| {}))
    }

    fn tickets_from(
        only_mine: bool,
        start: Option<crate::TicketList>,
        remember: Arc<dyn Fn(crate::TicketList) + Send + Sync>,
    ) -> TicketSource {
        let issue = |key: &str, summary: &str, mine: bool| pom_jira::SprintIssue {
            key: key.into(),
            summary: summary.into(),
            mine,
            ..pom_jira::SprintIssue::default()
        };
        let issues = vec![
            issue("PROJ-1", "Login page", false),
            issue("PROJ-2", "Checkout total", true),
            issue("PROJ-3", "Taken already", true),
        ];
        TicketSource {
            boards: Arc::new(|| {
                Ok(vec![
                    pom_jira::Board {
                        id: 7,
                        name: "Web".into(),
                    },
                    pom_jira::Board {
                        id: 9,
                        name: "Mobile".into(),
                    },
                ])
            }),
            issues: Arc::new(move |list| {
                Ok(match list {
                    crate::TicketList::Sprint(9) => issues.clone(),
                    crate::TicketList::Assigned => {
                        issues.iter().filter(|issue| issue.mine).cloned().collect()
                    }
                    _ => Vec::new(),
                })
            }),
            board: Some(9),
            start,
            remember,
            only_mine,
        }
    }

    fn settle(modal: &mut CreateWorkspaceModal) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while modal.busy() && std::time::Instant::now() < deadline {
            modal.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_picked_ticket_names_the_workspace_and_its_branch() {
        let mut modal = CreateWorkspaceModal::new(Vec::new(), vec!["proj-3-old".into()], namer())
            .with_tickets(tickets(false));
        settle(&mut modal);
        let keys = |modal: &CreateWorkspaceModal| -> Vec<String> {
            modal
                .tickets
                .as_ref()
                .map(|tickets| tickets.suggestions().into_iter().map(|i| i.key).collect())
                .unwrap_or_default()
        };
        assert_eq!(
            keys(&modal),
            ["PROJ-2", "PROJ-1"],
            "mine first, taken dropped"
        );
        modal.text("log");
        assert_eq!(keys(&modal), ["PROJ-1"]);
        modal.key(EditKey::Enter, false);
        assert_eq!(modal.ticket_text(), "PROJ-1");
        assert_eq!(modal.name.text(), "Login page");
        assert_eq!(modal.branch_text(), "proj-1");

        modal.click(TICKET_FIELD);
        modal.key(EditKey::SelectAll, false);
        modal.text("PROJ-");
        modal.key(EditKey::Down, false);
        modal.key(EditKey::Enter, false);
        assert_eq!(modal.ticket_text(), "PROJ-1", "down moved past PROJ-2");
        modal.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<CreateWorkspace>(modal.take_result()).map(|c| (c.branch, c.board)),
            Some(("proj-1".to_string(), Some(9)))
        );
    }

    #[test]
    fn only_mine_hides_other_tickets() {
        let mut modal =
            CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer()).with_tickets(tickets(true));
        settle(&mut modal);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("Checkout total") && !text.contains("Login page"),
            "{text}"
        );
        assert!(text.contains("Mobile"), "the remembered board: {text}");
    }

    #[test]
    fn the_source_menu_switches_to_my_tickets_and_escape_closes_the_list_first() {
        let mut modal =
            CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer()).with_tickets(tickets(false));
        settle(&mut modal);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("Login page") && text.contains("Mobile"),
            "{text}"
        );
        modal.click(SOURCE);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("Backlog") && text.contains("Assigned to me") && text.contains("Web"),
            "{text}"
        );
        modal.click(SOURCE_OPTION_BASE + 3);
        settle(&mut modal);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("Assigned to me") && text.contains("Checkout total"),
            "{text}"
        );
        assert!(!text.contains("Login page"), "only mine: {text}");

        modal.key(EditKey::Escape, false);
        assert!(!painted_text(&mut modal).contains("Checkout total"));
        assert!(modal.take_result().is_none(), "escape closed the list only");
        modal.key(EditKey::Enter, false);
        assert!(
            modal.take_result().is_none(),
            "enter with the list closed picks nothing and there is no branch yet"
        );
        modal.key(EditKey::Escape, false);
        assert!(matches!(modal.take_result(), Some(ModalResult::Cancelled)));
    }

    #[test]
    fn the_picked_list_is_remembered_and_opens_first_next_time() {
        let picked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = picked.clone();
        let mut modal =
            CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer()).with_tickets(tickets_from(
                false,
                None,
                Arc::new(move |list| record.lock().expect("picked").push(list)),
            ));
        settle(&mut modal);
        modal.click(SOURCE);
        modal.click(SOURCE_OPTION_BASE + 3);
        assert_eq!(
            *picked.lock().expect("picked"),
            [crate::TicketList::Assigned]
        );

        let mut reopened = CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer()).with_tickets(
            tickets_from(false, Some(crate::TicketList::Assigned), Arc::new(|_| {})),
        );
        settle(&mut reopened);
        let text = painted_text(&mut reopened);
        assert!(
            text.contains("Assigned to me") && text.contains("Checkout total"),
            "{text}"
        );
        assert!(!text.contains("Login page"), "{text}");
    }

    #[test]
    fn the_environment_is_picked_from_a_dropdown() {
        let names: Vec<String> = (1..=8).map(|index| format!("staging-{index}")).collect();
        let mut modal = CreateWorkspaceModal::new(vec!["api".into()], Vec::new(), namer())
            .with_options(names, vec!["api".into()]);
        modal.text("feat-login");
        let text = painted_text(&mut modal);
        assert!(
            text.contains("local") && !text.contains("staging-3"),
            "{text}"
        );
        modal.click(ENVIRONMENT_SELECT);
        assert!(painted_text(&mut modal).contains("staging-8"));
        modal.click(ENVIRONMENT_BASE + 3);
        assert!(!modal.environment_menu_open);
        modal.key(EditKey::Enter, false);
        let created = submitted::<CreateWorkspace>(modal.take_result()).expect("submitted");
        assert_eq!(created.environment, "staging-3");
    }

    #[test]
    fn a_ticket_fills_name_and_branch_and_says_so() {
        let mut modal =
            CreateWorkspaceModal::new(Vec::new(), Vec::new(), namer()).with_tickets(tickets(false));
        settle(&mut modal);
        modal.key(EditKey::Enter, false);
        let text = painted_text(&mut modal);
        assert_eq!(text.matches("from PROJ-2").count(), 2, "{text}");
        modal.key(EditKey::Backspace, false);
        let text = painted_text(&mut modal);
        assert_eq!(
            text.matches("from PROJ-2").count(),
            1,
            "the edited name drops it: {text}"
        );
    }

    #[test]
    fn options_and_modules_shape_the_summary_and_the_request() {
        let outlook: ModulesOutlook = Arc::new(|repo: &str| match repo {
            "api" => Outlook::FromStore,
            "web" => Outlook::InstallsOnce,
            _ => Outlook::NotNode,
        });
        let mut modal = CreateWorkspaceModal::new(
            vec!["api".into(), "web".into(), "infra".into()],
            Vec::new(),
            namer(),
        )
        .with_options(vec!["local".into(), "staging".into()], vec!["api".into()])
        .with_modules(outlook);
        settle(&mut modal);
        modal.text("feat-login");
        let text = painted_text(&mut modal);
        assert!(
            text.contains("node_modules instant") && text.contains("installs once"),
            "{text}"
        );
        assert!(
            text.contains("3 repos - 1 installs once - databases copied from main - local"),
            "{text}"
        );
        modal.click(ENVIRONMENT_BASE + 1);
        modal.click(DATA_FRESH);
        modal.click(REPO_BASE + 1);
        let text = painted_text(&mut modal);
        assert!(
            text.contains("2 repos - all instant - databases seeded empty - staging"),
            "{text}"
        );
        modal.click(REPO_BASE);
        assert!(
            !painted_text(&mut modal).contains("Copy from main"),
            "no picked repo copies main"
        );
        modal.click(REPO_BASE);
        modal.key(EditKey::Enter, false);
        let created = submitted::<CreateWorkspace>(modal.take_result()).expect("submitted");
        assert_eq!(created.environment, "staging");
        assert!(created.fresh_databases);
        assert_eq!(created.repos, ["api", "infra"]);
    }

    #[test]
    fn rename_starts_from_the_current_name_and_saves() {
        let mut modal = RenameWorkspaceModal::new("proj-101-fix", "", namer());
        assert_eq!(modal.name.text(), "PROJ-101 Fix");
        modal.key(EditKey::SelectAll, false);
        modal.text("Login");
        modal.click(CONFIRM);
        assert_eq!(
            submitted::<RenameWorkspace>(modal.take_result()),
            Some(RenameWorkspace {
                branch: "proj-101-fix".into(),
                display_name: "Login".into(),
            })
        );
        let text = painted_text(&mut RenameWorkspaceModal::new("b", "Shown", namer()));
        assert!(text.contains("Rename Workspace") && text.contains("Shown"));
    }
}
