//! The Git panel: every repo of the active workspace at once, in three tabs. Changes stages and commits
//! (one commit per repo, same message), Remote shows each branch against origin and main with its pull
//! request and review marks, History lists the branch's commits. Reading git runs on a background thread;
//! the panel refreshes while it is shown.

mod commit_area;
mod render;
mod rows;
mod scan;
mod tree;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commit_area::{CommitArea, CommitTarget, RemoteJob, RemoteRequest};
use git::history::{self, CommitSummary};
use git::working_copy::{self, Staging, Upstream};
use git::{ChangeStatus, FileChange};
use rows::Row;
use scan::{review_key, RepoScan, Scan, Scanner};
use workspace::{EditKey, MenuItem, PaneKind, PanelRequest, SidePanelView};

const ROW_STRIDE: u64 = 8;
/// Ids of the tabs, toolbars and footers start here; rows take the ids below.
const FIXED: u64 = 9_000_000;
const MENU_BASE: u64 = 9_500_000;
const TAB_CHANGES: u64 = FIXED;
const TAB_REMOTE: u64 = FIXED + 1;
const TAB_HISTORY: u64 = FIXED + 2;
const VIEW_DIFF: u64 = FIXED + 3;
const VIEW_OPTIONS: u64 = FIXED + 4;
const STAGE_ALL: u64 = FIXED + 5;
const STAGE_MENU: u64 = FIXED + 6;
const FETCH_ALL: u64 = FIXED + 7;
const OPEN_ALL_DIFFS: u64 = FIXED + 8;
const BRANCH_MENU: u64 = FIXED + 9;
const BRANCH_LINK: u64 = FIXED + 10;
const COMMIT_EDITOR: u64 = FIXED + 11;
const PLAN: u64 = FIXED + 12;
const COMMIT: u64 = FIXED + 13;
const COMMIT_MENU: u64 = FIXED + 14;
const LAST_COMMIT: u64 = FIXED + 15;
const UNCOMMIT: u64 = FIXED + 16;
const SYNC: u64 = FIXED + 17;
const SYNC_MENU: u64 = FIXED + 18;
const BACK: u64 = FIXED + 19;
const DRIFT_CHIP: u64 = FIXED + 20;
const FIXED_END: u64 = FIXED + 100;
/// While shown, the panel re-reads git this often (agents keep changing files).
const REFRESH_EVERY: Duration = Duration::from_secs(4);

/// A repo of the workspace to read.
#[derive(Clone, Debug)]
pub struct RepoSource {
    pub name: String,
    pub root: PathBuf,
    pub default_branch: String,
    /// The workspace's branch, which every repo is expected to have checked out.
    pub expected_branch: String,
    /// This repo stays on another branch on purpose, so being off the workspace's branch is not drift.
    pub kept: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Remote,
    History,
}

/// Which list a file row belongs to, which decides what its checkbox does and what its diff is against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Staged,
    Unstaged,
    /// Uncommitted, not split by staging.
    Working,
    /// Everything the branch changed since it left the default branch.
    Branch,
    /// What one commit changed.
    Commit,
}

