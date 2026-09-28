//! Side agents: short-lived Claude sessions next to a workspace's main agent, to ask, review or fix
//! something without filling the main conversation. Each starts with a context taken from the main
//! session (a fork, or a fork compacted inside itself) or a fresh one with a written summary, and never
//! reports its state as the workspace's: the main agent alone drives the workspace's dot.

use std::path::{Path, PathBuf};

use crate::launch::{
    main_session_key, resolve_claude, session_id, shell_quote, system_prompt, transcript_path,
    AgentLaunch, LaunchContext,
};
use crate::mcp_config_json;

/// Set in a side agent's environment; its hooks then record nothing.
pub const SIDE_AGENT_ENV: &str = "POM_SIDE_AGENT";

/// Above this many tokens, the main session is compacted inside the fork.
const COMPACT_ABOVE_TOKENS: usize = 50_000;
/// A transcript carries tool output and JSON around the text; roughly this many bytes per token.
const TRANSCRIPT_BYTES_PER_TOKEN: usize = 6;
/// A compacted session keeps about this share of the original.
const COMPACTED_SHARE: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideRole {
    Ask,
    Review,
    Fix,
}

impl SideRole {
    pub fn title(self) -> &'static str {
        match self {
            SideRole::Ask => "Ask",
            SideRole::Review => "Review",
            SideRole::Fix => "Fix",
        }
    }

    /// Ask and Review only read; Fix may change files.
    pub fn read_only(self) -> bool {
        self != SideRole::Fix
    }

    fn holder_role(self) -> &'static str {
        match self {
            SideRole::Ask => "ask",
            SideRole::Review => "review",
            SideRole::Fix => "fix",
        }
    }

    fn first_turn(self) -> &'static str {
        match self {
            SideRole::Review => {
                "Review the changes on this branch against the default branch in every repo of this workspace: \
                 read the diffs, then list bugs, risky changes and missing tests, most severe first, each with \
                 file:line. Do not edit anything."
            }
            SideRole::Ask | SideRole::Fix => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideStart {
    /// A copy of the main session as it is.
    Fork,
    /// A copy of the main session, compacted inside the copy; the main session is never compacted.
    Compacted,
    /// No history; a summary of the workspace written by Pomelo.
    Fresh,
}

impl SideStart {
    pub fn title(self) -> &'static str {
        match self {
            SideStart::Fork => "Fork",
            SideStart::Compacted => "Compacted",
            SideStart::Fresh => "Fresh",
        }
    }
}

/// How big the main session is, for choosing and showing what each start costs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MainSession {
    /// Estimated tokens of the main conversation; `None` when it has none yet.
    pub tokens: Option<usize>,
}

impl MainSession {
    /// What Auto picks: the whole session while it is short, compacted once it is long, fresh without one.
    pub fn auto(self) -> SideStart {
        match self.tokens {
            None => SideStart::Fresh,
            Some(tokens) if tokens > COMPACT_ABOVE_TOKENS => SideStart::Compacted,
            Some(_) => SideStart::Fork,
        }
    }

    /// Roughly how many tokens a side agent begins with for `start`.
    pub fn estimate(self, start: SideStart) -> Option<usize> {
        match start {
            SideStart::Fork => self.tokens,
            SideStart::Compacted => self.tokens.map(|tokens| tokens / COMPACTED_SHARE),
            SideStart::Fresh => Some(5_000),
        }
    }
}

/// The main session of the workspace `context` launches in, measured from its transcript.
pub fn main_session(context: &LaunchContext<'_>) -> MainSession {
    let id = session_id(&main_session_key(context));
    let tokens = transcript_path(context.home, context.cwd, &id)
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() as usize / TRANSCRIPT_BYTES_PER_TOKEN);
    MainSession { tokens }
}

/// "9.3k", "62k", "800".
pub fn format_tokens(tokens: usize) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=9_999 => format!("{:.1}k", tokens as f64 / 1000.0),
        _ => format!("{}k", tokens / 1000),
    }
}

/// A side agent as launched: its holder and command, its own session, and what it was asked first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideLaunch {
    pub launch: AgentLaunch,
    pub session: String,
    /// Typed into its prompt once it is up, without sending (a compacted start runs `/compact` first).
    pub pending_input: Option<String>,
}

