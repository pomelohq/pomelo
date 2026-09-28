use git::history::CommitSummary;
use git::working_copy::{Staging, StatusEntry};
use git::ChangeStatus;

use crate::scan::{RepoScan, Scan};
use crate::tree::{self, TreeItem};
use crate::widgets::ROW_H;
use crate::{FileEntry, GitPanel, Scope, Tab};

pub(crate) const COMMIT_ROW_H: f32 = 44.0;
pub(crate) const CARD_HEADER_H: f32 = 32.0;
pub(crate) const PULL_REQUEST_H: f32 = 48.0;
pub(crate) const PROGRESS_H: f32 = 40.0;
pub(crate) const GAP_H: f32 = 6.0;
pub(crate) const DAY_H: f32 = 24.0;
pub(crate) const COMMIT_HEADER_H: f32 = 56.0;

#[derive(Clone, Debug)]
pub(crate) enum Row {
    Message {
        text: String,
        indent: f32,
    },
    RepoHeader {
        repo: usize,
        history: bool,
    },
    Section {
        repo: usize,
        staged: bool,
        count: usize,
    },
    Directory {
        repo: usize,
        scope: Scope,
        path: String,
        label: String,
        depth: usize,
        staging: Option<Staging>,
        left_to_review: Option<usize>,
    },
    File {
        repo: usize,
        scope: Scope,
        entry: FileEntry,
        depth: usize,
        flat: bool,
    },
    Progress {
        reviewed: usize,
        total: usize,
    },
    Gap,
    Card {
        repo: usize,
    },
    PullRequest {
        repo: usize,
        pull_request: Box<pom_forge::PullRequest>,
    },
    CreatePullRequest {
        repo: usize,
    },
    CardNote {
        repo: usize,
        text: String,
    },
    CommitGroup {
        repo: usize,
        incoming: bool,
        count: usize,
        published: bool,
    },
    FilesHeader {
        repo: usize,
        added: u32,
        deleted: u32,
        count: usize,
    },
    Commit {
        repo: usize,
        commit: CommitSummary,
        incoming: bool,
        with_repo: bool,
        indent: f32,
        in_card: bool,
    },
    Day(&'static str),
    CommitHeader,
}

impl Row {
    pub fn height(&self) -> f32 {
        match self {
            Row::Progress { .. } => PROGRESS_H,
            Row::Gap => GAP_H,
            Row::Card { .. } => CARD_HEADER_H,
            Row::PullRequest { .. } => PULL_REQUEST_H,
            Row::Commit { .. } => COMMIT_ROW_H,
            Row::Day(_) => DAY_H,
            Row::CommitHeader => COMMIT_HEADER_H,
            _ => ROW_H,
        }
    }

