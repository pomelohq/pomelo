//! Where the Git panel's git runs: on this machine ([`LocalGit`]), or on the machine a remote project lives on,
//! through its server. Both take the same requests, checked the same way before anything runs.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::history;
use crate::scan::RepoScan;
use crate::working_copy::{self, CommandError, CommandOutput, CommitOptions, HeadState, PushMode};
use crate::{BlameEntry, DiffBases, FileChange, FileGit};

const MAX_PATH: usize = 4096;
const MAX_REF: usize = 255;
const MAX_PATHS: usize = 10_000;
const MAX_MESSAGE: usize = 64 * 1024;
/// The largest file whose git the editor reads from another machine: its text and both bases fit one frame.
pub const MAX_EDITOR_TEXT: usize = 256 * 1024;

/// A repo of the workspace to read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSource {
    pub name: String,
    pub root: PathBuf,
    pub default_branch: String,
    /// The workspace's branch, which every repo is expected to have checked out.
    pub expected_branch: String,
    /// This repo stays on another branch on purpose, so being off the workspace's branch is not drift.
    pub kept: bool,
    /// Its `origin` remote's address, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// A change to a repo's index, working tree or branch that runs on the repo's machine and needs no network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GitOp {
    Stage {
        paths: Vec<String>,
    },
    Unstage {
        paths: Vec<String>,
    },
    StageDirectory {
        path: String,
    },
    UnstageDirectory {
        path: String,
    },
    StageAll,
    UnstageAll,
    StashAll,
    StashPop,
    DiscardTracked,
    /// Puts one file back as HEAD has it; a file HEAD does not have is deleted.
    Discard {
        path: String,
    },
    /// Stages a conflicted file once no conflict markers are left in it.
    MarkResolved {
        path: String,
    },
    /// Undoes the last commit and keeps its changes staged.
    SoftReset,
    Commit {
        message: String,
        #[serde(default)]
        options: CommitOptions,
    },
    /// Puts `text` in the index as the file's staged version; `None` drops it from the index.
    WriteIndex {
        path: String,
        text: Option<String>,
    },
}

/// Something the panel reads of a repo besides its scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "read", rename_all = "snake_case")]
pub enum GitRead {
    FileAt {
        revision: String,
        path: String,
    },
    CommitFiles {
        sha: String,
    },
    ParentOf {
        sha: String,
    },
    Patch {
        against: String,
    },
    HeadMessage,
    HeadState,
    PushRemotes {
        branch: String,
    },
    RemoteUrl {
        remote: String,
    },
    /// The file's text at HEAD and in the index.
    Bases {
        path: String,
    },
    /// Who last changed each line of the file as `contents` reads.
    Blame {
        path: String,
        contents: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", content = "value", rename_all = "snake_case")]
pub enum GitAnswer {
    Text(Option<String>),
    Files(Vec<FileChange>),
    Head(HeadState),
    Names(Vec<String>),
    Bases(Option<DiffBases>),
    Blame(Option<Vec<BlameEntry>>),
}

/// A git command that reaches a remote, with the credentials of the machine it runs on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RemoteCommand {
    /// `None` fetches every remote.
    Fetch { remote: Option<String> },
    /// `branch` names the remote branch only while the local one tracks nothing.
    Pull {
        remote: String,
        branch: Option<String>,
        rebase: bool,
    },
    Push {
        remote: String,
        branch: String,
        remote_branch: String,
        mode: PushMode,
    },
}

impl RemoteCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            RemoteCommand::Fetch { .. } => "fetch",
            RemoteCommand::Pull { .. } => "pull",
            RemoteCommand::Push { .. } => "push",
        }
    }
}

/// A path inside a repo as git takes it: relative, '/'-separated, never climbing out or into `.git`.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err("a path must be 1 to 4096 bytes".into());
    }
    if path.contains(['\0', '\\']) || path.chars().any(char::is_control) {
        return Err(format!("{path:?} is not a valid path"));
    }
    let mut parts = Path::new(path).components();
    let first = parts.clone().next();
    if parts.any(|part| !matches!(part, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("{path:?} must stay inside the repository"));
    }
    if first.is_some_and(|part| part.as_os_str() == ".git") {
        return Err(format!("{path:?} is inside .git"));
    }
    Ok(())
}

