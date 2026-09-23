use std::path::{Path, PathBuf};

use workspace::{EditKey, FunctionView, Prompt, TreeAction, TreeMenuState};

use crate::text_field::TextField;
use crate::{FileItem, FilesView, Row, ROW_H};

const TREE_FONT: f32 = 13.0;

#[derive(Default)]
pub(crate) struct TreeOps {
    pub(crate) edit: Option<TreeEdit>,
    clipboard: Option<TreeClipboard>,
    prompt: Option<(u64, TreePrompt)>,
    next_token: u64,
    toast: Option<String>,
}

pub(crate) struct TreeEdit {
    pub(crate) target: EditTarget,
    pub(crate) field: TextField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditTarget {
    New { parent: String, is_dir: bool },
    Rename { path: String, is_dir: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreeClipboard {
    Cut(Vec<String>),
    Copied(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreePrompt {
    Remove { path: String, trash: bool },
    Restore { path: String },
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn is_within(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn stem_len(name: &str) -> usize {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].chars().count(),
        _ => name.chars().count(),
    }
}

/// Where pasting `source` onto `target` lands, and the chars of its new name worth selecting for a rename when
/// the name had to be disambiguated.
pub(crate) fn paste_destination(
    root: &Path,
    source: &str,
    target: &str,
    target_is_dir: bool,
) -> (String, Option<usize>) {
    let parent = if !target_is_dir || target == source {
        parent_of(target)
    } else {
        target
    };
    let name = file_name(source);
    let source_is_dir = root.join(source).is_dir();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && !source_is_dir => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    };
    let mut destination = join(parent, name);
    let mut selection = None;
    let mut attempt = 0;
    while files::exists(root, &destination) {
        let mut candidate = format!("{stem} copy");
        if attempt > 0 {
            candidate.push_str(&format!(" {attempt}"));
        }
        selection = Some(candidate.chars().count());
        if let Some(extension) = extension {
            candidate.push('.');
            candidate.push_str(extension);
        }
        destination = join(parent, &candidate);
        attempt += 1;
    }
    (destination, selection)
}

pub(crate) fn removal_prompt(
    name: &str,
    trash: bool,
    dirty: bool,
) -> (String, Option<String>, String) {
    let (start, confirm, detail) = if trash {
        ("Do you want to trash", "Trash", None)
    } else {
        (
            "Are you sure you want to permanently delete",
            "Delete",
            Some("This cannot be undone.".to_string()),
        )
    };
    let mut message = format!("{start} `{name}`?");
    if dirty {
        message.push_str("\n\nIt has unsaved changes, which will be lost.");
    }
    (message, detail, confirm.to_string())
}

/// Insert the pending new-entry row under its parent (folders first, then files, like the sort), or mark the
/// row being renamed.
pub(crate) fn place_edit_row(rows: &mut Vec<Row>, target: &EditTarget) {
    match target {
        EditTarget::Rename { path, .. } => {
            if let Some(row) = rows.iter_mut().find(|row| &row.path == path) {
                row.edit = true;
            }
        }
        EditTarget::New { parent, is_dir } => {
            let (start, depth) = if parent.is_empty() {
                (0, 0)
            } else {
                match rows.iter().position(|row| &row.path == parent) {
                    Some(index) => (index + 1, rows[index].depth + 1),
                    None => return,
                }
            };
            let mut at = start;
            if !is_dir {
                while let Some(row) = rows.get(at) {
                    let is_child_file = row.depth == depth && !row.is_dir;
                    if row.depth < depth || is_child_file {
                        break;
                    }
                    at += 1;
                }
            }
            rows.insert(
                at,
                Row {
                    name: String::new(),
                    path: join(parent, ""),
                    is_dir: *is_dir,
                    depth,
                    open: false,
                    edit: true,
                },
            );
        }
    }
}

fn git_work_dir(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn has_restorable_changes(root: &Path, path: &str) -> bool {
    git_output(root, &["status", "--porcelain", "--", path]).is_some_and(|status| {
        status.lines().any(|line| {
            let code = line.get(..2).unwrap_or("");
            code != "??" && (code.contains('M') || code.contains('D'))
        })
    })
}

impl FilesView {
    pub(crate) fn menu_state(&self, path: Option<&str>) -> TreeMenuState {
        let is_dir = path.is_none_or(|path| self.root.join(path).is_dir());
        let has_git_repo = git_work_dir(&self.root).is_some();
        TreeMenuState {
            is_dir,
            is_root: path.is_none(),
            has_git_repo,
            has_git_changes: has_git_repo
                && !is_dir
                && path.is_some_and(|path| has_restorable_changes(&self.root, path)),
            can_paste: self.tree_ops.clipboard.is_some(),
        }
    }

    pub(crate) fn run_tree_action(
        &mut self,
        path: Option<&str>,
        action: TreeAction,
    ) -> Option<Prompt> {
        let target = path.unwrap_or("").to_string();
        let target_is_dir = path.is_none_or(|path| self.root.join(path).is_dir());
        match action {
            TreeAction::NewFile | TreeAction::NewDirectory => {
                let parent = if target_is_dir {
                    target
                } else {
                    parent_of(&target).to_string()
                };
                self.start_new_entry(parent, action == TreeAction::NewDirectory);
            }
            TreeAction::Rename => {
                if path.is_some() {
                    self.start_rename(&target, None);
                }
            }
            TreeAction::OpenWithSystem => {
                if let Err(error) = std::process::Command::new("open")
                    .arg(self.root.join(&target))
                    .spawn()
                {
                    self.tree_ops.toast = Some(format!("Failed to open {target}: {error}"));
                }
            }
            TreeAction::Cut => {
                if path.is_some() {
                    self.tree_ops.clipboard = Some(TreeClipboard::Cut(vec![target]));
                }
            }
            TreeAction::Copy => {
                if path.is_some() {
                    self.tree_ops.clipboard = Some(TreeClipboard::Copied(vec![target]));
                }
            }
            TreeAction::Duplicate => {
                if path.is_some() {
                    self.tree_ops.clipboard = Some(TreeClipboard::Copied(vec![target.clone()]));
                    self.paste_onto(&target, target_is_dir);
                }
            }
            TreeAction::Paste => self.paste_onto(&target, target_is_dir),
            TreeAction::AddToGitignore => self.add_to_gitignore(&target, target_is_dir),
            TreeAction::RestoreFile => {
                if path.is_some() && !target_is_dir {
                    let message = format!("Discard changes to `{}`?", file_name(&target));
                    return Some(self.ask(
                        TreePrompt::Restore { path: target },
                        message,
                        None,
                        "Restore",
                    ));
                }
            }
            TreeAction::Trash | TreeAction::Delete => {
                if path.is_some() {
                    let trash = action == TreeAction::Trash;
                    let dirty = self.has_dirty_items_within(&target);
                    let (message, detail, confirm) =
                        removal_prompt(file_name(&target), trash, dirty);
                    return Some(self.ask(
                        TreePrompt::Remove {
                            path: target,
                            trash,
                        },
                        message,
                        detail,
                        &confirm,
                    ));
                }
            }
            TreeAction::ExpandAll => self.set_expanded_within(path, true),
            TreeAction::CollapseAll => self.set_expanded_within(path, false),
        }
        None
    }

    fn ask(
        &mut self,
        prompt: TreePrompt,
        message: String,
        detail: Option<String>,
        confirm: &str,
    ) -> Prompt {
        self.tree_ops.next_token += 1;
        let token = self.tree_ops.next_token;
        self.tree_ops.prompt = Some((token, prompt));
        Prompt {
            token,
            message,
            detail,
            buttons: vec![confirm.to_string(), "Cancel".to_string()],
        }
    }

    pub(crate) fn tree_prompt_answered(&mut self, token: u64, answer: usize) {
        let Some((pending, prompt)) = self.tree_ops.prompt.take() else {
            return;
        };
        if pending != token || answer != 0 {
            return;
        }
        match prompt {
            TreePrompt::Remove { path, trash } => {
                let result = if trash {
                    files::trash(&self.root, &path)
                } else {
                    files::remove(&self.root, &path)
                };
                if let Err(error) = result {
                    let verb = if trash { "trash" } else { "delete" };
                    self.tree_ops.toast = Some(format!("Failed to {verb} {path}: {error}"));
                }
                self.expanded.retain(|dir| !is_within(dir, &path));
                self.reload_tree();
            }
            TreePrompt::Restore { path } => {
                if git_output(&self.root, &["checkout", "HEAD", "--", &path]).is_none() {
                    self.tree_ops.toast = Some(format!("Failed to restore {}", file_name(&path)));
                }
                FunctionView::refresh_disk_state(self);
            }
        }
    }

    pub(crate) fn take_tree_toast(&mut self) -> Option<String> {
        self.tree_ops.toast.take()
    }

    fn has_dirty_items_within(&self, path: &str) -> bool {
        let mut dirty = false;
        self.panes.group.for_each_pane(&mut |pane| {
            dirty |= pane
                .open
                .iter()
                .any(|item| item.is_dirty() && item.id().is_some_and(|id| is_within(&id, path)));
        });
        dirty
    }

    fn set_expanded_within(&mut self, path: Option<&str>, expand: bool) {
        let scope = path.unwrap_or("");
        let within = |dir: &str| scope.is_empty() || is_within(dir, scope);
        if expand {
            for entry in files::list(&self.root) {
                if entry.is_dir && within(&entry.path) {
                    self.expanded.insert(entry.path);
                }
            }
        } else {
            self.expanded.retain(|dir| !within(dir));
        }
        self.flat_dirty = true;
    }

    fn add_to_gitignore(&mut self, path: &str, is_dir: bool) {
        let Some(work_dir) = git_work_dir(&self.root) else {
            return;
        };
        let full = self.root.join(path);
        let Ok(relative) = full.strip_prefix(&work_dir) else {
            return;
        };
        let mut pattern = relative.to_string_lossy().replace('\\', "/");
        if pattern.is_empty() {
            return;
        }
        if is_dir {
            pattern.push('/');
        }
        if let Err(error) = files::append_to_gitignore(&work_dir, &pattern) {
            self.tree_ops.toast = Some(format!("Failed to add to .gitignore: {error}"));
        }
        self.reload_tree();
    }

    fn paste_onto(&mut self, target: &str, target_is_dir: bool) {
        let Some(clipboard) = self.tree_ops.clipboard.clone() else {
            return;
        };
        let (sources, is_cut) = match &clipboard {
            TreeClipboard::Cut(sources) => (sources.clone(), true),
            TreeClipboard::Copied(sources) => (sources.clone(), false),
        };
        let mut pasted = Vec::new();
        let mut last = None;
        for source in &sources {
            let (destination, selection) =
                paste_destination(&self.root, source, target, target_is_dir);
            if is_cut && is_within(&destination, source) {
                continue;
            }
            let result = if is_cut {
                files::rename(&self.root, source, &destination)
            } else {
                files::copy_recursive(&self.root, source, &destination)
            };
            match result {
                Ok(()) => {
                    if is_cut {
                        self.retarget_paths(source, &destination);
                    }
                    pasted.push(destination.clone());
                    last = Some((destination, selection));
                }
                Err(error) => {
                    self.tree_ops.toast = Some(format!("Failed to paste {source}: {error}"));
                }
            }
        }
        if is_cut {
            self.tree_ops.clipboard = Some(TreeClipboard::Copied(pasted));
        }
        self.reload_tree();
        let Some((destination, selection)) = last.filter(|_| sources.len() == 1) else {
            return;
        };
        self.reveal_in_tree(&destination);
        if !self.root.join(&destination).is_dir() {
            self.open_file(&destination);
        }
        if selection.is_some() {
            self.start_rename(&destination, selection);
        }
    }

    fn start_new_entry(&mut self, parent: String, is_dir: bool) {
        if !parent.is_empty() {
            self.reveal_in_tree(&parent);
            self.expanded.insert(parent.clone());
        }
        let mut field = TextField::default();
        field.set_font_size(TREE_FONT);
        self.tree_ops.edit = Some(TreeEdit {
            target: EditTarget::New { parent, is_dir },
            field,
        });
        self.flat_dirty = true;
        self.scroll_edit_row_into_view();
    }

    fn start_rename(&mut self, path: &str, selection_end: Option<usize>) {
        let is_dir = self.root.join(path).is_dir();
        let name = file_name(path);
        let end = selection_end.unwrap_or(if is_dir {
            name.chars().count()
        } else {
            stem_len(name)
        });
        let mut field = TextField::default();
        field.set_font_size(TREE_FONT);
        field.set_text(name);
        field.select_range(0..end);
        self.reveal_in_tree(path);
        self.tree_ops.edit = Some(TreeEdit {
            target: EditTarget::Rename {
                path: path.to_string(),
                is_dir,
            },
            field,
        });
        self.flat_dirty = true;
        self.scroll_edit_row_into_view();
    }

    fn reveal_in_tree(&mut self, path: &str) {
        let mut ancestor = parent_of(path);
        while !ancestor.is_empty() {
            self.expanded.insert(ancestor.to_string());
            ancestor = parent_of(ancestor);
        }
        self.flat_dirty = true;
    }

    /// Expand the folders above `path` and scroll its row into the tree's view.
    pub(crate) fn reveal_row(&mut self, path: &str) {
        self.reveal_in_tree(path);
        let rows = self.visible_rows();
        let Some(index) = rows.iter().position(|row| row.path == path) else {
            return;
        };
        let top = 4.0 + index as f32 * ROW_H;
        if top < self.scroll || top + ROW_H > self.scroll + self.viewport_h {
            self.scroll = (top - self.viewport_h / 2.0).max(0.0);
        }
    }

    fn scroll_edit_row_into_view(&mut self) {
        let rows = self.visible_rows();
        let Some(index) = rows.iter().position(|row| row.edit) else {
            return;
        };
        let top = 4.0 + index as f32 * ROW_H;
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + self.viewport_h {
            self.scroll = (top + ROW_H - self.viewport_h).max(0.0);
        }
    }

    pub(crate) fn tree_edit_key(&mut self, key: EditKey, shift: bool) {
        match key {
            EditKey::Enter => self.confirm_tree_edit(false),
            EditKey::Escape => self.cancel_tree_edit(),
            _ => {
                if let Some(edit) = self.tree_ops.edit.as_mut() {
                    edit.field.key(key, shift);
                }
            }
        }
    }

    pub(crate) fn tree_edit_input(&mut self, text: &str) {
        if let Some(edit) = self.tree_ops.edit.as_mut() {
            edit.field.insert(&text.replace('\n', ""));
        }
    }

    pub(crate) fn cancel_tree_edit(&mut self) {
        if self.tree_ops.edit.take().is_some() {
            self.flat_dirty = true;
        }
    }

    /// Commit the pending name. An unusable name keeps the field open on Enter; losing focus drops it instead.
    pub(crate) fn confirm_tree_edit(&mut self, blurred: bool) {
        let Some(edit) = self.tree_ops.edit.as_ref() else {
            return;
        };
        let text = edit.field.text();
        let target = edit.target.clone();
        if text.trim().is_empty() {
            if blurred {
                self.cancel_tree_edit();
            }
            return;
        }
        let indicates_dir = text.ends_with('/');
        let name = text
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        if name.is_empty() || name.split('/').any(|part| part == ".." || part == ".") {
            if blurred {
                self.cancel_tree_edit();
            }
            return;
        }
        match target {
            EditTarget::New { parent, is_dir } => {
                let path = join(&parent, &name);
                if files::exists(&self.root, &path) {
                    if blurred {
                        self.cancel_tree_edit();
                    }
                    return;
                }
                let is_dir = is_dir || indicates_dir;
                let result = if is_dir {
                    files::create_dir(&self.root, &path)
                } else {
                    files::create_file(&self.root, &path)
                };
                self.tree_ops.edit = None;
                if let Err(error) = result {
                    self.tree_ops.toast = Some(format!("Failed to create {path}: {error}"));
                }
                self.reload_tree();
                self.reveal_in_tree(&path);
                if !is_dir {
                    self.open_file(&path);
                }
            }
            EditTarget::Rename { path, .. } => {
                let new_path = join(parent_of(&path), &name);
                if new_path == path {
                    self.cancel_tree_edit();
                    return;
                }
                if files::exists(&self.root, &new_path) {
                    if blurred {
                        self.cancel_tree_edit();
                    }
                    return;
                }
                self.tree_ops.edit = None;
                match files::rename(&self.root, &path, &new_path) {
                    Ok(()) => self.retarget_paths(&path, &new_path),
                    Err(error) => {
                        self.tree_ops.toast = Some(format!("Failed to rename {path}: {error}"));
                    }
                }
                self.reload_tree();
                self.reveal_in_tree(&new_path);
            }
        }
    }

    fn reload_tree(&mut self) {
        self.tree = files::build_tree(&files::list(&self.root));
        self.flat_dirty = true;
    }

    /// Follow a move on disk: open tabs and expanded folders at or under `from` now live under `to`.
    fn retarget_paths(&mut self, from: &str, to: &str) {
        let moved = |path: &str| -> Option<String> {
            is_within(path, from).then(|| format!("{to}{}", &path[from.len()..]))
        };
        self.expanded = self
            .expanded
            .drain()
            .map(|dir| moved(&dir).unwrap_or(dir))
            .collect();
        self.panes.group.for_each_pane_mut(&mut |pane| {
            for item in pane.open.iter_mut() {
                let Some(file) = item
                    .as_any_mut()
                    .and_then(|any| any.downcast_mut::<FileItem>())
                else {
                    continue;
                };
                if let Some(path) = moved(&file.path) {
                    file.retarget(&path);
                }
            }
        });
    }

    pub(crate) fn tree_edit_active(&self) -> bool {
        self.tree_ops.edit.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, is_dir: bool, depth: usize) -> Row {
        Row {
            name: file_name(path).to_string(),
            path: path.to_string(),
            is_dir,
            depth,
            open: is_dir,
            edit: false,
        }
    }

    #[test]
    fn new_file_row_goes_after_child_folders() {
        let mut rows = vec![
            row("src", true, 0),
            row("src/a", true, 1),
            row("src/a/x.rs", false, 2),
            row("src/main.rs", false, 1),
            row("README.md", false, 0),
        ];
        place_edit_row(
            &mut rows,
            &EditTarget::New {
                parent: "src".into(),
                is_dir: false,
            },
        );
        assert!(rows[3].edit);
        assert_eq!(rows[3].depth, 1);
        place_edit_row(
            &mut rows,
            &EditTarget::New {
                parent: String::new(),
                is_dir: true,
            },
        );
        assert!(rows[0].edit);
        assert_eq!(rows[0].depth, 0);
    }

    #[test]
    fn paste_disambiguates_with_copy_suffix() {
        let root = std::env::temp_dir().join(format!("pomelo-paste-{}", std::process::id()));
        std::fs::create_dir_all(root.join("dir")).ok();
        std::fs::write(root.join("dir/a.rs"), "").ok();
        let (destination, selection) = paste_destination(&root, "dir/a.rs", "dir/a.rs", false);
        assert_eq!(destination, "dir/a copy.rs");
        assert_eq!(selection, Some("a copy".len()));
        std::fs::write(root.join("dir/a copy.rs"), "").ok();
        let (destination, _) = paste_destination(&root, "dir/a.rs", "dir", true);
        assert_eq!(destination, "dir/a copy 1.rs");
        let (destination, selection) = paste_destination(&root, "dir", "dir", true);
        assert_eq!(destination, "dir copy");
        assert_eq!(selection, Some("dir copy".len()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn removal_prompt_matches_reference_wording() {
        let (message, detail, confirm) = removal_prompt("a.rs", false, true);
        assert_eq!(
            message,
            "Are you sure you want to permanently delete `a.rs`?\n\nIt has unsaved changes, which will be lost."
        );
        assert_eq!(detail.as_deref(), Some("This cannot be undone."));
        assert_eq!(confirm, "Delete");
        let (message, detail, confirm) = removal_prompt("src", true, false);
        assert_eq!(message, "Do you want to trash `src`?");
        assert_eq!(detail, None);
        assert_eq!(confirm, "Trash");
    }
}
