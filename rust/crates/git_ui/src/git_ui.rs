//! The Git panel: for each repo of the active workspace, the files its branch changed since it left the
//! default branch (committed or not), with a status icon, the path and line counts. Reading git runs on a
//! background thread; the panel refreshes while it is shown.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use git::{ChangeStatus, FileChange, RepoChanges};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::{MenuItem, PaneKind, PanelRequest, SidePanelView};

const ROW_H: f32 = 28.0;
const HEADER_H: f32 = 28.0;
const ROW_STRIDE: u64 = 4;
const MENU_BASE: u64 = 9_000_000;
/// While shown, the panel re-reads git this often (agents keep changing files).
const REFRESH_EVERY: Duration = Duration::from_secs(4);

/// A repo of the workspace to read.
#[derive(Clone, Debug)]
pub struct RepoSource {
    pub name: String,
    pub root: PathBuf,
    pub default_branch: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Row = 0,
    Refresh = 1,
}

#[derive(Clone, Debug)]
enum Row {
    Repo { index: usize },
    File { repo: usize, file: usize },
    Message(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuAction {
    OpenDiff,
    Open,
    CopyPath,
    CopyRelativePath,
    Discard,
}

struct OpenMenu {
    repo: usize,
    file: usize,
    items: Vec<(MenuItem, MenuAction)>,
}

#[derive(Default)]
struct Scan {
    repos: Vec<RepoChanges>,
    loaded: bool,
}

/// Reads every repo and publishes the result; wakes the UI only when something changed.
struct Scanner {
    sources: Arc<Vec<RepoSource>>,
    scan: Arc<Mutex<Scan>>,
    scanning: Arc<AtomicBool>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

impl Scanner {
    fn run(&self) {
        let repos: Vec<RepoChanges> = self
            .sources
            .iter()
            .map(|source| git::branch_changes(&source.root, &source.default_branch))
            .collect();
        let changed = self.scan.lock().is_ok_and(|mut scan| {
            let changed = !scan.loaded || scan.repos != repos;
            scan.repos = repos;
            scan.loaded = true;
            changed
        });
        self.scanning.store(false, Ordering::Release);
        if changed {
            (self.waker)();
        }
    }
}

pub struct GitPanel {
    sources: Arc<Vec<RepoSource>>,
    scan: Arc<Mutex<Scan>>,
    scanning: Arc<AtomicBool>,
    started: bool,
    drawn_at: Arc<Mutex<Option<Instant>>>,
    stop: Arc<AtomicBool>,
    waker: Arc<dyn Fn() + Send + Sync>,
    base: u64,
    rows: Vec<Row>,
    collapsed: HashSet<usize>,
    hover: Option<u64>,
    scroll: f32,
    viewport_h: f32,
    menu: Option<OpenMenu>,
    /// A discard waiting for confirmation: prompt tag, repo, file.
    confirm: Option<(u64, usize, usize)>,
    next_tag: u64,
    requests: Vec<PanelRequest>,
}

impl GitPanel {
    pub fn new(sources: Vec<RepoSource>, waker: Arc<dyn Fn() + Send + Sync>) -> GitPanel {
        GitPanel {
            sources: Arc::new(sources),
            scan: Arc::new(Mutex::new(Scan::default())),
            scanning: Arc::new(AtomicBool::new(false)),
            started: false,
            drawn_at: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            waker,
            base: workspace::side_panel_base(PaneKind::Git),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            hover: None,
            scroll: 0.0,
            viewport_h: 0.0,
            menu: None,
            confirm: None,
            next_tag: 1,
            requests: Vec::new(),
        }
    }

    fn scanner(&self) -> Scanner {
        Scanner {
            sources: self.sources.clone(),
            scan: self.scan.clone(),
            scanning: self.scanning.clone(),
            waker: self.waker.clone(),
        }
    }

    /// Starts a background re-read unless one is running.
    pub fn refresh(&mut self) {
        let scanner = self.scanner();
        if scanner.scanning.swap(true, Ordering::AcqRel) {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("git-panel".into())
            .spawn(move || scanner.run());
        if spawned.is_err() {
            self.scanning.store(false, Ordering::Release);
        }
    }

    /// Re-reads while the panel keeps being drawn; stops once the panel is dropped.
    fn spawn_poller(&self) {
        let (scanner, drawn_at, stop) = (self.scanner(), self.drawn_at.clone(), self.stop.clone());
        let spawned = std::thread::Builder::new()
            .name("git-panel-poll".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(REFRESH_EVERY);
                    let visible =
                        drawn_at
                            .lock()
                            .ok()
                            .and_then(|drawn| *drawn)
                            .is_some_and(|drawn| {
                                drawn.elapsed() < REFRESH_EVERY + Duration::from_secs(2)
                            });
                    if visible && !scanner.scanning.swap(true, Ordering::AcqRel) {
                        scanner.run();
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("git panel: poller: {error}");
        }
    }

    /// Blocks until the running re-read finishes (tests).
    pub fn wait_for_scan(&self) {
        while self.scanning.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn repos(&self) -> Vec<RepoChanges> {
        self.scan
            .lock()
            .map(|scan| scan.repos.clone())
            .unwrap_or_default()
    }

    fn loaded(&self) -> bool {
        self.scan.lock().is_ok_and(|scan| scan.loaded)
    }

    fn rebuild_rows(&mut self, repos: &[RepoChanges]) {
        let mut rows = Vec::new();
        if !self.loaded() {
            rows.push(Row::Message("Reading git...".into()));
        } else if self.sources.is_empty() {
            rows.push(Row::Message("No repositories in this workspace".into()));
        }
        for (index, repo) in repos.iter().enumerate() {
            rows.push(Row::Repo { index });
            if self.collapsed.contains(&index) {
                continue;
            }
            if let Some(error) = &repo.error {
                rows.push(Row::Message(
                    error.lines().next().unwrap_or_default().to_string(),
                ));
            } else if repo.files.is_empty() {
                rows.push(Row::Message("No changes on this branch".into()));
            }
            for file in 0..repo.files.len() {
                rows.push(Row::File { repo: index, file });
            }
        }
        self.rows = rows;
    }

    fn id(&self, row: usize, control: Control) -> u64 {
        self.base + row as u64 * ROW_STRIDE + control as u64
    }

    fn decode(&self, id: u64) -> Option<(usize, Control)> {
        let offset = id.checked_sub(self.base)?;
        if offset >= MENU_BASE {
            return None;
        }
        let control = match offset % ROW_STRIDE {
            0 => Control::Row,
            1 => Control::Refresh,
            _ => return None,
        };
        Some(((offset / ROW_STRIDE) as usize, control))
    }

    fn refresh_id(&self) -> u64 {
        self.base + MENU_BASE - ROW_STRIDE + Control::Refresh as u64
    }

    fn hovered_row(&self) -> Option<usize> {
        self.hover
            .and_then(|id| self.decode(id))
            .filter(|(_, control)| *control == Control::Row)
            .map(|(row, _)| row)
    }

    fn render_row(&self, index: usize, row: &Row, repos: &[RepoChanges]) -> Node {
        let hovered = self.hovered_row() == Some(index);
        let mut body = div()
            .row()
            .h_px(ROW_H)
            .pl(10.0)
            .pr(4.0)
            .gap(6.0)
            .items_center()
            .on_click(self.id(index, Control::Row));
        if hovered {
            body = body.bg(theme().ghost_element_hover);
        }
        match row {
            Row::Message(text) => div()
                .row()
                .h_px(ROW_H)
                .pl(28.0)
                .items_center()
                .child(
                    label(text.clone())
                        .size(12.0)
                        .color(theme().text_muted)
                        .truncate(),
                )
                .into(),
            Row::Repo { index: repo } => {
                let Some(changes) = repos.get(*repo) else {
                    return div().into();
                };
                let name = self
                    .sources
                    .get(*repo)
                    .map_or_else(String::new, |source| source.name.clone());
                let chevron = if self.collapsed.contains(repo) {
                    IconKind::ChevronRight
                } else {
                    IconKind::ChevronDown
                };
                let mut position = Vec::new();
                if changes.ahead > 0 {
                    position.push(format!("{} ahead", changes.ahead));
                }
                if changes.behind > 0 {
                    position.push(format!("{} behind", changes.behind));
                }
                body.child(icon(chevron).size(12.0).color(theme().icon_muted))
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .gap(6.0)
                            .items_center()
                            .child(label(name).size(12.0).color(theme().text))
                            .child(
                                label(changes.branch.clone())
                                    .size(12.0)
                                    .color(theme().text_muted)
                                    .truncate(),
                            ),
                    )
                    .child(
                        label(position.join(", "))
                            .size(12.0)
                            .color(theme().text_muted),
                    )
                    .into()
            }
            Row::File { repo, file } => {
                let Some(change) = repos
                    .get(*repo)
                    .and_then(|changes| changes.files.get(*file))
                else {
                    return div().into();
                };
                body.child(div().w_px(12.0))
                    .child(status_icon(change.status))
                    .child(path_label(change))
                    .child(diff_stat(change))
                    .into()
            }
        }
    }

    fn file(&self, repo: usize, file: usize) -> Option<(PathBuf, FileChange)> {
        let repos = self.repos();
        let change = repos.get(repo)?.files.get(file)?.clone();
        Some((repos.get(repo)?.root.join(&change.path), change))
    }

    /// The file as a diff against where the branch started (HEAD when only uncommitted work counts).
    fn open_diff(&mut self, repo: usize, file: usize) {
        let repos = self.repos();
        let Some(changes) = repos.get(repo) else {
            return;
        };
        let Some(change) = changes.files.get(file) else {
            return;
        };
        if change.status == ChangeStatus::Deleted {
            self.requests.push(PanelRequest::Toast(format!(
                "{} was deleted on this branch",
                change.path
            )));
            return;
        }
        let base = match change.status {
            ChangeStatus::Added => None,
            _ => {
                let revision = changes.fork_point.as_deref().unwrap_or("HEAD");
                let earlier = change.old_path.as_deref().unwrap_or(&change.path);
                git::file_at(&changes.root, revision, earlier)
            }
        };
        self.requests.push(PanelRequest::OpenDiff {
            path: changes.root.join(&change.path),
            base,
        });
    }

    fn toggle_repo(&mut self, repo: usize) {
        if !self.collapsed.remove(&repo) {
            self.collapsed.insert(repo);
        }
    }

    fn content_height(&self) -> f32 {
        HEADER_H + self.rows.len() as f32 * ROW_H + 8.0
    }

    fn apply_menu(&mut self, repo: usize, file: usize, action: MenuAction) {
        let Some((path, change)) = self.file(repo, file) else {
            return;
        };
        match action {
            MenuAction::OpenDiff => self.open_diff(repo, file),
            MenuAction::Open => self.requests.push(PanelRequest::OpenFile(path)),
            MenuAction::CopyPath => self
                .requests
                .push(PanelRequest::Copy(path.to_string_lossy().into_owned())),
            MenuAction::CopyRelativePath => self.requests.push(PanelRequest::Copy(change.path)),
            MenuAction::Discard => {
                let tag = self.next_tag;
                self.next_tag += 1;
                self.confirm = Some((tag, repo, file));
                let detail = if change.status == ChangeStatus::Added {
                    "The file is not committed, so it will be deleted.".to_string()
                } else {
                    "Its staged and unstaged changes are lost; the branch's commits stay."
                        .to_string()
                };
                self.requests.push(PanelRequest::Prompt {
                    tag,
                    message: format!("Discard uncommitted changes to {}?", change.path),
                    detail: Some(detail),
                    buttons: vec!["Discard".into(), "Cancel".into()],
                });
            }
        }
    }
}

fn status_icon(status: ChangeStatus) -> Node {
    let (kind, color) = match status {
        ChangeStatus::Conflicted => (IconKind::Warning, theme().warning),
        ChangeStatus::Deleted => (IconKind::SquareMinus, theme().version_control_deleted),
        ChangeStatus::Modified | ChangeStatus::Renamed => {
            (IconKind::SquareDot, theme().version_control_modified)
        }
        ChangeStatus::Added => (IconKind::SquarePlus, theme().version_control_added),
    };
    icon(kind).size(14.0).color(color).into()
}

/// The file name, then its folder muted and cut from the front when the row is narrow.
fn path_label(change: &FileChange) -> Node {
    let (folder, name) = match change.path.rsplit_once('/') {
        Some((folder, name)) => (Some(folder.to_string()), name.to_string()),
        None => (None, change.path.clone()),
    };
    let deleted = change.status == ChangeStatus::Deleted;
    let name_color = if deleted {
        theme().text_disabled
    } else {
        theme().text
    };
    let folder_color = if deleted {
        theme().text_disabled
    } else {
        theme().text_muted
    };
    let mut row = div()
        .row()
        .flex(1.0)
        .items_center()
        .child(label(format!("{name} ")).color(name_color));
    if let Some(folder) = folder {
        row = row.child(label(folder).color(folder_color).truncate_start());
    }
    row.into()
}

fn diff_stat(change: &FileChange) -> Node {
    let (Some(added), Some(deleted)) = (change.added, change.deleted) else {
        return label("binary").size(12.0).color(theme().text_muted).into();
    };
    div()
        .row()
        .gap(4.0)
        .child(
            label(format!("+{added}"))
                .size(12.0)
                .color(theme().version_control_added),
        )
        .child(
            label(format!("-{deleted}"))
                .size(12.0)
                .color(theme().version_control_deleted),
        )
        .into()
}

impl Drop for GitPanel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl SidePanelView for GitPanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Git
    }

    fn render(&mut self, width: f32, height: f32) -> Node {
        if let Ok(mut drawn) = self.drawn_at.lock() {
            *drawn = Some(Instant::now());
        }
        if !self.started {
            self.started = true;
            self.refresh();
            self.spawn_poller();
        }
        self.viewport_h = height;
        let repos = self.repos();
        self.rebuild_rows(&repos);
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let refresh_id = self.refresh_id();
        let hot = self.hover == Some(refresh_id);
        let total: usize = repos.iter().map(|repo| repo.files.len()).sum();
        let header = div()
            .row()
            .h_px(HEADER_H)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(label("Git").size(12.0).color(theme().text_muted))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(if total == 1 {
                        "1 changed file".to_string()
                    } else {
                        format!("{total} changed files")
                    })
                    .size(11.0)
                    .color(theme().text_placeholder)
                    .truncate(),
                ),
            )
            .child(
                div()
                    .w_px(18.0)
                    .h_px(18.0)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(refresh_id)
                    .bg(if hot {
                        theme().element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(
                        icon(IconKind::RotateCw)
                            .size(12.0)
                            .color(theme().icon_muted),
                    ),
            );
        let first = (self.scroll / ROW_H).floor() as usize;
        let visible = (height / ROW_H).ceil() as usize + 2;
        let offset = first as f32 * ROW_H - self.scroll;
        let mut list = div().col().child(div().h_px(offset.max(0.0)));
        for (index, row) in self.rows.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.render_row(index, row, &repos));
        }
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(list)
            .into()
    }

    fn click(&mut self, id: u64) {
        if id == self.refresh_id() {
            self.refresh();
            return;
        }
        let Some((index, _)) = self.decode(id) else {
            return;
        };
        match self.rows.get(index).cloned() {
            Some(Row::Repo { index: repo }) => self.toggle_repo(repo),
            Some(Row::File { repo, file }) => self.open_diff(repo, file),
            _ => {}
        }
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let id = id.filter(|id| self.decode(*id).is_some() || *id == self.refresh_id());
        if self.hover == id {
            return false;
        }
        self.hover = id;
        true
    }

    fn scroll(&mut self, dy: f32) -> bool {
        let max_scroll = (self.content_height() - self.viewport_h).max(0.0);
        let next = (self.scroll - dy).clamp(0.0, max_scroll);
        if (next - self.scroll).abs() < 0.01 {
            return false;
        }
        self.scroll = next;
        true
    }

    fn open_menu(&mut self, id: u64) -> bool {
        let Some((index, _)) = self.decode(id) else {
            return false;
        };
        let Some(Row::File { repo, file }) = self.rows.get(index).cloned() else {
            return false;
        };
        let Some((_, change)) = self.file(repo, file) else {
            return false;
        };
        let mut items = Vec::new();
        let mut push = |text: &'static str, action: MenuAction, sep: bool| {
            let id = self.base + MENU_BASE + items.len() as u64;
            items.push((
                MenuItem {
                    id,
                    label: text.into(),
                    checked: false,
                    sep,
                    disabled: false,
                },
                action,
            ));
        };
        if change.status != ChangeStatus::Deleted {
            push("Open Diff", MenuAction::OpenDiff, false);
            push("Open File", MenuAction::Open, false);
        }
        push("Copy Path", MenuAction::CopyPath, true);
        push("Copy Relative Path", MenuAction::CopyRelativePath, false);
        if change.uncommitted {
            push("Discard Uncommitted Changes", MenuAction::Discard, true);
        }
        self.menu = Some(OpenMenu { repo, file, items });
        true
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        self.menu
            .as_ref()
            .map(|menu| menu.items.iter().map(|(item, _)| item.clone()).collect())
            .unwrap_or_default()
    }

    fn menu_action(&mut self, item: u64) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        if let Some(action) = menu
            .items
            .iter()
            .find(|(entry, _)| entry.id == item)
            .map(|(_, action)| *action)
        {
            self.apply_menu(menu.repo, menu.file, action);
        }
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }

    fn prompt_answered(&mut self, tag: u64, answer: usize) {
        let Some((asked, repo, file)) = self.confirm.take() else {
            return;
        };
        if asked != tag || answer != 0 {
            return;
        }
        let repos = self.repos();
        let Some((root, path)) = repos.get(repo).and_then(|changes| {
            Some((changes.root.clone(), changes.files.get(file)?.path.clone()))
        }) else {
            return;
        };
        if let Err(error) = git::discard_uncommitted(&root, &path) {
            self.requests.push(PanelRequest::Toast(format!(
                "Could not discard {path}: {error}"
            )));
        }
        self.refresh();
    }
}
