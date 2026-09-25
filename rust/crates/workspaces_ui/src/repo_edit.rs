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
}