impl Scope {
    fn key(self) -> &'static str {
        match self {
            Scope::Staged => "staged",
            Scope::Unstaged => "unstaged",
            Scope::Working => "working",
            Scope::Branch => "branch",
            Scope::Commit => "commit",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    pub staging: Option<Staging>,
}

impl FileEntry {
    fn from_change(change: &FileChange) -> FileEntry {
        FileEntry {
            path: change.path.clone(),
            old_path: change.old_path.clone(),
            status: change.status,
            added: change.added,
            deleted: change.deleted,
            staging: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Main = 0,
    Check = 1,
    Eye = 2,
    Action = 3,
    Menu = 4,
    External = 5,
}

/// A commit shown in place of the tab's list: its files, each opening its diff.
#[derive(Clone, Debug)]
pub(crate) struct OpenedCommit {
    pub repo: usize,
    pub commit: CommitSummary,
    pub files: Vec<FileChange>,
    pub incoming: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuAction {
    Header,
    ViewTree,
    ViewList,
    GroupByStaging,
    GroupNone,
    HistoryByRepo,
    HistoryTimeline,
    StageAll,
    UnstageAll,
    StashAll,
    StashPop,
    DiscardTracked,
    Amend,
    Signoff,
    SkipHooks,
    ToggleTarget(String),
    PickBranch(String),
    KeepBranch(String, String),
    SwitchBranch(String),
    CopyBranch,
    PullAll,
    PushAll,
    FetchAll,
    Remote(usize, RemoteRequest),
    OpenDiff,
    OpenFile,
    ToggleReviewed,
    CopyPath,
    CopyRelativePath,
    Discard,
}

struct OpenMenu {
    file: Option<(usize, Scope, FileEntry)>,
    items: Vec<(MenuItem, MenuAction)>,
}

enum Confirm {
    Discard { repo: usize, path: String },
    DiscardTracked,
}

pub struct GitPanel {
    sources: Arc<Vec<RepoSource>>,
    scan: Arc<Mutex<Arc<Scan>>>,
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
    confirm: Option<(u64, Confirm)>,
    next_tag: u64,
    requests: Vec<PanelRequest>,
    commit: CommitArea,
    pull_requests: Option<pull_request_ui::PullRequests>,
    tab: Tab,
    tree_view: bool,
    group_by_staging: bool,
    timeline: bool,
    folded: HashSet<String>,
    /// Repos left out of the next commit though they have staged files.
    excluded: HashSet<String>,
    /// Repos the user chose to keep on their own branch since the panel opened.
    kept: HashSet<String>,
    opened: Option<OpenedCommit>,
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
            scan: Arc::new(Mutex::new(scan::remembered_scan(&sources))),
            sources: Arc::new(sources),
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
            pull_requests: None,
            tab: Tab::Changes,
            tree_view: true,
            group_by_staging: true,
            timeline: false,
            folded: HashSet::new(),
            excluded: HashSet::new(),
            kept: HashSet::new(),
            opened: None,
            waker,
        }
    }

    pub fn with_pull_requests(mut self, pull_requests: pull_request_ui::PullRequests) -> GitPanel {
        self.pull_requests = Some(pull_requests);
        self
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0.0;
        self.opened = None;
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

    fn current(&self) -> Arc<Scan> {
        self.scan
            .lock()
            .map(|scan| scan.clone())
            .unwrap_or_default()
    }

    fn repo_scan(&self, repo: usize) -> Option<RepoScan> {
        self.current().repos.get(repo).cloned()
    }

    fn repo_index(&self, name: &str) -> Option<usize> {
        self.sources.iter().position(|source| source.name == name)
    }

    fn workspace_branch(&self, scan: &Scan) -> String {
        if let Some(branch) = self
            .sources
            .iter()
            .map(|source| source.expected_branch.as_str())
            .find(|branch| !branch.is_empty())
        {
            return branch.to_string();
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for repo in &scan.repos {
            if let Some(branch) = repo.branch_name() {
                *counts.entry(branch).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(branch, count)| (*count, std::cmp::Reverse(*branch)))
            .map(|(branch, _)| branch.to_string())
            .unwrap_or_default()
    }

    fn is_kept(&self, repo: usize) -> bool {
        self.sources
            .get(repo)
            .is_some_and(|source| source.kept || self.kept.contains(&source.name))
    }

    /// The repo's branch when it is not the workspace's, and whether that is on purpose.
    fn other_branch(&self, scan: &Scan, repo: usize) -> Option<(String, bool)> {
        let workspace_branch = self.workspace_branch(scan);
        let state = scan.repos.get(repo)?;
        let branch = match state.branch_name() {
            Some(branch) => branch.to_string(),
            None => state
                .head
                .as_ref()
                .and_then(|head| head.short_sha.clone())
                .map(|sha| format!("detached at {sha}"))?,
        };
        (branch != workspace_branch && !workspace_branch.is_empty())
            .then(|| (branch, self.is_kept(repo)))
    }

    fn id(&self, row: usize, control: Control) -> u64 {
        self.base + row as u64 * ROW_STRIDE + control as u64
    }

    fn fixed(&self, offset: u64) -> u64 {
        self.base + offset
    }

    fn decode(&self, id: u64) -> Option<(usize, Control)> {
        let offset = id.checked_sub(self.base)?;
        if offset >= FIXED {
            return None;
        }
        let control = match offset % ROW_STRIDE {
            0 => Control::Main,
            1 => Control::Check,
            2 => Control::Eye,
            3 => Control::Action,
            4 => Control::Menu,
            _ => Control::External,
        };
        Some(((offset / ROW_STRIDE) as usize, control))
    }

    fn hot(&self, id: u64) -> bool {
        self.hover == Some(id)
    }

    fn toggle_fold(&mut self, key: String) {
        if !self.folded.remove(&key) {
            self.folded.insert(key);
        }
    }

    fn is_folded(&self, key: &str) -> bool {
        self.folded.contains(key)
    }

    /// Marked reviewed, and not changed since.
    fn is_reviewed(&self, scan: &Scan, repo: usize, path: &str) -> bool {
        let Some(source) = self.sources.get(repo) else {
            return false;
        };
        let key = review_key(&source.name, path);
        let current = scan.fingerprints.get(&key);
        current.is_some() && self.reviewed.get(&key) == current
    }

    fn toggle_reviewed(&mut self, repo: usize, path: &str) {
        let scan = self.current();
        let Some(source) = self.sources.get(repo) else {
            return;
        };
        let key = review_key(&source.name, path);
        if self.is_reviewed(&scan, repo, path) {
            self.reviewed.remove(&key);
        } else if let Some(current) = scan.fingerprints.get(&key) {
            self.reviewed.insert(key, current.clone());
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

    fn report(&mut self, result: Result<(), working_copy::CommandError>) {
        if let Err(error) = result {
            self.requests.push(PanelRequest::Toast(error.message));
        }
        self.refresh();
    }

    fn stage_file(&mut self, repo: usize, scope: Scope, entry: &FileEntry) {
        let Some(source) = self.sources.get(repo).cloned() else {
            return;
        };
        let mut paths = vec![entry.path.clone()];
        paths.extend(entry.old_path.clone());
        let unstage = match scope {
            Scope::Staged => true,
            Scope::Unstaged => false,
            _ => entry.staging == Some(Staging::Staged),
        };
        let result = if unstage {
            working_copy::unstage(&source.root, &paths)
        } else {
            working_copy::stage(&source.root, &paths)
        };
        self.report(result);
    }

    fn stage_directory(&mut self, repo: usize, scope: Scope, path: &str, staging: Staging) {
        let Some(source) = self.sources.get(repo).cloned() else {
            return;
        };
        let unstage = match scope {
            Scope::Staged => true,
            Scope::Unstaged => false,
            _ => staging == Staging::Staged,
        };
        let result = if unstage {
            working_copy::unstage_directory(&source.root, path)
        } else {
            working_copy::stage_directory(&source.root, path)
        };
        self.report(result);
    }

    fn stage_repo(&mut self, repo: usize, stage: bool) {
        let Some(source) = self.sources.get(repo).cloned() else {
            return;
        };
        let result = if stage {
            working_copy::stage_all(&source.root)
        } else {
            working_copy::unstage_all(&source.root)
        };
        self.report(result);
    }

    fn stage_everything(&mut self, stage: bool) {
        let scan = self.current();
        let mut failures = Vec::new();
        for (source, repo) in self.sources.iter().zip(&scan.repos) {
            if repo.status.is_empty() {
                continue;
            }
            let result = if stage {
                working_copy::stage_all(&source.root)
            } else {
                working_copy::unstage_all(&source.root)
            };
            if let Err(error) = result {
                failures.push(format!("{}: {}", source.name, error.message));
            }
        }
        if !failures.is_empty() {
            self.requests.push(PanelRequest::Toast(failures.join("; ")));
        }
        self.refresh();
    }

    fn everything_staged(scan: &Scan) -> bool {
        let mut entries = scan
            .repos
            .iter()
            .flat_map(|repo| repo.status.iter())
            .peekable();
        entries.peek().is_some() && entries.all(|entry| entry.staging() == Staging::Staged)
    }

    fn stash(&mut self, pop: bool) {
        let scan = self.current();
        let mut failures = Vec::new();
        for (source, repo) in self.sources.iter().zip(&scan.repos) {
            let result = if pop {
                if !repo.has_stash {
                    continue;
                }
                working_copy::stash_pop(&source.root)
            } else {
                if repo.status.is_empty() {
                    continue;
                }
                working_copy::stash_all(&source.root)
            };
            if let Err(error) = result {
                failures.push(format!("{}: {}", source.name, error.message));
            }
        }
        if !failures.is_empty() {
            self.requests.push(PanelRequest::Toast(failures.join("; ")));
        }
        self.refresh();
    }

    fn ask(&mut self, confirm: Confirm, message: String, detail: String, button: &str) {
        let tag = self.next_tag;
        self.next_tag += 1;
        self.confirm = Some((tag, confirm));
        self.requests.push(PanelRequest::Prompt {
            tag,
            message,
            detail: Some(detail),
            buttons: vec![button.into(), "Cancel".into()],
        });
    }

    /// The file as a diff: uncommitted work against HEAD, the branch's changes against where it left main.
    fn open_file_diff(&mut self, repo: usize, scope: Scope, entry: &FileEntry) {
        let Some(state) = self.repo_scan(repo) else {
            return;
        };
        if scope == Scope::Commit {
            if let Some(opened) = self.opened.clone() {
                self.open_commit_file(&opened, entry);
            }
            return;
        }
        if entry.status == ChangeStatus::Deleted {
            self.requests
                .push(PanelRequest::Toast(format!("{} was deleted", entry.path)));
            return;
        }
        let root = state.branch.root.clone();
        let base = match entry.status {
            ChangeStatus::Added => None,
            _ => {
                let revision = match scope {
                    Scope::Branch => state.branch.fork_point.as_deref().unwrap_or("HEAD"),
                    _ => "HEAD",
                };
                let earlier = entry.old_path.as_deref().unwrap_or(&entry.path);
                git::file_at(&root, revision, earlier)
            }
        };
        self.requests.push(PanelRequest::OpenDiff {
            path: root.join(&entry.path),
            base,
        });
    }

    fn open_commit(&mut self, repo: usize, commit: CommitSummary, incoming: bool) {
        let Some(source) = self.sources.get(repo) else {
            return;
        };
        let files = history::commit_files(&source.root, &commit.sha);
        self.opened = Some(OpenedCommit {
            repo,
            commit,
            files,
            incoming,
        });
        self.scroll = 0.0;
    }

    /// A read-only tab: the file as the commit left it against its parent's version.
    fn open_commit_file(&mut self, opened: &OpenedCommit, entry: &FileEntry) {
        let Some(source) = self.sources.get(opened.repo) else {
            return;
        };
        let sha = &opened.commit.sha;
        let text = (entry.status != ChangeStatus::Deleted)
            .then(|| git::file_at(&source.root, sha, &entry.path))
            .flatten();
        let base = history::parent_of(&source.root, sha).and_then(|parent| {
            let earlier = entry.old_path.as_deref().unwrap_or(&entry.path);
            git::file_at(&source.root, &parent, earlier)
        });
        let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        self.requests
            .push(PanelRequest::OpenItem(files_ui::revision_diff(
                format!("git-commit:{}:{sha}:{}", source.name, entry.path),
                format!("{name} @ {}", opened.commit.short_sha),
                source.root.clone(),
                &entry.path,
                text,
                base,
            )));
    }

    /// One tab with every repo's patch: uncommitted work against HEAD, or the branch against main.
    fn open_all_diffs(&mut self, uncommitted: bool) {
        let scan = self.current();
        let mut text = String::new();
        for (source, repo) in self.sources.iter().zip(&scan.repos) {
            let against = if uncommitted {
                "HEAD"
            } else {
                match repo.branch.fork_point.as_deref() {
                    Some(fork_point) => fork_point,
                    None => continue,
                }
            };
            let untracked: Vec<&str> = repo
                .status
                .iter()
                .filter(|entry| entry.untracked)
                .map(|entry| entry.path.as_str())
                .collect();
            let patch = history::patch(&source.root, against);
            if patch.is_none() && untracked.is_empty() {
                continue;
            }
            text.push_str(&format!("# {}\n", source.name));
            if let Some(patch) = patch {
                text.push_str(&patch);
                text.push('\n');
            }
            for path in untracked {
                text.push_str(&format!("new file, not added yet: {path}\n"));
            }
            text.push('\n');
        }
        if text.is_empty() {
            self.requests
                .push(PanelRequest::Toast("No changes to show".into()));
            return;
        }
        let (id, title) = if uncommitted {
            ("git-diff:uncommitted", "Uncommitted changes.diff")
        } else {
            ("git-diff:branch", "Branch changes.diff")
        };
        self.requests
            .push(PanelRequest::OpenItem(files_ui::text_tab(
                id.into(),
                title.into(),
                &text,
            )));
    }

    fn commit_targets(&self, scan: &Scan) -> Vec<CommitTarget> {
        self.sources
            .iter()
            .zip(&scan.repos)
            .filter(|(_, repo)| repo.staged_count() > 0)
            .map(|(source, repo)| CommitTarget {
                repo: source.name.clone(),
                root: source.root.clone(),
                staged: repo.staged_count(),
                conflicts: repo.unstaged_conflicts(),
            })
            .collect()
    }

    /// The newest commit at a repo's HEAD, preferring those the branch made itself: `(repo, commit)`.
    fn last_commit(&self, scan: &Scan) -> Option<(usize, CommitSummary)> {
        let heads = scan
            .repos
            .iter()
            .enumerate()
            .filter_map(|(index, repo)| Some((index, repo.head_commit.clone()?, repo)));
        let own: Vec<(usize, CommitSummary)> = heads
            .clone()
            .filter(|(_, commit, repo)| repo.history.iter().any(|mine| mine.sha == commit.sha))
            .map(|(index, commit, _)| (index, commit))
            .collect();
        let pool: Vec<(usize, CommitSummary)> = if own.is_empty() {
            heads.map(|(index, commit, _)| (index, commit)).collect()
        } else {
            own
        };
        pool.into_iter().max_by_key(|(_, commit)| commit.timestamp)
    }

    fn can_uncommit(scan: &Scan, repo: usize, commit: &CommitSummary) -> bool {
        scan.repos
            .get(repo)
            .is_some_and(|state| state.head_has_parent && state.is_unpushed(&commit.sha))
    }

    fn commit_message(&self, targets: &[CommitTarget], scan: &Scan) -> Option<String> {
        let typed = self.commit.editor.text();
        if !typed.trim().is_empty() {
            return Some(typed);
        }
        self.suggestion(targets, scan)
    }

    /// Only a single repo's single staged file names its own commit.
    fn suggestion(&self, targets: &[CommitTarget], scan: &Scan) -> Option<String> {
        let [only] = targets else {
            return None;
        };
        let repo = self.repo_index(&only.repo)?;
        commit_area::suggested_message(&scan.repos.get(repo)?.status)
    }

    fn amend_target(&self, scan: &Scan) -> Option<CommitTarget> {
        let (repo, _) = self.last_commit(scan)?;
        let source = self.sources.get(repo)?;
        Some(CommitTarget {
            repo: source.name.clone(),
            root: source.root.clone(),
            staged: 0,
            conflicts: false,
        })
    }

    fn plan(&self, scan: &Scan) -> commit_area::CommitPlan {
        let staged = self.commit_targets(scan);
        let included: Vec<CommitTarget> = staged
            .iter()
            .filter(|target| !self.excluded.contains(&target.repo))
            .cloned()
            .collect();
        let suggestion = self.suggestion(&included, scan).is_some();
        self.commit.plan(
            &staged,
            &self.excluded,
            suggestion,
            self.amend_target(scan).as_ref(),
        )
    }

    fn commit_now(&mut self) {
        let scan = self.current();
        let plan = self.plan(&scan);
        if !plan.enabled {
            self.requests.push(PanelRequest::Toast(plan.hint));
            return;
        }
        let message = self
            .commit_message(&plan.targets, &scan)
            .or_else(|| (self.commit.amend).then(String::new));
        let Some(message) = message else {
            return;
        };
        self.commit.commit(plan.targets, message);
    }

    fn toggle_amend(&mut self) {
        let scan = self.current();
        if let Some(target) = self.amend_target(&scan) {
            self.commit.toggle_amend(&target.root);
        } else {
            self.commit.amend = false;
        }
    }

    /// Undoes the last commit, keeping its changes staged and its message in the editor.
    fn uncommit(&mut self) {
        let scan = self.current();
        let Some((repo, commit)) = self.last_commit(&scan) else {
            return;
        };
        if !Self::can_uncommit(&scan, repo, &commit) {
            self.requests.push(PanelRequest::Toast(
                "That commit is already pushed, so it stays".into(),
            ));
            return;
        }
        let Some(source) = self.sources.get(repo).cloned() else {
            return;
        };
        let message = working_copy::head_message(&source.root);
        match working_copy::soft_reset(&source.root) {
            Ok(()) => {
                if self.commit.editor.text().trim().is_empty() {
                    if let Some(message) = message {
                        self.commit.editor.set_text(&message);
                    }
                }
                self.requests.push(PanelRequest::Toast(format!(
                    "Uncommitted \"{}\" in {}: its changes are staged again",
                    commit.subject, source.name
                )));
            }
            Err(error) => self.requests.push(PanelRequest::Toast(error.message)),
        }
        self.refresh();
    }

    fn remote_job(&self, repo: usize, request: RemoteRequest) -> Result<RemoteJob, String> {
        let source = self.sources.get(repo).ok_or("No such repository")?;
        let head = self
            .repo_scan(repo)
            .and_then(|state| state.head)
            .unwrap_or_else(|| working_copy::head_state(&source.root));
        let remote = if matches!(request, RemoteRequest::Fetch(_)) {
            String::new()
        } else {
            let branch = head
                .branch
                .clone()
                .ok_or_else(|| format!("{}: no branch is checked out", source.name))?;
            match head.upstream_ref.clone() {
                Some((remote, _)) if head.upstream != Upstream::Gone => remote,
                _ => {
                    let remotes = working_copy::push_remotes(&source.root, &branch);
                    remotes
                        .iter()
                        .find(|remote| remote.as_str() == "origin")
                        .or(remotes.first())
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "{}: no remote to push to. Add a remote to publish changes.",
                                source.name
                            )
                        })?
                }
            }
        };
        Ok(RemoteJob {
            repo: source.name.clone(),
            root: source.root.clone(),
            head,
            request,
            remote,
        })
    }

    fn run_remote(&mut self, work: Vec<(usize, RemoteRequest)>) {
        if self.commit.busy() {
            return;
        }
        let mut jobs = Vec::new();
        let mut problems = Vec::new();
        for (repo, request) in work {
            match self.remote_job(repo, request) {
                Ok(job) => jobs.push(job),
                Err(problem) => problems.push(problem),
            }
        }
        if !problems.is_empty() {
            self.requests.push(PanelRequest::Toast(problems.join("; ")));
        }
        self.commit.run_remote(jobs);
    }

    fn remote_primary(&mut self, repo: usize) {
        let Some(head) = self.repo_scan(repo).and_then(|state| state.head) else {
            return;
        };
        if let Some((_, request, _, _)) = commit_area::remote_action(&head) {
            self.run_remote(vec![(repo, request)]);
        }
    }

    fn fetch_all(&mut self) {
        let work = (0..self.sources.len())
            .map(|repo| (repo, RemoteRequest::Fetch(None)))
            .collect();
        self.run_remote(work);
    }

    /// Pull then push every repo that differs from origin (`pull`/`push` limit it to one direction).
    fn sync_all(&mut self, pull: bool, push: bool) {
        let scan = self.current();
        let mut work = Vec::new();
        for (index, repo) in scan.repos.iter().enumerate() {
            let Some(head) = repo.head.as_ref() else {
                continue;
            };
            let Some((_, request, ahead, behind)) = commit_area::remote_action(head) else {
                continue;
            };
            let request = match (pull, push) {
                (true, true) if matches!(request, RemoteRequest::Fetch(_)) => continue,
                (true, true) => request,
                (true, false) if behind > 0 => RemoteRequest::Pull { rebase: false },
                (false, true)
                    if ahead > 0 || !matches!(repo.upstream(), Upstream::Tracked { .. }) =>
                {
                    RemoteRequest::Push { force: false }
                }
                _ => continue,
            };
            work.push((index, request));
        }
        if work.is_empty() {
            self.requests
                .push(PanelRequest::Toast("Every repo matches origin".into()));
            return;
        }
        self.run_remote(work);
    }

    fn open_pull_request(&mut self, repo: usize) {
        let (Some(prs), Some(source)) =
            (self.pull_requests.clone(), self.sources.get(repo).cloned())
        else {
            return;
        };
        let Some((target, Some(_))) = prs.for_checkout(&source.root) else {
            return;
        };
        self.requests.push(PanelRequest::Reveal {
            id: pull_request_ui::PullRequests::item_id(&target),
            open: Box::new(move || Some(prs.item(target))),
        });
    }

    fn pull_request_in_browser(&mut self, repo: usize) {
        let url = self
            .pull_requests
            .as_ref()
            .zip(self.sources.get(repo))
            .and_then(|(prs, source)| prs.for_checkout(&source.root))
            .and_then(|(_, pr)| pr)
            .map(|pr| pr.url)
            .filter(|url| !url.is_empty());
        if let Some(url) = url {
            self.requests.push(PanelRequest::OpenUrl(url));
        }
    }

    fn create_pull_request(&mut self, repo: usize) {
        let Some(source) = self.sources.get(repo) else {
            return;
        };
        let branch = self
            .repo_scan(repo)
            .and_then(|state| state.branch_name().map(str::to_string));
        let url = branch.and_then(|branch| {
            working_copy::remote_url(&source.root, "origin")
                .and_then(|remote| working_copy::create_pull_request_url(&remote, &branch))
        });
        self.requests.push(match url {
            Some(url) => PanelRequest::OpenUrl(url),
            None => PanelRequest::Toast("Only GitHub remotes can start a pull request here".into()),
        });
    }

    fn open_menu_with(&mut self, entries: Vec<(String, MenuAction, bool, bool)>) {
        self.open_menu_for(None, entries);
        self.requests.push(PanelRequest::OpenMenu);
    }

    fn open_menu_for(
        &mut self,
        file: Option<(usize, Scope, FileEntry)>,
        entries: Vec<(String, MenuAction, bool, bool)>,
    ) {
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
                        disabled: action == MenuAction::Header,
                        danger: false,
                        icon: None,
                        hint: None,
                        color: None,
                        header: false,
                    },
                    action,
                )
            })
            .collect();
        self.menu = Some(OpenMenu { file, items });
    }

