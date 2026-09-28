use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use ui::{div, label, theme, IconKind, LabelSize, Node};
use workspace::{
    checkbox, modal_button, modal_footer, modal_frame, modal_header, modal_section, status_line,
    EditKey, InputField, ModalResult, WindowModal, WINDOW_MODAL_BASE,
};

const WIDTH: f32 = 460.0;
const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const FIELD: u64 = WINDOW_MODAL_BASE + 2;
const CANCEL: u64 = WINDOW_MODAL_BASE + 3;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 4;

use crate::repo_branch_picker::{
    BranchPicker, BranchSource, Entry, RepoBranches, ENTRY_BASE, ENTRY_END, PICKER_REFRESH,
};

/// A repo's new alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameAlias {
    pub repo: String,
    pub alias: String,
}

pub struct RenameAliasModal {
    repo: String,
    current: String,
    field: InputField,
    result: Option<ModalResult>,
}

impl RenameAliasModal {
    pub fn new(repo: &str, current: &str) -> RenameAliasModal {
        let mut field = InputField::new("Alias", "fe").mono();
        field.field.set_text(current);
        field.field.move_to_end();
        RenameAliasModal {
            repo: repo.to_string(),
            current: current.to_string(),
            field,
            result: None,
        }
    }

    fn alias(&self) -> String {
        self.field.text().trim().to_string()
    }

    fn error(&self) -> Option<&'static str> {
        let alias = self.alias();
        (!alias.is_empty()
            && !alias
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .then_some("letters, digits, - and _ only")
    }

    fn can_rename(&self) -> bool {
        let alias = self.alias();
        !alias.is_empty() && alias != self.current && self.error().is_none()
    }

    fn submit(&mut self) {
        if self.can_rename() {
            self.result = Some(ModalResult::Submitted(Box::new(RenameAlias {
                repo: self.repo.clone(),
                alias: self.alias(),
            })));
        }
    }
}

impl WindowModal for RenameAliasModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let hint = format!(
            "References like {{{{{}.web.url}}}} are rewritten too",
            self.current
        );
        let section =
            modal_section(10.0).child(self.field.render(FIELD, true, Some(&hint), self.error()));
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Rename",
                Some("enter"),
                self.can_rename(),
            ));
        modal_frame(WIDTH)
            .child(modal_header(
                &format!("Rename Alias of {}", self.repo),
                Some(CLOSE),
            ))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CONFIRM => self.submit(),
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            key => {
                self.field.field.key(key, shift);
            }
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        self.field.field.insert(&typed);
        true
    }

    fn copy(&self) -> Option<String> {
        self.field.field.selected_text()
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.field.field.key(EditKey::Backspace, false);
        Some(text)
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

/// Confirmed: take this repo out of the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveRepo {
    pub repo: String,
}

pub struct RemoveRepoModal {
    repo: String,
    result: Option<ModalResult>,
}

impl RemoveRepoModal {
    pub fn new(repo: &str) -> RemoveRepoModal {
        RemoveRepoModal {
            repo: repo.to_string(),
            result: None,
        }
    }
}