    pub fn card(&self) -> Option<usize> {
        match self {
            Row::Card { repo }
            | Row::PullRequest { repo, .. }
            | Row::CreatePullRequest { repo }
            | Row::CardNote { repo, .. }
            | Row::CommitGroup { repo, .. }
            | Row::FilesHeader { repo, .. } => Some(*repo),
            Row::Commit {
                repo,
                in_card: true,
                ..
            } => Some(*repo),
            Row::File {
                repo,
                scope: Scope::Branch,
                ..
            }
            | Row::Directory {
                repo,
                scope: Scope::Branch,
                ..
            } => Some(*repo),
            _ => None,
        }
    }
}

pub(crate) fn content_height(rows: &[Row]) -> f32 {
    rows.iter().map(Row::height).sum::<f32>() + 8.0
}

fn status_of(column: char, entry: &StatusEntry) -> ChangeStatus {
    if entry.conflicted {
        return ChangeStatus::Conflicted;
    }
    if entry.untracked {
        return ChangeStatus::Added;
    }
    match column {
        'A' | '?' => ChangeStatus::Added,
        'D' => ChangeStatus::Deleted,
        'R' | 'C' => ChangeStatus::Renamed,
        _ => ChangeStatus::Modified,
    }
}

/// A working-copy file as `scope` shows it: which column its status comes from, and its line counts.
fn working_entry(repo: &RepoScan, entry: &StatusEntry, scope: Scope) -> FileEntry {
    let column = match scope {
        Scope::Staged => entry.index,
        Scope::Unstaged => entry.worktree,
        _ if entry.index != '.' => entry.index,
        _ => entry.worktree,
    };
    let counts = repo.uncommitted_file(&entry.path);
    FileEntry {
        path: entry.path.clone(),
        old_path: entry.original_path.clone(),
        status: status_of(column, entry),
        added: counts.and_then(|file| file.added),
        deleted: counts.and_then(|file| file.deleted),
        staging: Some(entry.staging()),
    }
}

fn combined(stagings: impl Iterator<Item = Staging>) -> Staging {
    let (mut any_staged, mut any_unstaged) = (false, false);
    for staging in stagings {
        match staging {
            Staging::Staged => any_staged = true,
            Staging::Unstaged => any_unstaged = true,
            Staging::Partial => return Staging::Partial,
        }
    }
    match (any_staged, any_unstaged) {
        (true, false) => Staging::Staged,
        (false, _) => Staging::Unstaged,
        (true, true) => Staging::Partial,
    }
}

pub(crate) fn repo_staging(repo: &RepoScan) -> Staging {
    combined(repo.status.iter().map(StatusEntry::staging))
}

impl GitPanel {
    fn source_name(&self, repo: usize) -> String {
        self.sources
            .get(repo)
            .map(|source| source.name.clone())
            .unwrap_or_default()
    }

    fn file_rows(
        &self,
        scan: &Scan,
        repo: usize,
        scope: Scope,
        entries: Vec<FileEntry>,
        depth: usize,
        rows: &mut Vec<Row>,
    ) {
        if !self.tree_view || scope == Scope::Commit {
            rows.extend(entries.into_iter().map(|entry| Row::File {
                repo,
                scope,
                entry,
                depth,
                flat: true,
            }));
            return;
        }
        let name = self.source_name(repo);
        let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        let folded = |path: &str| self.is_folded(&format!("d:{}:{name}:{path}", scope.key()));
        for item in tree::build(&paths, depth, &folded) {
            match item {
                TreeItem::Directory {
                    path,
                    label,
                    depth,
                    files,
                } => {
                    let members = files.iter().filter_map(|index| entries.get(*index));
                    let (staging, left_to_review) = if scope == Scope::Branch {
                        let left = members
                            .filter(|entry| !self.is_reviewed(scan, repo, &entry.path))
                            .count();
                        (None, Some(left))
                    } else {
                        (
                            Some(combined(members.filter_map(|entry| entry.staging))),
                            None,
                        )
                    };
                    rows.push(Row::Directory {
                        repo,
                        scope,
                        path,
                        label,
                        depth,
                        staging,
                        left_to_review,
                    });
                }
                TreeItem::File { index, depth } => {
                    if let Some(entry) = entries.get(index) {
                        rows.push(Row::File {
                            repo,
                            scope,
                            entry: entry.clone(),
                            depth,
                            flat: false,
                        });
                    }
                }
            }
        }
    }

    fn message(rows: &mut Vec<Row>, text: impl Into<String>, indent: f32) {
        rows.push(Row::Message {
            text: text.into(),
            indent,
        });
    }

