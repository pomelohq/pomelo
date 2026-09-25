use std::path::{Path, PathBuf};

use ui::{div, icon, label, theme, IconKind, LabelSize, Node};
use workspace::text_field::{FieldFont, TextField};
use workspace::{
    modal_button, modal_footer, modal_frame, modal_header, modal_section, outlined_button,
    status_line, toggle_button_group, EditKey, InputField, ModalResult, WindowModal,
    WINDOW_MODAL_BASE,
};

const WIDTH: f32 = 544.0;
const ALIAS_WIDTH: f32 = 112.0;
const ALIAS_HEIGHT: f32 = 20.0;

const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const NAME_FIELD: u64 = WINDOW_MODAL_BASE + 2;
const BRANCH_FIELD: u64 = WINDOW_MODAL_BASE + 3;
const URL_FIELD: u64 = WINDOW_MODAL_BASE + 4;
const ADD_FOLDER: u64 = WINDOW_MODAL_BASE + 6;
const CANCEL: u64 = WINDOW_MODAL_BASE + 7;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 8;
const SETUP_AI: u64 = WINDOW_MODAL_BASE + 9;
const SETUP_MANUAL: u64 = WINDOW_MODAL_BASE + 10;
const ALIAS_BASE: u64 = WINDOW_MODAL_BASE + 200;
const REMOVE_BASE: u64 = WINDOW_MODAL_BASE + 300;
const ROW_LIMIT: u64 = 100;

/// What the new project form submits: `repos` are (local folder or git URL, alias).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub name: String,
    pub default_branch: String,
    pub repos: Vec<(String, String)>,
    /// Hand the drafted `pom.yml` to the coding agent to finish; otherwise the user reviews it.
    pub use_ai: bool,
}

/// Opens the system folder picker; returns the folders chosen.
pub type FolderChooser = Box<dyn Fn() -> Vec<PathBuf>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Name,
    Branch,
    Url,
    Alias(usize),
}

struct RepoRow {
    source: String,
    alias: TextField,
}

pub struct NewProjectModal {
    name: InputField,
    branch: InputField,
    url: InputField,
    repos: Vec<RepoRow>,
    focus: Focus,
    sessions_root: PathBuf,
    choose_folders: FolderChooser,
    ai_available: bool,
    use_ai: bool,
    result: Option<ModalResult>,
}

impl NewProjectModal {
    /// `sessions_root` is where the session folder will be created.
    pub fn new(sessions_root: PathBuf, choose_folders: FolderChooser) -> NewProjectModal {
        let mut branch = InputField::new("Default branch", "main").mono();
        branch.field.set_text("main");
        branch.field.move_to_end();
        NewProjectModal {
            name: InputField::new("Session name", "myproject"),
            branch,
            url: InputField::new("Git URL", "git@github.com:acme/api.git").mono(),
            repos: Vec::new(),
            focus: Focus::Name,
            sessions_root,
            choose_folders,
            ai_available: false,
            use_ai: false,
            result: None,
        }
    }

    /// Whether the agent CLI is installed, and whether the user last chose to set up with it.
    pub fn with_ai(mut self, available: bool, preferred: bool) -> NewProjectModal {
        self.ai_available = available;
        self.use_ai = available && preferred;
        self
    }

    pub fn use_ai(&self) -> bool {
        self.use_ai
    }

    fn name_text(&self) -> String {
        self.name.text().trim().to_string()
    }

    fn name_error(&self) -> Option<String> {
        let name = self.name_text();
        if name.is_empty() {
            return None;
        }
        if name.contains(['/', '\\']) || name.contains("..") {
            return Some("no slashes or '..'".into());
        }
        self.sessions_root.join(&name).exists().then(|| {
            format!(
                "{} already exists",
                self.sessions_root.join(&name).display()
            )
        })
    }

    fn branch_error(&self) -> Option<String> {
        let branch = self.branch.text().trim().to_string();
        if branch.is_empty() {
            return None;
        }
        pom_workspace::validate_branch_name(&branch).err()
    }

    fn can_create(&self) -> bool {
        !self.name_text().is_empty()
            && self.name_error().is_none()
            && self.branch_error().is_none()
            && !self.repos.is_empty()
    }

    fn add_repo(&mut self, source: String) {
        if source.is_empty() || self.repos.iter().any(|repo| repo.source == source) {
            return;
        }
        if self.repos.len() as u64 >= ROW_LIMIT {
            return;
        }
        self.repos.push(RepoRow {
            source,
            alias: TextField::default(),
        });
    }

    fn add_url(&mut self) {
        let url = self.url.text().trim().to_string();
        if url.is_empty() {
            return;
        }
        self.add_repo(url);
        self.url.field.set_text("");
    }

    fn add_folders(&mut self) {
        for folder in (self.choose_folders)() {
            self.add_repo(folder.to_string_lossy().into_owned());
        }
    }

