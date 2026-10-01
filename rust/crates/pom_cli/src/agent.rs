//! `pom agent`: the coding-agent sessions of one workspace, for scripts, orchestrators and other agents.
//! Every command works inside one workspace and goes through the same gate as the agent MCP tools.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use pom_agent::{
    split_handle, ApproveScope, Caller, Drive, DriveError, Gate, LaunchOptions, Limits,
    RecordedTurn, SessionView, WaitUntil, Workspace, SCHEMA,
};
use pom_ptyhost::SocketDir;
use serde_json::{json, Value};

use crate::args::Args;
use crate::{Invocation, Session};

const WATCH_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AgentCommand {
    Ls {
        all: bool,
        json: bool,
    },
    Start(StartRequest),
    Stop {
        handle: String,
        json: bool,
    },
    Read(ReadRequest),
    Watch {
        from_start: bool,
        timeout: Option<Duration>,
    },
    Drive(DriveRequest),
}

impl AgentCommand {
    fn json(&self) -> bool {
        match self {
            AgentCommand::Ls { json, .. } | AgentCommand::Stop { json, .. } => *json,
            AgentCommand::Read(request) => request.json,
            AgentCommand::Start(request) => request.json,
            AgentCommand::Watch { .. } => true,
            AgentCommand::Drive(request) => request.json,
        }
    }
}