impl WindowModal for RemoveRepoModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let section = modal_section(8.0)
            .child(
                label(format!(
                    "{} leaves the config. Its worktree goes from every workspace where it has no changes; \
                     worktrees with changes and main's clone stay on disk.",
                    self.repo
                ))
                .label_size(LabelSize::Small)
                .color(colors.text),
            )
            .child(status_line(
                IconKind::Warning,
                colors.warning,
                "Refused while other repos still refer to it",
            ));
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(CONFIRM, "Remove", Some("enter"), true));
        modal_frame(WIDTH)
            .child(modal_header(&format!("Remove {}?", self.repo), Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CONFIRM => {
                self.result = Some(ModalResult::Submitted(Box::new(RemoveRepo {
                    repo: self.repo.clone(),
                })))
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, _shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.click(CONFIRM),
            _ => {}
        }
        true
    }

    fn text(&mut self, _text: &str) -> bool {
        false
    }

    fn copy(&self) -> Option<String> {
        None
    }

    fn cut(&mut self) -> Option<String> {
        None
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

const PICK_BASE: u64 = WINDOW_MODAL_BASE + 100;
const PICK_LIMIT: u64 = 200;

/// Repos to check out in a workspace that started without them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickedRepos {
    pub branch: String,
    pub repos: Vec<String>,
}

pub struct PickReposModal {
    branch: String,
    repos: Vec<(String, bool)>,
    result: Option<ModalResult>,
}

impl PickReposModal {
    /// `available` are the config's repos the workspace has not checked out.
    pub fn new(branch: &str, available: Vec<String>) -> PickReposModal {
        PickReposModal {
            branch: branch.to_string(),
            repos: available.into_iter().map(|repo| (repo, false)).collect(),
            result: None,
        }
    }

    fn picked(&self) -> Vec<String> {
        self.repos
            .iter()
            .filter(|(_, picked)| *picked)
            .map(|(repo, _)| repo.clone())
            .collect()
    }

    fn submit(&mut self) {
        let repos = self.picked();
        if !repos.is_empty() {
            self.result = Some(ModalResult::Submitted(Box::new(PickedRepos {
                branch: self.branch.clone(),
                repos,
            })));
        }
    }
}

impl WindowModal for PickReposModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let mut section = modal_section(6.0);
        for (index, (repo, picked)) in self.repos.iter().enumerate().take(PICK_LIMIT as usize) {
            section = section.child(checkbox(PICK_BASE + index as u64, *picked, repo));
        }
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Add",
                Some("enter"),
                !self.picked().is_empty(),
            ));
        modal_frame(WIDTH)
            .child(modal_header(
                &format!("Add Repos to {}", self.branch),
                Some(CLOSE),
            ))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CONFIRM => self.submit(),
            id if (PICK_BASE..PICK_BASE + PICK_LIMIT).contains(&id) => {
                if let Some((_, picked)) = self.repos.get_mut((id - PICK_BASE) as usize) {
                    *picked = !*picked;
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, _shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            _ => {}
        }
        true
    }

    fn text(&mut self, _text: &str) -> bool {
        false
    }

    fn copy(&self) -> Option<String> {
        None
    }

    fn cut(&mut self) -> Option<String> {
        None
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

const USE_BRANCH_WIDTH: f32 = 360.0;

/// Check out this branch in the repo's worktree of the workspace and remember it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseBranch {
    pub repo: String,
    pub branch: String,
}

/// Picks the branch one repo of an existing workspace uses: the workspace's first, then the repo's own.
pub struct UseBranchModal {
    workspace_branch: String,
    /// The branch the worktree is on now, `None` when detached or unreadable.
    current: Option<String>,
    /// Where the worktree is, as the line under the title names it.
    checkout: String,
    picker: BranchPicker,
    source: BranchSource,
    known: HashMap<String, RepoBranches>,
    listing: Option<Receiver<Result<RepoBranches, String>>>,
    result: Option<ModalResult>,
}

impl UseBranchModal {
    pub fn new(
        repo: &str,
        workspace_branch: &str,
        current: Option<&str>,
        checkout: &str,
        source: BranchSource,
    ) -> UseBranchModal {
        let (sender, receiver) = mpsc::channel();
        let (list, listed_repo) = (source.list.clone(), repo.to_string());
        std::thread::spawn(move || {
            if sender.send(list(&listed_repo)).is_err() {
                eprintln!("workspaces: the branch form closed before {listed_repo} listed");
            }
        });
        UseBranchModal {
            workspace_branch: workspace_branch.to_string(),
            current: current.map(str::to_string),
            checkout: checkout.to_string(),
            picker: BranchPicker::open(repo, Some(&source)),
            source,
            known: HashMap::new(),
            listing: Some(receiver),
            result: None,
        }
    }

