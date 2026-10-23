use std::path::{Path, PathBuf};

use workspace::pane_group_view::PaneGroupView;
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
    pub(crate) toast: Option<String>,
}

impl TreeOps {
    pub(crate) fn next_save_token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }
}

/// A change to the tree, run on whichever machine the files are on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeChange {
    CreateFile(String),
    CreateDir(String),
    Rename { from: String, to: String },
    Copy { from: String, to: String },
    Remove { path: String, trash: bool },
    Gitignore { path: String, is_dir: bool },
}

impl TreeChange {
    fn run(&self, fs: &dyn files::ProjectFs) -> std::io::Result<()> {
        match self {
            TreeChange::CreateFile(path) => fs.create_file(path),
            TreeChange::CreateDir(path) => fs.create_dir(path),
            TreeChange::Rename { from, to } => fs.rename(from, to),
            TreeChange::Copy { from, to } => fs.copy(from, to),
            TreeChange::Remove { path, trash: true } => fs.trash(path),
            TreeChange::Remove { path, trash: false } => fs.remove(path),
            TreeChange::Gitignore { path, is_dir } => fs.add_to_gitignore(path, *is_dir),
        }
    }

    fn failure(&self, error: &std::io::Error) -> String {
        match self {
            TreeChange::CreateFile(path) | TreeChange::CreateDir(path) => {
                format!("Failed to create {path}: {error}")
            }
            TreeChange::Rename { from, .. } => format!("Failed to rename {from}: {error}"),
            TreeChange::Copy { from, .. } => format!("Failed to paste {from}: {error}"),
            TreeChange::Remove { path, trash } => {
                let verb = if *trash { "trash" } else { "delete" };
                format!("Failed to {verb} {path}: {error}")
            }
            TreeChange::Gitignore { .. } => format!("Failed to add to .gitignore: {error}"),
        }
    }
}

/// A change sent to the other machine and already shown: its answer, and how to take it back when it fails.
pub(crate) struct TreeJob {
    change: TreeChange,
    answer: std::sync::mpsc::Receiver<std::io::Result<()>>,
    undo: files::TreePatch,
}

fn entry(path: &str, is_dir: bool) -> files::FileEntry {
    files::FileEntry {
        repo: String::new(),
        path: path.to_string(),
        is_dir,
        size: 0,
    }
}

/// The folders above `path` the tree does not have yet, outermost first.
fn missing_parents(entries: &[files::FileEntry], path: &str) -> Vec<String> {
    let mut missing = Vec::new();
    let mut parent = parent_of(path);
    while !parent.is_empty() {
        let known = entries
            .binary_search_by(|entry| entry.path.as_str().cmp(parent))
            .is_ok();
        if known {
            break;
        }
        missing.push(parent.to_string());
        parent = parent_of(parent);
    }
    missing.reverse();
    missing
}

