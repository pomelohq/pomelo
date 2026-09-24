//! The create and rename forms. Both float over the window, take the keyboard (Tab between fields, Enter to
//! confirm, Escape to cancel) and can ask Claude for a name in the background.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use pom_agent::NameSuggestion;
use ui::{div, label, theme, IconKind, LabelSize, Node};
use workspace::{
    checkbox, modal_button, modal_footer, modal_frame, modal_header, modal_section,
    outlined_button, status_line, EditKey, InputField, ModalResult, WindowModal, WINDOW_MODAL_BASE,
};

use crate::{humanize_branch, slugify, Namer};

/// The reference's form modals are 34rem wide.
const WIDTH: f32 = 544.0;

const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const NAME_FIELD: u64 = WINDOW_MODAL_BASE + 2;
const BRANCH_FIELD: u64 = WINDOW_MODAL_BASE + 3;
const REFINE: u64 = WINDOW_MODAL_BASE + 4;
const CANCEL: u64 = WINDOW_MODAL_BASE + 5;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 6;
const REPO_BASE: u64 = WINDOW_MODAL_BASE + 100;

/// What the create form submits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateWorkspace {
    pub branch: String,
    pub display_name: String,
    /// Repos to check out; empty means all.
    pub repos: Vec<String>,
}

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
    Name,
    Branch,
}

pub struct CreateWorkspaceModal {
    name: InputField,
    branch: InputField,
    /// The branch was typed or refined, so it no longer follows the name.
    branch_edited: bool,
    focus: CreateFocus,
    repos: Vec<(String, bool)>,
    existing: Vec<String>,
    namer: Namer,
    refining: Option<Refining>,
    refine_error: Option<String>,
    result: Option<ModalResult>,
}

impl CreateWorkspaceModal {
    /// `repos` are the project's repos; `existing` the branches that already have a workspace.
    pub fn new(repos: Vec<String>, existing: Vec<String>, namer: Namer) -> CreateWorkspaceModal {
        CreateWorkspaceModal {
            name: InputField::new("Name", "Display name"),
            branch: InputField::new("Branch", "feat-login").mono(),
            branch_edited: false,
            focus: CreateFocus::Name,
            repos: repos.into_iter().map(|repo| (repo, false)).collect(),
            existing,
            namer,
            refining: None,
            refine_error: None,
            result: None,
        }
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
        !self.branch_text().is_empty() && self.branch_error().is_none()
    }

    fn focused(&mut self) -> &mut InputField {
        match self.focus {
            CreateFocus::Name => &mut self.name,
            CreateFocus::Branch => &mut self.branch,
        }
    }

    fn follow_name(&mut self) {
        if !self.branch_edited {
            self.branch.field.set_text(&slugify(&self.name.text()));
            self.branch.field.move_to_end();
        }
    }

    fn edited(&mut self) {
        match self.focus {
            CreateFocus::Name => self.follow_name(),
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
        self.refine_error = None;
        self.refining = Some(Refining::start(&self.namer, seed, self.name.text()));
    }

    fn submit(&mut self) {
        if !self.can_create() {
            return;
        }
        self.result = Some(ModalResult::Submitted(Box::new(CreateWorkspace {
            branch: self.branch_text(),
            display_name: self.name.text().trim().to_string(),
            repos: self
                .repos
                .iter()
                .filter(|(_, picked)| *picked)
                .map(|(repo, _)| repo.clone())
                .collect(),
        })));
    }

    fn repos_list(&self) -> Node {
        let colors = theme();
        let mut list = div().col().gap(2.0);
        for (index, (repo, picked)) in self.repos.iter().enumerate() {
            list = list.child(checkbox(REPO_BASE + index as u64, *picked, repo));
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
                        label("none checked means all")
                            .label_size(LabelSize::Small)
                            .color(colors.text_muted),
                    ),
            )
            .child(list)
            .into()
    }
}

impl WindowModal for CreateWorkspaceModal {
    fn width(&self) -> f32 {
        WIDTH
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
        let mut section = modal_section(10.0)
            .child(self.name.render(
                NAME_FIELD,
                self.focus == CreateFocus::Name,
                Some("kept as typed"),
                None,
            ))
            .child(self.branch.render(
                BRANCH_FIELD,
                self.focus == CreateFocus::Branch,
                Some("the git branch of every repo"),
                branch_error.as_deref(),
            ))
            .child(refine_row);
        if !self.repos.is_empty() {
            section = section.child(self.repos_list());
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
        modal_frame(WIDTH)
            .child(modal_header("Create Workspace", Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            NAME_FIELD => self.focus = CreateFocus::Name,
            BRANCH_FIELD => self.focus = CreateFocus::Branch,
            REFINE => self.refine(),
            CONFIRM => self.submit(),
            id if id >= REPO_BASE => {
                if let Some((_, picked)) = self.repos.get_mut((id - REPO_BASE) as usize) {
                    *picked = !*picked;
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            EditKey::Tab | EditKey::Backtab => {
                self.focus = match self.focus {
                    CreateFocus::Name => CreateFocus::Branch,
                    CreateFocus::Branch => CreateFocus::Name,
                };
            }
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
        self.focused().field.insert(&typed);
        self.edited();
        true
    }

    fn copy(&self) -> Option<String> {
        match self.focus {
            CreateFocus::Name => self.name.field.selected_text(),
            CreateFocus::Branch => self.branch.field.selected_text(),
        }
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        if self.focused().field.key(EditKey::Backspace, false) {
            self.edited();
        }
        Some(text)
    }

    fn tick(&mut self) -> bool {
        let Some(answer) = self.refining.as_ref().and_then(Refining::poll) else {
            return false;
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
    use std::sync::Arc;

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

        modal.click(REPO_BASE + 1);
        modal.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<CreateWorkspace>(modal.take_result()),
            Some(CreateWorkspace {
                branch: "fix-logi".into(),
                display_name: "Fix Login page".into(),
                repos: vec!["web".into()],
            })
        );
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