/// A revision, branch or remote name: never an option, never a range or a `rev:path`.
pub fn check_ref(what: &str, name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_REF
        && !name.starts_with('-')
        && !name.contains("..")
        && name.chars().all(|char| {
            !char.is_control()
                && !char.is_whitespace()
                && !matches!(char, ':' | '\\' | '?' | '*' | '[')
        });
    if valid {
        Ok(())
    } else {
        Err(format!("{name:?} is not a valid {what}"))
    }
}

impl GitOp {
    pub fn check(&self) -> Result<(), String> {
        match self {
            GitOp::Stage { paths } | GitOp::Unstage { paths } => {
                if paths.len() > MAX_PATHS {
                    return Err("too many paths".into());
                }
                paths.iter().try_for_each(|path| check_path(path))
            }
            GitOp::StageDirectory { path }
            | GitOp::UnstageDirectory { path }
            | GitOp::Discard { path }
            | GitOp::MarkResolved { path } => check_path(path.trim_end_matches('/')),
            GitOp::Commit { message, .. } => {
                if message.len() > MAX_MESSAGE || message.contains('\0') {
                    Err("the commit message is too long or holds a NUL".into())
                } else {
                    Ok(())
                }
            }
            GitOp::WriteIndex { path, text } => {
                check_path(path)?;
                check_editor_text(text.as_deref().unwrap_or_default())
            }
            GitOp::StageAll
            | GitOp::UnstageAll
            | GitOp::StashAll
            | GitOp::StashPop
            | GitOp::DiscardTracked
            | GitOp::SoftReset => Ok(()),
        }
    }
}

impl GitRead {
    pub fn check(&self) -> Result<(), String> {
        match self {
            GitRead::FileAt { revision, path } => {
                check_ref("revision", revision)?;
                check_path(path)
            }
            GitRead::CommitFiles { sha } | GitRead::ParentOf { sha } => check_ref("commit", sha),
            GitRead::Patch { against } => check_ref("revision", against),
            GitRead::PushRemotes { branch } => check_ref("branch", branch),
            GitRead::RemoteUrl { remote } => check_ref("remote", remote),
            GitRead::HeadMessage | GitRead::HeadState => Ok(()),
            GitRead::Bases { path } => check_path(path),
            GitRead::Blame { path, contents } => {
                check_path(path)?;
                check_editor_text(contents)
            }
        }
    }
}

fn check_editor_text(text: &str) -> Result<(), String> {
    if text.len() > MAX_EDITOR_TEXT {
        Err("the file is too large for git over the connection".into())
    } else {
        Ok(())
    }
}

impl RemoteCommand {
    pub fn check(&self) -> Result<(), String> {
        match self {
            RemoteCommand::Fetch { remote } => remote
                .as_deref()
                .map_or(Ok(()), |remote| check_ref("remote", remote)),
            RemoteCommand::Pull { remote, branch, .. } => {
                check_ref("remote", remote)?;
                branch
                    .as_deref()
                    .map_or(Ok(()), |branch| check_ref("branch", branch))
            }
            RemoteCommand::Push {
                remote,
                branch,
                remote_branch,
                ..
            } => {
                check_ref("remote", remote)?;
                check_ref("branch", branch)?;
                check_ref("branch", remote_branch)
            }
        }
    }
}

fn refused(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        output: CommandOutput::default(),
    }
}

/// `path` under `root` once symlinks are followed: the deepest part that exists must stay inside the repo.
pub fn inside(root: &Path, path: &str) -> Result<PathBuf, String> {
    check_path(path)?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let full = root.join(path);
    let mut existing = full.as_path();
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing
            .parent()
            .ok_or_else(|| format!("{path:?} is outside the repository"))?;
    }
    let resolved = if existing == full
        && std::fs::symlink_metadata(existing).is_ok_and(|meta| meta.is_symlink())
    {
        existing
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("{path:?} is outside the repository"))?
            .canonicalize()
    } else {
        existing.canonicalize()
    }
    .map_err(|error| format!("{path}: {error}"))?;
    if resolved.starts_with(&root) {
        Ok(full)
    } else {
        Err(format!("{path:?} is outside the repository"))
    }
}

