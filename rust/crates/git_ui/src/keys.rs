use git::working_copy::Staging;
use workspace::keymap::Action;
use workspace::list_nav::{self, NavMove};
use workspace::EditKey;

use crate::rows::Row;
use crate::{Control, FileEntry, GitPanel, MenuAction, Scope, Tab};

impl GitPanel {
    /// The key a row folds under, for the rows that fold.
    pub(crate) fn fold_key(&self, row: &Row) -> Option<String> {
        let name = |repo: usize| {
            self.sources
                .get(repo)
                .map(|source| source.name.clone())
                .unwrap_or_default()
        };
        Some(match row {
            Row::RepoHeader { repo, history } => {
                format!("{}:{}", if *history { "h" } else { "c" }, name(*repo))
            }
            Row::Section { repo, staged, .. } => format!("s:{}:{staged}", name(*repo)),
            Row::Directory {
                repo, scope, path, ..
            } => format!("d:{}:{}:{path}", scope.key(), name(*repo)),
            Row::Card { repo } => format!("r:{}", name(*repo)),
            Row::CommitGroup { repo, incoming, .. } => {
                format!("{}:{}", if *incoming { "in" } else { "out" }, name(*repo))
            }
            Row::FilesHeader { repo, .. } => format!("rf:{}", name(*repo)),
            _ => return None,
        })
    }

    fn selectable(row: &Row) -> bool {
        !matches!(
            row,
            Row::Message { .. }
                | Row::Progress { .. }
                | Row::Gap
                | Row::CardNote { .. }
                | Row::Day(_)
                | Row::CommitHeader
        )
    }

    /// The row the keyboard is on, while the list has the keyboard.
    pub(crate) fn keyboard_row(&self) -> Option<usize> {
        self.keyboard.then_some(self.nav.selected).flatten()
    }

    pub(crate) fn select_row(&mut self, index: usize) {
        if self.rows.get(index).is_some_and(Self::selectable) {
            self.nav.selected = Some(index);
        }
    }

    /// Keeps the selection on a pickable row after the rows were rebuilt.
    pub(crate) fn settle_selection(&mut self) {
        self.nav.clamp(self.rows.len());
        let Some(at) = self.nav.selected else {
            return;
        };
        if self.rows.get(at).is_some_and(Self::selectable) {
            return;
        }
        let after = self
            .rows
            .iter()
            .enumerate()
            .skip(at)
            .find(|(_, row)| Self::selectable(row));
        let before = self
            .rows
            .iter()
            .enumerate()
            .take(at)
            .rev()
            .find(|(_, row)| Self::selectable(row));
        self.nav.selected = after.or(before).map(|(index, _)| index);
    }

    fn move_selection(&mut self, movement: NavMove) {
        let rows = &self.rows;
        self.nav.apply(movement, rows.len(), false, |index| {
            rows.get(index).is_some_and(Self::selectable)
        });
        self.scroll_to_selected();
    }

    fn scroll_to_selected(&mut self) {
        let Some(index) = self.nav.selected else {
            return;
        };
        let top: f32 = self.rows.iter().take(index).map(Row::height).sum();
        let height = self.rows.get(index).map_or(0.0, Row::height);
        if top < self.scroll {
            self.scroll = top;
        } else if top + height > self.scroll + self.viewport_h {
            self.scroll = top + height - self.viewport_h;
        }
    }

    pub(crate) fn nav_key(&mut self, key: EditKey, shift: bool) -> bool {
        let Some(movement) = list_nav::nav_move(key, shift) else {
            return false;
        };
        self.move_selection(movement);
        true
    }

    pub(crate) fn take_keyboard(&mut self, on: bool) {
        self.keyboard = on;
        if on {
            self.commit.focused = false;
            if self.nav.selected.is_none() {
                self.move_selection(NavMove::First);
            }
        }
    }

    fn selected_file(&self) -> Option<(usize, Scope, FileEntry)> {
        match self.rows.get(self.nav.selected?)? {
            Row::File {
                repo, scope, entry, ..
            } => Some((*repo, *scope, entry.clone())),
            _ => None,
        }
    }

