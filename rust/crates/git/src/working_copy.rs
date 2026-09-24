use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LOCAL_TIMEOUT: Duration = Duration::from_secs(300);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staging {
    Staged,
    Partial,
    Unstaged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// The index and worktree columns of `git status` ('.' when unchanged).
    pub index: char,
    pub worktree: char,
    pub untracked: bool,
    pub conflicted: bool,
}

impl StatusEntry {
    pub fn staging(&self) -> Staging {
        match (self.untracked, self.index != '.', self.worktree != '.') {
            (true, _, _) | (false, false, _) => Staging::Unstaged,
            (false, true, false) => Staging::Staged,
            (false, true, true) => Staging::Partial,
        }
    }

    /// Created on this branch and not yet committed, so "commit tracked" leaves it out.
    pub fn is_new(&self) -> bool {
        self.untracked || self.index == 'A'
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upstream {
    None,
    /// Configured, but the remote branch no longer exists.
    Gone,
    Tracked {
        ahead: u32,
        behind: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadState {
    pub branch: Option<String>,
    pub short_sha: Option<String>,
    pub upstream: Upstream,
    /// The upstream's remote and branch names (`origin`, `feat-login`) while tracked.
    pub upstream_ref: Option<(String, String)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitOptions {
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushMode {
    Normal,
    SetUpstream,
    ForceWithLease,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn log(&self) -> String {
        match (self.stdout.trim().is_empty(), self.stderr.trim().is_empty()) {
            (false, false) => format!("{}\n{}", self.stdout.trim_end(), self.stderr.trim_end()),
            (false, true) => self.stdout.trim_end().to_string(),
            _ => self.stderr.trim_end().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandError {
    pub message: String,
    pub output: CommandOutput,
}

fn run(root: &Path, args: &[&str], timeout: Duration) -> Result<CommandOutput, CommandError> {
    let failed = |message: String| CommandError {
        message,
        output: CommandOutput::default(),
    };
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        // Without a terminal a credential prompt would hang the panel; failing shows the log instead.
        .env("GIT_TERMINAL_PROMPT", "0")
        // The panel polls status; that must never take the index lock another git is using.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| failed(format!("git: {error}")))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                if let Err(error) = pipe.read_to_string(&mut text) {
                    text.push_str(&format!("\n(output cut: {error})"));
                }
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                if let Err(error) = child.kill() {
                    eprintln!("git: kill after timeout: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("git: reap after timeout: {error}");
                }
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(failed(format!("git: {error}"))),
        }
    };
    let output = CommandOutput {
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    };
    match status {
        Some(status) if status.success() => Ok(output),
        Some(_) => Err(CommandError {
            message: first_error_line(&output.stderr)
                .unwrap_or_else(|| format!("git {} failed", args.first().unwrap_or(&""))),
            output,
        }),
        None => Err(CommandError {
            message: format!("git {} timed out", args.first().unwrap_or(&"")),
            output,
        }),
    }
}

fn first_error_line(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines
        .iter()
        .find(|line| line.starts_with("error:") || line.starts_with("fatal:"))
        .or(lines.first())
        .map(|line| line.to_string())
}

fn read(root: &Path, args: &[&str]) -> Option<String> {
    run(root, args, LOCAL_TIMEOUT)
        .ok()
        .map(|output| output.stdout.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub fn status(root: &Path) -> Result<Vec<StatusEntry>, String> {
    let output = run(
        root,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        LOCAL_TIMEOUT,
    )
    .map_err(|error| error.message)?;
    Ok(parse_status(&output.stdout))
}

fn parse_status(text: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut fields = text.split('\0');
    while let Some(record) = fields.next() {
        let entry = |xy: &str, path: &str, conflicted: bool| {
            let mut columns = xy.chars();
            StatusEntry {
                path: path.to_string(),
                index: columns.next().unwrap_or('.'),
                worktree: columns.next().unwrap_or('.'),
                untracked: false,
                conflicted,
            }
        };
        let parts: Vec<&str> = record.splitn(2, ' ').collect();
        match parts.as_slice() {
            ["1", rest] => {
                let columns: Vec<&str> = rest.splitn(8, ' ').collect();
                if let (Some(xy), Some(path)) = (columns.first(), columns.get(7)) {
                    entries.push(entry(xy, path, false));
                }
            }
            ["2", rest] => {
                let columns: Vec<&str> = rest.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (columns.first(), columns.get(8)) {
                    entries.push(entry(xy, path, false));
                }
                // A rename's original path follows as its own field.
                fields.next();
            }
            ["u", rest] => {
                let columns: Vec<&str> = rest.splitn(10, ' ').collect();
                if let (Some(xy), Some(path)) = (columns.first(), columns.get(9)) {
                    entries.push(entry(xy, path, true));
                }
            }
            ["?", path] => entries.push(StatusEntry {
                path: path.to_string(),
                index: '.',
                worktree: '?',
                untracked: true,
                conflicted: false,
            }),
            _ => {}
        }
    }
    entries
}

pub fn stage(root: &Path, paths: &[String]) -> Result<(), CommandError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["update-index", "--add", "--remove", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(root, &args, LOCAL_TIMEOUT).map(|_| ())
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), CommandError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["reset", "--quiet", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(root, &args, LOCAL_TIMEOUT).map(|_| ())
}

pub fn commit(root: &Path, message: &str, options: CommitOptions) -> Result<(), CommandError> {
    let mut args = vec!["commit", "--quiet", "-m", message, "--cleanup=strip"];
    if options.amend {
        args.push("--amend");
    }
    if options.signoff {
        args.push("--signoff");
    }
    if options.no_verify {
        args.push("--no-verify");
    }
    run(root, &args, LOCAL_TIMEOUT).map(|_| ())
}

pub fn head_message(root: &Path) -> Option<String> {
    read(root, &["log", "-1", "--format=%B"])
}

pub fn head_state(root: &Path) -> HeadState {
    let branch = read(root, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let short_sha = read(root, &["rev-parse", "--short=8", "HEAD"]);
    let tracking = branch.as_ref().and_then(|branch| {
        read(
            root,
            &[
                "for-each-ref",
                "--format=%(upstream:short)%00%(upstream:track)%00%(upstream:remotename)",
                &format!("refs/heads/{branch}"),
            ],
        )
    });
    let (upstream, upstream_ref) = match tracking
        .as_deref()
        .map(|line| line.split('\0').collect::<Vec<_>>())
    {
        Some(parts) if parts.first().is_some_and(|name| !name.is_empty()) => {
            let name = parts[0];
            let track = parts.get(1).copied().unwrap_or_default();
            let remote = parts.get(2).copied().unwrap_or_default();
            if track.contains("gone") {
                (Upstream::Gone, None)
            } else {
                let count = |word: &str| {
                    track
                        .split([',', '[', ']'])
                        .map(str::trim)
                        .find_map(|part| part.strip_prefix(word))
                        .and_then(|number| number.trim().parse().ok())
                        .unwrap_or(0)
                };
                let remote_branch = name
                    .strip_prefix(&format!("{remote}/"))
                    .unwrap_or(name)
                    .to_string();
                (
                    Upstream::Tracked {
                        ahead: count("ahead"),
                        behind: count("behind"),
                    },
                    Some((remote.to_string(), remote_branch)),
                )
            }
        }
        _ => (Upstream::None, None),
    };
    HeadState {
        branch,
        short_sha,
        upstream,
        upstream_ref,
    }
}

/// The remotes a push of `branch` could go to: the one git would push to when set, else every remote.
pub fn push_remotes(root: &Path, branch: &str) -> Vec<String> {
    let configured = read(
        root,
        &["rev-parse", "--abbrev-ref", &format!("{branch}@{{push}}")],
    )
    .and_then(|target| target.split_once('/').map(|(remote, _)| remote.to_string()));
    match configured {
        Some(remote) => vec![remote],
        None => remotes(root),
    }
}

pub fn remotes(root: &Path) -> Vec<String> {
    read(root, &["remote"])
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn remote_url(root: &Path, remote: &str) -> Option<String> {
    read(root, &["remote", "get-url", remote])
}

pub fn push(
    root: &Path,
    remote: &str,
    branch: &str,
    remote_branch: &str,
    mode: PushMode,
) -> Result<CommandOutput, CommandError> {
    let refspec = format!("{branch}:{remote_branch}");
    let mut args = vec!["push"];
    match mode {
        PushMode::Normal => {}
        PushMode::SetUpstream => args.push("--set-upstream"),
        PushMode::ForceWithLease => args.push("--force-with-lease"),
    }
    args.extend([remote, refspec.as_str()]);
    run(root, &args, NETWORK_TIMEOUT)
}

/// Pulls the tracked branch; `branch` names it only when the branch tracks nothing yet.
pub fn pull(
    root: &Path,
    remote: &str,
    branch: Option<&str>,
    rebase: bool,
) -> Result<CommandOutput, CommandError> {
    let mut args = vec!["pull"];
    if rebase {
        args.push("--rebase");
    }
    args.push(remote);
    args.extend(branch);
    run(root, &args, NETWORK_TIMEOUT)
}

/// Fetches `remote`, or every remote when `None`.
pub fn fetch(root: &Path, remote: Option<&str>) -> Result<CommandOutput, CommandError> {
    run(root, &["fetch", remote.unwrap_or("--all")], NETWORK_TIMEOUT)
}

pub fn parse_github_remote(url: &str) -> Option<(String, String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let rest = rest
            .rsplit_once('@')
            .map_or(rest, |(_, host_path)| host_path);
        rest.split_once('/')?
    } else {
        let rest = url.rsplit_once('@').map_or(url, |(_, host_path)| host_path);
        rest.split_once(':')?
    };
    let host = host.split(':').next()?.to_string();
    if !host.contains("github") {
        return None;
    }
    let mut segments = path.trim_matches('/').rsplitn(2, '/');
    let repo = segments.next()?.to_string();
    let owner = segments.next()?.rsplit('/').next()?.to_string();
    (!owner.is_empty() && !repo.is_empty()).then_some((host, owner, repo))
}

pub fn create_pull_request_url(remote_url: &str, branch: &str) -> Option<String> {
    let (host, owner, repo) = parse_github_remote(remote_url)?;
    let encoded: String = branch
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect();
    Some(format!("https://{host}/{owner}/{repo}/pull/new/{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        match run(root, args, LOCAL_TIMEOUT) {
            Ok(_) => {}
            Err(error) => panic!("git {args:?}: {}", error.output.log()),
        }
    }

    fn repo() -> tempfile::TempDir {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("temp dir: {error}"),
        };
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "dev@example.com"]);
        git(root, &["config", "user.name", "Dev"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        write(root, "kept.txt", "one\n");
        git(root, &["add", "kept.txt"]);
        git(root, &["commit", "-q", "-m", "start"]);
        dir
    }

    fn write(root: &Path, path: &str, text: &str) {
        if let Err(error) = std::fs::write(root.join(path), text) {
            panic!("write {path}: {error}");
        }
    }

    fn entry<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
        match entries.iter().find(|entry| entry.path == path) {
            Some(entry) => entry,
            None => panic!("{path} not in status"),
        }
    }

    #[test]
    fn staging_follows_the_index_and_worktree_columns() -> Result<(), String> {
        let dir = repo();
        let root = dir.path();
        write(root, "kept.txt", "two\n");
        write(root, "new file.txt", "fresh\n");
        let entries = status(root)?;
        assert_eq!(entry(&entries, "kept.txt").staging(), Staging::Unstaged);
        assert!(entry(&entries, "new file.txt").is_new());
        stage(root, &["kept.txt".into(), "new file.txt".into()]).map_err(|error| error.message)?;
        write(root, "kept.txt", "three\n");
        let entries = status(root)?;
        assert_eq!(entry(&entries, "kept.txt").staging(), Staging::Partial);
        assert_eq!(entry(&entries, "new file.txt").staging(), Staging::Staged);
        assert!(entry(&entries, "new file.txt").is_new());
        unstage(root, &["new file.txt".into()]).map_err(|error| error.message)?;
        assert_eq!(
            entry(&status(root)?, "new file.txt").staging(),
            Staging::Unstaged
        );
        std::fs::remove_file(root.join("kept.txt")).map_err(|error| error.to_string())?;
        stage(root, &["kept.txt".into()]).map_err(|error| error.message)?;
        assert_eq!(entry(&status(root)?, "kept.txt").index, 'D');
        Ok(())
    }

    #[test]
    fn commit_and_amend_rewrite_the_message() -> Result<(), String> {
        let dir = repo();
        let root = dir.path();
        write(root, "kept.txt", "two\n");
        stage(root, &["kept.txt".into()]).map_err(|error| error.message)?;
        commit(
            root,
            "Update kept\n\n# a comment line",
            CommitOptions::default(),
        )
        .map_err(|error| error.message)?;
        assert_eq!(head_message(root).as_deref(), Some("Update kept"));
        commit(
            root,
            "Update kept.txt",
            CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
        )
        .map_err(|error| error.message)?;
        assert_eq!(head_message(root).as_deref(), Some("Update kept.txt"));
        let empty = commit(root, "Nothing", CommitOptions::default());
        assert!(empty.is_err());
        Ok(())
    }

    #[test]
    fn publishing_sets_the_upstream_and_counts_follow() -> Result<(), String> {
        let remote_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        git(remote_dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        let dir = repo();
        let root = dir.path();
        let remote_path = remote_dir.path().to_string_lossy().into_owned();
        git(root, &["remote", "add", "origin", &remote_path]);
        let head = head_state(root);
        assert_eq!(head.branch.as_deref(), Some("main"));
        assert_eq!(head.upstream, Upstream::None);
        assert_eq!(push_remotes(root, "main"), ["origin"]);
        push(root, "origin", "main", "main", PushMode::SetUpstream)
            .map_err(|error| error.message)?;
        let head = head_state(root);
        assert_eq!(
            head.upstream,
            Upstream::Tracked {
                ahead: 0,
                behind: 0
            }
        );
        assert_eq!(
            head.upstream_ref,
            Some(("origin".to_string(), "main".to_string()))
        );
        write(root, "kept.txt", "two\n");
        stage(root, &["kept.txt".into()]).map_err(|error| error.message)?;
        commit(root, "Two", CommitOptions::default()).map_err(|error| error.message)?;
        assert_eq!(
            head_state(root).upstream,
            Upstream::Tracked {
                ahead: 1,
                behind: 0
            }
        );
        let pushed = push(root, "origin", "main", "main", PushMode::Normal)
            .map_err(|error| error.message)?;
        assert!(pushed.stderr.contains("main -> main"));
        fetch(root, None).map_err(|error| error.message)?;
        let up_to_date = pull(root, "origin", None, false).map_err(|error| error.message)?;
        assert!(up_to_date.stdout.contains("Already up to date"));
        git(root, &["checkout", "-q", "-b", "feat-login"]);
        push(
            root,
            "origin",
            "feat-login",
            "feat-login",
            PushMode::SetUpstream,
        )
        .map_err(|error| error.message)?;
        git(root, &["push", "-q", "origin", "--delete", "feat-login"]);
        git(root, &["fetch", "-q", "--prune"]);
        assert_eq!(head_state(root).upstream, Upstream::Gone);
        Ok(())
    }

    #[test]
    fn a_push_to_nowhere_fails_with_a_reason() {
        let dir = repo();
        let root = dir.path();
        git(
            root,
            &["remote", "add", "origin", "/nonexistent/remote.git"],
        );
        let Err(error) = push(root, "origin", "main", "main", PushMode::SetUpstream) else {
            panic!("pushing to a missing remote should fail");
        };
        assert!(!error.message.is_empty());
        assert!(!error.output.log().is_empty());
    }

    #[test]
    fn github_remotes_give_the_new_pull_request_page() {
        for url in [
            "https://github.com/acme/web.git",
            "git@github.com:acme/web.git",
            "ssh://git@github.com/acme/web",
            "https://token@github.com/acme/web/",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(("github.com".into(), "acme".into(), "web".into())),
                "{url}"
            );
        }
        assert_eq!(
            create_pull_request_url("git@github.com:acme/web.git", "feat/login"),
            Some("https://github.com/acme/web/pull/new/feat%2Flogin".into())
        );
        assert_eq!(
            create_pull_request_url("https://gitlab.com/acme/web.git", "main"),
            None
        );
    }

    #[test]
    fn porcelain_records_are_parsed_by_kind() {
        let text = "1 .M N... 100644 100644 100644 abc abc src/a b.rs\u{0}2 R. N... 100644 100644 100644 abc abc R100 new.rs\u{0}old.rs\u{0}u UU N... 100644 100644 100644 100644 a b c conflict.rs\u{0}? notes.md\u{0}";
        let entries = parse_status(text);
        let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, ["src/a b.rs", "new.rs", "conflict.rs", "notes.md"]);
        assert_eq!(entries[1].staging(), Staging::Staged);
        assert!(entries[2].conflicted);
        assert!(entries[3].untracked);
    }
}