    pub(crate) fn build_rows(&self, scan: &Scan) -> Vec<Row> {
        let mut rows = Vec::new();
        if !scan.loaded {
            Self::message(&mut rows, "Reading git...", 10.0);
            return rows;
        }
        if self.sources.is_empty() {
            Self::message(&mut rows, "No repositories in this workspace", 10.0);
            return rows;
        }
        if let Some(opened) = self.opened.clone() {
            rows.push(Row::CommitHeader);
            if opened.files.is_empty() {
                Self::message(
                    &mut rows,
                    "No file changes to show (a merge or an empty commit)",
                    10.0,
                );
            }
            let entries = opened.files.iter().map(FileEntry::from_change).collect();
            self.file_rows(scan, opened.repo, Scope::Commit, entries, 0, &mut rows);
            return rows;
        }
        match self.tab {
            Tab::Changes => self.changes_rows(scan, &mut rows),
            Tab::Remote => self.remote_rows(scan, &mut rows),
            Tab::History => self.history_rows(scan, &mut rows),
        }
        rows
    }

    fn changes_rows(&self, scan: &Scan, rows: &mut Vec<Row>) {
        let mut quiet = Vec::new();
        for (index, repo) in scan.repos.iter().enumerate() {
            let name = self.source_name(index);
            if let Some(error) = &repo.branch.error {
                if repo.status.is_empty() {
                    Self::message(
                        rows,
                        format!("{name}: {}", error.lines().next().unwrap_or_default()),
                        10.0,
                    );
                    continue;
                }
            }
            if repo.status.is_empty() {
                quiet.push(name);
                continue;
            }
            rows.push(Row::RepoHeader {
                repo: index,
                history: false,
            });
            if self.is_folded(&format!("c:{name}")) {
                continue;
            }
            if !self.group_by_staging {
                let entries = repo
                    .status
                    .iter()
                    .map(|entry| working_entry(repo, entry, Scope::Working))
                    .collect();
                self.file_rows(scan, index, Scope::Working, entries, 0, rows);
                continue;
            }
            for staged in [true, false] {
                let scope = if staged {
                    Scope::Staged
                } else {
                    Scope::Unstaged
                };
                let entries: Vec<FileEntry> = repo
                    .status
                    .iter()
                    .filter(|entry| match entry.staging() {
                        Staging::Partial => true,
                        Staging::Staged => staged,
                        Staging::Unstaged => !staged,
                    })
                    .map(|entry| working_entry(repo, entry, scope))
                    .collect();
                rows.push(Row::Section {
                    repo: index,
                    staged,
                    count: entries.len(),
                });
                if self.is_folded(&format!("s:{name}:{staged}")) {
                    continue;
                }
                if entries.is_empty() {
                    let text = if staged {
                        "Nothing staged yet - tick a file below"
                    } else {
                        "Everything is staged"
                    };
                    Self::message(rows, text, 42.0);
                    continue;
                }
                self.file_rows(scan, index, scope, entries, 1, rows);
            }
        }
        if !quiet.is_empty() {
            Self::message(
                rows,
                format!("Nothing to commit in {}", quiet.join(", ")),
                10.0,
            );
        }
    }

