//! Everything the Git panel shows of one repo, read in one go: the branch against its default branch, the
//! uncommitted files, the head and its upstream, the branch's commits and a fingerprint of each changed file.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::history::{self, CommitSummary};
use crate::working_copy::{self, HeadState, Staging, StatusEntry, Upstream};
use crate::{FileChange, RepoChanges};

/// Lists longer than this are cut, so one repo always fits in a message to a remote app.
pub const LIST_LIMIT: usize = 3000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoScan {
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
    /// Path of each file the branch changed -> fingerprint of the file as it is now.
    pub fingerprints: BTreeMap<String, String>,
    /// Some lists were longer than [`LIST_LIMIT`] and were cut.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

/// FNV-1a over the file's bytes: stable across runs and machines, which a review mark must be.
pub fn fingerprint(path: &Path) -> String {
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

impl RepoScan {
    /// Reads the repo at `root`, its branch compared with `default_branch`.
    pub fn read(root: &Path, default_branch: &str) -> RepoScan {
        let branch = crate::branch_changes(root, default_branch);
        let base = branch.base.clone();
        let history = if branch.fork_point.is_some() {
            history::commits_since(root, &base)
        } else {
            Vec::new()
        };
        let head_commit = history::head_commit(root);
        let head_has_parent = head_commit
            .as_ref()
            .is_some_and(|commit| history::parent_of(root, &commit.sha).is_some());
        let fingerprints = branch
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    fingerprint(&branch.root.join(&file.path)),
                )
            })
            .collect();
        let mut scan = RepoScan {
            uncommitted: crate::uncommitted_changes(root).unwrap_or_default(),
            status: working_copy::status(root).unwrap_or_default(),
            head: Some(working_copy::head_state(root)),
            outgoing: history::outgoing(root),
            incoming: history::incoming(root),
            has_stash: working_copy::has_stash(root),
            history,
            head_commit,
            head_has_parent,
            branch,
            fingerprints,
            cut: false,
        };
        scan.cut_to(LIST_LIMIT);
        scan
    }

    /// A repo that could not be read, saying why.
    pub fn failed(root: &Path, error: impl Into<String>) -> RepoScan {
        RepoScan {
            branch: RepoChanges {
                root: root.to_path_buf(),
                error: Some(error.into()),
                ..RepoChanges::default()
            },
            ..RepoScan::default()
        }
    }

    fn cut_to(&mut self, limit: usize) {
        let mut cut = false;
        if self.branch.files.len() > limit {
            self.branch.files.truncate(limit);
            let kept: std::collections::HashSet<&str> = self
                .branch
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect();
            self.fingerprints
                .retain(|path, _| kept.contains(path.as_str()));
            cut = true;
        }
        if self.uncommitted.len() > limit {
            self.uncommitted.truncate(limit);
            cut = true;
        }
        if self.status.len() > limit {
            self.status.truncate(limit);
            cut = true;
        }
        self.cut |= cut;
    }

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