    fn view_options_menu(&mut self) {
        let mut entries = Vec::new();
        if self.tab == Tab::History {
            entries.push(("Show".to_string(), MenuAction::Header, false, false));
            entries.push((
                "By Repo".into(),
                MenuAction::HistoryByRepo,
                !self.timeline,
                false,
            ));
            entries.push((
                "One Timeline".into(),
                MenuAction::HistoryTimeline,
                self.timeline,
                false,
            ));
        } else {
            entries.push(("View".to_string(), MenuAction::Header, false, false));
            entries.push(("Tree".into(), MenuAction::ViewTree, self.tree_view, false));
            entries.push(("List".into(), MenuAction::ViewList, !self.tree_view, false));
            if self.tab == Tab::Changes {
                entries.push(("Group By".into(), MenuAction::Header, false, true));
                entries.push((
                    "Staged & Not Staged".into(),
                    MenuAction::GroupByStaging,
                    self.group_by_staging,
                    false,
                ));
                entries.push((
                    "None".into(),
                    MenuAction::GroupNone,
                    !self.group_by_staging,
                    false,
                ));
            }
        }
        self.open_menu_with(entries);
    }

    fn branch_menu(&mut self) {
        let scan = self.current();
        let workspace_branch = self.workspace_branch(&scan);
        let mut entries = vec![(
            "Each repo's branch - click one to use another".to_string(),
            MenuAction::Header,
            false,
            false,
        )];
        for (index, source) in self.sources.iter().enumerate() {
            let state = scan.repos.get(index);
            let branch = state
                .and_then(|state| state.branch_name().map(str::to_string))
                .unwrap_or_else(|| "(no branch)".into());
            let status = state.map_or("not read yet".to_string(), origin_status);
            entries.push((
                format!("{}   {branch}   {status}", source.name),
                MenuAction::PickBranch(source.name.clone()),
                false,
                false,
            ));
        }
        for index in 0..self.sources.len() {
            let Some((branch, false)) = self.other_branch(&scan, index) else {
                continue;
            };
            let Some(source) = self.sources.get(index) else {
                continue;
            };
            let name = source.name.clone();
            entries.push((
                format!("{name} is on {branch}, not {workspace_branch}"),
                MenuAction::Header,
                false,
                true,
            ));
            if scan
                .repos
                .get(index)
                .and_then(RepoScan::branch_name)
                .is_some()
            {
                entries.push((
                    format!("Keep {branch} for {name}"),
                    MenuAction::KeepBranch(name.clone(), branch.clone()),
                    false,
                    false,
                ));
            }
            entries.push((
                format!("Switch {name} to {workspace_branch}"),
                MenuAction::SwitchBranch(name),
                false,
                false,
            ));
        }
        entries.push((
            "Copy Branch Name".into(),
            MenuAction::CopyBranch,
            false,
            true,
        ));
        self.open_menu_with(entries);
    }