/// The commands that drive a session's turns or its lease.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DriveAction {
    Send {
        text: String,
        queue: bool,
        take: bool,
    },
    Wait {
        until: WaitUntil,
        turn: Option<u64>,
    },
    Ask {
        text: String,
        take: bool,
        full: bool,
    },
    Interrupt,
    Approve {
        request: String,
        scope: ApproveScope,
    },
    Takeover {
        session_id: String,
        settings: Option<String>,
        mcp_config: Option<String>,
    },
    Release,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DriveRequest {
    pub handle: String,
    pub action: DriveAction,
    pub timeout: Option<Duration>,
    pub json: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StartRequest {
    pub role: String,
    pub fresh: bool,
    pub options: LaunchOptions,
    pub json: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReadRequest {
    pub handle: String,
    pub turn: Option<u64>,
    pub since: Option<u64>,
    pub full: bool,
    pub json: bool,
}

/// `pom agent <command> ...`: returns the command and the workspace it names, if any.
pub(crate) fn parse(rest: &[&str]) -> Result<(AgentCommand, Option<String>), String> {
    let (verb, words) = rest
        .split_first()
        .ok_or("usage: pom agent ls|start|stop|read|watch ...")?;
    let valued = [
        "--role",
        "--prompt",
        "--prompt-file",
        "--system-prompt-file",
        "--mcp-config",
        "--extra-mcp-config",
        "--tools",
        "--allowed-tools",
        "--disallowed-tools",
        "--permission-mode",
        "--model",
        "--turn",
        "--since",
        "--timeout",
        "--file",
        "--until",
        "--session-id",
        "--settings",
        "-o",
        "--output",
    ];
    let args = Args::parse(words, &valued)?;
    let json = args.has("--json") || args.json()?;
    let handle = |what: &str| -> Result<(String, Option<String>), String> {
        let handle = args
            .positional
            .first()
            .ok_or(format!("pom agent {verb} needs {what}"))?
            .clone();
        let workspace = handle
            .contains('/')
            .then(|| split_handle(&handle, "").0)
            .filter(|branch| !branch.is_empty() && !handle.starts_with("ws-"));
        Ok((handle, workspace))
    };
    let number = |name: &str| -> Result<Option<u64>, String> {
        args.value(&[name])
            .map(|value| value.parse().map_err(|_| format!("{name} needs a number")))
            .transpose()
    };
    match *verb {
        "ls" => {
            args.allow(&["--all-workspaces", "--json", "-o", "--output"])?;
            args.at_most(1, "pom agent ls")?;
            Ok((
                AgentCommand::Ls {
                    all: args.has("--all-workspaces"),
                    json,
                },
                args.positional.first().cloned(),
            ))
        }
        "start" => {
            args.allow(&[
                "--role",
                "--fresh",
                "--trust",
                "--prompt",
                "--prompt-file",
                "--system-prompt-file",
                "--mcp-config",
                "--extra-mcp-config",
                "--tools",
                "--allowed-tools",
                "--disallowed-tools",
                "--permission-mode",
                "--model",
                "--json",
                "-o",
                "--output",
            ])?;
            args.at_most(1, "pom agent start")?;
            let read = |flag: &str| -> Result<Option<String>, String> {
                args.value(&[flag])
                    .map(|path| {
                        std::fs::read_to_string(&path)
                            .map_err(|error| format!("{flag} {path}: {error}"))
                    })
                    .transpose()
            };
            let list = |flag: &str| -> Vec<String> {
                args.values(flag)
                    .iter()
                    .flat_map(|value| value.split(','))
                    .map(str::trim)
                    .filter(|tool| !tool.is_empty())
                    .map(str::to_string)
                    .collect()
            };
            let prompt = match args.value(&["--prompt"]) {
                Some(prompt) => Some(prompt),
                None => read("--prompt-file")?,
            };
            let options = LaunchOptions {
                tools: args.value(&["--tools"]),
                allowed_tools: list("--allowed-tools"),
                disallowed_tools: list("--disallowed-tools"),
                permission_mode: args.value(&["--permission-mode"]),
                model: args.value(&["--model"]),
                extra_mcp_config: args.value(&["--extra-mcp-config", "--mcp-config"]),
                system_prompt: read("--system-prompt-file")?,
                prompt,
                isolated: true,
                trust: args.has("--trust"),
            };
            Ok((
                AgentCommand::Start(StartRequest {
                    role: args.value(&["--role"]).unwrap_or_else(|| "claude".into()),
                    fresh: args.has("--fresh"),
                    options,
                    json,
                }),
                args.positional.first().cloned(),
            ))
        }
        "stop" => {
            args.allow(&["--json", "-o", "--output"])?;
            let (handle, workspace) = handle("a session (<workspace>/<role>)")?;
            Ok((AgentCommand::Stop { handle, json }, workspace))
        }
        "read" => {
            args.allow(&[
                "--turn", "--since", "--full", "--format", "--json", "-o", "--output",
            ])?;
            let (handle, workspace) = handle("a session (<workspace>/<role>)")?;
            Ok((
                AgentCommand::Read(ReadRequest {
                    handle,
                    turn: number("--turn")?,
                    since: number("--since")?,
                    full: args.has("--full"),
                    json,
                }),
                workspace,
            ))
        }
        "watch" => {
            args.allow(&["--from-start", "--timeout"])?;
            args.at_most(1, "pom agent watch")?;
            Ok((
                AgentCommand::Watch {
                    from_start: args.has("--from-start"),
                    timeout: args
                        .value(&["--timeout"])
                        .map(|text| parse_duration(&text))
                        .transpose()?,
                },
                args.positional.first().cloned(),
            ))
        }
        "send" | "ask" | "wait" | "interrupt" | "approve" | "deny" | "release" => {
            args.allow(&[
                "--file", "--queue", "--take", "--until", "--turn", "--timeout", "--full",
                "--always", "--json", "-o", "--output",
            ])?;
            let (handle, workspace) = handle("a session (<workspace>/<role>)")?;
            let text = || -> Result<String, String> {
                match (args.positional.get(1), args.value(&["--file"])) {
                    (Some(text), None) => Ok(text.clone()),
                    (None, Some(path)) => std::fs::read_to_string(&path)
                        .map_err(|error| format!("--file {path}: {error}")),
                    _ => Err(format!("pom agent {verb} needs the text, or --file")),
                }
            };
            let action = match *verb {
                "send" => DriveAction::Send {
                    text: text()?,
                    queue: args.has("--queue"),
                    take: args.has("--take"),
                },
                "ask" => DriveAction::Ask {
                    text: text()?,
                    take: args.has("--take"),
                    full: args.has("--full"),
                },
                "wait" => DriveAction::Wait {
                    until: match args.value(&["--until"]) {
                        Some(until) => WaitUntil::parse(&until).ok_or(format!(
                            "--until {until}: idle, awaiting_input or turn-end"
                        ))?,
                        None => WaitUntil::TurnEnd,
                    },
                    turn: number("--turn")?,
                },
                "interrupt" => DriveAction::Interrupt,
                "release" => DriveAction::Release,
                _ => DriveAction::Approve {
                    request: args
                        .positional
                        .get(1)
                        .cloned()
                        .ok_or(format!("pom agent {verb} needs the request id"))?,
                    scope: match (*verb, args.has("--always")) {
                        ("deny", _) => ApproveScope::Deny,
                        (_, true) => ApproveScope::Always,
                        _ => ApproveScope::Once,
                    },
                },
            };
            let timeout = args
                .value(&["--timeout"])
                .map(|text| parse_duration(&text))
                .transpose()?;
            Ok((
                AgentCommand::Drive(DriveRequest { handle, action, timeout, json }),
                workspace,
            ))
        }
        "takeover" => {
            args.allow(&["--session-id", "--settings", "--mcp-config", "--json", "-o", "--output"])?;
            args.at_most(1, "pom agent takeover")?;
            let session_id = args
                .value(&["--session-id"])
                .ok_or("pom agent takeover needs --session-id")?;
            Ok((
                AgentCommand::Drive(DriveRequest {
                    handle: session_id.clone(),
                    action: DriveAction::Takeover {
                        session_id,
                        settings: args.value(&["--settings"]),
                        mcp_config: args.value(&["--mcp-config"]),
                    },
                    timeout: None,
                    json,
                }),
                args.positional.first().cloned(),
            ))
        }
        other => Err(format!(
            "unknown agent command {other} (ls, start, stop, read, watch, send, wait, ask, interrupt, approve, deny, takeover, release)"
        )),
    }
}

/// `90`, `30s`, `10m`, `2h`.
pub(crate) fn parse_duration(text: &str) -> Result<Duration, String> {
    let (number, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let value: u64 = number
        .parse()
        .map_err(|_| format!("{text} is not a duration (like 30s, 10m, 2h)"))?;
    let seconds = match unit {
        "" | "s" => value,
        "m" => value * 60,
        "h" => value * 3600,
        _ => return Err(format!("{text} is not a duration (like 30s, 10m, 2h)")),
    };
    Ok(Duration::from_secs(seconds))
}

fn drive(session: &Session) -> Drive {
    Drive {
        state: session.state.clone(),
        holders: SocketDir::from_env(),
        project: session.config.session.clone(),
        home: std::env::var_os("HOME").map(Into::into).unwrap_or_default(),
        binary: std::env::current_exe().unwrap_or_default(),
        tool_path: pom_services::tool_path().to_string(),
    }
}

fn line(out: &mut dyn Write, text: &str) -> Result<(), DriveError> {
    writeln!(out, "{text}").map_err(|error| DriveError::Failed(error.to_string()))?;
    out.flush()
        .map_err(|error| DriveError::Failed(error.to_string()))
}

/// Runs an agent command and returns its exit status.
pub(crate) fn execute(
    command: &AgentCommand,
    invocation: &Invocation,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let wants_json = command.json();
    let result = Session::open(invocation, cwd)
        .map_err(DriveError::Failed)
        .and_then(|session| run(command, &session, out));
    match result {
        Ok(code) => code,
        Err(error) => {
            let message = if wants_json {
                error.to_json().to_string()
            } else {
                format!("error: {error}")
            };
            if let Err(write_error) = writeln!(err, "{message}") {
                eprintln!("{message} ({write_error})");
            }
            error.exit_code()
        }
    }
}

fn run(command: &AgentCommand, session: &Session, out: &mut dyn Write) -> Result<i32, DriveError> {
    if let AgentCommand::Drive(request) = command {
        return drive_turn(request, session, out);
    }
    run_view(command, session, out).map(|()| 0)
}

fn drive_turn(
    request: &DriveRequest,
    session: &Session,
    out: &mut dyn Write,
) -> Result<i32, DriveError> {
    let drive = drive(session);
    let workspace = Workspace {
        project: session.config.session.clone(),
        branch: session.branch.clone(),
    };
    let gate = Gate::open(
        &drive.state,
        Caller::current(&drive.holders),
        workspace.clone(),
        Limits::OPERATOR,
    )?;
    let timeout = gate.wait(request.timeout)?;
    let print = |out: &mut dyn Write, value: Value, text: String| {
        if request.json {
            let mut value = value;
            value["schema"] = json!(SCHEMA);
            line(out, &value.to_string())
        } else {
            line(out, &text)
        }
    };
    if let DriveAction::Takeover {
        session_id,
        settings,
        mcp_config,
    } = &request.action
    {
        let view = drive
            .list(&workspace.branch)
            .into_iter()
            .find(|view| view.session_id == *session_id)
            .ok_or_else(|| {
                DriveError::NotFound(format!(
                    "no session {session_id} in workspace {}",
                    workspace.branch
                ))
            })?;
        let cwd =
            pom_layout::workspace_root(&session.project.root, &workspace.branch, session.is_main);
        let view = drive.takeover(
            &gate,
            &view,
            &cwd,
            settings.as_deref(),
            mcp_config.as_deref(),
        )?;
        print(
            out,
            json!({"handle": view.handle, "holder": view.holder, "lease": "person"}),
            format!("{} is yours: attach to {}", view.handle, view.holder),
        )?;
        return Ok(0);
    }
    let view = drive.resolve(&gate, &request.handle)?;
    match &request.action {
        DriveAction::Send { text, queue, take } => {
            let turn = drive.send(&gate, &view, text, *queue, *take, timeout)?;
            print(
                out,
                json!({"handle": view.handle, "turn": turn}),
                format!("sent turn {turn} to {}", view.handle),
            )?;
            Ok(0)
        }
        DriveAction::Wait { until, turn } => {
            let outcome = drive.wait(&view, *until, *turn, timeout);
            let code = outcome.exit_code;
            print(
                out,
                json!(outcome),
                format!(
                    "{} {} (turn {})",
                    view.handle, outcome.reached, outcome.turn
                ),
            )?;
            Ok(code)
        }
        DriveAction::Ask { text, take, full } => {
            let turn = drive.send(&gate, &view, text, false, *take, timeout)?;
            let outcome = drive.wait(&view, WaitUntil::TurnEnd, Some(turn), timeout);
            let turns = drive.read(&view, |number| number == turn, *full)?;
            let code = outcome.exit_code;
            if request.json {
                print(
                    out,
                    json!({"handle": view.handle, "turn": turn, "wait": outcome, "result": turns.first()}),
                    String::new(),
                )?;
            } else {
                print_turns(out, &view, &turns, false)?;
                if code != 0 {
                    line(out, &format!("({})", outcome.reached))?;
                }
            }
            Ok(code)
        }
        DriveAction::Interrupt => {
            drive.interrupt(&gate, &view)?;
            print(
                out,
                json!({"handle": view.handle, "interrupted": true}),
                format!("interrupted {}", view.handle),
            )?;
            Ok(0)
        }
        DriveAction::Approve { request: id, scope } => {
            drive.approve(&gate, &view, id, *scope)?;
            print(
                out,
                json!({"handle": view.handle, "request": id, "scope": scope}),
                format!("{scope:?} {id} for {}", view.handle).to_lowercase(),
            )?;
            Ok(0)
        }
        DriveAction::Release => {
            let lease = drive.release(&gate, &view)?;
            print(
                out,
                json!({"handle": view.handle, "lease": lease.class}),
                format!("{} goes back to {}", view.handle, lease.by),
            )?;
            Ok(0)
        }
        DriveAction::Takeover { .. } => Ok(0),
    }
}

fn run_view(
    command: &AgentCommand,
    session: &Session,
    out: &mut dyn Write,
) -> Result<(), DriveError> {
    let drive = drive(session);
    let workspace = Workspace {
        project: session.config.session.clone(),
        branch: session.branch.clone(),
    };
    let caller = Caller::current(&drive.holders);
    let open = || {
        Gate::open(
            &drive.state,
            caller.clone(),
            workspace.clone(),
            Limits::OPERATOR,
        )
    };
    match command {
        AgentCommand::Ls { all, json } => {
            if *all {
                // Read-only overview: listing never lets anything reach another workspace's sessions.
                let mut views = Vec::new();
                for known in &session.project.workspaces {
                    views.extend(drive.list(&known.branch));
                }
                return print_list(out, &views, *json, None);
            }
            open()?;
            print_list(
                out,
                &drive.list(&workspace.branch),
                *json,
                Some(&workspace.branch),
            )
        }
        AgentCommand::Start(request) => {
            let gate = open()?;
            let cwd = pom_layout::workspace_root(
                &session.project.root,
                &workspace.branch,
                session.is_main,
            );
            let view = drive.start(
                &gate,
                &cwd,
                session.is_main,
                &request.role,
                request.fresh,
                &request.options,
            )?;
            if request.json {
                line(out, &json!({"schema": SCHEMA, "session": view}).to_string())
            } else {
                line(out, &format!("started {} ({})", view.handle, view.holder))
            }
        }
        AgentCommand::Stop { handle, json } => {
            let gate = open()?;
            let view = drive.resolve(&gate, handle)?;
            drive.stop(&gate, &view)?;
            if *json {
                line(
                    out,
                    &json!({"schema": SCHEMA, "stopped": view.handle}).to_string(),
                )
            } else {
                line(out, &format!("stopped {}", view.handle))
            }
        }
        AgentCommand::Read(request) => {
            let gate = open()?;
            let view = drive.resolve(&gate, &request.handle)?;
            let last = view.turn;
            let select = |turn: u64| match (request.turn, request.since) {
                (Some(wanted), _) => turn == wanted,
                (None, Some(since)) => turn >= since,
                (None, None) => turn == last,
            };
            let turns = drive.read(&view, select, request.full)?;
            print_turns(out, &view, &turns, request.json)
        }
        AgentCommand::Watch {
            from_start,
            timeout,
        } => {
            open()?;
            let mut seen: HashMap<String, usize> = HashMap::new();
            if !from_start {
                drive.new_events(&workspace.branch, &mut seen);
            }
            let started = Instant::now();
            loop {
                for event in drive.new_events(&workspace.branch, &mut seen) {
                    line(out, &event.to_string())?;
                }
                if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                    return Ok(());
                }
                std::thread::sleep(WATCH_POLL);
            }
        }
        AgentCommand::Drive(_) => Ok(()),
    }
}

fn print_list(
    out: &mut dyn Write,
    views: &[SessionView],
    json: bool,
    workspace: Option<&str>,
) -> Result<(), DriveError> {
    if json {
        let mut value = json!({"schema": SCHEMA, "sessions": views});
        if let Some(workspace) = workspace {
            value["workspace"] = json!(workspace);
        }
        return line(out, &value.to_string());
    }
    if views.is_empty() {
        return line(out, "no agent sessions");
    }
    line(
        out,
        &format!(
            "{:<28} {:<16} {:<8} {:>5} {:>8}  {}",
            "HANDLE", "STATE", "DRIVER", "TURN", "AGE", "HOLDER"
        ),
    )?;
    for view in views {
        let state = if view.stale {
            format!("{} (stale)", view.state)
        } else {
            view.state.clone()
        };
        line(
            out,
            &format!(
                "{:<28} {:<16} {:<8} {:>5} {:>7}s  {}",
                view.handle, state, view.driver, view.turn, view.last_event_age_s, view.holder
            ),
        )?;
    }
    Ok(())
}

fn print_turns(
    out: &mut dyn Write,
    view: &SessionView,
    turns: &[RecordedTurn],
    json: bool,
) -> Result<(), DriveError> {
    if json {
        return line(
            out,
            &json!({"schema": SCHEMA, "handle": view.handle, "session_id": view.session_id, "turns": turns})
                .to_string(),
        );
    }
    for turn in turns {
        line(
            out,
            &format!(
                "turn {} ({}, {:?})",
                turn.turn, turn.stop_reason, turn.origin
            )
            .to_lowercase(),
        )?;
        line(
            out,
            &format!("> {}", turn.content.prompt.replace('\n', "\n> ")),
        )?;
        for item in &turn.content.items {
            match item {
                pom_agent::Item::Text { text } => line(out, text)?,
                pom_agent::Item::ToolCall(call) => {
                    line(out, &format!("[{}] {}", call.name, compact(&call.input)))?;
                    if let Some(result) = &call.result {
                        let marker = if call.is_error { "error" } else { "result" };
                        let cut = if call.truncated {
                            " (cut, --full for all)"
                        } else {
                            ""
                        };
                        line(
                            out,
                            &format!("  {marker}{cut}: {}", result.replace('\n', "\n  ")),
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 200 {
        let mut end = 200;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_commands_parse_with_their_workspace() {
        let (command, workspace) = parse(&["ls", "feat-login", "--json"]).expect("ls");
        assert_eq!(
            command,
            AgentCommand::Ls {
                all: false,
                json: true
            }
        );
        assert_eq!(workspace.as_deref(), Some("feat-login"));
        let (command, workspace) = parse(&[
            "start",
            "feat-login",
            "--role",
            "reviewer",
            "--fresh",
            "--tools",
            "Read,Grep",
            "--allowed-tools",
            "Read",
            "--disallowed-tools",
            "Bash,Write",
            "--model",
            "sonnet",
            "--permission-mode",
            "plan",
            "--prompt",
            "review the diff",
        ])
        .expect("start");
        let AgentCommand::Start(request) = command else {
            panic!("start");
        };
        assert_eq!(workspace.as_deref(), Some("feat-login"));
        assert_eq!((request.role.as_str(), request.fresh), ("reviewer", true));
        assert_eq!(request.options.tools.as_deref(), Some("Read,Grep"));
        assert_eq!(request.options.disallowed_tools, ["Bash", "Write"]);
        assert_eq!(request.options.prompt.as_deref(), Some("review the diff"));
        assert!(request.options.isolated);
        let (command, workspace) =
            parse(&["read", "feat/login/reviewer", "--turn", "3", "--full"]).expect("read");
        assert_eq!(workspace.as_deref(), Some("feat/login"));
        assert!(matches!(
            command,
            AgentCommand::Read(ReadRequest {
                turn: Some(3),
                full: true,
                ..
            })
        ));
        assert!(parse(&["read"]).is_err());
        assert!(parse(&["nope"]).is_err());
        assert_eq!(parse_duration("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse_duration("45"), Ok(Duration::from_secs(45)));
        assert!(parse_duration("soon").is_err());
    }
}