    /// Whether the selected row is staged, for the rows whose checkbox stages.
    fn selected_staged(&self) -> Option<bool> {
        let by_scope = |scope: Scope, staging: Option<Staging>| match scope {
            Scope::Staged => Some(true),
            Scope::Unstaged => Some(false),
            Scope::Commit => None,
            Scope::Working | Scope::Branch => staging.map(|staging| staging == Staging::Staged),
        };
        match self.rows.get(self.nav.selected?)? {
            Row::File { scope, entry, .. } => by_scope(*scope, entry.staging),
            Row::Directory { scope, staging, .. } => by_scope(*scope, *staging),
            Row::Section { staged, .. } => Some(*staged),
            Row::RepoHeader {
                repo,
                history: false,
            } => {
                let scan = self.current();
                let status = &scan.repos.get(*repo)?.status;
                (!status.is_empty()).then(|| {
                    status
                        .iter()
                        .all(|entry| entry.staging() == Staging::Staged)
                })
            }
            _ => None,
        }
    }

    /// Stages (`Some(true)`), unstages (`Some(false)`) or flips (`None`) the selected row.
    fn stage_selected(&mut self, stage: Option<bool>) -> bool {
        let (Some(index), Some(staged)) = (self.nav.selected, self.selected_staged()) else {
            return false;
        };
        if stage != Some(staged) {
            self.click_row(index, Control::Check);
        }
        true
    }

    fn file_action(&mut self, action: MenuAction) -> bool {
        let Some((repo, scope, entry)) = self.selected_file() else {
            return false;
        };
        if action == MenuAction::Discard {
            let uncommitted = matches!(scope, Scope::Staged | Scope::Unstaged | Scope::Working)
                || (scope == Scope::Branch
                    && self
                        .current()
                        .repos
                        .get(repo)
                        .and_then(|state| state.uncommitted_file(&entry.path))
                        .is_some());
            if !uncommitted {
                return false;
            }
        }
        self.apply_file_action(repo, scope, &entry, action);
        true
    }

    /// Folds the selected row, or from a row that does not fold open, goes to the row it sits under.
    fn collapse_selected(&mut self) -> bool {
        let Some(index) = self.nav.selected else {
            return false;
        };
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        if let Some(key) = self.fold_key(row) {
            if !self.is_folded(&key) {
                self.toggle_fold(key);
                return true;
            }
        }
        let depth = match row {
            Row::File { depth, .. } | Row::Directory { depth, .. } => *depth,
            _ => return false,
        };
        let parent = (0..index).rev().find(|above| match self.rows.get(*above) {
            Some(Row::Directory { depth: parent, .. }) => *parent < depth,
            Some(row) => self.fold_key(row).is_some(),
            None => false,
        });
        if let Some(parent) = parent {
            self.nav.selected = Some(parent);
            self.scroll_to_selected();
        }
        true
    }

    /// Unfolds the selected row, or steps into it when it is open already.
    fn expand_selected(&mut self) -> bool {
        let Some(key) = self
            .nav
            .selected
            .and_then(|index| self.rows.get(index))
            .and_then(|row| self.fold_key(row))
        else {
            return false;
        };
        if self.is_folded(&key) {
            self.toggle_fold(key);
        } else {
            self.move_selection(NavMove::Next);
        }
        true
    }

    pub(crate) fn run_key_action(&mut self, action: Action) -> bool {
        match action {
            Action::GitOpenEntry => {
                let Some(index) = self.nav.selected else {
                    return false;
                };
                self.click_row(index, Control::Main);
            }
            Action::GitToggleStaged => return self.stage_selected(None),
            Action::GitStageFile => return self.stage_selected(Some(true)),
            Action::GitUnstageFile => return self.stage_selected(Some(false)),
            Action::GitStageAll => self.stage_everything(true),
            Action::GitUnstageAll => self.stage_everything(false),
            Action::GitRestoreFile => return self.file_action(MenuAction::Discard),
            Action::GitCopyPath => return self.file_action(MenuAction::CopyPath),
            Action::GitCopyRelativePath => return self.file_action(MenuAction::CopyRelativePath),
            Action::GitFocusCommitEditor => {
                if self.tab != Tab::Changes || self.opened.is_some() {
                    return false;
                }
                self.keyboard = false;
                self.commit.focused = true;
            }
            Action::GitCollapse => return self.collapse_selected(),
            Action::GitExpand => return self.expand_selected(),
            Action::GitFetch => self.fetch_all(),
            Action::GitPush => self.sync_all(false, true),
            Action::GitPull => self.sync_all(true, false),
            Action::GitChangesTab => self.set_tab(Tab::Changes),
            Action::GitRemoteTab => self.set_tab(Tab::Remote),
            Action::GitHistoryTab => self.set_tab(Tab::History),
            _ => return false,
        }
        true
    }
}