    fn targets_menu(&mut self) {
        let scan = self.current();
        let mut entries = vec![(
            "One commit per repo, same message".to_string(),
            MenuAction::Header,
            false,
            false,
        )];
        for target in self.commit_targets(&scan) {
            entries.push((
                format!("{} ({} staged)", target.repo, target.staged),
                MenuAction::ToggleTarget(target.repo.clone()),
                !self.excluded.contains(&target.repo),
                false,
            ));
        }
        self.open_menu_with(entries);
    }

    fn remote_menu(&mut self, repo: usize) {
        let entries = vec![
            (
                "Fetch".to_string(),
                MenuAction::Remote(repo, RemoteRequest::Fetch(None)),
                false,
                false,
            ),
            (
                "Pull".into(),
                MenuAction::Remote(repo, RemoteRequest::Pull { rebase: false }),
                false,
                false,
            ),
            (
                "Pull (Rebase)".into(),
                MenuAction::Remote(repo, RemoteRequest::Pull { rebase: true }),
                false,
                false,
            ),
            (
                "Push".into(),
                MenuAction::Remote(repo, RemoteRequest::Push { force: false }),
                false,
                true,
            ),
            (
                "Force Push".into(),
                MenuAction::Remote(repo, RemoteRequest::Push { force: true }),
                false,
                false,
            ),
        ];
        self.open_menu_with(entries);
    }