    fn repo(&self) -> &str {
        &self.picker.repo
    }

    fn choice(&self) -> Option<&str> {
        match self.current.as_deref() {
            Some(branch) if branch == self.workspace_branch => None,
            Some(branch) => Some(branch),
            // A detached worktree is on no listed branch, so no row gets the check.
            None => Some(""),
        }
    }

    fn entries(&self) -> Vec<Entry> {
        self.picker
            .entries(&self.workspace_branch, self.known.get(self.repo()))
    }

    fn typed(&mut self) {
        let count = self.entries().len();
        self.picker.typed(count);
    }

    fn submit(&mut self, index: usize) {
        let branch = match self.entries().into_iter().nth(index) {
            Some(Entry::WorkspaceBranch) => self.workspace_branch.clone(),
            Some(Entry::Branch(info)) => info.name,
            Some(Entry::Create(name)) => {
                if let Err(error) = pom_workspace::validate_branch_name(&name) {
                    self.picker.error = Some(error);
                    return;
                }
                name
            }
            None => return,
        };
        self.result = Some(ModalResult::Submitted(Box::new(UseBranch {
            repo: self.repo().to_string(),
            branch,
        })));
    }

    fn poll_listing(&mut self) -> bool {
        let Some(receiver) = &self.listing else {
            return false;
        };
        let listed = match receiver.try_recv() {
            Ok(listed) => listed,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("the listing stopped".into()),
        };
        self.listing = None;
        match listed {
            Ok(branches) => {
                self.known
                    .entry(self.repo().to_string())
                    .or_insert(branches);
            }
            Err(error) => eprintln!("workspaces: branches of {}: {error}", self.repo()),
        }
        true
    }
}

