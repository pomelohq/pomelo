//! What a workspace branch changed: every file that differs between the working tree and the point the
//! branch left its default branch (the merge-base), committed or not, plus new untracked files. This is
//! what an agent did on the branch, which is what a person reviews.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Untracked files larger than this are counted as changed without counting their lines.
const LINE_COUNT_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository root.
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    /// Line counts; `None` for binary files.
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    /// The working tree or index still differs from HEAD for this file.
    pub uncommitted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoChanges {
    pub root: PathBuf,
    pub branch: String,
    /// The ref the branch is compared with (`origin/<default>` when it exists).
    pub base: String,
    pub ahead: u32,
    pub behind: u32,
    /// The commit the branch left the base at; `None` when only uncommitted work is compared.
    pub fork_point: Option<String>,
    pub files: Vec<FileChange>,
    /// Why nothing could be read, when git failed.
    pub error: Option<String>,
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        // Reading must never take the index lock another git (or the user) is using.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn resolves(root: &Path, reference: &str) -> bool {
    git(root, &["rev-parse", "--verify", "--quiet", reference]).is_ok()
}

/// The branch's changes against `default_branch` (the remote copy when there is one).
pub fn branch_changes(root: &Path, default_branch: &str) -> RepoChanges {
    let mut changes = RepoChanges {
        root: root.to_path_buf(),
        ..RepoChanges::default()
    };
    if let Err(error) = fill(root, default_branch, &mut changes) {
        changes.error = Some(error);
    }
    changes
}

fn fill(root: &Path, default_branch: &str, changes: &mut RepoChanges) -> Result<(), String> {
    changes.branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let remote = format!("origin/{default_branch}");
    changes.base = if resolves(root, &remote) {
        remote
    } else {
        default_branch.to_string()
    };
    let has_head = resolves(root, "HEAD");
    let fork_point = if has_head && resolves(root, &changes.base) {
        git(root, &["merge-base", &changes.base, "HEAD"])
            .map(|output| output.trim().to_string())
            .ok()
    } else {
        None
    };
    if let Some(counts) = fork_point.as_ref().and_then(|_| {
        git(
            root,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{}...HEAD", changes.base),
            ],
        )
        .ok()
    }) {
        let mut numbers = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
        changes.behind = numbers.next().unwrap_or(0);
        changes.ahead = numbers.next().unwrap_or(0);
    }
    // Without a fork point (no default branch, or no commits yet) only uncommitted work shows.
    let against = match (&fork_point, has_head) {
        (Some(point), _) => point.clone(),
        (None, true) => "HEAD".to_string(),
        (None, false) => String::new(),
    };
    changes.fork_point = fork_point.clone();
    let uncommitted = uncommitted_paths(root)?;
    let mut files = Vec::new();
    if !against.is_empty() {
        let statuses = git(root, &["diff", "--name-status", "-z", "-M", &against])?;
        let counts = numstat(&git(root, &["diff", "--numstat", "-z", "-M", &against])?);
        for (status, path, old_path) in parse_name_status(&statuses) {
            let (added, deleted) = counts
                .iter()
                .find(|(counted, _, _)| *counted == path)
                .map_or((None, None), |(_, added, deleted)| (*added, *deleted));
            files.push(FileChange {
                uncommitted: uncommitted.contains(&path),
                path,
                old_path,
                status,
                added,
                deleted,
            });
        }
    }
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        files.push(FileChange {
            path: path.to_string(),
            old_path: None,
            status: ChangeStatus::Added,
            added: count_lines(&root.join(path)),
            deleted: Some(0),
            uncommitted: true,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    changes.files = files;
    Ok(())
}

fn uncommitted_paths(root: &Path) -> Result<HashSet<String>, String> {
    let output = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )?;
    let mut paths = HashSet::new();
    let mut entries = output.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let Some(path) = entry.get(3..) else {
            continue;
        };
        paths.insert(path.to_string());
        // A rename or copy is followed by its source path.
        if matches!(entry.as_bytes().first(), Some(b'R' | b'C')) {
            entries.next();
        }
    }
    Ok(paths)
}

/// `(status, path, old path)` from `git diff --name-status -z`.
fn parse_name_status(output: &str) -> Vec<(ChangeStatus, String, Option<String>)> {
    let mut out = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(code) = fields.next() {
        let status = match code.as_bytes().first() {
            Some(b'A' | b'C') => ChangeStatus::Added,
            Some(b'D') => ChangeStatus::Deleted,
            Some(b'R') => ChangeStatus::Renamed,
            Some(b'U') => ChangeStatus::Conflicted,
            _ => ChangeStatus::Modified,
        };
        if status == ChangeStatus::Renamed || code.starts_with('C') {
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                break;
            };
            out.push((status, new.to_string(), Some(old.to_string())));
        } else if let Some(path) = fields.next() {
            out.push((status, path.to_string(), None));
        }
    }
    out
}

