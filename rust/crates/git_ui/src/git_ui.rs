//! The Git panel: for each repo of the active workspace, the files its branch changed since it left the
//! default branch (committed or not), with a status icon, the path and line counts. Reading git runs on a
//! background thread; the panel refreshes while it is shown.

mod commit_area;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commit_area::{CommitArea, RemoteRequest};
use git::working_copy::{self, HeadState, Staging, StatusEntry, Upstream};
use git::{ChangeStatus, FileChange, RepoChanges};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::{EditKey, MenuItem, PaneKind, PanelRequest, SidePanelView};

const ROW_H: f32 = 28.0;
const HEADER_H: f32 = 28.0;
const ROW_STRIDE: u64 = 4;
const MENU_BASE: u64 = 9_000_000;
const FOOTER: u64 = MENU_BASE + 1_000;
const COMMIT_EDITOR: u64 = FOOTER + 1;
const COMMIT: u64 = FOOTER + 2;
const COMMIT_MENU: u64 = FOOTER + 3;
const REMOTE: u64 = FOOTER + 4;
const REMOTE_MENU: u64 = FOOTER + 5;
const SWITCH_REPO: u64 = FOOTER + 6;
const STAGE_ALL: u64 = FOOTER + 7;
const SELECTOR_QUERY: u64 = FOOTER + 8;
/// A repository in the selector: id = base + index into its filtered list.
const SELECTOR_ITEM: u64 = FOOTER + 20;
const SELECTOR_MAX: usize = 60;
const SELECTOR_ROWS: usize = 10;
const ALL_REMOTES: &str = "All";
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
    Review = 2,
    Stage = 3,
}

#[derive(Clone, Debug)]
enum Row {
    Repo {
        index: usize,
    },
    File {
        repo: usize,
        file: usize,
    },
    Message(String),
    /// The workspace's pull requests, one row per repo that has one.
    PullRequests {
        count: usize,
    },
    PullRequest {
        repo: usize,
    },
}

/// The repository picker opened from the footer: the typed filter and the highlighted match.
#[derive(Default)]
struct RepoSelector {
    query: String,
    highlight: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuAction {
    ToggleReviewed,
    OpenDiff,
    Open,
    CopyPath,
    CopyRelativePath,
    Discard,
    Amend,
    Signoff,
    SkipHooks,
    Fetch,
    FetchFrom,
    Pull,
    PullRebase,
    Push,
    PushTo,
    ForcePush,
}

struct OpenMenu {
    repo: usize,
    file: usize,
    items: Vec<(MenuItem, MenuAction)>,
}

#[derive(Default)]
struct Scan {
    repos: Vec<RepoChanges>,
    status: Vec<Vec<StatusEntry>>,
    heads: Vec<HeadState>,
    /// `repo/path` -> fingerprint of the file as it is now, to tell whether a review still holds.
    fingerprints: HashMap<String, String>,
    loaded: bool,
}

/// FNV-1a over the file's bytes: stable across runs, which a review mark must be.
fn fingerprint(path: &std::path::Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
            });
            format!("{hash:016x}")
        }
        Err(_) => "deleted".to_string(),
    }
}

