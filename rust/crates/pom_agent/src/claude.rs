//! Claude Code finds pom's MCP server through `~/.claude.json`. The entry runs a small wrapper script in
//! the state dir instead of the app binary itself, so moving or updating the app only rewrites the
//! wrapper; the previous core uses the same wrapper and entry, so either app can own it.

use std::path::{Path, PathBuf};

use pom_paths::StateDir;
use serde_json::{json, Map, Value};

const SERVER_NAME: &str = "pom";
const MCP_WRAPPER: &str = "pom-mcp";
/// Claude splits `command` on spaces, so a binary path with one is reached through this link.
const SPACELESS_LINK: &str = "pom-mcp-bin";

/// Where Claude Code keeps its user config.
#[derive(Clone, Debug)]
pub struct ClaudeHome {
    pub home: PathBuf,
}

impl ClaudeHome {
    pub fn from_env() -> Option<ClaudeHome> {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| ClaudeHome { home: home.into() })
    }

    fn user_config(&self) -> PathBuf {
        self.home.join(".claude.json")
    }
}

#[derive(Debug)]
pub enum InstallError {
    /// The user's file is not JSON we can safely rewrite; it is left alone.
    Unreadable(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Unreadable(path) => write!(
                formatter,
                "{} is not valid JSON - not modifying it",
                path.display()
            ),
            InstallError::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(error: std::io::Error) -> InstallError {
        InstallError::Io(error)
    }
}

/// `#!/bin/sh` that runs `<binary> <subcommand>`, or does nothing once the app is gone.
pub(crate) fn write_wrapper(
    state: &StateDir,
    name: &str,
    binary: &Path,
    subcommand: &str,
) -> std::io::Result<PathBuf> {
    let path = state.path(name);
    let quoted = binary.to_string_lossy().replace('\'', r"'\''");
    let body = format!(
        "#!/bin/sh\nBIN='{quoted}'\n[ -x \"$BIN\" ] || exit 0\nexec \"$BIN\" {subcommand}\n"
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        pom_paths::write_atomic(&path, body.as_bytes(), 0o755)?;
    }
    Ok(path)
}

/// Reads a JSON object file; a missing or empty file is an empty object, anything else unparseable is
/// refused so a hand-edited file is never clobbered.
pub(crate) fn read_object(path: &Path) -> Result<Map<String, Value>, InstallError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(error.into()),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(InstallError::Unreadable(path.to_path_buf())),
    }
}

pub(crate) fn write_object(path: &Path, object: Map<String, Value>) -> Result<(), InstallError> {
    let mut text = serde_json::to_string_pretty(&Value::Object(object))
        .map_err(|error| InstallError::Io(std::io::Error::other(error)))?;
    text.push('\n');
    pom_paths::write_atomic(path, text.as_bytes(), 0o644)?;
    Ok(())
}

/// Points Claude Code's `pom` MCP server at `binary` (through the wrapper). The user config is only
/// rewritten when the entry is missing or different: Claude writes that file constantly.
pub fn install_mcp(
    claude: &ClaudeHome,
    state: &StateDir,
    binary: &Path,
) -> Result<bool, InstallError> {
    let wrapper = write_wrapper(state, MCP_WRAPPER, binary, "mcp")?;
    let entry = json!({
        "command": "sh",
        "args": [wrapper.to_string_lossy()],
        "env": {},
    });
    let path = claude.user_config();
    let mut root = read_object(&path)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    let Some(servers) = servers.as_object_mut() else {
        return Ok(false);
    };
    if servers.get(SERVER_NAME) == Some(&entry) {
        return Ok(false);
    }
    servers.insert(SERVER_NAME.to_string(), entry);
    write_object(&path, root)?;
    Ok(true)
}

/// `--mcp-config` for an agent launched in `branch`'s workspace: the server pinned to that branch.
pub fn mcp_config_json(state: &StateDir, binary: &Path, branch: &str) -> String {
    let mut command = binary.to_path_buf();
    if binary.to_string_lossy().contains(' ') {
        let link = state.path(SPACELESS_LINK);
        let current = std::fs::read_link(&link).ok();
        let linked = current.as_deref() == Some(binary) || {
            if link.symlink_metadata().is_ok() {
                if let Err(error) = std::fs::remove_file(&link) {
                    eprintln!("agent: replace {}: {error}", link.display());
                }
            }
            std::os::unix::fs::symlink(binary, &link).is_ok()
        };
        if linked {
            command = link;
        }
    }
    json!({
        "mcpServers": {
            SERVER_NAME: {
                "command": command.to_string_lossy(),
                "args": ["mcp", "--branch", branch],
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, ClaudeHome, StateDir) {
        let temp = tempfile::tempdir().expect("temp");
        let claude = ClaudeHome {
            home: temp.path().join("home"),
        };
        std::fs::create_dir_all(&claude.home).expect("home");
        let state = StateDir::new(temp.path().join("state"));
        (temp, claude, state)
    }

    #[test]
    fn registers_the_server_once_and_keeps_the_rest_of_the_file() {
        let (_temp, claude, state) = setup();
        std::fs::write(
            claude.user_config(),
            r#"{"numStartups": 7, "mcpServers": {"other": {"command": "x"}}}"#,
        )
        .expect("config");
        let binary = Path::new("/Apps/Pomelo.app/Contents/MacOS/pomelo");
        assert!(install_mcp(&claude, &state, binary).expect("install"));
        assert!(
            !install_mcp(&claude, &state, binary).expect("again"),
            "unchanged: no rewrite"
        );

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(claude.user_config()).expect("read"))
                .expect("json");
        assert_eq!(written["numStartups"], 7);
        assert_eq!(written["mcpServers"]["other"]["command"], "x");
        let wrapper = state.path("pom-mcp");
        assert_eq!(
            written["mcpServers"]["pom"],
            json!({"command": "sh", "args": [wrapper.to_string_lossy()], "env": {}})
        );
        let script = std::fs::read_to_string(&wrapper).expect("wrapper");
        assert_eq!(
            script,
            "#!/bin/sh\nBIN='/Apps/Pomelo.app/Contents/MacOS/pomelo'\n[ -x \"$BIN\" ] || exit 0\nexec \"$BIN\" mcp\n"
        );
    }

    #[test]
    fn a_broken_user_config_is_left_alone() {
        let (_temp, claude, state) = setup();
        std::fs::write(claude.user_config(), "{ not json").expect("config");
        assert!(matches!(
            install_mcp(&claude, &state, Path::new("/bin/pomelo")),
            Err(InstallError::Unreadable(_))
        ));
        assert_eq!(
            std::fs::read_to_string(claude.user_config()).expect("read"),
            "{ not json"
        );
    }

    #[test]
    fn session_config_pins_the_branch_and_avoids_spaces() {
        let (temp, _claude, state) = setup();
        std::fs::create_dir_all(state.root()).expect("state");
        let plain: Value =
            serde_json::from_str(&mcp_config_json(&state, Path::new("/bin/pomelo"), "feat/x"))
                .expect("json");
        assert_eq!(
            plain["mcpServers"]["pom"],
            json!({"command": "/bin/pomelo", "args": ["mcp", "--branch", "feat/x"]})
        );
        let spaced = temp.path().join("My Apps/pomelo");
        let config: Value =
            serde_json::from_str(&mcp_config_json(&state, &spaced, "main")).expect("json");
        let command = config["mcpServers"]["pom"]["command"]
            .as_str()
            .expect("command");
        assert!(!command.contains(' '), "{command}");
        assert_eq!(std::fs::read_link(command).expect("link"), spaced);
    }
}