    fn apply_menu(&mut self, file: Option<(usize, Scope, FileEntry)>, action: MenuAction) {
        match action {
            MenuAction::Header => {}
            MenuAction::ViewTree => self.tree_view = true,
            MenuAction::ViewList => self.tree_view = false,
            MenuAction::GroupByStaging => self.group_by_staging = true,
            MenuAction::GroupNone => self.group_by_staging = false,
            MenuAction::HistoryByRepo => self.timeline = false,
            MenuAction::HistoryTimeline => self.timeline = true,
            MenuAction::StageAll => self.stage_everything(true),
            MenuAction::UnstageAll => self.stage_everything(false),
            MenuAction::StashAll => self.stash(false),
            MenuAction::StashPop => self.stash(true),
            MenuAction::DiscardTracked => self.ask(
                Confirm::DiscardTracked,
                "Discard every tracked change in every repo?".into(),
                "Staged and unstaged changes to tracked files are lost; new files stay.".into(),
                "Discard",
            ),
            MenuAction::Amend => self.toggle_amend(),
            MenuAction::Signoff => self.commit.signoff = !self.commit.signoff,
            MenuAction::SkipHooks => self.commit.skip_hooks = !self.commit.skip_hooks,
            MenuAction::ToggleTarget(repo) => {
                if !self.excluded.remove(&repo) {
                    self.excluded.insert(repo);
                }
            }
            MenuAction::PickBranch(repo) => self.requests.push(PanelRequest::PickBranch { repo }),
            MenuAction::KeepBranch(repo, branch) => {
                self.kept.insert(repo.clone());
                self.requests
                    .push(PanelRequest::KeepBranch { repo, branch });
            }
            MenuAction::SwitchBranch(repo) => {
                self.requests.push(PanelRequest::SwitchBranch { repo })
            }
            MenuAction::CopyBranch => {
                let branch = self.workspace_branch(&self.current());
                self.requests.push(PanelRequest::Copy(branch));
            }
            MenuAction::PullAll => self.sync_all(true, false),
            MenuAction::PushAll => self.sync_all(false, true),
            MenuAction::FetchAll => self.fetch_all(),
            MenuAction::Remote(repo, request) => self.run_remote(vec![(repo, request)]),
            MenuAction::OpenDiff
            | MenuAction::OpenFile
            | MenuAction::ToggleReviewed
            | MenuAction::CopyPath
            | MenuAction::CopyRelativePath
            | MenuAction::Discard => {
                if let Some((repo, scope, entry)) = file {
                    self.apply_file_action(repo, scope, &entry, action);
                }
            }
        }
    }