    fn remote_rows(&self, scan: &Scan, rows: &mut Vec<Row>) {
        let (reviewed, total) = self.review_progress(scan);
        rows.push(Row::Progress { reviewed, total });
        for (index, repo) in scan.repos.iter().enumerate() {
            let name = self.source_name(index);
            rows.push(Row::Gap);
            rows.push(Row::Card { repo: index });
            if self.is_folded(&format!("r:{name}")) {
                continue;
            }
            let published = repo.outgoing.is_some();
            let pull_request = self
                .pull_requests
                .as_ref()
                .zip(self.sources.get(index))
                .and_then(|(prs, source)| prs.for_checkout(&source.root))
                .and_then(|(_, pr)| pr);
            let default_branch = self
                .sources
                .get(index)
                .map(|source| source.default_branch.as_str())
                .unwrap_or_default();
            if let Some(error) = &repo.branch.error {
                rows.push(Row::CardNote {
                    repo: index,
                    text: error.lines().next().unwrap_or_default().to_string(),
                });
            } else if let Some(pull_request) = pull_request {
                rows.push(Row::PullRequest {
                    repo: index,
                    pull_request: Box::new(pull_request),
                });
            } else if !published && repo.branch_name().is_some() {
                rows.push(Row::CardNote {
                    repo: index,
                    text: "Publish the branch to open a pull request".into(),
                });
            } else if repo.branch.files.is_empty() || repo.branch_name() == Some(default_branch) {
                rows.push(Row::CardNote {
                    repo: index,
                    text: "Nothing on this branch yet".into(),
                });
            } else {
                rows.push(Row::CreatePullRequest { repo: index });
            }
            let outgoing = repo
                .outgoing
                .clone()
                .unwrap_or_else(|| repo.history.clone());
            for (incoming, commits) in [(false, outgoing), (true, repo.incoming.clone())] {
                if commits.is_empty() {
                    continue;
                }
                rows.push(Row::CommitGroup {
                    repo: index,
                    incoming,
                    count: commits.len(),
                    published,
                });
                let key = format!("{}:{name}", if incoming { "in" } else { "out" });
                if self.is_folded(&key) {
                    continue;
                }
                rows.extend(commits.into_iter().map(|commit| Row::Commit {
                    repo: index,
                    commit,
                    incoming,
                    with_repo: false,
                    indent: 28.0,
                    in_card: true,
                }));
            }
            if repo.branch.files.is_empty() {
                continue;
            }
            let (added, deleted) =
                repo.branch
                    .files
                    .iter()
                    .fold((0, 0), |(added, deleted), file| {
                        (
                            added + file.added.unwrap_or(0),
                            deleted + file.deleted.unwrap_or(0),
                        )
                    });
            rows.push(Row::FilesHeader {
                repo: index,
                added,
                deleted,
                count: repo.branch.files.len(),
            });
            if self.is_folded(&format!("rf:{name}")) {
                continue;
            }
            let entries = repo
                .branch
                .files
                .iter()
                .map(FileEntry::from_change)
                .collect();
            self.file_rows(scan, index, Scope::Branch, entries, 0, rows);
        }
        rows.push(Row::Gap);
    }

    pub(crate) fn review_progress(&self, scan: &Scan) -> (usize, usize) {
        let mut reviewed = 0;
        let mut total = 0;
        for (index, repo) in scan.repos.iter().enumerate() {
            for file in &repo.branch.files {
                total += 1;
                if self.is_reviewed(scan, index, &file.path) {
                    reviewed += 1;
                }
            }
        }
        (reviewed, total)
    }

    fn history_rows(&self, scan: &Scan, rows: &mut Vec<Row>) {
        if self.timeline {
            let mut all: Vec<(usize, CommitSummary)> = scan
                .repos
                .iter()
                .enumerate()
                .flat_map(|(index, repo)| {
                    repo.history
                        .iter()
                        .map(move |commit| (index, commit.clone()))
                })
                .collect();
            all.sort_by_key(|(_, commit)| std::cmp::Reverse(commit.timestamp));
            if all.is_empty() {
                Self::message(rows, "No commits on this branch yet", 10.0);
            }
            let mut day = "";
            for (repo, commit) in all {
                let label = git::history::day_label(commit.timestamp);
                if label != day {
                    day = label;
                    rows.push(Row::Day(label));
                }
                rows.push(Row::Commit {
                    repo,
                    commit,
                    incoming: false,
                    with_repo: true,
                    indent: 8.0,
                    in_card: false,
                });
            }
            return;
        }
        for (index, repo) in scan.repos.iter().enumerate() {
            rows.push(Row::RepoHeader {
                repo: index,
                history: true,
            });
            if self.is_folded(&format!("h:{}", self.source_name(index))) {
                continue;
            }
            if repo.history.is_empty() {
                Self::message(rows, "No commits on this branch yet", 26.0);
            }
            rows.extend(repo.history.iter().map(|commit| Row::Commit {
                repo: index,
                commit: commit.clone(),
                incoming: false,
                with_repo: false,
                indent: 26.0,
                in_card: false,
            }));
        }
    }
}