/// A side agent for `role`, numbered `number` in its workspace, starting as `start`. `prompt` is its first
/// question (empty to let the user type); `packet` is the written summary a fresh start reads.
pub fn side_launch(
    context: &LaunchContext<'_>,
    number: u64,
    role: SideRole,
    start: SideStart,
    prompt: &str,
    packet: Option<&Path>,
) -> SideLaunch {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let session = session_id(&format!("side:{}:{number}:{stamp}", context.branch));
    let main = session_id(&main_session_key(context));
    let claude = resolve_claude(context.home, context.tool_path);
    let mcp = mcp_config_json(context.state, context.binary, context.branch);
    let question = if prompt.trim().is_empty() {
        role.first_turn().to_string()
    } else {
        prompt.trim().to_string()
    };
    let (history, first, pending_input) = match start {
        SideStart::Fork => (
            format!(
                "--resume {} --fork-session --session-id {}",
                shell_quote(&main),
                shell_quote(&session)
            ),
            question,
            None,
        ),
        SideStart::Compacted => (
            format!(
                "--resume {} --fork-session --session-id {}",
                shell_quote(&main),
                shell_quote(&session)
            ),
            "/compact".to_string(),
            (!question.is_empty()).then_some(question),
        ),
        SideStart::Fresh => {
            let intro = packet.map_or(String::new(), |packet| {
                format!(
                    "What this workspace is about is summarized in {}; read it first. ",
                    packet.display()
                )
            });
            (
                format!("--session-id {}", shell_quote(&session)),
                format!("{intro}{question}").trim().to_string(),
                None,
            )
        }
    };
    let mode = if role.read_only() {
        "plan"
    } else {
        "acceptEdits"
    };
    let mut script = format!(
        "export PATH={path}; export TERM=xterm-256color COLORTERM=truecolor {side}=1; unsetopt monitor 2>/dev/null; cd {cwd} && exec {claude} {history} --permission-mode {mode} --mcp-config {mcp} --append-system-prompt {system}",
        path = shell_quote(context.tool_path),
        side = SIDE_AGENT_ENV,
        cwd = shell_quote(&context.cwd.to_string_lossy()),
        claude = shell_quote(&claude),
        mcp = shell_quote(&mcp),
        system = shell_quote(&side_system_prompt(role)),
    );
    if !first.is_empty() {
        script.push(' ');
        script.push_str(&shell_quote(&first));
    }
    SideLaunch {
        launch: AgentLaunch {
            holder: format!(
                "ws-{}-{}-side-{}-{number}",
                context.session.replace('/', "_"),
                context.branch.replace('/', "_"),
                role.holder_role()
            ),
            cwd: context.cwd.to_path_buf(),
            argv: vec!["zsh".into(), "-c".into(), script],
            title: role.title().into(),
        },
        session,
        pending_input,
    }
}

fn side_system_prompt(role: SideRole) -> String {
    let scope = if role.read_only() {
        "You only read: do not edit files, commit, or start and stop services."
    } else {
        "Keep changes to what was asked; do not commit."
    };
    format!(
        "{} You are a side agent next to this workspace's main agent: the user asked you something on the side. {scope} Answer briefly and concretely; the user may send your answer on to the main agent.",
        system_prompt()
    )
}

/// The last thing the side agent said, from its transcript: what "Send to main" sends when nothing is
/// selected.
pub fn last_answer(home: &Path, cwd: &Path, session: &str) -> Option<String> {
    let path = transcript_path(home, cwd, session)?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "assistant" {
            return None;
        }
        let content = value.get("message")?.get("content")?.as_array()?;
        let said: Vec<&str> = content
            .iter()
            .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect();
        let said = said.join("\n").trim().to_string();
        (!said.is_empty()).then_some(said)
    })
}

/// A short summary of the workspace for a fresh side agent: branch, ticket, and each repo's changes.
pub fn write_packet(
    state: &pom_paths::StateDir,
    branch: &str,
    ticket: Option<&str>,
    repos: &[(String, PathBuf)],
) -> std::io::Result<PathBuf> {
    let mut text = format!("# Workspace {branch}\n\n");
    if let Some(ticket) = ticket {
        text.push_str(&format!("Ticket: {ticket}\n\n"));
    }
    for (name, dir) in repos {
        text.push_str(&format!("## {name}\n\n"));
        let git = |args: &[&str]| -> String {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_default()
        };
        let log = git(&["log", "--oneline", "-15"]);
        if !log.is_empty() {
            text.push_str(&format!("Recent commits:\n```\n{log}\n```\n\n"));
        }
        let status = git(&["status", "--short"]);
        if !status.is_empty() {
            text.push_str(&format!("Uncommitted:\n```\n{status}\n```\n\n"));
        }
    }
    let path = state
        .path("agents")
        .join(format!("packet-{}.md", branch.replace('/', "_")));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    pom_paths::write_atomic(&path, text.as_bytes(), 0o644)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_forks_a_short_session_and_compacts_a_long_one() {
        assert_eq!(MainSession { tokens: None }.auto(), SideStart::Fresh);
        assert_eq!(
            MainSession {
                tokens: Some(12_000)
            }
            .auto(),
            SideStart::Fork
        );
        assert_eq!(
            MainSession {
                tokens: Some(80_000)
            }
            .auto(),
            SideStart::Compacted
        );
        assert_eq!(format_tokens(9_300), "9.3k");
        assert_eq!(format_tokens(62_000), "62k");
    }

    #[test]
    fn a_side_agent_forks_the_main_session_read_only_and_never_reports_its_state() {
        let temp = tempfile::tempdir().expect("temp");
        let state = pom_paths::StateDir::new(temp.path().join("state"));
        let context = LaunchContext {
            state: &state,
            home: temp.path(),
            binary: Path::new("/app/pomelo"),
            tool_path: "/usr/bin",
            session: "demo",
            branch: "feat-login",
            is_main: false,
            cwd: Path::new("/work/feat-login"),
        };
        let side = side_launch(
            &context,
            2,
            SideRole::Ask,
            SideStart::Fork,
            "why this?",
            None,
        );
        let script = &side.launch.argv[2];
        assert!(script.contains("--fork-session"), "{script}");
        assert!(script.contains("--permission-mode plan"), "{script}");
        assert!(script.contains("POM_SIDE_AGENT=1"), "{script}");
        assert!(script.ends_with("'why this?'"), "{script}");
        assert_eq!(side.launch.holder, "ws-demo-feat-login-side-ask-2");
        assert!(!crate::is_agent_holder(
            &side.launch.holder,
            "demo",
            "feat-login"
        ));

        let compacted = side_launch(
            &context,
            3,
            SideRole::Fix,
            SideStart::Compacted,
            "fix it",
            None,
        );
        assert!(compacted.launch.argv[2].ends_with("'/compact'"));
        assert!(compacted.launch.argv[2].contains("acceptEdits"));
        assert_eq!(compacted.pending_input.as_deref(), Some("fix it"));
    }
}
