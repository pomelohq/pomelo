use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use git::working_copy::{
    self, CommandError, CommandOutput, CommitOptions, HeadState, PushMode, Staging, StatusEntry,
    Upstream,
};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::text_field::TextArea;
use workspace::PanelRequest;

pub(crate) const EDITOR_ROWS: usize = 6;
const EDITOR_FONT: f32 = 13.0;
const EDITOR_PAD: f32 = 8.0;
const REPO_ROW_H: f32 = 30.0;
const COMMIT_ROW_H: f32 = 34.0;
const BUTTON_H: f32 = 22.0;
const CHEVRON_W: f32 = 20.0;

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
    PushTo {
        force: bool,
        remote: String,
    },
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

    pub fn height(&self) -> f32 {
        REPO_ROW_H
            + 1.0
            + EDITOR_PAD
            + self.editor.line_height() * EDITOR_ROWS as f32
            + 1.0
            + COMMIT_ROW_H
    }

    pub fn plan(&self, status: &[StatusEntry], has_head: bool) -> CommitPlan {
        let staged = status
            .iter()
            .any(|entry| entry.staging() != Staging::Unstaged);
        let tracked = status.iter().any(|entry| !entry.is_new());
        let conflicts = status.iter().any(|entry| entry.conflicted);
        let title = match (self.amend, staged, tracked) {
            (true, false, true) => "Amend Tracked",
            (true, _, _) => "Amend",
            (false, true, _) => "Commit",
            (false, false, _) => "Commit Tracked",
        };
        let has_message =
            !self.editor.text().trim().is_empty() || suggested_message(status).is_some();
        let enabled = !self.busy()
            && !conflicts
            && (staged || tracked || (self.amend && has_head))
            && has_message;
        CommitPlan { title, enabled }
    }

    /// Commits the staged files, or every tracked change when nothing is staged.
    pub fn commit(&mut self, root: PathBuf, status: &[StatusEntry]) -> Option<PanelRequest> {
        if self.busy() {
            return None;
        }
        if status
            .iter()
            .any(|entry| entry.conflicted && entry.staging() != Staging::Staged)
        {
            return Some(PanelRequest::Toast(
                "There are still conflicts. You must stage these before committing".into(),
            ));
        }
        let typed = self.editor.text();
        let message = if typed.trim().is_empty() {
            suggested_message(status)?
        } else {
            typed
        };
        let nothing_staged = status
            .iter()
            .all(|entry| entry.staging() == Staging::Unstaged);
        let tracked: Vec<String> = if nothing_staged {
            status
                .iter()
                .filter(|entry| !entry.is_new())
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        if nothing_staged && tracked.is_empty() && !self.amend {
            return Some(PanelRequest::Toast("No changes to commit".into()));
        }
        let options = CommitOptions {
            amend: self.amend,
            signoff: self.signoff,
            no_verify: self.skip_hooks,
        };
        self.start(None, move || {
            let staged = working_copy::stage(&root, &tracked);
            match staged.and_then(|()| working_copy::commit(&root, &message, options)) {
                Ok(()) => Outcome::Committed,
                Err(error) => failed("commit", &error),
            }
        });
        None
    }

    pub fn toggle_amend(&mut self, root: &std::path::Path) {
        self.amend = !self.amend;
        if self.amend && self.editor.text().trim().is_empty() {
            if let Some(message) = working_copy::head_message(root) {
                self.editor.set_text(&message);
            }
        }
    }

    pub fn run_remote(
        &mut self,
        root: PathBuf,
        head: HeadState,
        request: RemoteRequest,
        remote: String,
    ) {
        if self.busy() {
            return;
        }
        let kind = match request {
            RemoteRequest::Fetch(_) => RemoteKind::Fetch,
            RemoteRequest::Pull { .. } => RemoteKind::Pull,
            RemoteRequest::Push { .. } | RemoteRequest::PushTo { .. } => RemoteKind::Push,
        };
        self.start(Some(kind), move || {
            remote_outcome(&root, &head, &request, &remote)
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

fn remote_outcome(
    root: &std::path::Path,
    head: &HeadState,
    request: &RemoteRequest,
    remote: &str,
) -> Outcome {
    let branch = head.branch.clone().unwrap_or_default();
    match request {
        RemoteRequest::Fetch(target) => match working_copy::fetch(root, target.as_deref()) {
            Ok(output) => format_fetch(target.as_deref(), output),
            Err(error) => failed(RemoteKind::Fetch.name(), &error),
        },
        RemoteRequest::Pull { rebase } => {
            let branch_arg = (head.upstream_ref.is_none()).then_some(branch.as_str());
            match working_copy::pull(root, remote, branch_arg, *rebase) {
                Ok(output) => format_pull(remote, output),
                Err(error) => failed(RemoteKind::Pull.name(), &error),
            }
        }
        RemoteRequest::Push { force } | RemoteRequest::PushTo { force, .. } => {
            let mode = if *force {
                PushMode::ForceWithLease
            } else if matches!(head.upstream, Upstream::None | Upstream::Gone)
                || matches!(request, RemoteRequest::PushTo { .. })
            {
                PushMode::SetUpstream
            } else {
                PushMode::Normal
            };
            let remote_branch = head
                .upstream_ref
                .as_ref()
                .filter(|(tracked_remote, _)| tracked_remote == remote)
                .map_or(branch.clone(), |(_, name)| name.clone());
            match working_copy::push(root, remote, &branch, &remote_branch, mode) {
                Ok(output) => format_push(&branch, remote, output),
                Err(error) => failed(RemoteKind::Push.name(), &error),
            }
        }
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

pub(crate) fn remote_button_state(
    head: &HeadState,
) -> Option<(&'static str, Option<IconKind>, u32, u32)> {
    head.branch.as_ref()?;
    Some(match head.upstream {
        Upstream::Tracked {
            ahead: 0,
            behind: 0,
        } => ("Fetch", Some(IconKind::RotateCw), 0, 0),
        Upstream::Tracked { ahead, behind: 0 } => ("Push", None, ahead, 0),
        Upstream::Tracked { ahead, behind } => ("Pull", None, ahead, behind),
        Upstream::Gone => ("Republish", Some(IconKind::ExpandUp), 0, 0),
        Upstream::None => ("Publish", Some(IconKind::ExpandUp), 0, 0),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn split_button(
    main_id: u64,
    menu_id: u64,
    leading: Vec<Node>,
    text: &str,
    enabled: bool,
    menu_open: bool,
    hot: Option<u64>,
) -> Node {
    let colors = theme();
    let fill = |id: u64| {
        if hot == Some(id) {
            colors.element_hover
        } else {
            colors.element_background
        }
    };
    let mut main = div()
        .row()
        .h_px(BUTTON_H)
        .px(6.0)
        .gap(3.0)
        .items_center()
        .bg(if enabled {
            fill(main_id)
        } else {
            colors.element_background
        })
        .border(1.0, colors.border_variant);
    if enabled {
        main = main.on_click(main_id);
    }
    for node in leading {
        main = main.child(node);
    }
    main = main.child(label(text.to_string()).size(12.0).color(if enabled {
        colors.text
    } else {
        colors.text_disabled
    }));
    let chevron = div()
        .row()
        .w_px(CHEVRON_W)
        .h_px(BUTTON_H)
        .items_center()
        .justify_center()
        .bg(if menu_open {
            colors.element_selected
        } else {
            fill(menu_id)
        })
        .border(1.0, colors.border_variant)
        .on_click(menu_id)
        .child(
            icon(if menu_open {
                IconKind::ChevronUp
            } else {
                IconKind::ChevronDown
            })
            .size(10.0)
            .color(colors.icon_muted),
        );
    div()
        .row()
        .rounded(4.0)
        .items_center()
        .child(main)
        .child(chevron)
        .into()
}

pub(crate) fn stage_checkbox(id: u64, staging: Staging, hot: bool) -> Node {
    let colors = theme();
    let border = if hot {
        let border = colors.border;
        Rgba::new(border.r, border.g, border.b, border.a * 0.7)
    } else {
        colors.border
    };
    let mut square = div()
        .w_px(16.0)
        .h_px(16.0)
        .rounded(2.0)
        .items_center()
        .justify_center()
        .border(1.0, border);
    square = match staging {
        Staging::Staged => square.child(icon(IconKind::Check).size(14.0).color(colors.text_accent)),
        Staging::Partial => square.child(icon(IconKind::Dash).size(14.0).color(colors.text_accent)),
        Staging::Unstaged => square,
    };
    div()
        .w_px(20.0)
        .h_px(20.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(square)
        .into()
}

pub(crate) fn repo_row_height() -> f32 {
    REPO_ROW_H
}

pub(crate) fn commit_row_height() -> f32 {
    COMMIT_ROW_H
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

    #[test]
    fn the_button_says_what_a_commit_would_take() {
        let area = CommitArea::new(Arc::new(|| {}));
        let unstaged = [entry("a.rs", '.', 'M'), entry("b.rs", '.', 'M')];
        let plan = area.plan(&unstaged, true);
        assert_eq!(plan.title, "Commit Tracked");
        assert!(!plan.enabled, "no message and no single-file suggestion");
        let staged = [entry("a.rs", 'M', '.')];
        let plan = area.plan(&staged, true);
        assert_eq!(plan.title, "Commit");
        assert!(
            plan.enabled,
            "the suggestion stands in for an empty message"
        );
        assert!(!area.plan(&[entry("new.rs", '.', '?')], true).enabled);
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
        assert_eq!(
            remote_button_state(&head(Upstream::None)).map(|state| state.0),
            Some("Publish")
        );
        assert_eq!(
            remote_button_state(&head(Upstream::Gone)).map(|state| state.0),
            Some("Republish")
        );
        assert_eq!(
            remote_button_state(&head(Upstream::Tracked {
                ahead: 2,
                behind: 0
            }))
            .map(|state| (state.0, state.2)),
            Some(("Push", 2))
        );
        assert_eq!(
            remote_button_state(&head(Upstream::Tracked {
                ahead: 1,
                behind: 3
            }))
            .map(|state| (state.0, state.3)),
            Some(("Pull", 3))
        );
        assert_eq!(
            remote_button_state(&head(Upstream::Tracked {
                ahead: 0,
                behind: 0
            }))
            .map(|state| state.0),
            Some("Fetch")
        );
    }
}
