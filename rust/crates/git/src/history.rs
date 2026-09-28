//! Commits of a branch read through the git CLI: those since it left its base, those not pushed yet, those on
//! its upstream not pulled yet, and the files one commit changed.

use std::path::Path;

use time::{OffsetDateTime, UtcOffset};

use crate::branch_changes::{numstat, parse_name_status};
use crate::working_copy::read;
use crate::{ChangeStatus, FileChange};

/// At most this many commits are listed for one range.
pub const LOG_LIMIT: usize = 200;

const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ct%x1e";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitSummary {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    /// Commit time, seconds since the epoch.
    pub timestamp: i64,
}

fn parse_log(text: &str) -> Vec<CommitSummary> {
    text.split('\u{1e}')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split('\u{1f}');
            let sha = fields.next().filter(|sha| !sha.is_empty())?.to_string();
            Some(CommitSummary {
                sha,
                short_sha: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
                author: fields.next()?.to_string(),
                timestamp: fields.next()?.trim().parse().unwrap_or(0),
            })
        })
        .collect()
}

fn log(root: &Path, range: &str) -> Vec<CommitSummary> {
    let limit = format!("--max-count={LOG_LIMIT}");
    read(root, &["log", LOG_FORMAT, &limit, range, "--"])
        .map(|text| parse_log(&text))
        .unwrap_or_default()
}

/// The branch's own commits, newest first: those HEAD has and `base` does not.
pub fn commits_since(root: &Path, base: &str) -> Vec<CommitSummary> {
    log(root, &format!("{base}..HEAD"))
}

