use ui::{div, label, theme, IconKind, LabelSize, Node};
use workspace::{
    checkbox, modal_button, modal_footer, modal_frame, modal_header, modal_section,
    outlined_button, status_line, EditKey, InputField, ModalResult, WindowModal, WINDOW_MODAL_BASE,
};

use crate::new_project::FolderChooser;

const WIDTH: f32 = 520.0;

const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const SOURCE_FIELD: u64 = WINDOW_MODAL_BASE + 2;
const ALIAS_FIELD: u64 = WINDOW_MODAL_BASE + 3;
const CHOOSE_FOLDER: u64 = WINDOW_MODAL_BASE + 4;
const USE_AI: u64 = WINDOW_MODAL_BASE + 5;
const CANCEL: u64 = WINDOW_MODAL_BASE + 6;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 7;
const WORKSPACE_BASE: u64 = WINDOW_MODAL_BASE + 100;
const WORKSPACE_LIMIT: u64 = 200;

/// What the form submits: where the repo comes from, its alias, the workspaces that get a worktree of it
/// (main always does), and whether Claude wires its env afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddRepo {
    pub source: String,
    pub alias: String,
    pub workspaces: Vec<String>,
    pub use_ai: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Source,
    Alias,
}

pub struct AddRepoModal {
    source: InputField,
    alias: InputField,
    focus: Focus,
    /// Every workspace but main, with whether it gets the repo.
    workspaces: Vec<(String, bool)>,
    existing: Vec<String>,
    choose_folder: FolderChooser,
    ai_available: bool,
    use_ai: bool,
    result: Option<ModalResult>,
}

impl AddRepoModal {
    /// `workspaces` are the branches besides main; `existing` the repos the config already has.
    pub fn new(
        workspaces: Vec<String>,
        existing: Vec<String>,
        choose_folder: FolderChooser,
        ai_available: bool,
    ) -> AddRepoModal {
        AddRepoModal {
            source: InputField::new("Git URL or folder", "git@github.com:acme/web.git").mono(),
            alias: InputField::new("Alias", "optional, e.g. fe").mono(),
            focus: Focus::Source,
            workspaces: workspaces
                .into_iter()
                .map(|branch| (branch, true))
                .collect(),
            existing,
            choose_folder,
            ai_available,
            use_ai: false,
            result: None,
        }
    }

    fn source_text(&self) -> String {
        self.source.text().trim().to_string()
    }

    fn source_error(&self) -> Option<String> {
        let source = self.source_text();
        if source.is_empty() {
            return None;
        }
        let name = pom_core::repo_name(&source);
        if name.is_empty() {
            return Some("cannot tell the repo name".into());
        }
        self.existing
            .contains(&name)
            .then(|| format!("{name} is already in the project"))
    }

    fn can_add(&self) -> bool {
        !self.source_text().is_empty() && self.source_error().is_none()
    }

    fn submit(&mut self) {
        if !self.can_add() {
            return;
        }
        self.result = Some(ModalResult::Submitted(Box::new(AddRepo {
            source: self.source_text(),
            alias: self.alias.text().trim().to_string(),
            workspaces: self
                .workspaces
                .iter()
                .filter(|(_, chosen)| *chosen)
                .map(|(branch, _)| branch.clone())
                .collect(),
            use_ai: self.use_ai,
        })));
    }

    fn focused(&mut self) -> &mut workspace::text_field::TextField {
        match self.focus {
            Focus::Source => &mut self.source.field,
            Focus::Alias => &mut self.alias.field,
        }
    }

    fn workspaces_section(&self) -> Node {
        let colors = theme();
        let mut list = div().col().gap(2.0);
        if self.workspaces.is_empty() {
            list = list.child(
                label("No other workspaces yet")
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted),
            );
        }
        for (index, (branch, chosen)) in self.workspaces.iter().enumerate() {
            list = list.child(checkbox(WORKSPACE_BASE + index as u64, *chosen, branch));
        }
        div()
            .col()
            .gap(6.0)
            .child(
                label("Also check it out in")
                    .label_size(LabelSize::Small)
                    .color(colors.text),
            )
            .child(list)
            .into()
    }
}