/// Runs `op` in the repo at `root`, after checking it.
pub fn apply(root: &Path, op: &GitOp) -> Result<(), CommandError> {
    op.check().map_err(refused)?;
    match op {
        GitOp::Stage { paths } => working_copy::stage(root, paths),
        GitOp::Unstage { paths } => working_copy::unstage(root, paths),
        GitOp::StageDirectory { path } => working_copy::stage_directory(root, path),
        GitOp::UnstageDirectory { path } => working_copy::unstage_directory(root, path),
        GitOp::StageAll => working_copy::stage_all(root),
        GitOp::UnstageAll => working_copy::unstage_all(root),
        GitOp::StashAll => working_copy::stash_all(root).map(|_| ()),
        GitOp::StashPop => working_copy::stash_pop(root).map(|_| ()),
        GitOp::DiscardTracked => working_copy::discard_tracked(root),
        GitOp::Discard { path } => {
            inside(root, path).map_err(refused)?;
            crate::discard_uncommitted(root, path)
                .map_err(|error| refused(format!("Could not discard {path}: {error}")))
        }
        GitOp::MarkResolved { path } => {
            let full = inside(root, path).map_err(refused)?;
            let text = std::fs::read_to_string(&full)
                .map_err(|error| refused(format!("Could not read {path}: {error}")))?;
            let left = crate::conflict::conflicts(&text).len();
            if left > 0 {
                let noun = if left == 1 { "conflict" } else { "conflicts" };
                return Err(refused(format!(
                    "{path} still has {left} {noun}; resolve them first"
                )));
            }
            working_copy::stage(root, std::slice::from_ref(path))
        }
        GitOp::SoftReset => working_copy::soft_reset(root),
        GitOp::Commit { message, options } => working_copy::commit(root, message, *options),
        GitOp::WriteIndex { path, text } => {
            let full = inside(root, path).map_err(refused)?;
            crate::write_index(&full, text.as_deref()).map_err(refused)
        }
    }
}

/// Answers `query` about the repo at `root`, after checking it.
pub fn answer(root: &Path, query: &GitRead) -> Result<GitAnswer, String> {
    query.check()?;
    Ok(match query {
        GitRead::FileAt { revision, path } => GitAnswer::Text(crate::file_at(root, revision, path)),
        GitRead::CommitFiles { sha } => GitAnswer::Files(history::commit_files(root, sha)),
        GitRead::ParentOf { sha } => GitAnswer::Text(history::parent_of(root, sha)),
        GitRead::Patch { against } => GitAnswer::Text(history::patch(root, against)),
        GitRead::HeadMessage => GitAnswer::Text(working_copy::head_message(root)),
        GitRead::HeadState => GitAnswer::Head(working_copy::head_state(root)),
        GitRead::PushRemotes { branch } => {
            GitAnswer::Names(working_copy::push_remotes(root, branch))
        }
        GitRead::RemoteUrl { remote } => GitAnswer::Text(working_copy::remote_url(root, remote)),
        GitRead::Bases { path } => GitAnswer::Bases(crate::load_bases(&inside(root, path)?)),
        GitRead::Blame { path, contents } => {
            GitAnswer::Blame(crate::blame(&inside(root, path)?, contents))
        }
    })
}

/// Runs `command` in the repo at `root`; `progress` hears git's progress lines as they come.
pub fn run_remote(
    root: &Path,
    command: &RemoteCommand,
    progress: Option<&(dyn Fn(&str) + Sync)>,
) -> Result<CommandOutput, CommandError> {
    command.check().map_err(refused)?;
    match command {
        RemoteCommand::Fetch { remote } => {
            working_copy::fetch_with(root, remote.as_deref(), progress)
        }
        RemoteCommand::Pull {
            remote,
            branch,
            rebase,
        } => working_copy::pull_with(root, remote, branch.as_deref(), *rebase, progress),
        RemoteCommand::Push {
            remote,
            branch,
            remote_branch,
            mode,
        } => working_copy::push_with(root, remote, branch, remote_branch, *mode, progress),
    }
}