impl WindowModal for UseBranchModal {
    fn width(&self) -> f32 {
        USE_BRANCH_WIDTH
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let repo = self.repo().to_string();
        let on = match &self.current {
            Some(branch) => format!("{repo} is on {branch}."),
            None => format!("{repo} is not on a branch."),
        };
        let intro = modal_section(0.0).child(
            label(format!(
                "{on} Pick the branch it should use in this workspace; Pomelo fetches, checks it out in \
                 {}/{repo} and remembers it.",
                self.checkout
            ))
            .label_size(LabelSize::Small)
            .color(colors.text_muted)
            .wrap(USE_BRANCH_WIDTH - 24.0),
        );
        let picker =
            self.picker
                .render_inline(&self.workspace_branch, self.known.get(&repo), self.choice());
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Check Out",
                Some("enter"),
                !self.entries().is_empty(),
            ));
        modal_frame(USE_BRANCH_WIDTH)
            .child(modal_header(
                &format!("Use another branch in {repo}"),
                Some(CLOSE),
            ))
            .child(intro)
            .child(picker)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CONFIRM => self.submit(self.picker.highlighted()),
            PICKER_REFRESH => self.picker.fetch(&self.source),
            id if (ENTRY_BASE..ENTRY_END).contains(&id) => {
                let index = (id - ENTRY_BASE) as usize;
                self.picker.highlight(index);
                self.submit(index);
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(self.picker.highlighted()),
            EditKey::Up | EditKey::Down => {
                let count = self.entries().len();
                self.picker.move_highlight(key == EditKey::Down, count);
            }
            EditKey::Tab | EditKey::Backtab => {}
            key => {
                if self.picker.query.key(key, shift) {
                    self.typed();
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
        self.picker.query.insert(&typed);
        self.typed();
        true
    }

    fn copy(&self) -> Option<String> {
        self.picker.query.selected_text()
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        if self.picker.query.key(EditKey::Backspace, false) {
            self.typed();
        }
        Some(text)
    }

    fn tick(&mut self) -> bool {
        let listed = self.poll_listing();
        self.picker.poll(&mut self.known) || listed
    }

    fn busy(&self) -> bool {
        self.listing.is_some() || self.picker.busy()
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_picked_until_the_user_ticks_a_repo() {
        let mut form = PickReposModal::new("feat-a", vec!["api".into(), "web".into()]);
        form.key(EditKey::Enter, false);
        assert!(form.take_result().is_none());
        form.click(PICK_BASE + 1);
        form.key(EditKey::Enter, false);
        match form.take_result() {
            Some(ModalResult::Submitted(value)) => assert_eq!(
                value.downcast_ref::<PickedRepos>().cloned(),
                Some(PickedRepos {
                    branch: "feat-a".into(),
                    repos: vec!["web".into()]
                })
            ),
            _ => panic!("submitted"),
        }
    }

    #[test]
    fn renames_only_to_a_new_valid_alias() {
        let mut form = RenameAliasModal::new("api", "be");
        form.key(EditKey::Enter, false);
        assert!(form.take_result().is_none(), "unchanged");
        form.text("/x");
        assert!(form.error().is_some());
        form.field.field.set_text("backend");
        form.key(EditKey::Enter, false);
        match form.take_result() {
            Some(ModalResult::Submitted(value)) => assert_eq!(
                value.downcast_ref::<RenameAlias>().cloned(),
                Some(RenameAlias {
                    repo: "api".into(),
                    alias: "backend".into()
                })
            ),
            _ => panic!("submitted"),
        }
    }

    fn use_branch_modal() -> UseBranchModal {
        let info = |name: &str, remote: bool| pom_workspace::BranchInfo {
            name: name.into(),
            remote,
            author: "Ana Lima".into(),
            relative_time: "2 hours ago".into(),
            subject: "Retry failed mail".into(),
        };
        let source = BranchSource {
            list: std::sync::Arc::new(move |_| {
                Ok(RepoBranches {
                    base: "fix-checkout-page".into(),
                    branches: vec![
                        info("fix-checkout-page", false),
                        info("main", false),
                        info("ana/mail-retry", true),
                    ],
                })
            }),
            fetch: std::sync::Arc::new(|_| Ok(())),
        };
        let mut modal = UseBranchModal::new(
            "worker",
            "fix-checkout-page",
            Some("fix-checkout-page"),
            "workspace--fix-checkout-page",
            source,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while modal.busy() && std::time::Instant::now() < deadline {
            modal.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        modal
    }

    fn submitted_branch(modal: &mut UseBranchModal) -> Option<UseBranch> {
        match modal.take_result() {
            Some(ModalResult::Submitted(value)) => value.downcast_ref::<UseBranch>().cloned(),
            _ => None,
        }
    }

    #[test]
    fn checks_out_the_branch_typed_for() {
        let mut modal = use_branch_modal();
        let painted = ui::render(
            &modal.render(),
            ui::Rect::new(0.0, 0.0, USE_BRANCH_WIDTH, 800.0, ui::Rgba::TRANSPARENT),
        );
        let text: String = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(
            text.contains("Use another branch in worker")
                && text.contains("Branches of worker")
                && text.contains("origin/ana/mail-retry")
                && text.contains("Fetched just now"),
            "{text}"
        );
        modal.text("mail");
        modal.key(EditKey::Enter, false);
        assert_eq!(
            submitted_branch(&mut modal),
            Some(UseBranch {
                repo: "worker".into(),
                branch: "ana/mail-retry".into()
            })
        );
    }

    #[test]
    fn the_workspace_branch_leads_and_escape_cancels() {
        let mut modal = use_branch_modal();
        modal.key(EditKey::Down, false);
        modal.key(EditKey::Up, false);
        modal.key(EditKey::Enter, false);
        assert_eq!(
            submitted_branch(&mut modal).map(|choice| choice.branch),
            Some("fix-checkout-page".into())
        );
        modal.key(EditKey::Escape, false);
        assert!(matches!(modal.take_result(), Some(ModalResult::Cancelled)));
    }
}