impl WindowModal for AddRepoModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let source_error = self.source_error();
        let name = pom_core::repo_name(&self.source_text());
        let hint = if name.is_empty() {
            "Cloned into main, its services detected".to_string()
        } else {
            format!("Cloned into main as {name}, its services detected")
        };
        let source_row = div()
            .row()
            .items_center()
            .gap(8.0)
            .child(div().col().flex(1.0).child(self.source.render(
                SOURCE_FIELD,
                self.focus == Focus::Source,
                Some(&hint),
                source_error.as_deref(),
            )))
            .child(outlined_button(
                CHOOSE_FOLDER,
                Some(IconKind::FolderOpen),
                "Folder...",
                true,
            ));
        let mut section = modal_section(10.0)
            .child(source_row)
            .child(
                self.alias
                    .render(ALIAS_FIELD, self.focus == Focus::Alias, None, None),
            )
            .child(self.workspaces_section());
        if self.ai_available {
            section = section.child(checkbox(
                USE_AI,
                self.use_ai,
                "Let Claude wire its env and shared services",
            ));
        } else {
            section = section.child(status_line(
                IconKind::File,
                colors.icon_muted,
                "Review its entry in the config afterwards",
            ));
        }
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(CONFIRM, "Add", Some("enter"), self.can_add()));
        modal_frame(WIDTH)
            .child(modal_header("Add Repository", Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            SOURCE_FIELD => self.focus = Focus::Source,
            ALIAS_FIELD => self.focus = Focus::Alias,
            CHOOSE_FOLDER => {
                if let Some(folder) = (self.choose_folder)().into_iter().next() {
                    self.source.field.set_text(&folder.to_string_lossy());
                    self.source.field.move_to_end();
                }
            }
            USE_AI => self.use_ai = !self.use_ai && self.ai_available,
            CONFIRM => self.submit(),
            id if (WORKSPACE_BASE..WORKSPACE_BASE + WORKSPACE_LIMIT).contains(&id) => {
                if let Some((_, chosen)) = self.workspaces.get_mut((id - WORKSPACE_BASE) as usize) {
                    *chosen = !*chosen;
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
                    Focus::Source => Focus::Alias,
                    Focus::Alias => Focus::Source,
                }
            }
            key => {
                self.focused().key(key, shift);
            }
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        self.focused().insert(&typed);
        true
    }

    fn copy(&self) -> Option<String> {
        match self.focus {
            Focus::Source => self.source.field.selected_text(),
            Focus::Alias => self.alias.field.selected_text(),
        }
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.focused().key(EditKey::Backspace, false);
        Some(text)
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

const CLONE_SOURCE_BASE: u64 = WINDOW_MODAL_BASE + 400;
const CLONE_FOLDER_BASE: u64 = WINDOW_MODAL_BASE + 600;

/// Where to clone each repo main lacks from: (repo, git URL or folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneRepos {
    pub sources: Vec<(String, String)>,
}

/// Asks where to clone the repos the config names but main does not have yet (a teammate added them, or the
/// config came from a bundle).
pub struct CloneReposModal {
    rows: Vec<(String, InputField)>,
    focus: usize,
    choose_folder: FolderChooser,
    result: Option<ModalResult>,
}

impl CloneReposModal {
    /// `repos` are (name, a guessed URL or empty).
    pub fn new(repos: Vec<(String, String)>, choose_folder: FolderChooser) -> CloneReposModal {
        let rows = repos
            .into_iter()
            .map(|(name, guess)| {
                let mut field = InputField::new("Clone from", "git URL or folder").mono();
                field.field.set_text(&guess);
                field.field.move_to_end();
                (name, field)
            })
            .collect();
        CloneReposModal {
            rows,
            focus: 0,
            choose_folder,
            result: None,
        }
    }

    fn sources(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .map(|(name, field)| (name.clone(), field.text().trim().to_string()))
            .filter(|(_, source)| !source.is_empty())
            .collect()
    }

    fn submit(&mut self) {
        let sources = self.sources();
        if sources.is_empty() {
            return;
        }
        self.result = Some(ModalResult::Submitted(Box::new(CloneRepos { sources })));
    }

    fn focused(&mut self) -> Option<&mut workspace::text_field::TextField> {
        self.rows
            .get_mut(self.focus)
            .map(|(_, field)| &mut field.field)
    }
}

impl WindowModal for CloneReposModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let mut section = modal_section(10.0).child(
            label("These repos are in the config but not cloned into main yet.")
                .label_size(LabelSize::Small)
                .color(theme().text_muted),
        );
        for (index, (name, field)) in self.rows.iter().enumerate() {
            section = section.child(
                label(name.clone())
                    .label_size(LabelSize::Small)
                    .color(theme().text),
            );
            section = section.child(
                div()
                    .row()
                    .items_center()
                    .gap(8.0)
                    .child(div().col().flex(1.0).child(field.render(
                        CLONE_SOURCE_BASE + index as u64,
                        self.focus == index,
                        None,
                        None,
                    )))
                    .child(outlined_button(
                        CLONE_FOLDER_BASE + index as u64,
                        Some(IconKind::FolderOpen),
                        "Folder...",
                        true,
                    )),
            );
        }
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Clone",
                Some("enter"),
                !self.sources().is_empty(),
            ));
        modal_frame(WIDTH)
            .child(modal_header("Clone Missing Repos", Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        let count = self.rows.len() as u64;
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CONFIRM => self.submit(),
            id if (CLONE_SOURCE_BASE..CLONE_SOURCE_BASE + count).contains(&id) => {
                self.focus = (id - CLONE_SOURCE_BASE) as usize;
            }
            id if (CLONE_FOLDER_BASE..CLONE_FOLDER_BASE + count).contains(&id) => {
                let index = (id - CLONE_FOLDER_BASE) as usize;
                if let Some(folder) = (self.choose_folder)().into_iter().next() {
                    if let Some((_, field)) = self.rows.get_mut(index) {
                        field.field.set_text(&folder.to_string_lossy());
                        field.field.move_to_end();
                    }
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            EditKey::Tab | EditKey::Backtab if !self.rows.is_empty() => {
                let count = self.rows.len();
                self.focus = if key == EditKey::Backtab || shift {
                    (self.focus + count - 1) % count
                } else {
                    (self.focus + 1) % count
                };
            }
            key => {
                if let Some(field) = self.focused() {
                    field.key(key, shift);
                }
            }
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.focused() {
            Some(field) if !typed.is_empty() => {
                field.insert(&typed);
                true
            }
            _ => false,
        }
    }

    fn copy(&self) -> Option<String> {
        self.rows.get(self.focus)?.1.field.selected_text()
    }

    fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.focused()?.key(EditKey::Backspace, false);
        Some(text)
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn submitted(modal: &mut AddRepoModal) -> Option<AddRepo> {
        match modal.take_result() {
            Some(ModalResult::Submitted(value)) => value.downcast_ref::<AddRepo>().cloned(),
            _ => None,
        }
    }

    #[test]
    fn clones_only_the_repos_given_a_source() {
        let mut form = CloneReposModal::new(
            vec![
                ("web".into(), "git@github.com:acme/web.git".into()),
                ("jobs".into(), String::new()),
            ],
            Box::new(Vec::new),
        );
        form.key(EditKey::Enter, false);
        match form.take_result() {
            Some(ModalResult::Submitted(value)) => assert_eq!(
                value
                    .downcast_ref::<CloneRepos>()
                    .map(|clone| clone.sources.clone()),
                Some(vec![("web".into(), "git@github.com:acme/web.git".into())])
            ),
            _ => panic!("submitted"),
        }
    }

    #[test]
    fn adds_to_every_workspace_unless_unchecked_and_refuses_a_repo_already_there() {
        let mut form = AddRepoModal::new(
            vec!["feat-a".into(), "feat-b".into()],
            vec!["api".into()],
            Box::new(|| vec![PathBuf::from("/src/api")]),
            true,
        );
        form.click(CHOOSE_FOLDER);
        assert!(form
            .source_error()
            .is_some_and(|error| error.contains("already")));
        form.key(EditKey::Enter, false);
        assert!(submitted(&mut form).is_none());

        form.source.field.set_text("git@github.com:acme/web.git");
        form.click(WORKSPACE_BASE + 1);
        form.click(USE_AI);
        form.key(EditKey::Tab, false);
        form.text("fe");
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted(&mut form),
            Some(AddRepo {
                source: "git@github.com:acme/web.git".into(),
                alias: "fe".into(),
                workspaces: vec!["feat-a".into()],
                use_ai: true,
            })
        );
    }
}
