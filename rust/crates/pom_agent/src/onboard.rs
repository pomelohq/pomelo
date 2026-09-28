//! The onboarding agent can be any installed agent CLI. Claude Code gets pom's MCP server through
//! `--mcp-config`; Codex through `-c mcp_servers.*` overrides; Gemini CLI reads it from the
//! workspace's `.gemini/settings.json`. The others have no system-prompt flag, so theirs leads the
//! first message.

use std::path::Path;

use serde_json::{json, Value};

use crate::claude::{mcp_command, MCP_SERVER_NAME};
use crate::launch::{
    onboard_launch, onboard_system_prompt, resolve_claude, shell_quote, AgentLaunch, LaunchContext,
    ONBOARD_FIRST_TURN,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentCli {
    Claude,
    Codex,
    Gemini,
}

impl AgentCli {
    pub const ALL: [AgentCli; 3] = [AgentCli::Claude, AgentCli::Codex, AgentCli::Gemini];

    pub fn title(self) -> &'static str {
        match self {
            AgentCli::Claude => "Claude Code",
            AgentCli::Codex => "Codex",
            AgentCli::Gemini => "Gemini CLI",
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            AgentCli::Claude => "claude",
            AgentCli::Codex => "codex",
            AgentCli::Gemini => "gemini",
        }
    }

    pub fn from_binary(name: &str) -> Option<AgentCli> {
        AgentCli::ALL.into_iter().find(|cli| cli.binary() == name)
    }

    pub fn installed(self, home: &Path, tool_path: &str) -> bool {
        match self {
            AgentCli::Claude => Path::new(&resolve_claude(home, tool_path)).is_absolute(),
            cli => tool_path
                .split(':')
                .any(|dir| Path::new(dir).join(cli.binary()).is_file()),
        }
    }
}

/// The onboarding agent in `cli`, holding the same holder name whichever CLI runs it.
pub fn onboard_launch_with(context: &LaunchContext<'_>, cli: AgentCli) -> AgentLaunch {
    if cli == AgentCli::Claude {
        return onboard_launch(context);
    }
    let prompt = format!("{}\n\n{ONBOARD_FIRST_TURN}", onboard_system_prompt());
    let command = mcp_command(context.state, context.binary);
    let args = json!(["mcp", "--branch", context.branch]);
    let run = match cli {
        AgentCli::Codex => format!(
            "codex -c {command} -c {args} {prompt}",
            command = shell_quote(&format!(
                "mcp_servers.{MCP_SERVER_NAME}.command={}",
                json!(command.to_string_lossy())
            )),
            args = shell_quote(&format!("mcp_servers.{MCP_SERVER_NAME}.args={args}")),
            prompt = shell_quote(&prompt),
        ),
        _ => {
            if let Err(error) = write_gemini_settings(context.cwd, &command, &args) {
                eprintln!("onboard: gemini settings: {error}");
            }
            format!("gemini -i {}", shell_quote(&prompt))
        }
    };
    let script = format!(
        "export PATH={path}; export TERM=xterm-256color COLORTERM=truecolor; unsetopt monitor 2>/dev/null; cd {cwd} && exec {run}",
        path = shell_quote(context.tool_path),
        cwd = shell_quote(&context.cwd.to_string_lossy()),
    );
    AgentLaunch {
        holder: format!(
            "ws-{}-{}-onboarder",
            context.session.replace('/', "_"),
            context.branch.replace('/', "_")
        ),
        cwd: context.cwd.to_path_buf(),
        argv: vec!["zsh".into(), "-c".into(), script],
        title: format!("{} (onboarder)", cli.title()),
    }
}

fn write_gemini_settings(cwd: &Path, command: &Path, args: &Value) -> std::io::Result<()> {
    let path = cwd.join(".gemini/settings.json");
    let mut settings: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    settings["mcpServers"][MCP_SERVER_NAME] = json!({
        "command": command.to_string_lossy(),
        "args": args,
    });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pom_paths::StateDir;

    #[test]
    fn every_cli_gets_the_mcp_server_and_the_onboarding_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = StateDir::new(temp.path().join("state"));
        let context = LaunchContext {
            state: &state,
            home: temp.path(),
            binary: Path::new("/app/pom"),
            tool_path: "/usr/bin",
            session: "demo",
            branch: "main",
            is_main: true,
            cwd: temp.path(),
        };
        let codex = onboard_launch_with(&context, AgentCli::Codex);
        let script = codex.argv.last().cloned().unwrap_or_default();
        assert_eq!(codex.holder, "ws-demo-main-onboarder");
        assert!(script.contains("exec codex -c 'mcp_servers.pom.command=\"/app/pom\"'"));
        assert!(script.contains(r#"mcp_servers.pom.args=["mcp","--branch","main"]"#));
        assert!(script.contains("Pomelo"));

        let gemini = onboard_launch_with(&context, AgentCli::Gemini);
        assert!(gemini
            .argv
            .last()
            .is_some_and(|script| script.contains("exec gemini -i ")));
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join(".gemini/settings.json")).expect("settings"),
        )
        .expect("json");
        assert_eq!(settings["mcpServers"]["pom"]["command"], "/app/pom");
        assert_eq!(
            AgentCli::from_binary("gemini").map(AgentCli::title),
            Some("Gemini CLI")
        );
    }
}
