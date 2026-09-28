//! Claude Code hands its status line command the account's plan limits (`rate_limits`) with every
//! refresh. The agents Pomelo starts run their status line through `claude-statusline`, which keeps the
//! limits for the app and then runs the user's own status line command, so what the user sees is unchanged.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use pom_paths::StateDir;
use serde_json::{json, Value};

use crate::claude::write_wrapper;

const STATUSLINE_WRAPPER: &str = "claude-statusline";
/// The last plan limits a status line refresh reported, with when (`at`, unix seconds).
pub const RATE_LIMITS_FILE: &str = "agents/rate_limits.json";

/// The user's own status line: its command and padding, from `~/.claude/settings.json`.
fn user_statusline(home: &Path) -> Option<(String, Value)> {
    let text = std::fs::read_to_string(home.join(".claude/settings.json")).ok()?;
    let settings: Value = serde_json::from_str(&text).ok()?;
    let line = settings.get("statusLine")?;
    let command = line.get("command")?.as_str()?.to_string();
    if command.contains(STATUSLINE_WRAPPER) {
        return None;
    }
    Some((command, line.get("padding").cloned().unwrap_or(Value::Null)))
}

/// `--settings` for a Claude agent Pomelo starts: its status line goes through `claude-statusline`.
pub fn statusline_settings(state: &StateDir, binary: &Path, home: &Path) -> Option<String> {
    let wrapper = write_wrapper(state, STATUSLINE_WRAPPER, binary, STATUSLINE_WRAPPER)
        .map_err(|error| eprintln!("agent: status line wrapper: {error}"))
        .ok()?;
    let mut line = json!({
        "type": "command",
        "command": format!("sh '{}'", wrapper.to_string_lossy().replace('\'', r"'\''")),
    });
    if let Some((_, padding)) = user_statusline(home).filter(|(_, padding)| !padding.is_null()) {
        line["padding"] = padding;
    }
    Some(json!({ "statusLine": line }).to_string())
}

/// Keeps the `rate_limits` a status line refresh carries.
pub fn record_rate_limits(state: &StateDir, input: &[u8]) -> std::io::Result<bool> {
    let Ok(value) = serde_json::from_slice::<Value>(input) else {
        return Ok(false);
    };
    let Some(limits) = value.get("rate_limits").filter(|limits| limits.is_object()) else {
        return Ok(false);
    };
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut kept = limits.clone();
    kept["at"] = json!(at);
    let path = state.path(RATE_LIMITS_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    pom_paths::write_atomic(&path, kept.to_string().as_bytes(), 0o644)?;
    Ok(true)
}

/// Handles `<binary> claude-statusline`: the status line JSON arrives on stdin; what the user's own
/// command prints goes to stdout. Never fails: a broken status line would show inside the session.
pub fn run_statusline(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(STATUSLINE_WRAPPER) {
        return None;
    }
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("claude-statusline: {error}");
        return Some(0);
    }
    if let Err(error) = record_rate_limits(&StateDir::from_env(), &input) {
        eprintln!("claude-statusline: {error}");
    }
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Some(0);
    };
    let Some((command, _)) = user_statusline(&home) else {
        return Some(0);
    };
    let child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Some(0);
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(&input) {
            eprintln!("claude-statusline: {error}");
        }
    }
    if let Ok(output) = child.wait_with_output() {
        if let Err(error) = std::io::stdout().write_all(&output.stdout) {
            eprintln!("claude-statusline: {error}");
        }
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_with_limits_is_kept_and_the_user_line_stays_theirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = StateDir::new(temp.path().join("state"));
        assert_eq!(
            record_rate_limits(&state, br#"{"model":{}}"#).ok(),
            Some(false)
        );
        let input = br#"{"rate_limits":{"five_hour":{"used_percentage":23,"resets_at":1790644200},"seven_day":{"used_percentage":29,"resets_at":1790900000}}}"#;
        assert_eq!(record_rate_limits(&state, input).ok(), Some(true));
        let kept: Value = serde_json::from_str(
            &std::fs::read_to_string(state.path(RATE_LIMITS_FILE)).expect("kept"),
        )
        .expect("json");
        assert_eq!(kept["five_hour"]["used_percentage"], 23);
        assert!(kept["at"].as_u64().is_some());

        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).expect("home");
        std::fs::write(
            home.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"command","command":"bash ~/.claude/line.sh","padding":0}}"#,
        )
        .expect("settings");
        let settings: Value = serde_json::from_str(
            &statusline_settings(&state, Path::new("/app/pom"), &home).expect("settings"),
        )
        .expect("json");
        let command = settings["statusLine"]["command"]
            .as_str()
            .unwrap_or_default();
        assert!(command.contains("claude-statusline"), "{command}");
        assert_eq!(settings["statusLine"]["padding"], 0);
        assert_eq!(
            user_statusline(&home).map(|(command, _)| command),
            Some("bash ~/.claude/line.sh".to_string())
        );
    }
}
