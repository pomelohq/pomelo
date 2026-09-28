use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use git::history::{self, CommitSummary};
use git::working_copy::{self, HeadState, Staging, StatusEntry, Upstream};
use git::{FileChange, RepoChanges};

use crate::RepoSource;

/// One repo as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RepoScan {
    /// Everything the branch changed since it left the default branch, committed or not.
    pub branch: RepoChanges,
    /// Uncommitted files against HEAD, with line counts.
    pub uncommitted: Vec<FileChange>,
    pub status: Vec<StatusEntry>,
    pub head: Option<HeadState>,
    /// The branch's own commits since it left the default branch, newest first.
    pub history: Vec<CommitSummary>,
    /// Commits not on the upstream yet; `None` when the branch tracks nothing.
    pub outgoing: Option<Vec<CommitSummary>>,
    /// Commits on the upstream that are not pulled yet (as of the last fetch).
    pub incoming: Vec<CommitSummary>,
    pub head_commit: Option<CommitSummary>,
    pub head_has_parent: bool,
    pub has_stash: bool,
}

impl RepoScan {
    pub fn branch_name(&self) -> Option<&str> {
        self.head.as_ref()?.branch.as_deref()
    }

    pub fn upstream(&self) -> Upstream {
        self.head
            .as_ref()
            .map_or(Upstream::None, |head| head.upstream)
    }

    /// Needs a push, a pull or a first publish.
    pub fn differs_from_origin(&self) -> bool {
        self.branch_name().is_some()
            && match self.upstream() {
                Upstream::None | Upstream::Gone => true,
                Upstream::Tracked { ahead, behind } => ahead > 0 || behind > 0,
            }
    }

    /// A commit that no remote has yet (every commit, while the branch is not published).
    pub fn is_unpushed(&self, sha: &str) -> bool {
        match &self.outgoing {
            Some(outgoing) => outgoing.iter().any(|commit| commit.sha == sha),
            None => true,
        }
    }

    pub fn unpushed(&self) -> usize {
        match &self.outgoing {
            Some(outgoing) => outgoing.len(),
            None => self.history.len(),
        }
    }

    pub fn staged_count(&self) -> usize {
        self.status
            .iter()
            .filter(|entry| entry.staging() != Staging::Unstaged)
            .count()
    }

    pub fn unstaged_conflicts(&self) -> bool {
        self.status
            .iter()
            .any(|entry| entry.conflicted && entry.staging() != Staging::Staged)
    }

    pub fn uncommitted_file(&self, path: &str) -> Option<&FileChange> {
        self.uncommitted.iter().find(|file| file.path == path)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Scan {
    pub repos: Vec<RepoScan>,
    /// `repo/path` -> fingerprint of the file as it is now, to tell whether a review still holds.
    pub fingerprints: HashMap<String, String>,
    pub loaded: bool,
}

/// FNV-1a over the file's bytes: stable across runs, which a review mark must be.
fn fingerprint(path: &Path) -> String {
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

pub(crate) fn review_key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
}

/// The last read of each workspace (by its repo roots), so a panel rebuilt on switching back shows it at once
/// and refreshes in the background instead of starting blank.
fn last_scans() -> &'static Mutex<HashMap<Vec<PathBuf>, Arc<Scan>>> {
    static SCANS: OnceLock<Mutex<HashMap<Vec<PathBuf>, Arc<Scan>>>> = OnceLock::new();
    SCANS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn scan_key(sources: &[RepoSource]) -> Vec<PathBuf> {
    sources.iter().map(|source| source.root.clone()).collect()
}

pub(crate) fn remembered_scan(sources: &[RepoSource]) -> Arc<Scan> {
    last_scans()
        .lock()
        .ok()
        .and_then(|last| last.get(&scan_key(sources)).cloned())
        .unwrap_or_default()
}

fn read_repo(source: &RepoSource) -> RepoScan {
    let branch = git::branch_changes(&source.root, &source.default_branch);
    let base = branch.base.clone();
    let history = if branch.fork_point.is_some() {
        history::commits_since(&source.root, &base)
    } else {
        Vec::new()
    };
    let head_commit = history::head_commit(&source.root);
    let head_has_parent = head_commit
        .as_ref()
        .is_some_and(|commit| history::parent_of(&source.root, &commit.sha).is_some());
    RepoScan {
        uncommitted: git::uncommitted_changes(&source.root).unwrap_or_default(),
        status: working_copy::status(&source.root).unwrap_or_default(),
        head: Some(working_copy::head_state(&source.root)),
        outgoing: history::outgoing(&source.root),
        incoming: history::incoming(&source.root),
        has_stash: working_copy::has_stash(&source.root),
        history,
        head_commit,
        head_has_parent,
        branch,
    }
}

/// Reads every repo and publishes the result; wakes the UI only when something changed.
pub(crate) struct Scanner {
    pub sources: Arc<Vec<RepoSource>>,
    pub scan: Arc<Mutex<Arc<Scan>>>,
    pub scanning: Arc<AtomicBool>,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl Scanner {
    pub fn run(&self) {
        // Repos are read side by side: one after another, a few of them keep the panel waiting over a second.
        let repos: Vec<RepoScan> = std::thread::scope(|threads| {
            let handles: Vec<_> = self
                .sources
                .iter()
                .map(|source| threads.spawn(move || read_repo(source)))
                .collect();
            handles
                .into_iter()
                .zip(self.sources.iter())
                .map(|(handle, source)| {
                    handle.join().unwrap_or_else(|_| RepoScan {
                        branch: RepoChanges {
                            root: source.root.clone(),
                            error: Some("reading git failed".into()),
                            ..RepoChanges::default()
                        },
                        ..RepoScan::default()
                    })
                })
                .collect()
        });
        let fingerprints: HashMap<String, String> = self
            .sources
            .iter()
            .zip(&repos)
            .flat_map(|(source, repo)| {
                repo.branch.files.iter().map(|file| {
                    (
                        review_key(&source.name, &file.path),
                        fingerprint(&repo.branch.root.join(&file.path)),
                    )
                })
            })
            .collect();
        let changed = self.scan.lock().is_ok_and(|mut scan| {
            let changed = !scan.loaded || scan.repos != repos || scan.fingerprints != fingerprints;
            let next = Arc::new(Scan {
                repos,
                fingerprints,
                loaded: true,
            });
            if let Ok(mut last) = last_scans().lock() {
                last.insert(scan_key(&self.sources), next.clone());
            }
            *scan = next;
            changed
        });
        self.scanning.store(false, Ordering::Release);
        if changed {
            (self.waker)();
        }
    }
}