fn review_key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
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
        let fingerprints: HashMap<String, String> = self
            .sources
            .iter()
            .zip(&repos)
            .flat_map(|(source, changes)| {
                changes.files.iter().map(|file| {
                    (
                        review_key(&source.name, &file.path),
                        fingerprint(&changes.root.join(&file.path)),
                    )
                })
            })
            .collect();
        let status: Vec<Vec<StatusEntry>> = self
            .sources
            .iter()
            .map(|source| working_copy::status(&source.root).unwrap_or_default())
            .collect();
        let heads: Vec<HeadState> = self
            .sources
            .iter()
            .map(|source| working_copy::head_state(&source.root))
            .collect();
        let changed = self.scan.lock().is_ok_and(|mut scan| {
            let changed = !scan.loaded
                || scan.repos != repos
                || scan.fingerprints != fingerprints
                || scan.status != status
                || scan.heads != heads;
            scan.repos = repos;
            scan.fingerprints = fingerprints;
            scan.status = status;
            scan.heads = heads;
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
    hover: Option<u64>,
    scroll: f32,
    viewport_h: f32,
    menu: Option<OpenMenu>,
    /// Where review marks are kept, and the marks: `repo/path` -> fingerprint when marked.
    reviews_file: Option<PathBuf>,
    reviewed: HashMap<String, String>,
    /// A discard waiting for confirmation: prompt tag, repo, file.
    confirm: Option<(u64, usize, usize)>,
    next_tag: u64,
    requests: Vec<PanelRequest>,
    commit: CommitArea,
    active_repo: usize,
    remote_prompt: Option<(u64, Vec<String>, RemoteRequest)>,
    pull_requests: Option<pull_request_ui::PullRequests>,
    prs_collapsed: bool,
    selector: Option<RepoSelector>,
}

impl GitPanel {
    /// `reviews_file` keeps which files were marked reviewed; `None` keeps marks only while open.
    pub fn new(
        sources: Vec<RepoSource>,
        reviews_file: Option<PathBuf>,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> GitPanel {
        let reviewed = reviews_file
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        GitPanel {
            reviews_file,
            reviewed,
            sources: Arc::new(sources),
            scan: Arc::new(Mutex::new(Scan::default())),
            scanning: Arc::new(AtomicBool::new(false)),
            started: false,
            drawn_at: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            base: workspace::side_panel_base(PaneKind::Git),
            rows: Vec::new(),
            hover: None,
            scroll: 0.0,
            viewport_h: 0.0,
            menu: None,
            confirm: None,
            next_tag: 1,
            requests: Vec::new(),
            commit: CommitArea::new(waker.clone()),
            active_repo: 0,
            remote_prompt: None,
            pull_requests: None,
            prs_collapsed: false,
            selector: None,
            waker,
        }
    }

    pub fn with_pull_requests(mut self, pull_requests: pull_request_ui::PullRequests) -> GitPanel {
        self.pull_requests = Some(pull_requests);
        self
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

    pub fn operation_running(&self) -> bool {
        self.commit.busy()
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
        let with_prs = self.repos_with_pull_requests();
        if !with_prs.is_empty() {
            rows.push(Row::PullRequests {
                count: with_prs.len(),
            });
            if !self.prs_collapsed {
                rows.extend(with_prs.into_iter().map(|repo| Row::PullRequest { repo }));
            }
        }
        let active = self.active_repo.min(repos.len().saturating_sub(1));
        for (index, repo) in repos.iter().enumerate() {
            if index != active {
                continue;
            }
            rows.push(Row::Repo { index });
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

    /// Repos (by index) whose checked-out branch has a pull request.
    fn repos_with_pull_requests(&self) -> Vec<usize> {
        let Some(prs) = self.pull_requests.as_ref() else {
            return Vec::new();
        };
        (0..self.sources.len())
            .filter(|repo| {
                self.sources
                    .get(*repo)
                    .and_then(|source| prs.for_checkout(&source.root))
                    .is_some_and(|(_, pr)| pr.is_some())
            })
            .collect()
    }

    /// The selector's matches: repo indices sorted by name, filtered by the typed text.
    fn selector_matches(&self) -> Vec<usize> {
        let query = self
            .selector
            .as_ref()
            .map(|selector| selector.query.to_lowercase())
            .unwrap_or_default();
        let mut matches: Vec<usize> = (0..self.sources.len())
            .filter(|index| self.sources[*index].name.to_lowercase().contains(&query))
            .collect();
        matches.sort_by_key(|index| self.sources[*index].name.to_lowercase());
        matches.truncate(SELECTOR_MAX);
        matches
    }

    fn open_selector(&mut self) {
        if self.selector.take().is_some() {
            return;
        }
        self.selector = Some(RepoSelector::default());
        let highlight = self
            .selector_matches()
            .iter()
            .position(|repo| *repo == self.active_repo)
            .unwrap_or(0);
        if let Some(selector) = self.selector.as_mut() {
            selector.highlight = highlight;
        }
        self.commit.focused = false;
    }

    fn choose_repo(&mut self, repo: usize) {
        if repo < self.sources.len() {
            self.active_repo = repo;
            self.scroll = 0.0;
        }
        self.selector = None;
    }

    /// The picker over the footer, after the reference's repository selector: each repo with a check on the
    /// active one and its change status at the end, the filter below.
    fn render_selector(&self, width: f32) -> Option<Node> {
        let selector = self.selector.as_ref()?;
        let colors = theme();
        let matches = self.selector_matches();
        let first = selector
            .highlight
            .saturating_sub(SELECTOR_ROWS.saturating_sub(1));
        let mut list = div().col().p(4.0).gap(1.0);
        if matches.is_empty() {
            list = list.child(
                div()
                    .row()
                    .h_px(26.0)
                    .px(8.0)
                    .items_center()
                    .child(label("No matches").size(12.0).color(colors.text_muted)),
            );
        }
        for (position, repo) in matches.iter().enumerate().skip(first).take(SELECTOR_ROWS) {
            let Some(source) = self.sources.get(*repo) else {
                continue;
            };
            let id = self.base + SELECTOR_ITEM + position as u64;
            let highlighted = position == selector.highlight || self.hover == Some(id);
            let mut name = div().row().flex(1.0).gap(4.0).items_center().child(
                label(source.name.clone())
                    .size(13.0)
                    .color(colors.text)
                    .truncate(),
            );
            if *repo == self.active_repo {
                name = name.child(icon(IconKind::Check).size(12.0).color(colors.text_accent));
            }
            let mut row = div()
                .row()
                .h_px(26.0)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(name);
            if let Some(status) = worst_status(&self.status_of(*repo)) {
                row = row.child(status_icon(status));
            }
            if highlighted {
                row = row.bg(colors.element_selected);
            }
            list = list.child(row);
        }
        let query: Node = if selector.query.is_empty() {
            label("Select a repository...")
                .size(13.0)
                .color(colors.text_placeholder)
                .into()
        } else {
            label(selector.query.clone())
                .size(13.0)
                .color(colors.text)
                .into()
        };
        Some(
            div()
                .col()
                .w_px(width)
                .px(6.0)
                .pb(4.0)
                .child(
                    div()
                        .col()
                        .rounded(6.0)
                        .bg(colors.elevated_surface_background)
                        .border(1.0, colors.border_variant)
                        .child(list)
                        .child(div().h_px(1.0).bg(colors.border_variant))
                        .child(
                            div()
                                .row()
                                .h_px(32.0)
                                .px(10.0)
                                .items_center()
                                .on_click(self.base + SELECTOR_QUERY)
                                .child(query),
                        ),
                )
                .into(),
        )
    }

    fn selector_height(&self) -> f32 {
        if self.selector.is_none() {
            return 0.0;
        }
        let rows = self.selector_matches().len().clamp(1, SELECTOR_ROWS) as f32;
        8.0 + rows * 27.0 + 1.0 + 32.0 + 2.0 + 4.0
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
            2 => Control::Review,
            _ => Control::Stage,
        };
        Some(((offset / ROW_STRIDE) as usize, control))
    }

    fn refresh_id(&self) -> u64 {
        self.base + MENU_BASE - ROW_STRIDE + Control::Refresh as u64
    }

    fn hovered_row(&self) -> Option<usize> {
        self.hover
            .and_then(|id| self.decode(id))
            .filter(|(_, control)| *control != Control::Refresh)
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
            Row::PullRequests { count } => {
                let chevron = if self.prs_collapsed {
                    IconKind::ChevronRight
                } else {
                    IconKind::ChevronDown
                };
                body.child(icon(chevron).size(12.0).color(theme().icon_muted))
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .child(label("Pull Requests").size(12.0).color(theme().text_muted)),
                    )
                    .child(
                        label(count.to_string())
                            .size(12.0)
                            .color(theme().text_muted),
                    )
                    .into()
            }
            Row::PullRequest { repo } => {
                let Some((source, pr)) = self.sources.get(*repo).and_then(|source| {
                    let (_, pr) = self.pull_requests.as_ref()?.for_checkout(&source.root)?;
                    Some((source, pr?))
                }) else {
                    return div().into();
                };
                let color = pr_color(&pr);
                body.child(icon(IconKind::PullRequest).size(13.0).color(color))
                    .child(label(format!("#{}", pr.number)).size(12.0).color(color))
                    .child(
                        div().row().flex(1.0).child(
                            label(pr.title.clone())
                                .size(12.0)
                                .color(theme().text)
                                .truncate(),
                        ),
                    )
                    .child(
                        label(source.name.clone())
                            .size(11.0)
                            .color(theme().text_muted)
                            .truncate(),
                    )
                    .into()
            }
            Row::Repo { index: repo } => {
                let Some(changes) = repos.get(*repo) else {
                    return div().into();
                };
                let name = self
                    .sources
                    .get(*repo)
                    .map_or_else(String::new, |source| source.name.clone());
                let mut position = Vec::new();
                if changes.ahead > 0 {
                    position.push(format!("{} ahead", changes.ahead));
                }
                if changes.behind > 0 {
                    position.push(format!("{} behind", changes.behind));
                }
                let badge = self.pr_badge(index, *repo);
                let mut title = div()
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
                    );
                if let Some(badge) = badge {
                    title = title.child(badge);
                }
                body.child(title)
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
                let reviewed = self.is_reviewed(*repo, &change.path);
                let toggle = self.id(index, Control::Review);
                let stage = self.id(index, Control::Stage);
                let leading: Node = match self.staging_for(*repo, change) {
                    Some(staging) => {
                        commit_area::stage_checkbox(stage, staging, self.hover == Some(stage))
                    }
                    None => div().w_px(20.0).into(),
                };
                body.child(leading)
                    .child(status_icon(change.status))
                    .child(path_label(change, reviewed))
                    .child(diff_stat(change))
                    .child(review_box(toggle, reviewed, self.hover == Some(toggle)))
                    .into()
            }
        }
    }

    fn pr_badge(&self, row: usize, repo: usize) -> Option<Node> {
        let source = self.sources.get(repo)?;
        let (target, pr) = self.pull_requests.as_ref()?.for_checkout(&source.root)?;
        if pr.is_none() && target.head == source.default_branch {
            return None;
        }
        let colors = theme();
        let id = self.id(row, Control::Review);
        let hot = self.hover == Some(id);
        let (text, color) = match &pr {
            Some(pr) => (format!("#{}", pr.number), pr_color(pr)),
            None => ("Create Pull Request".to_string(), colors.text_accent),
        };
        Some(
            div()
                .row()
                .h_px(18.0)
                .px(5.0)
                .gap(3.0)
                .items_center()
                .rounded(9.0)
                .bg(Rgba::new(
                    color.r,
                    color.g,
                    color.b,
                    if hot { 0.24 } else { 0.14 },
                ))
                .on_click(id)
                .child(icon(IconKind::PullRequest).size(11.0).color(color))
                .child(label(text).size(11.0).color(color))
                .into(),
        )
    }

    fn open_pull_request(&mut self, repo: usize) {
        let (Some(prs), Some(source)) =
            (self.pull_requests.clone(), self.sources.get(repo).cloned())
        else {
            return;
        };
        let Some((target, pr)) = prs.for_checkout(&source.root) else {
            return;
        };
        if pr.is_some() {
            self.requests.push(PanelRequest::Reveal {
                id: pull_request_ui::PullRequests::item_id(&target),
                open: Box::new(move || Some(prs.item(target))),
            });
            return;
        }
        let url = working_copy::remote_url(&source.root, "origin")
            .and_then(|remote| working_copy::create_pull_request_url(&remote, &target.head));
        self.requests.push(match url {
            Some(url) => PanelRequest::OpenUrl(url),
            None => PanelRequest::Toast("Only GitHub remotes can start a pull request here".into()),
        });
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

    /// Marked reviewed, and not changed since.
    fn is_reviewed(&self, repo: usize, path: &str) -> bool {
        let Some(source) = self.sources.get(repo) else {
            return false;
        };
        let key = review_key(&source.name, path);
        let current = self
            .scan
            .lock()
            .ok()
            .and_then(|scan| scan.fingerprints.get(&key).cloned());
        current.is_some() && self.reviewed.get(&key) == current.as_ref()
    }

    fn toggle_reviewed(&mut self, repo: usize, file: usize) {
        let (Some(source), Some((_, change))) = (self.sources.get(repo), self.file(repo, file))
        else {
            return;
        };
        let key = review_key(&source.name, &change.path);
        if self.is_reviewed(repo, &change.path) {
            self.reviewed.remove(&key);
        } else {
            let current = self
                .scan
                .lock()
                .ok()
                .and_then(|scan| scan.fingerprints.get(&key).cloned());
            if let Some(current) = current {
                self.reviewed.insert(key, current);
            }
        }
        self.save_reviews();
    }

    fn save_reviews(&self) {
        let Some(path) = self.reviews_file.as_ref() else {
            return;
        };
        let written = serde_json::to_string_pretty(&self.reviewed)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, text)
            });
        if let Err(error) = written {
            eprintln!("git panel: save reviews: {error}");
        }
    }

    fn content_height(&self) -> f32 {
        self.rows.len() as f32 * ROW_H + 8.0
    }

    fn status_of(&self, repo: usize) -> Vec<StatusEntry> {
        self.scan
            .lock()
            .ok()
            .and_then(|scan| scan.status.get(repo).cloned())
            .unwrap_or_default()
    }

    fn head_of(&self, repo: usize) -> Option<HeadState> {
        self.scan
            .lock()
            .ok()
            .and_then(|scan| scan.heads.get(repo).cloned())
    }

    fn staging_for(&self, repo: usize, change: &FileChange) -> Option<Staging> {
        self.scan.lock().ok().and_then(|scan| {
            scan.status
                .get(repo)?
                .iter()
                .find(|entry| entry.path == change.path)
                .map(StatusEntry::staging)
        })
    }

    fn report(&mut self, result: Result<(), working_copy::CommandError>) {
        if let Err(error) = result {
            self.requests.push(PanelRequest::Toast(error.message));
        }
        self.refresh();
    }

    fn toggle_stage(&mut self, repo: usize, file: usize) {
        let (Some(source), Some((_, change))) =
            (self.sources.get(repo).cloned(), self.file(repo, file))
        else {
            return;
        };
        self.active_repo = repo;
        let mut paths = vec![change.path.clone()];
        paths.extend(change.old_path.clone());
        let result = match self.staging_for(repo, &change) {
            Some(Staging::Staged) => working_copy::unstage(&source.root, &paths),
            _ => working_copy::stage(&source.root, &paths),
        };
        self.report(result);
    }

    fn stages_all(status: &[StatusEntry]) -> bool {
        status.is_empty()
            || status
                .iter()
                .any(|entry| entry.staging() != Staging::Staged)
    }

    fn stage_all(&mut self) {
        let Some(source) = self.sources.get(self.active_repo).cloned() else {
            return;
        };
        let status = self.status_of(self.active_repo);
        let result = if Self::stages_all(&status) {
            let paths: Vec<String> = status
                .iter()
                .filter(|entry| entry.staging() != Staging::Staged)
                .map(|entry| entry.path.clone())
                .collect();
            working_copy::stage(&source.root, &paths)
        } else {
            let paths: Vec<String> = status.iter().map(|entry| entry.path.clone()).collect();
            working_copy::unstage(&source.root, &paths)
        };
        self.report(result);
    }

    fn commit_now(&mut self) {
        let Some(source) = self.sources.get(self.active_repo).cloned() else {
            return;
        };
        let status = self.status_of(self.active_repo);
        if let Some(request) = self.commit.commit(source.root, &status) {
            self.requests.push(request);
        }
    }

    fn remote(&mut self, request: RemoteRequest) {
        if self.commit.busy() {
            return;
        }
        let repo = self.active_repo;
        let (Some(source), Some(head)) = (self.sources.get(repo).cloned(), self.head_of(repo))
        else {
            return;
        };
        let choices = match &request {
            RemoteRequest::Fetch(_) => {
                self.commit
                    .run_remote(source.root, head, request, String::new());
                return;
            }
            RemoteRequest::PushTo { remote, .. } => vec![remote.clone()],
            RemoteRequest::Pull { .. } | RemoteRequest::Push { .. } => {
                let Some(branch) = head.branch.clone() else {
                    self.requests
                        .push(PanelRequest::Toast("No branch is checked out".into()));
                    return;
                };
                match head.upstream_ref.clone() {
                    Some((remote, _)) if head.upstream != Upstream::Gone => vec![remote],
                    _ => working_copy::push_remotes(&source.root, &branch),
                }
            }
        };
        match choices.as_slice() {
            [] => self.requests.push(PanelRequest::Toast(
                "No remote available to push to. Add a remote to be able to publish changes."
                    .into(),
            )),
            [remote] => {
                let remote = remote.clone();
                self.commit.run_remote(source.root, head, request, remote);
            }
            _ => self.pick_remote("Pick which remote to push to", choices, request),
        }
    }

    fn pick_remote(&mut self, message: &str, choices: Vec<String>, request: RemoteRequest) {
        let tag = self.next_tag;
        self.next_tag += 1;
        let mut buttons = choices.clone();
        buttons.push("Cancel".into());
        self.remote_prompt = Some((tag, choices, request));
        self.requests.push(PanelRequest::Prompt {
            tag,
            message: message.to_string(),
            detail: None,
            buttons,
        });
    }

    fn remote_choices(&self) -> Vec<String> {
        self.sources
            .get(self.active_repo)
            .map(|source| working_copy::remotes(&source.root))
            .unwrap_or_default()
    }

    fn primary_remote_action(&mut self) {
        let Some(head) = self.head_of(self.active_repo) else {
            return;
        };
        let request = match head.upstream {
            Upstream::Tracked {
                ahead: 0,
                behind: 0,
            } => RemoteRequest::Fetch(None),
            Upstream::Tracked { behind: 0, .. } | Upstream::None | Upstream::Gone => {
                RemoteRequest::Push { force: false }
            }
            Upstream::Tracked { .. } => RemoteRequest::Pull { rebase: false },
        };
        self.remote(request);
    }

    fn open_footer_menu(&mut self, entries: Vec<(&'static str, MenuAction, bool, bool)>) {
        let items = entries
            .into_iter()
            .enumerate()
            .map(|(index, (text, action, checked, sep))| {
                (
                    MenuItem {
                        id: self.base + MENU_BASE + index as u64,
                        label: text.into(),
                        checked,
                        sep,
                        disabled: false,
                    },
                    action,
                )
            })
            .collect();
        self.menu = Some(OpenMenu {
            repo: self.active_repo,
            file: 0,
            items,
        });
        self.requests.push(PanelRequest::OpenMenu);
    }

    fn footer_hot(&self, offset: u64) -> Option<u64> {
        self.hover
            .filter(|id| *id == self.base + offset)
            .map(|id| id - self.base)
    }

    fn render_footer(&mut self, width: f32) -> Node {
        let colors = theme();
        let repo = self.active_repo.min(self.sources.len().saturating_sub(1));
        self.active_repo = repo;
        let head = self.head_of(repo);
        let status = self.status_of(repo);
        let several = self.sources.len() > 1;
        let hot = |offset: u64| self.footer_hot(offset);
        let mut left = div().row().flex(1.0).gap(2.0).items_center().child(
            icon(IconKind::Branch).size(14.0).color(if several {
                colors.icon_muted
            } else {
                colors.text_disabled
            }),
        );
        if several {
            let name = self
                .sources
                .get(repo)
                .map_or_else(String::new, |source| source.name.clone());
            left = left
                .child(
                    div()
                        .row()
                        .h_px(20.0)
                        .px(4.0)
                        .items_center()
                        .rounded(4.0)
                        .on_click(self.base + SWITCH_REPO)
                        .bg(if hot(SWITCH_REPO).is_some() {
                            colors.ghost_element_hover
                        } else {
                            Rgba::TRANSPARENT
                        })
                        .child(label(name).size(12.0).color(colors.text)),
                )
                .child(label("/").size(12.0).color(Rgba::new(
                    colors.text_muted.r,
                    colors.text_muted.g,
                    colors.text_muted.b,
                    colors.text_muted.a * 0.4,
                )));
        }
        let branch = head
            .as_ref()
            .and_then(|head| head.branch.clone().or_else(|| head.short_sha.clone()))
            .unwrap_or_else(|| " (no branch)".into());
        left = left.child(
            div()
                .row()
                .flex(1.0)
                .px(4.0)
                .items_center()
                .child(label(branch).size(12.0).color(colors.text).truncate()),
        );
        let mut repo_row = div()
            .row()
            .h_px(commit_area::repo_row_height())
            .px(8.0)
            .gap(4.0)
            .items_center()
            .child(left);
        if let Some((text, kind, ahead, behind)) =
            head.as_ref().and_then(commit_area::remote_button_state)
        {
            let mut leading: Vec<Node> = Vec::new();
            if self.commit.remote_running().is_some() {
                leading.push(
                    icon(IconKind::RotateCw)
                        .size(12.0)
                        .color(colors.icon_muted)
                        .into(),
                );
            } else if let Some(kind) = kind {
                leading.push(icon(kind).size(12.0).color(colors.icon_muted).into());
            } else {
                for (count, arrow) in [(behind, IconKind::ArrowDown), (ahead, IconKind::ArrowUp)] {
                    if count > 0 {
                        leading.push(icon(arrow).size(10.0).color(colors.icon_muted).into());
                        leading.push(
                            label(count.to_string())
                                .size(10.0)
                                .color(colors.text)
                                .into(),
                        );
                    }
                }
            }
            repo_row = repo_row.child(commit_area::split_button(
                self.base + REMOTE,
                self.base + REMOTE_MENU,
                leading,
                text,
                !self.commit.busy(),
                false,
                self.hover,
            ));
        }
        let placeholder = commit_area::suggested_message(&status)
            .unwrap_or_else(|| "Enter commit message".into());
        let editor = self
            .commit
            .render_editor(width, &placeholder, self.base + COMMIT_EDITOR);
        let has_head = head.as_ref().is_some_and(|head| head.short_sha.is_some());
        let plan = self.commit.plan(&status, has_head);
        let commit_row = div()
            .row()
            .h_px(commit_area::commit_row_height())
            .px(6.0)
            .items_center()
            .bg(colors.editor_background)
            .child(div().row().flex(1.0))
            .child(commit_area::split_button(
                self.base + COMMIT,
                self.base + COMMIT_MENU,
                Vec::new(),
                plan.title,
                plan.enabled,
                false,
                self.hover,
            ));
        div()
            .col()
            .w_px(width)
            .h_px(self.commit.height())
            .child(repo_row)
            .child(div().h_px(1.0).bg(colors.border))
            .child(editor)
            .child(div().h_px(1.0).bg(colors.border))
            .child(commit_row)
            .into()
    }

    fn apply_menu(&mut self, repo: usize, file: usize, action: MenuAction) {
        let footer_request = match action {
            MenuAction::Amend => {
                if let Some(source) = self.sources.get(self.active_repo) {
                    let root = source.root.clone();
                    self.commit.toggle_amend(&root);
                }
                return;
            }
            MenuAction::Signoff => {
                self.commit.signoff = !self.commit.signoff;
                return;
            }
            MenuAction::SkipHooks => {
                self.commit.skip_hooks = !self.commit.skip_hooks;
                return;
            }
            MenuAction::Fetch => Some(RemoteRequest::Fetch(None)),
            MenuAction::Pull => Some(RemoteRequest::Pull { rebase: false }),
            MenuAction::PullRebase => Some(RemoteRequest::Pull { rebase: true }),
            MenuAction::Push => Some(RemoteRequest::Push { force: false }),
            MenuAction::ForcePush => Some(RemoteRequest::Push { force: true }),
            MenuAction::FetchFrom => {
                let mut remotes = self.remote_choices();
                match remotes.len() {
                    0 => self
                        .requests
                        .push(PanelRequest::Toast("No remotes to fetch from".into())),
                    1 => self.remote(RemoteRequest::Fetch(remotes.pop())),
                    _ => {
                        remotes.push(ALL_REMOTES.into());
                        self.pick_remote(
                            "Fetch from which remote?",
                            remotes,
                            RemoteRequest::Fetch(None),
                        );
                    }
                }
                return;
            }
            MenuAction::PushTo => {
                let remotes = self.remote_choices();
                match remotes.as_slice() {
                    [] => self.requests.push(PanelRequest::Toast(
                        "No remote available to push to. Add a remote to be able to publish changes."
                            .into(),
                    )),
                    [remote] => self.remote(RemoteRequest::PushTo {
                        force: false,
                        remote: remote.clone(),
                    }),
                    _ => self.pick_remote(
                        "Pick which remote to push to",
                        remotes,
                        RemoteRequest::PushTo {
                            force: false,
                            remote: String::new(),
                        },
                    ),
                }
                return;
            }
            _ => None,
        };
        if let Some(request) = footer_request {
            self.remote(request);
            return;
        }
        let Some((path, change)) = self.file(repo, file) else {
            return;
        };
        match action {
            MenuAction::ToggleReviewed => self.toggle_reviewed(repo, file),
            MenuAction::OpenDiff => self.open_diff(repo, file),
            MenuAction::Open => self.requests.push(PanelRequest::OpenFile(path)),
            MenuAction::CopyPath => self
                .requests
                .push(PanelRequest::Copy(path.to_string_lossy().into_owned())),
            MenuAction::CopyRelativePath => self.requests.push(PanelRequest::Copy(change.path)),
            MenuAction::Amend
            | MenuAction::Signoff
            | MenuAction::SkipHooks
            | MenuAction::Fetch
            | MenuAction::FetchFrom
            | MenuAction::Pull
            | MenuAction::PullRebase
            | MenuAction::Push
            | MenuAction::PushTo
            | MenuAction::ForcePush => {}
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

fn pr_color(pr: &pom_forge::PullRequest) -> Rgba {
    let colors = theme();
    match (pr.state.as_str(), pr.is_draft) {
        ("MERGED", _) => workspace::PrSeverity::Merged.color(),
        ("CLOSED", _) => colors.error,
        (_, true) => colors.text_muted,
        _ if pr.conflict || pr.checks == "fail" || pr.review == "changes" => colors.error,
        _ if pr.checks == "pending" || pr.review == "review" => colors.warning,
        _ => colors.success,
    }
}

/// What a repo's uncommitted changes add up to, for its status icon: a conflict, else a deletion, else a
/// modification, else an addition.
fn worst_status(status: &[StatusEntry]) -> Option<ChangeStatus> {
    if status.is_empty() {
        return None;
    }
    let any = |test: &dyn Fn(&StatusEntry) -> bool| status.iter().any(test);
    Some(if any(&|entry| entry.conflicted) {
        ChangeStatus::Conflicted
    } else if any(&|entry| entry.index == 'D' || entry.worktree == 'D') {
        ChangeStatus::Deleted
    } else if any(&|entry| !entry.untracked && (entry.index == 'M' || entry.worktree == 'M')) {
        ChangeStatus::Modified
    } else {
        ChangeStatus::Added
    })
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
/// The "reviewed" checkbox at the end of a file row.
fn review_box(id: u64, checked: bool, hot: bool) -> Node {
    let mut square = div()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(3.0)
        .items_center()
        .justify_center()
        .on_click(id);
    if checked {
        square = square.bg(theme().text_accent).child(
            icon(IconKind::Check)
                .size(10.0)
                .color(theme().editor_background),
        );
    } else {
        square = square.border(
            1.0,
            if hot {
                theme().border_focused
            } else {
                theme().border
            },
        );
    }
    square.into()
}

fn path_label(change: &FileChange, reviewed: bool) -> Node {
    let (folder, name) = match change.path.rsplit_once('/') {
        Some((folder, name)) => (Some(folder.to_string()), name.to_string()),
        None => (None, change.path.clone()),
    };
    let deleted = change.status == ChangeStatus::Deleted;
    let name_color = if deleted {
        theme().text_disabled
    } else if reviewed {
        theme().text_muted
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
        .gap(6.0)
        .items_center()
        .child(label(name).color(name_color).truncate());
    if let Some(folder) = folder {
        row = row.child(
            label(folder)
                .size(12.0)
                .color(folder_color)
                .truncate_start(),
        );
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
        .items_center()
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
        if let Some(outcome) = self.commit.poll() {
            self.requests.extend(commit_area::outcome_requests(outcome));
            self.refresh();
        }
        let footer_h = self.commit.height();
        let selector_h = self.selector_height();
        let list_h = (height - HEADER_H - footer_h - selector_h).max(0.0);
        self.viewport_h = list_h;
        let repos = self.repos();
        self.rebuild_rows(&repos);
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let refresh_id = self.refresh_id();
        let hot = self.hover == Some(refresh_id);
        let total: usize = repos.iter().map(|repo| repo.files.len()).sum();
        let reviewed: usize = repos
            .iter()
            .enumerate()
            .map(|(index, changes)| {
                changes
                    .files
                    .iter()
                    .filter(|file| self.is_reviewed(index, &file.path))
                    .count()
            })
            .sum();
        let status = self.status_of(self.active_repo);
        let stage_all_label = if Self::stages_all(&status) {
            "Stage All"
        } else {
            "Unstage All"
        };
        let stage_all_hot = self.footer_hot(STAGE_ALL).is_some();
        let header = div()
            .row()
            .h_px(HEADER_H)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(label("Git").size(12.0).color(theme().text_muted))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(match (total, reviewed) {
                        (1, 0) => "1 changed file".to_string(),
                        (total, 0) => format!("{total} changed files"),
                        (total, reviewed) => format!("{reviewed} of {total} reviewed"),
                    })
                    .size(11.0)
                    .color(theme().text_placeholder)
                    .truncate(),
                ),
            )
            .child(
                div()
                    .row()
                    .h_px(18.0)
                    .px(6.0)
                    .rounded(4.0)
                    .items_center()
                    .on_click(self.base + STAGE_ALL)
                    .bg(if stage_all_hot {
                        theme().element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(label(stage_all_label).size(12.0).color(theme().text)),
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
        let visible = (list_h / ROW_H).floor() as usize;
        let mut list = div().col().h_px(list_h);
        for (index, row) in self.rows.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.render_row(index, row, &repos));
        }
        let footer = self.render_footer(width);
        let mut panel = div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(list);
        if let Some(selector) = self.render_selector(width) {
            panel = panel.child(selector);
        }
        panel.child(footer).into()
    }

    fn click(&mut self, id: u64) {
        self.commit.focused = false;
        if id == self.refresh_id() {
            self.refresh();
            return;
        }
        match id.checked_sub(self.base) {
            Some(COMMIT_EDITOR) => {
                self.commit.focused = true;
                return;
            }
            Some(COMMIT) => return self.commit_now(),
            Some(COMMIT_MENU) => {
                let has_head = self
                    .head_of(self.active_repo)
                    .is_some_and(|head| head.short_sha.is_some());
                let mut entries = Vec::new();
                if has_head {
                    entries.push(("Amend", MenuAction::Amend, self.commit.amend, false));
                }
                entries.push(("Signoff", MenuAction::Signoff, self.commit.signoff, false));
                entries.push((
                    "Skip Hooks",
                    MenuAction::SkipHooks,
                    self.commit.skip_hooks,
                    false,
                ));
                return self.open_footer_menu(entries);
            }
            Some(REMOTE) => return self.primary_remote_action(),
            Some(REMOTE_MENU) => {
                return self.open_footer_menu(vec![
                    ("Fetch", MenuAction::Fetch, false, false),
                    ("Fetch From", MenuAction::FetchFrom, false, false),
                    ("Pull", MenuAction::Pull, false, false),
                    ("Pull (Rebase)", MenuAction::PullRebase, false, false),
                    ("Push", MenuAction::Push, false, true),
                    ("Push To", MenuAction::PushTo, false, false),
                    ("Force Push", MenuAction::ForcePush, false, false),
                ])
            }
            Some(SWITCH_REPO) => return self.open_selector(),
            Some(SELECTOR_QUERY) => return,
            Some(offset)
                if (SELECTOR_ITEM..SELECTOR_ITEM + SELECTOR_MAX as u64).contains(&offset) =>
            {
                let position = (offset - SELECTOR_ITEM) as usize;
                if let Some(repo) = self.selector_matches().get(position).copied() {
                    self.choose_repo(repo);
                }
                return;
            }
            Some(STAGE_ALL) => return self.stage_all(),
            _ => {}
        }
        let Some((index, control)) = self.decode(id) else {
            return;
        };
        match self.rows.get(index).cloned() {
            Some(Row::Repo { index: repo }) if control == Control::Review => {
                self.active_repo = repo;
                self.open_pull_request(repo)
            }
            Some(Row::Repo { .. }) => {}
            Some(Row::PullRequests { .. }) => self.prs_collapsed = !self.prs_collapsed,
            Some(Row::PullRequest { repo }) => self.open_pull_request(repo),
            Some(Row::File { repo, file }) if control == Control::Review => {
                self.toggle_reviewed(repo, file)
            }
            Some(Row::File { repo, file }) if control == Control::Stage => {
                self.toggle_stage(repo, file)
            }
            Some(Row::File { repo, file }) => {
                self.active_repo = repo;
                self.open_diff(repo, file)
            }
            _ => {}
        }
    }

    fn click_at(&mut self, id: u64, x: f32, y: f32) {
        self.click(id);
        if id == self.base + COMMIT_EDITOR {
            self.commit.editor.click(x - 8.0, y - 8.0);
        }
    }

    fn text_focused(&self) -> bool {
        self.commit.focused || self.selector.is_some()
    }

    fn text(&mut self, text: &str) -> bool {
        if let Some(selector) = self.selector.as_mut() {
            let typed: String = text.chars().filter(|c| !c.is_control()).collect();
            if typed.is_empty() {
                return false;
            }
            selector.query.push_str(&typed);
            selector.highlight = 0;
            return true;
        }
        let typed: String = text
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        if typed.is_empty() {
            return false;
        }
        self.commit.editor.insert(&typed);
        true
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        if self.selector.is_some() {
            let count = self.selector_matches().len();
            let Some(selector) = self.selector.as_mut() else {
                return false;
            };
            match key {
                EditKey::Escape => self.selector = None,
                EditKey::Up => selector.highlight = selector.highlight.saturating_sub(1),
                EditKey::Down => {
                    selector.highlight = (selector.highlight + 1).min(count.saturating_sub(1))
                }
                EditKey::Backspace => {
                    selector.query.pop();
                    selector.highlight = 0;
                }
                EditKey::Enter => {
                    let highlight = selector.highlight;
                    if let Some(repo) = self.selector_matches().get(highlight).copied() {
                        self.choose_repo(repo);
                    }
                }
                _ => return false,
            }
            return true;
        }
        match key {
            EditKey::ReplaceAll if shift && !self.commit.amend => {
                if let Some(source) = self.sources.get(self.active_repo) {
                    let root = source.root.clone();
                    self.commit.toggle_amend(&root);
                }
            }
            EditKey::ReplaceAll => self.commit_now(),
            EditKey::Escape => self.commit.focused = false,
            _ => return self.commit.editor.key(key, shift),
        }
        true
    }

    fn blur(&mut self) {
        self.commit.focused = false;
        self.selector = None;
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let footer = |id: u64| {
            id.checked_sub(self.base)
                .is_some_and(|offset| (FOOTER..FOOTER + 100).contains(&offset))
        };
        let id =
            id.filter(|id| self.decode(*id).is_some() || *id == self.refresh_id() || footer(*id));
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
        let reviewed_label = if self.is_reviewed(repo, &change.path) {
            "Mark as Not Reviewed"
        } else {
            "Mark as Reviewed"
        };
        push(reviewed_label, MenuAction::ToggleReviewed, true);
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
        if let Some((asked, choices, request)) = self.remote_prompt.take() {
            if asked == tag {
                let Some(chosen) = choices.get(answer).cloned() else {
                    return;
                };
                let request = match request {
                    RemoteRequest::Fetch(_) if chosen == ALL_REMOTES => RemoteRequest::Fetch(None),
                    RemoteRequest::Fetch(_) => RemoteRequest::Fetch(Some(chosen.clone())),
                    RemoteRequest::PushTo { force, .. } => RemoteRequest::PushTo {
                        force,
                        remote: chosen.clone(),
                    },
                    other => other,
                };
                let repo = self.active_repo;
                if let (Some(source), Some(head)) =
                    (self.sources.get(repo).cloned(), self.head_of(repo))
                {
                    self.commit.run_remote(source.root, head, request, chosen);
                }
                return;
            }
            self.remote_prompt = Some((asked, choices, request));
        }
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