/// `(path, added, deleted)` from `git diff --numstat -z`; binary files have no counts.
fn numstat(output: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut out = Vec::new();
    let mut fields = output.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, '\t');
        let added = parts.next().and_then(|n| n.parse().ok());
        let deleted = parts.next().and_then(|n| n.parse().ok());
        let path = match parts.next() {
            Some(path) if !path.is_empty() => path.to_string(),
            // A rename: the old and new paths follow as their own fields.
            _ => {
                fields.next();
                fields.next().unwrap_or_default().to_string()
            }
        };
        out.push((path, added, deleted));
    }
    out
}

fn count_lines(path: &Path) -> Option<u32> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > LINE_COUNT_LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count();
    let unterminated = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    u32::try_from(lines + unterminated).ok()
}

/// The file's text at `revision`, `None` when it did not exist there (or is not text).
pub fn file_at(root: &Path, revision: &str, path: &str) -> Option<String> {
    let text = git(root, &["show", &format!("{revision}:{path}")]).ok()?;
    Some(if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text
    })
}

/// Puts a file back as HEAD has it (staged and working-tree changes gone); a file HEAD does not have
/// is deleted.
pub fn discard_uncommitted(root: &Path, path: &str) -> Result<(), String> {
    let tracked = git(root, &["ls-files", "--error-unmatch", "--", path]).is_ok();
    let in_head = git(root, &["cat-file", "-e", &format!("HEAD:{path}")]).is_ok();
    if in_head {
        git(
            root,
            &[
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--",
                path,
            ],
        )?;
        return Ok(());
    }
    if tracked {
        git(root, &["rm", "--cached", "--quiet", "--", path])?;
    }
    let full = root.join(path);
    if full.exists() {
        std::fs::remove_file(&full).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git");
        assert!(
            status.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        run(root, &["init", "-q", "-b", "main"]);
        run(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("keep.txt"), "a\nb\nc\n").expect("write");
        std::fs::write(root.join("gone.txt"), "x\n").expect("write");
        std::fs::write(root.join("old.txt"), "one\ntwo\nthree\nfour\n").expect("write");
        run(root, &["add", "."]);
        run(root, &["commit", "-q", "-m", "base"]);
        run(root, &["checkout", "-q", "-b", "feat"]);
        temp
    }

    #[test]
    fn committed_uncommitted_and_new_files_against_the_fork_point() {
        let temp = repo();
        let root = temp.path();
        std::fs::write(root.join("keep.txt"), "a\nB\nc\nd\n").expect("write");
        run(root, &["rm", "-q", "gone.txt"]);
        run(root, &["mv", "old.txt", "new.txt"]);
        run(root, &["commit", "-q", "-am", "work"]);
        std::fs::write(root.join("keep.txt"), "a\nB\nc\nd\ne\n").expect("write");
        std::fs::create_dir_all(root.join("src")).expect("dir");
        std::fs::write(root.join("src/fresh.rs"), "fn main() {}\n").expect("write");

        let changes = branch_changes(root, "main");
        assert_eq!(changes.error, None);
        assert_eq!(changes.branch, "feat");
        assert_eq!(changes.base, "main");
        assert_eq!((changes.ahead, changes.behind), (1, 0));
        let summary: Vec<_> = changes
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.status,
                    file.added,
                    file.deleted,
                    file.uncommitted,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("gone.txt", ChangeStatus::Deleted, Some(0), Some(1), false),
                ("keep.txt", ChangeStatus::Modified, Some(3), Some(1), true),
                ("new.txt", ChangeStatus::Renamed, Some(0), Some(0), false),
                ("src/fresh.rs", ChangeStatus::Added, Some(1), Some(0), true),
            ]
        );
        assert_eq!(changes.files[2].old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn discarding_puts_head_back_and_removes_new_files() {
        let temp = repo();
        let root = temp.path();
        std::fs::write(root.join("keep.txt"), "changed\n").expect("write");
        run(root, &["add", "keep.txt"]);
        std::fs::write(root.join("fresh.txt"), "new\n").expect("write");
        discard_uncommitted(root, "keep.txt").expect("discard tracked");
        discard_uncommitted(root, "fresh.txt").expect("discard new");
        assert_eq!(
            std::fs::read_to_string(root.join("keep.txt")).expect("read"),
            "a\nb\nc\n"
        );
        assert!(!root.join("fresh.txt").exists());
        assert!(branch_changes(root, "main").files.is_empty());
    }

    #[test]
    fn a_repository_without_the_default_branch_shows_uncommitted_work() {
        let temp = repo();
        let root = temp.path();
        std::fs::write(root.join("keep.txt"), "a\n").expect("write");
        let changes = branch_changes(root, "trunk");
        assert_eq!(changes.error, None);
        assert_eq!(changes.files.len(), 1);
        assert!(changes.files[0].uncommitted);
    }
}