    fn apply_file_action(
        &mut self,
        repo: usize,
        scope: Scope,
        entry: &FileEntry,
        action: MenuAction,
    ) {
        let Some(source) = self.sources.get(repo).cloned() else {
            return;
        };
        let path = source.root.join(&entry.path);
        match action {
            MenuAction::OpenDiff => self.open_file_diff(repo, scope, entry),
            MenuAction::OpenFile => self.requests.push(PanelRequest::OpenFile(path)),
            MenuAction::ToggleReviewed => self.toggle_reviewed(repo, &entry.path),
            MenuAction::CopyPath => self
                .requests
                .push(PanelRequest::Copy(path.to_string_lossy().into_owned())),
            MenuAction::CopyRelativePath => {
                self.requests.push(PanelRequest::Copy(entry.path.clone()))
            }
            MenuAction::Discard => {
                let detail = if entry.status == ChangeStatus::Added {
                    "The file is not committed, so it will be deleted."
                } else {
                    "Its staged and unstaged changes are lost; the branch's commits stay."
                };
                self.ask(
                    Confirm::Discard {
                        repo,
                        path: entry.path.clone(),
                    },
                    format!("Discard uncommitted changes to {}?", entry.path),
                    detail.into(),
                    "Discard",
                );
            }
            _ => {}
        }
    }

