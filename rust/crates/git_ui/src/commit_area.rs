use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use git::working_copy::{
    self, CommandError, CommandOutput, CommitOptions, HeadState, PushMode, Staging, StatusEntry,
    Upstream,
};
use ui::{div, theme, Node};
use workspace::text_field::TextArea;
use workspace::PanelRequest;

const EDITOR_ROWS: usize = 6;
const EDITOR_FONT: f32 = 13.0;
const EDITOR_PAD: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteKind {
    Fetch,
    Pull,
    Push,
}

impl RemoteKind {
    fn name(self) -> &'static str {
        match self {
            RemoteKind::Fetch => "fetch",
            RemoteKind::Pull => "pull",
            RemoteKind::Push => "push",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRequest {
    /// `None` fetches every remote.
    Fetch(Option<String>),
    Pull {
        rebase: bool,
    },
    Push {
        force: bool,
    },
    /// Pull, then push what is left to push.
    Sync,
}

impl RemoteRequest {
    fn kind(&self) -> RemoteKind {
        match self {
            RemoteRequest::Fetch(_) => RemoteKind::Fetch,
            RemoteRequest::Pull { .. } => RemoteKind::Pull,
            RemoteRequest::Push { .. } | RemoteRequest::Sync => RemoteKind::Push,
        }
    }
}

/// One repo's part of a remote operation: where it runs, the branch it acts on, what to do and with which remote.
pub(crate) struct RemoteJob {
    pub repo: String,
    pub root: PathBuf,
    pub head: HeadState,
    pub request: RemoteRequest,
    pub remote: String,
}

/// A repo a commit would go into: its staged file count and whether unresolved conflicts block it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitTarget {
    pub repo: String,
    pub root: PathBuf,
    pub staged: usize,
    pub conflicts: bool,
}

pub(crate) enum Outcome {
    Toast(String),
    WithLog {
        action: &'static str,
        message: String,
        log: String,
    },
    PullRequest {
        message: String,
        label: &'static str,
        url: String,
    },
    Failed {
        action: &'static str,
        log: String,
    },
    Committed,
}

struct Running {
    answer: Receiver<Outcome>,
    remote: Option<RemoteKind>,
}

pub(crate) struct CommitArea {
    pub editor: TextArea,
    pub focused: bool,
    pub amend: bool,
    pub signoff: bool,
    pub skip_hooks: bool,
    running: Option<Running>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

pub(crate) struct CommitPlan {
    pub title: &'static str,
    pub enabled: bool,
    /// The git command the button runs, or why it can't.
    pub hint: String,
    /// What the button will make: "1 commit in api (2 files)", "2 commits: api 2, web 1".
    pub summary: String,
    pub targets: Vec<CommitTarget>,
}

impl CommitArea {
    pub fn new(waker: Arc<dyn Fn() + Send + Sync>) -> CommitArea {
        let mut editor = TextArea::default();
        editor.set_font_size(EDITOR_FONT);
        CommitArea {
            editor,
            focused: false,
            amend: false,
            signoff: false,
            skip_hooks: false,
            running: None,
            waker,
        }
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn remote_running(&self) -> Option<RemoteKind> {
        self.running.as_ref().and_then(|running| running.remote)
    }

    pub fn editor_height(&self) -> f32 {
        EDITOR_PAD + self.editor.line_height() * EDITOR_ROWS as f32
    }

    /// `staged` lists every repo with staged files; `amend_target` is the repo whose last commit an amend
    /// with nothing staged rewrites.
    pub fn plan(
        &self,
        staged: &[CommitTarget],
        excluded: &HashSet<String>,
        suggestion: bool,
        amend_target: Option<&CommitTarget>,
    ) -> CommitPlan {
        let mut targets: Vec<CommitTarget> = staged
            .iter()
            .filter(|target| !excluded.contains(&target.repo))
            .cloned()
            .collect();
        if self.amend && targets.is_empty() && staged.is_empty() {
            targets.extend(amend_target.cloned());
        }
        let has_message = !self.editor.text().trim().is_empty() || suggestion;
        let why_not = if self.busy() {
            Some("Commit in progress")
        } else if targets.iter().any(|target| target.conflicts) {
            Some("You must resolve conflicts before committing")
        } else if staged.is_empty() && !self.amend {
            Some("Stage files to commit")
        } else if targets.is_empty() && !staged.is_empty() {
            Some("Every repo with staged files is left out")
        } else if targets.is_empty() {
            Some("No commit to amend")
        } else if !has_message {
            Some("Enter a commit message")
        } else {
            None
        };
        let flags: String = [
            (self.amend, " --amend"),
            (self.signoff, " --signoff"),
            (self.skip_hooks, " --no-verify"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, flag)| *flag)
        .collect();
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        let summary = match targets.as_slice() {
            [] if staged.is_empty() => "nothing staged".to_string(),
            [] => "no repo chosen".to_string(),
            [only] if self.amend && only.staged == 0 => {
                format!("amend the last commit in {}", only.repo)
            }
            [only] => format!(
                "1 commit in {} ({} file{})",
                only.repo,
                only.staged,
                plural(only.staged)
            ),
            many => format!(
                "{} commits: {}",
                many.len(),
                many.iter()
                    .map(|target| format!("{} {}", target.repo, target.staged))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        CommitPlan {
            title: if self.amend { "Amend" } else { "Commit" },
            enabled: why_not.is_none(),
            hint: why_not.map_or_else(|| format!("git commit{flags}"), str::to_string),
            summary,
            targets,
        }
    }

    /// One commit per target repo with the same message, each holding that repo's staged files.
    pub fn commit(&mut self, targets: Vec<CommitTarget>, message: String) {
        if self.busy() || targets.is_empty() {
            return;
        }
        let options = CommitOptions {
            amend: self.amend,
            signoff: self.signoff,
            no_verify: self.skip_hooks,
        };
        let repos = targets.len();
        self.start(None, move || {
            let failures: Vec<String> = targets
                .iter()
                .filter_map(|target| {
                    working_copy::commit(&target.root, &message, options)
                        .err()
                        .map(|error| labelled(&target.repo, &error, repos))
                })
                .collect();
            if failures.is_empty() {
                Outcome::Committed
            } else {
                Outcome::Failed {
                    action: "commit",
                    log: failures.join("\n\n"),
                }
            }
        });
    }

    pub fn toggle_amend(&mut self, root: &std::path::Path) {
        self.amend = !self.amend;
        if self.amend && self.editor.text().trim().is_empty() {
            if let Some(message) = working_copy::head_message(root) {
                self.editor.set_text(&message);
            }
        }
    }

    /// Runs each repo's job in turn; a single job reports as itself, several as one summary.
    pub fn run_remote(&mut self, jobs: Vec<RemoteJob>) {
        if self.busy() || jobs.is_empty() {
            return;
        }
        let kind = jobs
            .iter()
            .map(|job| job.request.kind())
            .reduce(|first, next| {
                if first == next {
                    first
                } else {
                    RemoteKind::Push
                }
            })
            .unwrap_or(RemoteKind::Fetch);
        self.start(Some(kind), move || {
            if let [job] = jobs.as_slice() {
                return remote_outcome(&job.root, &job.head, &job.request, &job.remote);
            }
            let outcomes: Vec<(String, Outcome)> = jobs
                .iter()
                .map(|job| {
                    (
                        job.repo.clone(),
                        remote_outcome(&job.root, &job.head, &job.request, &job.remote),
                    )
                })
                .collect();
            combine_outcomes(kind, outcomes)
        });
    }

    fn start(
        &mut self,
        remote: Option<RemoteKind>,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        let (sender, answer) = mpsc::channel();
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("git-panel-op".into())
            .spawn(move || {
                if sender.send(work()).is_err() {
                    eprintln!("git panel: the panel closed before its operation finished");
                }
                waker();
            });
        match spawned {
            Ok(_) => self.running = Some(Running { answer, remote }),
            Err(error) => eprintln!("git panel: operation thread: {error}"),
        }
    }

    pub fn poll(&mut self) -> Option<Outcome> {
        let answer = match self.running.as_ref()?.answer.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Outcome::Failed {
                action: "operation",
                log: "The operation stopped unexpectedly.".into(),
            },
        };
        self.running = None;
        if let Outcome::Committed = answer {
            self.editor.set_text("");
            self.amend = false;
            self.skip_hooks = false;
        }
        Some(answer)
    }

    pub fn render_editor(&mut self, width: f32, placeholder: &str, click_id: u64) -> Node {
        let rows_h = self.editor.line_height() * EDITOR_ROWS as f32;
        let text = self.editor.render(
            placeholder,
            self.focused,
            (width - 2.0 * EDITOR_PAD).max(40.0),
            EDITOR_ROWS,
        );
        div()
            .col()
            .w_px(width)
            .h_px(EDITOR_PAD + rows_h)
            .pt(EDITOR_PAD)
            .px(EDITOR_PAD)
            .bg(theme().editor_background)
            .on_click(click_id)
            .child(text)
            .into()
    }
}

/// "Update|Create|Delete <file>" when exactly one file is staged, or nothing is and one tracked file changed.
pub(crate) fn suggested_message(status: &[StatusEntry]) -> Option<String> {
    let staged: Vec<&StatusEntry> = status
        .iter()
        .filter(|entry| entry.staging() != Staging::Unstaged)
        .collect();
    let entry = match staged.as_slice() {
        [single] => *single,
        [] => {
            let tracked: Vec<&StatusEntry> =
                status.iter().filter(|entry| !entry.untracked).collect();
            match tracked.as_slice() {
                [single] => *single,
                _ => return None,
            }
        }
        _ => return None,
    };
    let column = if entry.index != '.' {
        entry.index
    } else {
        entry.worktree
    };
    let action = match column {
        'D' => "Delete",
        'A' | '?' => "Create",
        'M' | 'R' | 'T' => "Update",
        _ => return None,
    };
    let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
    Some(format!("{action} {name}"))
}

fn failed(action: &'static str, error: &CommandError) -> Outcome {
    let log = error.output.log();
    Outcome::Failed {
        action,
        log: if log.trim().is_empty() {
            error.message.clone()
        } else {
            log
        },
    }
}

fn labelled(repo: &str, error: &CommandError, repos: usize) -> String {
    let log = error.output.log();
    let text = if log.trim().is_empty() {
        error.message.clone()
    } else {
        log
    };
    if repos > 1 {
        format!("{repo}:\n{text}")
    } else {
        text
    }
}

fn remote_outcome(
    root: &std::path::Path,
    head: &HeadState,
    request: &RemoteRequest,
    remote: &str,
) -> Outcome {
    let branch = head.branch.clone().unwrap_or_default();
    let push = |force: bool| {
        let mode = if force {
            PushMode::ForceWithLease
        } else if matches!(head.upstream, Upstream::None | Upstream::Gone) {
            PushMode::SetUpstream
        } else {
            PushMode::Normal
        };
        let remote_branch = head
            .upstream_ref
            .as_ref()
            .filter(|(tracked_remote, _)| tracked_remote == remote)
            .map_or(branch.clone(), |(_, name)| name.clone());
        working_copy::push(root, remote, &branch, &remote_branch, mode)
    };
    let pull = |rebase: bool| {
        let branch_arg = (head.upstream_ref.is_none()).then_some(branch.as_str());
        working_copy::pull(root, remote, branch_arg, rebase)
    };
    match request {
        RemoteRequest::Fetch(target) => match working_copy::fetch(root, target.as_deref()) {
            Ok(output) => format_fetch(target.as_deref(), output),
            Err(error) => failed(RemoteKind::Fetch.name(), &error),
        },
        RemoteRequest::Pull { rebase } => match pull(*rebase) {
            Ok(output) => format_pull(remote, output),
            Err(error) => failed(RemoteKind::Pull.name(), &error),
        },
        RemoteRequest::Push { force } => match push(*force) {
            Ok(output) => format_push(&branch, remote, output),
            Err(error) => failed(RemoteKind::Push.name(), &error),
        },
        RemoteRequest::Sync => {
            let pulled = match pull(false) {
                Ok(output) => output,
                Err(error) => return failed(RemoteKind::Pull.name(), &error),
            };
            match push(false) {
                Ok(pushed) => Outcome::WithLog {
                    action: "sync",
                    message: format!("Synced {branch} with {remote}"),
                    log: format!("{}\n{}", pulled.log(), pushed.log()),
                },
                Err(error) => failed(RemoteKind::Push.name(), &error),
            }
        }
    }
}

/// Several repos' results as one toast: what each did, or every failure with its repo.
fn combine_outcomes(kind: RemoteKind, outcomes: Vec<(String, Outcome)>) -> Outcome {
    let mut messages = Vec::new();
    let mut logs = Vec::new();
    let mut failures = Vec::new();
    for (repo, outcome) in outcomes {
        match outcome {
            Outcome::Toast(message) => messages.push(format!("{repo}: {message}")),
            Outcome::WithLog { message, log, .. } => {
                messages.push(format!("{repo}: {message}"));
                logs.push(format!("{repo}:\n{log}"));
            }
            Outcome::PullRequest { message, .. } => messages.push(format!("{repo}: {message}")),
            Outcome::Failed { log, .. } => failures.push(format!("{repo}:\n{log}")),
            Outcome::Committed => {}
        }
    }
    if !failures.is_empty() {
        failures.extend(logs);
        return Outcome::Failed {
            action: kind.name(),
            log: failures.join("\n\n"),
        };
    }
    Outcome::WithLog {
        action: kind.name(),
        message: messages.join("; "),
        log: logs.join("\n\n"),
    }
}

const PULL_REQUEST_HINTS: &[(&str, &str)] = &[
    ("Create a pull request", "Create Pull Request"),
    ("Create pull request", "Create Pull Request"),
    ("create a merge request", "Create Merge Request"),
    ("View merge request", "View Merge Request"),
];

/// The "open a pull request" link a host prints after a push (`remote:` lines).
fn pull_request_link(stderr: &str) -> Option<(&'static str, String)> {
    let mut pending: Option<&'static str> = None;
    for line in stderr.lines() {
        let Some(remote_line) = line.trim_start().strip_prefix("remote:") else {
            pending = None;
            continue;
        };
        if let Some((_, label)) = PULL_REQUEST_HINTS
            .iter()
            .find(|(hint, _)| remote_line.contains(hint))
        {
            pending = Some(label);
        }
        let url = remote_line
            .find("https://")
            .or_else(|| remote_line.find("http://"))
            .and_then(|at| remote_line[at..].split_whitespace().next())
            .map(|url| url.trim_end_matches([',', '.', ')', ']', '>']).to_string());
        if let (Some(url), Some(label)) = (url, pending) {
            return Some((label, url));
        }
    }
    None
}

fn format_fetch(remote: Option<&str>, output: CommandOutput) -> Outcome {
    if output.stderr.is_empty() {
        return Outcome::Toast("Fetch: Already up to date".into());
    }
    Outcome::WithLog {
        action: "fetch",
        message: match remote {
            Some(remote) => format!("Synchronized with {remote}"),
            None => "Synchronized with remotes".into(),
        },
        log: output.log(),
    }
}

fn format_pull(remote: &str, output: CommandOutput) -> Outcome {
    let files_changed = output
        .stdout
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().next())
        .and_then(|count| count.parse::<u32>().ok());
    let plural = |count: u32| if count == 1 { "" } else { "s" };
    let message = if output.stdout.ends_with("Already up to date.\n") {
        return Outcome::Toast("Pull: Already up to date".into());
    } else if output.stdout.starts_with("Updating") {
        match files_changed {
            Some(count) => format!(
                "Received {count} file change{} from {remote}",
                plural(count)
            ),
            None => format!("Fast forwarded from {remote}"),
        }
    } else if output.stdout.starts_with("Merge") {
        match files_changed {
            Some(count) => format!("Merged {count} file change{} from {remote}", plural(count)),
            None => format!("Merged from {remote}"),
        }
    } else if output.stdout.contains("Successfully rebased") {
        format!("Successfully rebased from {remote}")
    } else {
        format!("Successfully pulled from {remote}")
    };
    Outcome::WithLog {
        action: "pull",
        message,
        log: output.log(),
    }
}

fn format_push(branch: &str, remote: &str, output: CommandOutput) -> Outcome {
    if output.stderr.ends_with("Everything up-to-date\n") {
        return Outcome::Toast("Push: Everything is up-to-date".into());
    }
    let message = format!("Pushed {branch} to {remote}");
    match pull_request_link(&output.stderr) {
        Some((label, url)) => Outcome::PullRequest {
            message,
            label,
            url,
        },
        None => Outcome::WithLog {
            action: "push",
            message,
            log: output.log(),
        },
    }
}

pub(crate) fn outcome_requests(outcome: Outcome) -> Vec<PanelRequest> {
    let log_tab = |action: &str, log: String| {
        let title = format!("Output from git {action}");
        PanelRequest::OpenItem(files_ui::text_tab(
            format!("git-output:{action}"),
            title,
            &log,
        ))
    };
    match outcome {
        Outcome::Toast(message) => vec![PanelRequest::Toast(message)],
        Outcome::WithLog {
            action,
            message,
            log,
        } => vec![PanelRequest::ToastAction {
            message,
            action: "View Log".into(),
            then: Box::new(log_tab(action, log)),
        }],
        Outcome::PullRequest {
            message,
            label,
            url,
        } => vec![PanelRequest::ToastAction {
            message,
            action: label.into(),
            then: Box::new(PanelRequest::OpenUrl(url)),
        }],
        Outcome::Failed { action, log } => vec![PanelRequest::ToastAction {
            message: format!("git {action} failed"),
            action: "View Log".into(),
            then: Box::new(log_tab(action, log)),
        }],
        Outcome::Committed => Vec::new(),
    }
}

/// What a repo's one remote button offers, as its label says: `(label, request, ahead, behind)`.
pub(crate) fn remote_action(head: &HeadState) -> Option<(&'static str, RemoteRequest, u32, u32)> {
    head.branch.as_ref()?;
    Some(match head.upstream {
        Upstream::Tracked {
            ahead: 0,
            behind: 0,
        } => ("Up to date", RemoteRequest::Fetch(None), 0, 0),
        Upstream::Tracked { ahead, behind: 0 } => {
            ("Push", RemoteRequest::Push { force: false }, ahead, 0)
        }
        Upstream::Tracked { ahead: 0, behind } => {
            ("Pull", RemoteRequest::Pull { rebase: false }, 0, behind)
        }
        Upstream::Tracked { ahead, behind } => ("Sync", RemoteRequest::Sync, ahead, behind),
        Upstream::Gone | Upstream::None => ("Publish", RemoteRequest::Push { force: false }, 0, 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, index: char, worktree: char) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            index,
            worktree,
            untracked: index == '.' && worktree == '?',
            conflicted: false,
            original_path: None,
        }
    }

    #[test]
    fn the_suggestion_names_the_single_file_and_what_happened_to_it() {
        assert_eq!(
            suggested_message(&[entry("src/app.rs", 'M', '.')]).as_deref(),
            Some("Update app.rs")
        );
        assert_eq!(
            suggested_message(&[entry("new.rs", 'A', '.'), entry("other.rs", '.', 'M')]).as_deref(),
            Some("Create new.rs")
        );
        assert_eq!(
            suggested_message(&[entry("gone.rs", '.', 'D'), entry("notes.md", '.', '?')])
                .as_deref(),
            Some("Delete gone.rs")
        );
        assert_eq!(
            suggested_message(&[entry("a.rs", '.', 'M'), entry("b.rs", '.', 'M')]),
            None
        );
    }

    fn target(repo: &str, staged: usize) -> CommitTarget {
        CommitTarget {
            repo: repo.into(),
            root: PathBuf::from("/tmp").join(repo),
            staged,
            conflicts: false,
        }
    }

    #[test]
    fn the_plan_says_what_a_commit_would_make_or_why_not() {
        let mut area = CommitArea::new(Arc::new(|| {}));
        let none = HashSet::new();
        let plan = area.plan(&[], &none, false, None);
        assert!(!plan.enabled);
        assert_eq!(plan.hint, "Stage files to commit");
        assert_eq!(plan.summary, "nothing staged");
        let staged = [target("api", 2), target("web", 1)];
        let plan = area.plan(&staged, &none, false, None);
        assert_eq!(plan.hint, "Enter a commit message");
        assert_eq!(plan.summary, "2 commits: api 2, web 1");
        let plan = area.plan(&staged, &none, true, None);
        assert!(plan.enabled, "a suggestion stands in for an empty message");
        assert_eq!(plan.hint, "git commit");
        let without_web: HashSet<String> = ["web".to_string()].into();
        let plan = area.plan(&staged, &without_web, true, None);
        assert_eq!(plan.summary, "1 commit in api (2 files)");
        let everything_out: HashSet<String> = ["web".to_string(), "api".to_string()].into();
        assert_eq!(
            area.plan(&staged, &everything_out, true, None).hint,
            "Every repo with staged files is left out"
        );
        area.amend = true;
        area.signoff = true;
        let last = target("api", 0);
        let plan = area.plan(&[], &none, true, Some(&last));
        assert_eq!(plan.title, "Amend");
        assert_eq!(plan.hint, "git commit --amend --signoff");
        assert_eq!(plan.summary, "amend the last commit in api");
        let mut conflicted = target("api", 1);
        conflicted.conflicts = true;
        assert!(!area.plan(&[conflicted], &none, true, None).enabled);
    }

    #[test]
    fn remote_output_becomes_the_right_toast() {
        let pushed = format_push(
            "feat-login",
            "origin",
            CommandOutput {
                stdout: String::new(),
                stderr: "remote:\nremote: Create a pull request for 'feat-login' on GitHub by visiting:\nremote:      https://github.com/acme/web/pull/new/feat-login\nremote:\nTo github.com:acme/web.git\n * [new branch]      feat-login -> feat-login\n".into(),
            },
        );
        match pushed {
            Outcome::PullRequest {
                message,
                label,
                url,
            } => {
                assert_eq!(message, "Pushed feat-login to origin");
                assert_eq!(label, "Create Pull Request");
                assert_eq!(url, "https://github.com/acme/web/pull/new/feat-login");
            }
            _ => panic!("a new branch push should offer the pull request page"),
        }
        assert!(matches!(
            format_push("main", "origin", CommandOutput { stdout: String::new(), stderr: "Everything up-to-date\n".into() }),
            Outcome::Toast(message) if message == "Push: Everything is up-to-date"
        ));
        assert!(matches!(
            format_pull("origin", CommandOutput { stdout: "Updating 1..2\nFast-forward\n a.rs | 2 +-\n 3 files changed, 4 insertions(+)\n".into(), stderr: String::new() }),
            Outcome::WithLog { message, .. } if message == "Received 3 file changes from origin"
        ));
        assert!(matches!(
            format_fetch(None, CommandOutput::default()),
            Outcome::Toast(message) if message == "Fetch: Already up to date"
        ));
    }

    #[test]
    fn the_remote_button_follows_the_upstream() {
        let head = |upstream| HeadState {
            branch: Some("feat".into()),
            short_sha: None,
            upstream,
            upstream_ref: None,
        };
        let label = |upstream| remote_action(&head(upstream)).map(|action| (action.0, action.1));
        assert_eq!(
            label(Upstream::None),
            Some(("Publish", RemoteRequest::Push { force: false }))
        );
        assert_eq!(
            label(Upstream::Gone).map(|action| action.0),
            Some("Publish")
        );
        assert_eq!(
            label(Upstream::Tracked {
                ahead: 2,
                behind: 0
            }),
            Some(("Push", RemoteRequest::Push { force: false }))
        );
        assert_eq!(
            label(Upstream::Tracked {
                ahead: 0,
                behind: 3
            }),
            Some(("Pull", RemoteRequest::Pull { rebase: false }))
        );
        assert_eq!(
            label(Upstream::Tracked {
                ahead: 1,
                behind: 3
            }),
            Some(("Sync", RemoteRequest::Sync))
        );
        assert_eq!(
            label(Upstream::Tracked {
                ahead: 0,
                behind: 0
            }),
            Some(("Up to date", RemoteRequest::Fetch(None)))
        );
    }
}