/// Commits not on the upstream yet, newest first; `None` when the branch tracks nothing.
pub fn outgoing(root: &Path) -> Option<Vec<CommitSummary>> {
    read(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    Some(log(root, "@{upstream}..HEAD"))
}

/// Commits the upstream has (as of the last fetch) that HEAD does not, newest first.
pub fn incoming(root: &Path) -> Vec<CommitSummary> {
    if read(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .is_none()
    {
        return Vec::new();
    }
    log(root, "HEAD..@{upstream}")
}

pub fn head_commit(root: &Path) -> Option<CommitSummary> {
    let limit = "--max-count=1";
    read(root, &["log", LOG_FORMAT, limit, "HEAD", "--"])
        .and_then(|text| parse_log(&text).into_iter().next())
}

/// The commit's first parent, `None` for a root commit.
pub fn parent_of(root: &Path, sha: &str) -> Option<String> {
    read(
        root,
        &["rev-parse", "--verify", "--quiet", &format!("{sha}^")],
    )
}

/// What the commit changed against its first parent, with line counts; empty for a merge.
pub fn commit_files(root: &Path, sha: &str) -> Vec<FileChange> {
    let diff = |format: &str| {
        read(
            root,
            &[
                "diff-tree",
                "-r",
                "-M",
                "--root",
                "--no-commit-id",
                format,
                "-z",
                sha,
            ],
        )
        .unwrap_or_default()
    };
    let counts = numstat(&diff("--numstat"));
    let mut files: Vec<FileChange> = parse_name_status(&diff("--name-status"))
        .into_iter()
        .map(|(status, path, old_path)| {
            let (added, deleted) = counts
                .iter()
                .find(|(counted, _, _)| *counted == path)
                .map_or((None, None), |(_, added, deleted)| (*added, *deleted));
            FileChange {
                path,
                old_path,
                status: if status == ChangeStatus::Conflicted {
                    ChangeStatus::Modified
                } else {
                    status
                },
                added,
                deleted,
                uncommitted: false,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// The working copy (staged or not) against `against` as one unified patch; `None` when nothing differs.
pub fn patch(root: &Path, against: &str) -> Option<String> {
    read(
        root,
        &["diff", "--no-color", "--no-ext-diff", "-M", against, "--"],
    )
}

fn local(timestamp: i64) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let at = OffsetDateTime::from_unix_timestamp(timestamp).ok()?;
    Some((
        at.to_offset(offset),
        OffsetDateTime::now_utc().to_offset(offset),
    ))
}

pub fn relative_time(timestamp: i64) -> String {
    local(timestamp).map_or_else(String::new, |(at, now)| crate::relative_timestamp(at, now))
}

pub fn day_label(timestamp: i64) -> &'static str {
    match local(timestamp).map(|(at, now)| (now.date() - at.date()).whole_days()) {
        Some(days) if days <= 0 => "Today",
        Some(1) => "Yesterday",
        _ => "Earlier",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working_copy::{self, CommitOptions, PushMode};

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .output();
        match output {
            Ok(output) if output.status.success() => {}
            Ok(output) => panic!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)),
            Err(error) => panic!("git {args:?}: {error}"),
        }
    }

    fn write(root: &Path, path: &str, text: &str) {
        if let Some(parent) = root.join(path).parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                panic!("mkdir {path}: {error}");
            }
        }
        if let Err(error) = std::fs::write(root.join(path), text) {
            panic!("write {path}: {error}");
        }
    }

    fn commit(root: &Path, path: &str, text: &str, message: &str) {
        write(root, path, text);
        git(root, &["add", "--all"]);
        git(root, &["commit", "-q", "-m", message]);
    }

    fn subjects(commits: &[CommitSummary]) -> Vec<&str> {
        commits
            .iter()
            .map(|commit| commit.subject.as_str())
            .collect()
    }

    #[test]
    fn the_branch_log_outgoing_and_incoming_commits() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let remote = temp.path().join("remote.git");
        git(
            temp.path(),
            &["init", "-q", "--bare", "-b", "main", "remote.git"],
        );
        let root = temp.path().join("api");
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        git(&root, &["config", "user.name", "Dev"]);
        git(&root, &["config", "user.email", "dev@example.com"]);
        commit(&root, "a.txt", "1\n", "Base");
        git(
            &root,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&root, &["push", "-q", "-u", "origin", "main"]);
        git(&root, &["checkout", "-q", "-b", "feat"]);
        assert_eq!(outgoing(&root), None, "nothing tracked yet");
        commit(&root, "src/b.txt", "1\n2\n", "Add b");
        working_copy::push(&root, "origin", "feat", "feat", PushMode::SetUpstream)
            .map_err(|error| error.message)?;
        commit(&root, "a.txt", "1\nmore\n", "Grow a");
        assert_eq!(subjects(&commits_since(&root, "main")), ["Grow a", "Add b"]);
        assert_eq!(
            outgoing(&root).as_deref().map(subjects),
            Some(vec!["Grow a"])
        );
        let head = head_commit(&root).ok_or("head")?;
        assert_eq!(head.subject, "Grow a");
        assert_eq!(head.author, "Dev");
        assert!(head.timestamp > 0);
        let files = commit_files(&root, &head.sha);
        assert_eq!(files.len(), 1);
        assert_eq!(
            (files[0].path.as_str(), files[0].added, files[0].deleted),
            ("a.txt", Some(1), Some(0))
        );
        assert!(parent_of(&root, &head.sha).is_some());

        let other = temp.path().join("other");
        git(
            temp.path(),
            &[
                "clone",
                "-q",
                "-b",
                "feat",
                &remote.to_string_lossy(),
                "other",
            ],
        );
        git(&other, &["config", "commit.gpgsign", "false"]);
        commit(&other, "c.txt", "c\n", "From elsewhere");
        git(&other, &["push", "-q", "origin", "feat"]);
        assert!(incoming(&root).is_empty(), "not fetched yet");
        working_copy::fetch(&root, None).map_err(|error| error.message)?;
        assert_eq!(subjects(&incoming(&root)), ["From elsewhere"]);

        working_copy::soft_reset(&root).map_err(|error| error.message)?;
        assert_eq!(
            head_commit(&root).map(|commit| commit.subject),
            Some("Add b".into())
        );
        let staged = working_copy::status(&root)?;
        assert_eq!(
            staged
                .iter()
                .map(|entry| (entry.path.as_str(), entry.staging()))
                .collect::<Vec<_>>(),
            [("a.txt", working_copy::Staging::Staged)],
            "an uncommit keeps its changes staged"
        );
        working_copy::commit(&root, "Grow a again", CommitOptions::default())
            .map_err(|error| error.message)?;
        Ok(())
    }

    #[test]
    fn directories_stage_and_unstage_as_one() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let root = temp.path();
        git(root, &["init", "-q", "-b", "main"]);
        write(root, "src/one.rs", "1\n");
        write(root, "src/deep/two.rs", "2\n");
        write(root, "top.rs", "3\n");
        working_copy::stage_directory(root, "src").map_err(|error| error.message)?;
        let staged: Vec<String> = working_copy::status(root)?
            .into_iter()
            .filter(|entry| entry.staging() == working_copy::Staging::Staged)
            .map(|entry| entry.path)
            .collect();
        assert_eq!(staged, ["src/deep/two.rs", "src/one.rs"]);
        working_copy::unstage_directory(root, "src/deep").map_err(|error| error.message)?;
        let staged = working_copy::status(root)?
            .into_iter()
            .filter(|entry| entry.staging() == working_copy::Staging::Staged)
            .count();
        assert_eq!(staged, 1, "unstaging works before the first commit too");
        assert!(working_copy::stage_directory(root, "../outside").is_err());
        assert!(working_copy::stage_directory(root, "/etc").is_err());
        working_copy::stage_all(root).map_err(|error| error.message)?;
        working_copy::unstage_all(root).map_err(|error| error.message)?;
        assert!(working_copy::status(root)?
            .iter()
            .all(|entry| entry.staging() == working_copy::Staging::Unstaged));
        Ok(())
    }
}