/// Stops pushing scans when dropped.
pub struct Watching {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Watching {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Watching {
        Watching {
            stop: Some(Box::new(stop)),
        }
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

/// Hears a repo's new scan: its index in the watched list, and the scan.
pub type ScanSink = Arc<dyn Fn(usize, RepoScan) + Send + Sync>;

/// The Git panel's git, wherever the repos are.
pub trait GitAccess: Send + Sync {
    fn scan(&self, repo: &RepoSource) -> RepoScan;
    fn act(&self, repo: &RepoSource, op: GitOp) -> Result<(), CommandError>;
    fn read(&self, repo: &RepoSource, query: GitRead) -> Result<GitAnswer, String>;
    /// Runs a git command that talks to a remote; `progress` hears what it says while it runs.
    fn remote(
        &self,
        repo: &RepoSource,
        command: RemoteCommand,
        progress: &(dyn Fn(&str) + Sync),
    ) -> Result<CommandOutput, CommandError>;
    /// Pushes each repo's scan when it changes, until the result is dropped; `None` means ask with `scan`.
    fn watch(&self, _repos: &[RepoSource], _changed: ScanSink) -> Option<Watching> {
        None
    }
    /// Why nothing can be read here, when nothing can.
    fn refusal(&self) -> Option<String> {
        None
    }
    /// What to tell when a remote refused to let git sign in.
    fn sign_in_advice(&self) -> Option<String> {
        None
    }

    fn file_at(&self, repo: &RepoSource, revision: &str, path: &str) -> Option<String> {
        let query = GitRead::FileAt {
            revision: revision.to_string(),
            path: path.to_string(),
        };
        match self.read(repo, query) {
            Ok(GitAnswer::Text(text)) => text,
            _ => None,
        }
    }

    fn commit_files(&self, repo: &RepoSource, sha: &str) -> Vec<FileChange> {
        match self.read(repo, GitRead::CommitFiles { sha: sha.into() }) {
            Ok(GitAnswer::Files(files)) => files,
            _ => Vec::new(),
        }
    }

    fn parent_of(&self, repo: &RepoSource, sha: &str) -> Option<String> {
        match self.read(repo, GitRead::ParentOf { sha: sha.into() }) {
            Ok(GitAnswer::Text(text)) => text,
            _ => None,
        }
    }

    fn patch(&self, repo: &RepoSource, against: &str) -> Option<String> {
        match self.read(
            repo,
            GitRead::Patch {
                against: against.into(),
            },
        ) {
            Ok(GitAnswer::Text(text)) => text,
            _ => None,
        }
    }

    fn head_message(&self, repo: &RepoSource) -> Option<String> {
        match self.read(repo, GitRead::HeadMessage) {
            Ok(GitAnswer::Text(text)) => text,
            _ => None,
        }
    }

    fn head_state(&self, repo: &RepoSource) -> Option<HeadState> {
        match self.read(repo, GitRead::HeadState) {
            Ok(GitAnswer::Head(head)) => Some(head),
            _ => None,
        }
    }

    fn push_remotes(&self, repo: &RepoSource, branch: &str) -> Vec<String> {
        match self.read(
            repo,
            GitRead::PushRemotes {
                branch: branch.into(),
            },
        ) {
            Ok(GitAnswer::Names(names)) => names,
            _ => Vec::new(),
        }
    }

    fn remote_url(&self, repo: &RepoSource, remote: &str) -> Option<String> {
        match self.read(
            repo,
            GitRead::RemoteUrl {
                remote: remote.into(),
            },
        ) {
            Ok(GitAnswer::Text(text)) => text,
            _ => None,
        }
    }
}

/// The repos of a workspace whose git runs elsewhere, for the git of the files open in its editor.
#[derive(Clone)]
pub struct RepoFiles {
    access: Arc<dyn GitAccess>,
    repos: Vec<RepoSource>,
}

impl RepoFiles {
    pub fn new(access: Arc<dyn GitAccess>, repos: Vec<RepoSource>) -> RepoFiles {
        RepoFiles { access, repos }
    }