    fn remove(&mut self, index: usize) {
        if index >= self.repos.len() {
            return;
        }
        self.repos.remove(index);
        self.focus = match self.focus {
            Focus::Alias(at) if at == index => Focus::Name,
            Focus::Alias(at) if at > index => Focus::Alias(at - 1),
            focus => focus,
        };
    }

    fn submit(&mut self) {
        if !self.can_create() {
            return;
        }
        self.result = Some(ModalResult::Submitted(Box::new(NewProject {
            name: self.name_text(),
            default_branch: self.branch.text().trim().to_string(),
            repos: self
                .repos
                .iter()
                .map(|repo| (repo.source.clone(), repo.alias.text().trim().to_string()))
                .collect(),
            use_ai: self.use_ai,
        })));
    }

    fn cycle_focus(&mut self, back: bool) {
        let mut order = vec![Focus::Name, Focus::Branch];
        order.extend((0..self.repos.len()).map(Focus::Alias));
        order.push(Focus::Url);
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

    fn focused(&mut self) -> &mut TextField {
        match self.focus {
            Focus::Name => &mut self.name.field,
            Focus::Branch => &mut self.branch.field,
            Focus::Url => &mut self.url.field,
            Focus::Alias(index) => match self.repos.get_mut(index) {
                Some(repo) => &mut repo.alias,
                None => &mut self.name.field,
            },
        }
    }

    fn repo_row(&self, index: usize, repo: &RepoRow) -> Node {
        let colors = theme();
        let remote = !Path::new(&repo.source).is_absolute();
        let focused = self.focus == Focus::Alias(index);
        let alias = div()
            .row()
            .items_center()
            .w_px(ALIAS_WIDTH)
            .h_px(ALIAS_HEIGHT + 6.0)
            .px(6.0)
            .rounded(4.0)
            .bg(colors.editor_background)
            .border(
                1.0,
                if focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                },
            )
            .on_click(ALIAS_BASE + index as u64)
            .child(
                repo.alias
                    .render("alias", focused, colors.text, ALIAS_HEIGHT, FieldFont::Mono),
            );
        let remove = div()
            .row()
            .items_center()
            .justify_center()
            .w_px(20.0)
            .h_px(20.0)
            .rounded(4.0)
            .on_click(REMOVE_BASE + index as u64)
            .child(icon(IconKind::Close).size(12.0).color(colors.icon_muted));
        div()
            .row()
            .items_center()
            .gap(8.0)
            .child(
                icon(if remote {
                    IconKind::ArrowUpRight
                } else {
                    IconKind::Folder
                })
                .size(14.0)
                .color(colors.icon_muted),
            )
            .child(
                div().row().items_center().flex(1.0).child(
                    label(repo.source.clone())
                        .label_size(LabelSize::Small)
                        .mono()
                        .color(colors.text),
                ),
            )
            .child(alias)
            .child(remove)
            .into()
    }

    fn setup_section(&self) -> Node {
        let colors = theme();
        let choices = [
            (SETUP_AI, Some(IconKind::Sparkle), "Set up with AI"),
            (SETUP_MANUAL, Some(IconKind::File), "Set up manually"),
        ];
        let hint = if !self.ai_available {
            status_line(
                IconKind::Warning,
                colors.warning,
                "Install Claude Code to set up with AI",
            )
        } else if self.use_ai {
            status_line(
                IconKind::Sparkle,
                colors.icon_muted,
                "Claude opens in a terminal to finish pom.yml; you approve each step",
            )
        } else {
            status_line(
                IconKind::File,
                colors.icon_muted,
                "pom.yml is drafted from what is detected; you review and edit it",
            )
        };
        div()
            .col()
            .gap(6.0)
            .child(
                label("Setup")
                    .label_size(LabelSize::Small)
                    .color(colors.text),
            )
            .child(toggle_button_group(&choices, usize::from(!self.use_ai)))
            .child(hint)
            .into()
    }

    fn repos_section(&self) -> Node {
        let colors = theme();
        let mut list = div().col().gap(4.0);
        if self.repos.is_empty() {
            list = list.child(
                label("Add at least one repository")
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted),
            );
        }
        for (index, repo) in self.repos.iter().enumerate() {
            list = list.child(self.repo_row(index, repo));
        }
        let url_row = self.url.render(
            URL_FIELD,
            self.focus == Focus::Url,
            Some("enter to add"),
            None,
        );
        div()
            .col()
            .gap(8.0)
            .child(
                div()
                    .row()
                    .items_center()
                    .justify_between()
                    .child(
                        label("Repositories")
                            .label_size(LabelSize::Small)
                            .color(colors.text),
                    )
                    .child(outlined_button(
                        ADD_FOLDER,
                        Some(IconKind::FolderOpen),
                        "Add folder...",
                        true,
                    )),
            )
            .child(list)
            .child(url_row)
            .into()
    }
}

