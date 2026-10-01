//! The tools an agent uses to work with the other agent sessions of its own workspace. They take a role,
//! never a workspace: the workspace is this server's, and the gate refuses anything else.

use std::rc::Rc;
use std::time::Duration;

use pom_agent::{
    ApproveScope, Caller, Drive, DriveError, Gate, LaunchOptions, Limits, WaitUntil, SCHEMA,
};
use pom_ptyhost::SocketDir;
use serde_json::{json, Value};

use crate::protocol::{Tool, ToolArgs, ToolFn};
use crate::tools::Workspace;

fn text(args: &ToolArgs, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(args: &ToolArgs, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

fn list(args: &ToolArgs, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn failed(error: DriveError) -> String {
    error.to_json().to_string()
}

fn answer(mut value: Value) -> Result<String, String> {
    value["schema"] = json!(SCHEMA);
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

impl Workspace {
    fn drive(&self) -> Result<Drive, String> {
        let config = self.config_for_agents()?;
        Ok(Drive {
            state: self.state.clone(),
            holders: SocketDir::from_env(),
            project: config.session,
            home: std::env::var_os("HOME").map(Into::into).unwrap_or_default(),
            binary: std::env::current_exe().unwrap_or_default(),
            tool_path: pom_services::tool_path().to_string(),
        })
    }

    /// Runs `body` with a gate on this workspace for whoever called the tool.
    fn gated<T>(
        &self,
        body: impl FnOnce(&Drive, &Gate<'_>) -> Result<T, DriveError>,
    ) -> Result<T, String> {
        let drive = self.drive()?;
        let workspace = pom_agent::Workspace {
            project: drive.project.clone(),
            branch: self.branch.clone(),
        };
        let caller = Caller::current(&drive.holders);
        let gate = Gate::open(&drive.state, caller, workspace, Limits::OPERATOR)
            .map_err(|refusal| failed(refusal.into()))?;
        body(&drive, &gate).map_err(failed)
    }
}

/// A role inside this workspace; a handle naming another workspace reaches the gate and is refused there.
fn role(args: &ToolArgs) -> Result<String, String> {
    let role = text(args, "role");
    if role.is_empty() {
        return Err("role is required (see agent_list)".into());
    }
    Ok(role)
}

fn timeout(args: &ToolArgs) -> Option<Duration> {
    number(args, "timeout_s").map(Duration::from_secs)
}

pub fn agent_tools(workspace: Rc<Workspace>) -> Vec<Tool> {
    let tool = |name: &'static str,
                description: &'static str,
                schema: Value,
                read_only: bool,
                run: ToolFn| Tool {
        name,
        description,
        schema: Some(schema),
        read_only,
        destructive: false,
        max_result_chars: 0,
        run,
    };
    let role_schema = json!({"type": "string", "description": "The session's role in this workspace, from agent_list (claude, reviewer, fixer...)."});
    let mut tools = Vec::new();

    let ws = workspace.clone();
    tools.push(tool(
        "agent_list",
        "The coding-agent sessions of THIS workspace: role, state (idle, thinking, tool_use, awaiting_input, died), turn, and whether you may drive it. Only this workspace's sessions exist for you.",
        json!({"type": "object", "properties": {}}),
        true,
        Box::new(move |_| {
            ws.gated(|drive, gate| Ok(drive.list(&gate.workspace().branch)))
                .and_then(|sessions| answer(json!({"workspace": ws.branch, "sessions": sessions})))
        }),
    ));

    let ws = workspace.clone();
    let schema = json!({"type": "object", "required": ["role"], "properties": {
        "role": {"type": "string", "description": "A new role for the session, like reviewer (lowercase letters, digits, dashes)."},
        "prompt": {"type": "string", "description": "Its first turn."},
        "system_prompt": {"type": "string"},
        "tools": {"type": "string", "description": "The tool set it has, like Read,Grep,Glob."},
        "allowed_tools": {"type": "array", "items": {"type": "string"}},
        "disallowed_tools": {"type": "array", "items": {"type": "string"}},
        "model": {"type": "string"}
    }});
    tools.push(tool(
        "agent_start",
        "Start another agent session in THIS workspace on a fresh conversation of its own (for example a reviewer that must not share your context). You hold its lease: you may send it turns.",
        schema,
        false,
        Box::new(move |args| {
            let role = role(args)?;
            let options = LaunchOptions {
                tools: Some(text(args, "tools")).filter(|tools| !tools.is_empty()),
                allowed_tools: list(args, "allowed_tools"),
                disallowed_tools: list(args, "disallowed_tools"),
                model: Some(text(args, "model")).filter(|model| !model.is_empty()),
                system_prompt: Some(text(args, "system_prompt")).filter(|prompt| !prompt.is_empty()),
                prompt: Some(text(args, "prompt")).filter(|prompt| !prompt.is_empty()),
                ..LaunchOptions::default()
            };
            let is_main = ws.is_main_workspace();
            let cwd = pom_layout::workspace_root(&ws.root(), &ws.branch, is_main);
            ws.gated(|drive, gate| drive.start(gate, &cwd, is_main, &role, true, &options))
                .and_then(|session| answer(json!({"session": session})))
        }),
    ));

    let ws = workspace.clone();
    let schema = json!({"type": "object", "required": ["role", "text"], "properties": {"role": role_schema, "text": {"type": "string"}}});
    tools.push(tool(
        "agent_send",
        "Send one turn to another agent session of THIS workspace and return its turn number. Refused if it is busy, if a person drives it, if you are sending too fast (once per 5 s, 20 per hour), or if agents are already two deep. Use agent_wait and agent_read for the result, or agent_ask for all three.",
        schema,
        false,
        Box::new(move |args| {
            let role = role(args)?;
            let message = text(args, "text");
            ws.gated(|drive, gate| {
                let view = drive.resolve(gate, &role)?;
                drive.send(gate, &view, &message, false, false, None)
            })
            .and_then(|turn| answer(json!({"role": role, "turn": turn})))
        }),
    ));

    let ws = workspace.clone();
    let schema = json!({"type": "object", "required": ["role"], "properties": {
        "role": role_schema,
        "until": {"type": "string", "enum": ["turn-end", "idle", "awaiting_input"]},
        "turn": {"type": "integer"},
        "timeout_s": {"type": "integer", "description": "At most 600; wait again to keep waiting."}
    }});
    tools.push(tool(
        "agent_wait",
        "Wait for another agent session of THIS workspace: until its turn ends (default), it is idle, or it asks for a permission. Returns reached (turn-end, idle, awaiting_input, timeout, died) and the stop reason.",
        schema,
        true,
        Box::new(move |args| {
            let role = role(args)?;
            let until = WaitUntil::parse(&text(args, "until")).unwrap_or(WaitUntil::TurnEnd);
            ws.gated(|drive, gate| {
                let view = drive.resolve(gate, &role)?;
                let limit = gate.wait(timeout(args))?;
                Ok(drive.wait(&view, until, number(args, "turn"), limit))
            })
            .and_then(|outcome| answer(json!(outcome)))
        }),
    ));

    let ws = workspace.clone();
    let schema = json!({"type": "object", "required": ["role"], "properties": {
        "role": role_schema,
        "turn": {"type": "integer", "description": "One turn; the last by default."},
        "since": {"type": "integer"},
        "full": {"type": "boolean", "description": "Tool results in full instead of cut at 2 KB."}
    }});
    tools.push(tool(
        "agent_read",
        "What another agent session of THIS workspace did in a turn: its prompt, text, tool calls with results, stop reason and token usage, read from its transcript.",
        schema,
        true,
        Box::new(move |args| {
            let role = role(args)?;
            let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
            ws.gated(|drive, gate| {
                let view = drive.resolve(gate, &role)?;
                let last = view.turn;
                let (turn, since) = (number(args, "turn"), number(args, "since"));
                let turns = drive.read(
                    &view,
                    |number| match (turn, since) {
                        (Some(wanted), _) => number == wanted,
                        (None, Some(since)) => number >= since,
                        (None, None) => number == last,
                    },
                    full,
                )?;
                Ok((view.handle, turns))
            })
            .and_then(|(handle, turns)| answer(json!({"handle": handle, "turns": turns})))
        }),
    ));

    let ws = workspace.clone();
    let schema = json!({"type": "object", "required": ["role", "text"], "properties": {
        "role": role_schema,
        "text": {"type": "string"},
        "timeout_s": {"type": "integer", "description": "At most 600."}
    }});
    tools.push(tool(
        "agent_ask",
        "Ask another agent session of THIS workspace one question and get its answer: sends a turn, waits for it to end, and returns what it did. The same limits as agent_send apply.",
        schema,
        false,
        Box::new(move |args| {
            let role = role(args)?;
            let message = text(args, "text");
            ws.gated(|drive, gate| {
                let view = drive.resolve(gate, &role)?;
                let limit = gate.wait(timeout(args))?;
                let turn = drive.send(gate, &view, &message, false, false, limit)?;
                let outcome = drive.wait(&view, WaitUntil::TurnEnd, Some(turn), limit);
                let result = drive.read(&view, |number| number == turn, false)?;
                Ok(json!({"role": role, "turn": turn, "wait": outcome, "result": result.first()}))
            })
            .and_then(answer)
        }),
    ));

    let ws = workspace;
    let schema = json!({"type": "object", "required": ["role", "request"], "properties": {
        "role": role_schema,
        "request": {"type": "string", "description": "The id from its permission_request (pending approval <id>)."},
        "deny": {"type": "boolean"}
    }});
    tools.push(tool(
        "agent_approve",
        "Answer a pending approval of another agent session of THIS workspace, once (approve or deny). Use it only for a call you would make yourself.",
        schema,
        false,
        Box::new(move |args| {
            let role = role(args)?;
            let request = text(args, "request");
            let scope = if args.get("deny").and_then(Value::as_bool).unwrap_or(false) {
                ApproveScope::Deny
            } else {
                ApproveScope::Once
            };
            ws.gated(|drive, gate| {
                let view = drive.resolve(gate, &role)?;
                drive.approve(gate, &view, &request, scope)
            })
            .and_then(|()| answer(json!({"role": role, "request": request, "scope": scope})))
        }),
    ));

    tools
}