    /// The git of the file at `full`, in the deepest repo holding it.
    pub fn file(&self, full: &Path) -> Option<Arc<dyn FileGit>> {
        let (repo, relative) = self
            .repos
            .iter()
            .filter_map(|repo| Some((repo, full.strip_prefix(&repo.root).ok()?)))
            .max_by_key(|(repo, _)| repo.root.components().count())?;
        let path = relative
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()?
            .join("/");
        check_path(&path).ok()?;
        Some(Arc::new(RepoFile {
            access: self.access.clone(),
            repo: repo.clone(),
            path,
        }))
    }
}

struct RepoFile {
    access: Arc<dyn GitAccess>,
    repo: RepoSource,
    path: String,
}

impl FileGit for RepoFile {
    fn bases(&self) -> Option<DiffBases> {
        match self.access.read(
            &self.repo,
            GitRead::Bases {
                path: self.path.clone(),
            },
        ) {
            Ok(GitAnswer::Bases(bases)) => bases,
            _ => None,
        }
    }

    fn blame(&self, contents: &str) -> Option<Vec<BlameEntry>> {
        let query = GitRead::Blame {
            path: self.path.clone(),
            contents: contents.to_string(),
        };
        match self.access.read(&self.repo, query) {
            Ok(GitAnswer::Blame(entries)) => entries,
            _ => None,
        }
    }

    fn write_index(&self, text: Option<&str>) -> Result<(), String> {
        let op = GitOp::WriteIndex {
            path: self.path.clone(),
            text: text.map(str::to_string),
        };
        self.access
            .act(&self.repo, op)
            .map_err(|error| error.message)
    }
}

/// The repos on this machine, read and changed in place.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalGit;

impl GitAccess for LocalGit {
    fn scan(&self, repo: &RepoSource) -> RepoScan {
        RepoScan::read(&repo.root, &repo.default_branch)
    }

    fn act(&self, repo: &RepoSource, op: GitOp) -> Result<(), CommandError> {
        apply(&repo.root, &op)
    }

    fn read(&self, repo: &RepoSource, query: GitRead) -> Result<GitAnswer, String> {
        answer(&repo.root, &query)
    }

    fn remote(
        &self,
        repo: &RepoSource,
        command: RemoteCommand,
        _progress: &(dyn Fn(&str) + Sync),
    ) -> Result<CommandOutput, CommandError> {
        run_remote(&repo.root, &command, None)
    }
}

/// Repos this window cannot reach, saying why for every request.
#[derive(Clone, Debug)]
pub struct UnservedGit {
    reason: String,
}

impl UnservedGit {
    pub fn new(reason: impl Into<String>) -> UnservedGit {
        UnservedGit {
            reason: reason.into(),
        }
    }
}

impl GitAccess for UnservedGit {
    fn scan(&self, repo: &RepoSource) -> RepoScan {
        RepoScan::failed(&repo.root, self.reason.clone())
    }

    fn act(&self, _repo: &RepoSource, _op: GitOp) -> Result<(), CommandError> {
        Err(refused(self.reason.clone()))
    }

    fn read(&self, _repo: &RepoSource, _query: GitRead) -> Result<GitAnswer, String> {
        Err(self.reason.clone())
    }

    fn remote(
        &self,
        _repo: &RepoSource,
        _command: RemoteCommand,
        _progress: &(dyn Fn(&str) + Sync),
    ) -> Result<CommandOutput, CommandError> {
        Err(refused(self.reason.clone()))
    }

    fn refusal(&self) -> Option<String> {
        Some(self.reason.clone())
    }
}

/// Whether git's output says a remote refused to let it sign in (no key, no token, or a prompt it could not show).
pub fn sign_in_failed(output: &CommandOutput) -> bool {
    const SIGNS: &[&str] = &[
        "Authentication failed",
        "could not read Username",
        "could not read Password",
        "terminal prompts disabled",
        "Permission denied (publickey",
        "Host key verification failed",
        "invalid credentials",
        "Invalid username or password",
        "The requested URL returned error: 403",
        "The requested URL returned error: 401",
    ];
    SIGNS
        .iter()
        .any(|sign| output.stderr.contains(sign) || output.stdout.contains(sign))
}