/// What `change` does to the tree as shown, and the patch that undoes it.
fn optimistic(
    entries: &[files::FileEntry],
    change: &TreeChange,
) -> (files::TreePatch, files::TreePatch) {
    let under = |path: &str| -> Vec<files::FileEntry> {
        entries
            .iter()
            .filter(|entry| is_within(&entry.path, path))
            .cloned()
            .collect()
    };
    let placed = |to: &str, moved: &[files::FileEntry], from: &str| -> Vec<files::FileEntry> {
        let mut placed: Vec<files::FileEntry> = missing_parents(entries, to)
            .iter()
            .map(|parent| entry(parent, true))
            .collect();
        placed.extend(moved.iter().map(|entry| files::FileEntry {
            path: format!("{to}{}", &entry.path[from.len()..]),
            ..entry.clone()
        }));
        placed
    };
    let added = |to: &str| -> String {
        missing_parents(entries, to)
            .into_iter()
            .next()
            .unwrap_or_else(|| to.to_string())
    };
    match change {
        TreeChange::CreateFile(path) | TreeChange::CreateDir(path) => {
            let is_dir = matches!(change, TreeChange::CreateDir(_));
            let made = [entry(path, is_dir)];
            let apply = files::TreePatch {
                upserted: placed(path, &made, path),
                ..files::TreePatch::default()
            };
            let undo = files::TreePatch {
                removed: vec![added(path)],
                ..files::TreePatch::default()
            };
            (apply, undo)
        }
        TreeChange::Rename { from, to } => {
            let moved = under(from);
            let apply = files::TreePatch {
                removed: vec![from.clone()],
                upserted: placed(to, &moved, from),
                ..files::TreePatch::default()
            };
            let undo = files::TreePatch {
                removed: vec![added(to)],
                upserted: moved,
                ..files::TreePatch::default()
            };
            (apply, undo)
        }
        TreeChange::Copy { from, to } => {
            let apply = files::TreePatch {
                upserted: placed(to, &under(from), from),
                ..files::TreePatch::default()
            };
            let undo = files::TreePatch {
                removed: vec![added(to)],
                ..files::TreePatch::default()
            };
            (apply, undo)
        }
        TreeChange::Remove { path, .. } => {
            let apply = files::TreePatch {
                removed: vec![path.clone()],
                ..files::TreePatch::default()
            };
            let undo = files::TreePatch {
                upserted: under(path),
                ..files::TreePatch::default()
            };
            (apply, undo)
        }
        TreeChange::Gitignore { .. } => (files::TreePatch::default(), files::TreePatch::default()),
    }
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
    exists: &dyn Fn(&str) -> bool,
    source: &str,
    source_is_dir: bool,
    target: &str,
    target_is_dir: bool,
) -> (String, Option<usize>) {
    let parent = if !target_is_dir || target == source {
        parent_of(target)
    } else {
        target
    };
    let name = file_name(source);
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && !source_is_dir => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    };
    let mut destination = join(parent, name);
    let mut selection = None;
    let mut attempt = 0;
    while exists(&destination) {
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

fn has_restorable_changes(work_dir: &Path, full: &Path) -> bool {
    let full = full.to_string_lossy();
    git_output(work_dir, &["status", "--porcelain", "--", &full]).is_some_and(|status| {
        status.lines().any(|line| {
            let code = line.get(..2).unwrap_or("");
            code != "??" && (code.contains('M') || code.contains('D'))
        })
    })
}

impl FilesView {
    /// Whether something is at `path`: on disk here, or in the tree of files on another machine.
    fn path_exists(&self, path: &str) -> bool {
        if !self.is_remote() {
            return files::exists(&self.root, path);
        }
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .is_ok()
    }

    /// Runs `change`: at once here, the toast saying why it failed; on another machine shown at once and sent
    /// off, taken back when it fails there. Returns false when it failed here.
    pub(crate) fn change_tree(&mut self, change: TreeChange) -> bool {
        if !self.is_remote() {
            return match change.run(&*self.fs) {
                Ok(()) => true,
                Err(error) => {
                    self.tree_ops.toast = Some(change.failure(&error));
                    false
                }
            };
        }
        let (apply, undo) = optimistic(&self.entries, &change);
        self.patch_tree(&apply);
        let (sender, answer) = std::sync::mpsc::channel();
        let (fs, sent) = (self.fs.clone(), change.clone());
        let spawned = std::thread::Builder::new()
            .name("file-tree-change".into())
            .spawn(move || {
                if sender.send(sent.run(&*fs)).is_ok() {
                    ui::wake();
                }
            });
        match spawned {
            Ok(_) => {
                self.tree_jobs.push(TreeJob {
                    change,
                    answer,
                    undo,
                });
                true
            }
            Err(error) => {
                self.patch_tree(&undo);
                self.tree_ops.toast = Some(change.failure(&error));
                false
            }
        }
    }

    fn patch_tree(&mut self, patch: &files::TreePatch) {
        if patch.is_empty() {
            return;
        }
        files::fs::apply_patch(&mut self.entries, patch);
        self.tree = files::build_tree(&self.entries);
        self.flat_dirty = true;
    }

    /// Takes the answers to tree changes sent off; one that failed is taken back from the tree and its tabs.
    pub(crate) fn poll_tree_jobs(&mut self) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < self.tree_jobs.len() {
            let answer = match self.tree_jobs[index].answer.try_recv() {
                Ok(answer) => answer,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err(std::io::Error::other("the change stopped"))
                }
            };
            let job = self.tree_jobs.remove(index);
            changed = true;
            let Err(error) = answer else {
                continue;
            };
            self.patch_tree(&job.undo);
            if let TreeChange::Rename { from, to } = &job.change {
                self.retarget_paths(to, from);
            }
            self.tree_ops.toast = Some(job.change.failure(&error));
        }
        changed
    }

    /// Whether `path` is a folder, as the tree knows it for files on another machine.
    fn is_dir_entry(&self, path: &str) -> bool {
        if !self.is_remote() {
            return self.root.join(path).is_dir();
        }
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .and_then(|index| self.entries.get(index))
            .is_some_and(|entry| entry.is_dir)
    }

    pub(crate) fn menu_state(&self, path: Option<&str>) -> TreeMenuState {
        let is_dir = path.is_none_or(|path| self.is_dir_entry(path));
        let full = self.root.join(path.unwrap_or(""));
        let work_dir = (!self.is_remote()).then(|| git_work_dir(&full)).flatten();
        TreeMenuState {
            is_dir,
            is_root: path.is_none(),
            has_git_repo: work_dir.is_some() || self.is_remote(),
            has_git_changes: !is_dir
                && work_dir
                    .as_deref()
                    .is_some_and(|work_dir| has_restorable_changes(work_dir, &full)),
            can_paste: self.tree_ops.clipboard.is_some(),
        }
    }

    pub(crate) fn run_tree_action(
        &mut self,
        path: Option<&str>,
        action: TreeAction,
    ) -> Option<Prompt> {
        let target = path.unwrap_or("").to_string();
        let target_is_dir = path.is_none_or(|path| self.is_dir_entry(path));
        let changes = !matches!(
            action,
            TreeAction::ExpandAll
                | TreeAction::CollapseAll
                | TreeAction::Cut
                | TreeAction::Copy
                | TreeAction::OpenWithSystem
        );
        if self.is_remote() && self.read_only && changes {
            self.tree_ops.toast =
                Some("main is read-only: make changes in a branch workspace".into());
            return None;
        }
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
            TreeAction::OpenWithSystem if self.is_remote() => {
                self.tree_ops.toast =
                    Some("Files on another machine open here in the editor only".into());
            }
            TreeAction::OpenWithSystem => {
                if let Err(error) = desktop::open(self.root.join(&target)) {
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
            warning: None,
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
                self.change_tree(TreeChange::Remove {
                    path: path.clone(),
                    trash,
                });
                self.expanded.retain(|dir| !is_within(dir, &path));
                self.reload_tree();
            }
            TreePrompt::Restore { path } => {
                let full = self.root.join(&path);
                let restored = git_work_dir(&full).and_then(|work_dir| {
                    git_output(
                        &work_dir,
                        &["checkout", "HEAD", "--", &full.to_string_lossy()],
                    )
                });
                if restored.is_none() {
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
        let mut dirty = self.other_dirty.iter().any(|id| is_within(id, path));
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
            let listed = if self.is_remote() {
                self.entries.clone()
            } else {
                files::list(&self.root)
            };
            for entry in listed {
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
        self.change_tree(TreeChange::Gitignore {
            path: path.to_string(),
            is_dir,
        });
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
            let source_is_dir = self.is_dir_entry(source);
            let (destination, selection) = paste_destination(
                &|path| self.path_exists(path),
                source,
                source_is_dir,
                target,
                target_is_dir,
            );
            if is_cut && is_within(&destination, source) {
                continue;
            }
            let change = if is_cut {
                TreeChange::Rename {
                    from: source.clone(),
                    to: destination.clone(),
                }
            } else {
                TreeChange::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                }
            };
            if self.change_tree(change) {
                if is_cut {
                    self.retarget_paths(source, &destination);
                }
                pasted.push(destination.clone());
                last = Some((destination, selection));
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
        if !self.is_dir_entry(&destination) {
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
        let is_dir = self.is_dir_entry(path);
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
        let top = index as f32 * ROW_H;
        if top < self.scroll || top + ROW_H > self.scroll + self.viewport_h {
            self.scroll = (top - self.viewport_h / 2.0).max(0.0);
        }
    }

    fn scroll_edit_row_into_view(&mut self) {
        let rows = self.visible_rows();
        let Some(index) = rows.iter().position(|row| row.edit) else {
            return;
        };
        let top = index as f32 * ROW_H;
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
                if self.path_exists(&path) {
                    if blurred {
                        self.cancel_tree_edit();
                    }
                    return;
                }
                let is_dir = is_dir || indicates_dir;
                self.tree_ops.edit = None;
                self.change_tree(if is_dir {
                    TreeChange::CreateDir(path.clone())
                } else {
                    TreeChange::CreateFile(path.clone())
                });
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
                if self.path_exists(&new_path) {
                    if blurred {
                        self.cancel_tree_edit();
                    }
                    return;
                }
                self.tree_ops.edit = None;
                if self.change_tree(TreeChange::Rename {
                    from: path.clone(),
                    to: new_path.clone(),
                }) {
                    self.retarget_paths(&path, &new_path);
                }
                self.reload_tree();
                self.reveal_in_tree(&new_path);
            }
        }
    }

    /// Reads the tree here again; another machine's follows by itself.
    fn reload_tree(&mut self) {
        if self.is_remote() {
            return;
        }
        self.entries = files::list(&self.root);
        self.tree = files::build_tree(&self.entries);
        self.flat_dirty = true;
    }

    /// Follow a move on disk: open tabs and expanded folders at or under `from` now live under `to`.
    pub(crate) fn retarget_paths(&mut self, from: &str, to: &str) {
        let moved = |path: &str| -> Option<String> {
            is_within(path, from).then(|| format!("{to}{}", &path[from.len()..]))
        };
        self.expanded = self
            .expanded
            .drain()
            .map(|dir| moved(&dir).unwrap_or(dir))
            .collect();
        retarget_files(&mut self.panes, from, to);
        self.pending_moves.push((from.to_string(), to.to_string()));
    }

    /// Bring another pane group (the terminal panel) up to date with the tree: tabs follow files moved since the
    /// last frame, and its dirty files count when deleting asks about unsaved changes.
    pub(crate) fn sync_other_group(&mut self, other: Option<&mut PaneGroupView>) {
        let moves = std::mem::take(&mut self.pending_moves);
        let Some(group) = other else {
            self.other_dirty.clear();
            return;
        };
        for (from, to) in &moves {
            retarget_files(group, from, to);
        }
        let mut dirty = Vec::new();
        group.for_each_item_mut(&mut |item| {
            if item.is_dirty() {
                dirty.extend(item.id());
            }
        });
        self.other_dirty = dirty;
    }

    /// The tree takes or gives back the keyboard; taking it starts on the open file's row, else the first.
    pub(crate) fn set_tree_focus(&mut self, focused: bool) {
        self.tree_focused = focused;
        if !focused {
            return;
        }
        let rows = self.visible_rows();
        let still_there = self
            .tree_selected
            .as_ref()
            .is_some_and(|path| rows.iter().any(|row| &row.path == path));
        if still_there {
            return;
        }
        let open = self.active_path();
        if let Some(open) = open.filter(|path| !path.is_empty()) {
            self.reveal_row(&open);
            self.tree_selected = Some(open);
        } else {
            self.tree_selected = rows.first().map(|row| row.path.clone());
        }
        self.flat_dirty = true;
    }

    /// The row the tree's keys act on, as (path, is a folder); the open file when nothing is picked.
    pub(crate) fn selected_tree_entry(&self) -> Option<(String, bool)> {
        let rows = self.visible_rows();
        let path = self.tree_selected.clone().or_else(|| self.active_path())?;
        let is_dir = rows
            .iter()
            .find(|row| row.path == path)
            .map(|row| row.is_dir)
            .unwrap_or_else(|| self.root.join(&path).is_dir());
        Some((path, is_dir))
    }

    /// Up, down, Home and End over the visible rows; the list stops at its ends.
    pub(crate) fn tree_nav_key(&mut self, key: EditKey, shift: bool) -> bool {
        let Some(movement) = workspace::list_nav::nav_move(key, shift) else {
            return false;
        };
        let rows = self.visible_rows();
        let mut nav = workspace::list_nav::ListNav {
            selected: self
                .tree_selected
                .as_ref()
                .and_then(|path| rows.iter().position(|row| &row.path == path)),
        };
        nav.apply(movement, rows.len(), false, |_| true);
        if let Some(index) = nav.selected {
            self.tree_selected = rows.get(index).map(|row| row.path.clone());
            self.scroll_row_into_view(index);
        }
        true
    }

    /// Right opens the picked folder (or steps into it when open); Left closes it, or from inside one closes
    /// the folder it is in and moves there. With `all`, every folder opens or closes.
    pub(crate) fn tree_expand_selected(&mut self, expand: bool, all: bool) -> bool {
        if all {
            self.set_expanded_within(None, expand);
            self.flat_dirty = true;
            if !expand {
                self.tree_selected = self
                    .tree_selected
                    .as_ref()
                    .map(|path| path.split('/').next().unwrap_or(path).to_string());
            }
            return true;
        }
        let Some((path, is_dir)) = self.selected_tree_entry() else {
            return false;
        };
        if expand {
            if !is_dir {
                return false;
            }
            if self.expanded.insert(path.clone()) {
                self.flat_dirty = true;
            } else {
                let rows = self.visible_rows();
                let child = rows
                    .iter()
                    .position(|row| row.path == path)
                    .and_then(|index| rows.get(index + 1))
                    .filter(|row| row.path.starts_with(&format!("{path}/")));
                if let Some(child) = child {
                    self.tree_selected = Some(child.path.clone());
                }
            }
        } else if is_dir && self.expanded.remove(&path) {
            self.flat_dirty = true;
        } else if let Some((parent, _)) = path.rsplit_once('/') {
            self.expanded.remove(parent);
            self.tree_selected = Some(parent.to_string());
            self.flat_dirty = true;
        }
        if let Some(index) = self.tree_selected.as_ref().and_then(|selected| {
            self.visible_rows()
                .iter()
                .position(|row| &row.path == selected)
        }) {
            self.scroll_row_into_view(index);
        }
        true
    }

    fn scroll_row_into_view(&mut self, index: usize) {
        let top = index as f32 * ROW_H;
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + self.viewport_h {
            self.scroll = (top + ROW_H - self.viewport_h).max(0.0);
        }
    }

    pub(crate) fn tree_edit_active(&self) -> bool {
        self.tree_ops.edit.is_some()
    }
}

/// Point the file tabs of `group` at or under `from` to where they moved under `to`.
fn retarget_files(group: &mut PaneGroupView, from: &str, to: &str) {
    group.for_each_item_mut(&mut |item| {
        let Some(file) = item
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<FileItem>())
        else {
            return;
        };
        if is_within(&file.path, from) {
            let moved = format!("{to}{}", file.path.get(from.len()..).unwrap_or_default());
            file.retarget(&moved);
        }
    });
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
        let exists = |path: &str| files::exists(&root, path);
        let (destination, selection) =
            paste_destination(&exists, "dir/a.rs", false, "dir/a.rs", false);
        assert_eq!(destination, "dir/a copy.rs");
        assert_eq!(selection, Some("a copy".len()));
        std::fs::write(root.join("dir/a copy.rs"), "").ok();
        let (destination, _) = paste_destination(&exists, "dir/a.rs", false, "dir", true);
        assert_eq!(destination, "dir/a copy 1.rs");
        let (destination, selection) = paste_destination(&exists, "dir", true, "dir", true);
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

    #[test]
    fn renames_reach_tabs_in_the_other_pane_group() {
        let root = std::env::temp_dir().join(format!("pomelo-tree-other-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let mut view = FilesView::scanned(root.clone());
        let mut other = PaneGroupView::new(workspace::pane_group_view::PaneGroupConfig {
            id_base: 0,
            show_nav: false,
            buttons: Vec::new(),
            max_panes: 2,
            split_filter: None,
            zoom_whole_group: false,
        });
        let mut file = FileItem::new(root.clone(), "src/a.rs", Some("fn a() {}\n".into()));
        if let Some(buffer) = file.buffer.as_mut() {
            buffer.edit(vec![(0..0, "// x\n".into())]);
        }
        if let Some(pane) = other.active_pane_mut() {
            pane.add_item(Box::new(file));
        }
        view.retarget_paths("src", "lib");
        view.sync_other_group(Some(&mut other));
        let id = other.active_item().and_then(|item| item.id());
        assert_eq!(id.as_deref(), Some("lib/a.rs"));
        assert!(view.has_dirty_items_within("lib"));
        view.sync_other_group(None);
        assert!(!view.has_dirty_items_within("lib"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