    fn click_row(&mut self, index: usize, control: Control) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        let scan = self.current();
        match row {
            Row::RepoHeader { repo, history } => {
                let name = self
                    .sources
                    .get(repo)
                    .map(|source| source.name.clone())
                    .unwrap_or_default();
                if control == Control::Check {
                    let all_staged = scan.repos.get(repo).is_some_and(|state| {
                        state
                            .status
                            .iter()
                            .all(|entry| entry.staging() == Staging::Staged)
                    });
                    self.stage_repo(repo, !all_staged);
                } else {
                    self.toggle_fold(format!("{}:{name}", if history { "h" } else { "c" }));
                }
            }
            Row::Section { repo, staged, .. } => {
                if control == Control::Check {
                    self.stage_repo(repo, !staged);
                } else {
                    let name = self
                        .sources
                        .get(repo)
                        .map(|source| source.name.clone())
                        .unwrap_or_default();
                    self.toggle_fold(format!("s:{name}:{staged}"));
                }
            }
            Row::Directory {
                repo,
                scope,
                path,
                staging,
                ..
            } => {
                if control == Control::Check {
                    if let Some(staging) = staging {
                        self.stage_directory(repo, scope, &path, staging);
                    }
                } else {
                    let name = self
                        .sources
                        .get(repo)
                        .map(|source| source.name.clone())
                        .unwrap_or_default();
                    self.toggle_fold(format!("d:{}:{name}:{path}", scope.key()));
                }
            }
            Row::File {
                repo, scope, entry, ..
            } => match control {
                Control::Check => self.stage_file(repo, scope, &entry),
                Control::Eye => self.toggle_reviewed(repo, &entry.path),
                _ => self.open_file_diff(repo, scope, &entry),
            },
            Row::Card { repo } => match control {
                Control::Action => self.remote_primary(repo),
                Control::Menu => self.remote_menu(repo),
                _ => {
                    let name = self
                        .sources
                        .get(repo)
                        .map(|source| source.name.clone())
                        .unwrap_or_default();
                    self.toggle_fold(format!("r:{name}"));
                }
            },
            Row::PullRequest { repo, .. } => {
                if control == Control::External {
                    self.pull_request_in_browser(repo);
                } else {
                    self.open_pull_request(repo);
                }
            }
            Row::CreatePullRequest { repo } => self.create_pull_request(repo),
            Row::CommitGroup { repo, incoming, .. } => {
                let name = self
                    .sources
                    .get(repo)
                    .map(|source| source.name.clone())
                    .unwrap_or_default();
                self.toggle_fold(format!("{}:{name}", if incoming { "in" } else { "out" }));
            }
            Row::FilesHeader { repo, .. } => {
                let name = self
                    .sources
                    .get(repo)
                    .map(|source| source.name.clone())
                    .unwrap_or_default();
                self.toggle_fold(format!("rf:{name}"));
            }
            Row::Commit {
                repo,
                commit,
                incoming,
                ..
            } => self.open_commit(repo, commit, incoming),
            Row::Message { .. }
            | Row::Progress { .. }
            | Row::Gap
            | Row::CardNote { .. }
            | Row::Day(_)
            | Row::CommitHeader => {}
        }
    }

    fn click_fixed(&mut self, offset: u64) -> bool {
        match offset {
            TAB_CHANGES => self.set_tab(Tab::Changes),
            TAB_REMOTE => self.set_tab(Tab::Remote),
            TAB_HISTORY => self.set_tab(Tab::History),
            VIEW_DIFF => self.open_all_diffs(true),
            OPEN_ALL_DIFFS => self.open_all_diffs(false),
            VIEW_OPTIONS => self.view_options_menu(),
            STAGE_ALL => {
                let everything = Self::everything_staged(&self.current());
                self.stage_everything(!everything);
            }
            STAGE_MENU => self.open_menu_with(vec![
                ("Stage All".into(), MenuAction::StageAll, false, false),
                ("Unstage All".into(), MenuAction::UnstageAll, false, false),
                ("Stash All".into(), MenuAction::StashAll, false, true),
                ("Stash Pop".into(), MenuAction::StashPop, false, false),
                (
                    "Discard Tracked Changes".into(),
                    MenuAction::DiscardTracked,
                    false,
                    true,
                ),
            ]),
            FETCH_ALL => self.fetch_all(),
            BRANCH_MENU | DRIFT_CHIP => self.branch_menu(),
            BRANCH_LINK => self.set_tab(Tab::Remote),
            COMMIT_EDITOR => self.commit.focused = true,
            PLAN => self.targets_menu(),
            COMMIT => self.commit_now(),
            COMMIT_MENU => {
                let has_commit = self.last_commit(&self.current()).is_some();
                let mut entries = vec![(
                    "Commit Options".to_string(),
                    MenuAction::Header,
                    false,
                    false,
                )];
                if has_commit {
                    entries.push(("Amend".into(), MenuAction::Amend, self.commit.amend, false));
                }
                entries.push((
                    "Signoff".into(),
                    MenuAction::Signoff,
                    self.commit.signoff,
                    false,
                ));
                entries.push((
                    "Skip Hooks".into(),
                    MenuAction::SkipHooks,
                    self.commit.skip_hooks,
                    false,
                ));
                self.open_menu_with(entries);
            }
            LAST_COMMIT => {
                if let Some((repo, commit)) = self.last_commit(&self.current()) {
                    self.open_commit(repo, commit, false);
                }
            }
            UNCOMMIT => self.uncommit(),
            SYNC => self.sync_all(true, true),
            SYNC_MENU => self.open_menu_with(vec![
                ("Pull All".into(), MenuAction::PullAll, false, false),
                ("Push All".into(), MenuAction::PushAll, false, false),
                ("Fetch All".into(), MenuAction::FetchAll, false, false),
            ]),
            BACK => {
                self.opened = None;
                self.scroll = 0.0;
            }
            _ => return false,
        }
        true
    }
}