impl WindowModal for NewProjectModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let name_error = self.name_error();
        let branch_error = self.branch_error();
        let location = match self.name_text() {
            name if name.is_empty() => self.sessions_root.display().to_string(),
            name => self.sessions_root.join(name).display().to_string(),
        };
        let section = modal_section(10.0)
            .child(self.name.render(
                NAME_FIELD,
                self.focus == Focus::Name,
                Some(&location),
                name_error.as_deref(),
            ))
            .child(self.branch.render(
                BRANCH_FIELD,
                self.focus == Focus::Branch,
                None,
                branch_error.as_deref(),
            ))
            .child(self.repos_section())
            .child(self.setup_section());
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
            .child(modal_header("New Project", Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            NAME_FIELD => self.focus = Focus::Name,
            BRANCH_FIELD => self.focus = Focus::Branch,
            URL_FIELD => self.focus = Focus::Url,
            ADD_FOLDER => self.add_folders(),
            SETUP_AI => self.use_ai = self.ai_available,
            SETUP_MANUAL => self.use_ai = false,
            CONFIRM => self.submit(),
            id if (ALIAS_BASE..ALIAS_BASE + ROW_LIMIT).contains(&id) => {
                let index = (id - ALIAS_BASE) as usize;
                if index < self.repos.len() {
                    self.focus = Focus::Alias(index);
                }
            }
            id if (REMOVE_BASE..REMOVE_BASE + ROW_LIMIT).contains(&id) => {
                self.remove((id - REMOVE_BASE) as usize)
            }
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter if self.focus == Focus::Url && !self.url.text().trim().is_empty() => {
                self.add_url()
            }
            EditKey::Enter => self.submit(),
            EditKey::Tab | EditKey::Backtab => self.cycle_focus(key == EditKey::Backtab || shift),
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
            Focus::Name => self.name.field.selected_text(),
            Focus::Branch => self.branch.field.selected_text(),
            Focus::Url => self.url.field.selected_text(),
            Focus::Alias(index) => self.repos.get(index)?.alias.selected_text(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn modal(root: &Path, folders: Vec<PathBuf>) -> NewProjectModal {
        NewProjectModal::new(root.to_path_buf(), Box::new(move || folders.clone()))
    }

    fn submitted(modal: &mut NewProjectModal) -> Option<NewProject> {
        match modal.take_result() {
            Some(ModalResult::Submitted(value)) => value.downcast_ref::<NewProject>().cloned(),
            _ => None,
        }
    }

    #[test]
    fn needs_a_name_and_a_repo_then_submits_sources_with_aliases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut form = modal(temp.path(), vec![PathBuf::from("/src/api")]);
        form.text("demo");
        form.key(EditKey::Enter, false);
        assert!(submitted(&mut form).is_none());

        form.click(ADD_FOLDER);
        form.click(ADD_FOLDER);
        form.click(URL_FIELD);
        form.text("git@github.com:acme/web.git");
        form.key(EditKey::Enter, false);
        assert_eq!(form.repos.len(), 2);
        assert!(form.url.text().is_empty());

        form.click(ALIAS_BASE + 1);
        form.text("fe");
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted(&mut form),
            Some(NewProject {
                name: "demo".into(),
                default_branch: "main".into(),
                repos: vec![
                    ("/src/api".into(), String::new()),
                    ("git@github.com:acme/web.git".into(), "fe".into()),
                ],
                use_ai: false,
            })
        );
    }

    #[test]
    fn setup_with_ai_is_offered_only_when_the_agent_is_installed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut form = modal(temp.path(), Vec::new()).with_ai(false, true);
        assert!(!form.use_ai());
        form.click(SETUP_AI);
        assert!(!form.use_ai(), "no agent installed, stays manual");

        let mut form = modal(temp.path(), vec![PathBuf::from("/src/api")]).with_ai(true, true);
        assert!(form.use_ai());
        form.click(SETUP_MANUAL);
        form.click(ADD_FOLDER);
        form.text("demo");
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted(&mut form).map(|project| project.use_ai),
            Some(false)
        );
    }

    #[test]
    fn rejects_bad_or_taken_names_and_removes_rows() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("taken")).ok();
        let mut form = modal(temp.path(), vec![PathBuf::from("/src/api")]);
        form.click(ADD_FOLDER);
        form.text("taken");
        assert!(form
            .name_error()
            .is_some_and(|error| error.contains("already exists")));
        form.name.field.set_text("a/b");
        assert!(form.name_error().is_some());
        form.name.field.set_text("fresh");
        assert!(form.can_create());
        form.click(REMOVE_BASE);
        assert!(!form.can_create());
    }
}