fn origin_status(repo: &RepoScan) -> String {
    match repo.upstream() {
        Upstream::None => "not published".into(),
        Upstream::Gone => "gone from origin".into(),
        Upstream::Tracked {
            ahead: 0,
            behind: 0,
        } => "up to date".into(),
        Upstream::Tracked { ahead, behind: 0 } => format!("{ahead} to push"),
        Upstream::Tracked { ahead: 0, behind } => format!("{behind} to pull"),
        Upstream::Tracked { ahead, behind } => format!("{ahead} to push, {behind} to pull"),
    }
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

    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        if let Ok(mut drawn) = self.drawn_at.lock() {
            *drawn = Some(Instant::now());
        }
        if !self.started {
            self.started = true;
            self.refresh();
            self.spawn_poller();
        }
        if let Some(outcome) = self.commit.poll() {
            if matches!(outcome, commit_area::Outcome::Committed) {
                self.excluded.clear();
            }
            self.requests.extend(commit_area::outcome_requests(outcome));
            self.refresh();
        }
        self.render_panel(width, height)
    }

    fn click(&mut self, id: u64) {
        self.commit.focused = false;
        if let Some(offset) = id.checked_sub(self.base) {
            if (FIXED..FIXED_END).contains(&offset) && self.click_fixed(offset) {
                return;
            }
        }
        if let Some((index, control)) = self.decode(id) {
            self.click_row(index, control);
        }
    }

    fn click_at(&mut self, id: u64, x: f32, y: f32) {
        self.click(id);
        if id == self.fixed(COMMIT_EDITOR) {
            self.commit.editor.click(x - 8.0, y - 8.0);
        }
    }

    fn text_focused(&self) -> bool {
        self.commit.focused
    }

    fn text(&mut self, text: &str) -> bool {
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
        match key {
            EditKey::ReplaceAll if shift && !self.commit.amend => self.toggle_amend(),
            EditKey::ReplaceAll => self.commit_now(),
            EditKey::Escape => self.commit.focused = false,
            _ => return self.commit.editor.key(key, shift),
        }
        true
    }

    fn blur(&mut self) {
        self.commit.focused = false;
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let ours = |id: u64| {
            id.checked_sub(self.base)
                .is_some_and(|offset| offset < FIXED_END)
        };
        let id = id.filter(|id| ours(*id));
        if self.hover == id {
            return false;
        }
        self.hover = id;
        true
    }

    fn scroll(&mut self, dy: f32) -> bool {
        let max_scroll = (rows::content_height(&self.rows) - self.viewport_h).max(0.0);
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
        let Some(Row::File {
            repo, scope, entry, ..
        }) = self.rows.get(index).cloned()
        else {
            return false;
        };
        let scan = self.current();
        let mut entries = Vec::new();
        if entry.status != ChangeStatus::Deleted {
            entries.push(("Open Diff".to_string(), MenuAction::OpenDiff, false, false));
            if scope != Scope::Commit {
                entries.push(("Open File".into(), MenuAction::OpenFile, false, false));
            }
        }
        if scope == Scope::Branch {
            let reviewed_label = if self.is_reviewed(&scan, repo, &entry.path) {
                "Mark as Not Reviewed"
            } else {
                "Mark as Reviewed"
            };
            entries.push((
                reviewed_label.into(),
                MenuAction::ToggleReviewed,
                false,
                true,
            ));
        }
        entries.push(("Copy Path".into(), MenuAction::CopyPath, false, true));
        entries.push((
            "Copy Relative Path".into(),
            MenuAction::CopyRelativePath,
            false,
            false,
        ));
        let uncommitted = matches!(scope, Scope::Staged | Scope::Unstaged | Scope::Working)
            || scan
                .repos
                .get(repo)
                .and_then(|state| state.uncommitted_file(&entry.path))
                .is_some();
        if uncommitted && scope != Scope::Commit {
            entries.push((
                "Discard Uncommitted Changes".into(),
                MenuAction::Discard,
                false,
                true,
            ));
        }
        self.open_menu_for(Some((repo, scope, entry)), entries);
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
            .map(|(_, action)| action.clone())
        {
            self.apply_menu(menu.file, action);
        }
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }

    fn prompt_answered(&mut self, tag: u64, answer: usize) {
        let Some((asked, confirm)) = self.confirm.take() else {
            return;
        };
        if asked != tag {
            self.confirm = Some((asked, confirm));
            return;
        }
        if answer != 0 {
            return;
        }
        match confirm {
            Confirm::Discard { repo, path } => {
                let Some(source) = self.sources.get(repo).cloned() else {
                    return;
                };
                if let Err(error) = git::discard_uncommitted(&source.root, &path) {
                    self.requests.push(PanelRequest::Toast(format!(
                        "Could not discard {path}: {error}"
                    )));
                }
            }
            Confirm::DiscardTracked => {
                let sources = self.sources.clone();
                let scan = self.current();
                let failures: Vec<String> = sources
                    .iter()
                    .zip(&scan.repos)
                    .filter(|(_, repo)| repo.head_commit.is_some() && !repo.status.is_empty())
                    .filter_map(|(source, _)| {
                        working_copy::discard_tracked(&source.root)
                            .err()
                            .map(|error| format!("{}: {}", source.name, error.message))
                    })
                    .collect();
                if !failures.is_empty() {
                    self.requests.push(PanelRequest::Toast(failures.join("; ")));
                }
            }
        }
        self.refresh();
    }
}
