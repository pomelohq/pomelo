//! Claude Code finds pom's MCP server through `~/.claude.json`. The entry runs a small wrapper script in
//! the state dir instead of the app binary itself, so moving or updating the app only rewrites the
//! wrapper; the previous core uses the same wrapper and entry, so either app can own it.

use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Rewrites a JSON object file atomically, keeping its permissions; a new file is private to the user, as the
/// agent creates its own config.
pub(crate) fn write_object(path: &Path, object: Map<String, Value>) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    let mut text = serde_json::to_string_pretty(&Value::Object(object))
        .map_err(|error| InstallError::Io(std::io::Error::other(error)))?;
    text.push('\n');
    let mode = std::fs::metadata(path).map_or(0o600, |meta| meta.permissions().mode() & 0o777);
    pom_paths::write_atomic(path, text.as_bytes(), mode)?;
    Ok(())
}

const TRUST_KEY: &str = "hasTrustDialogAccepted";
const CONFIG_LOCK_WAIT: Duration = Duration::from_secs(2);
/// The agent refreshes a lock it holds well within this; an older one was left by a process that died.
const CONFIG_LOCK_STALE: Duration = Duration::from_secs(10);

/// The agent's user config: in `CLAUDE_CONFIG_DIR` when that is set, else in the home folder.
pub(crate) fn user_config_path(home: &Path) -> PathBuf {
    config_path_in(home, std::env::var_os("CLAUDE_CONFIG_DIR").as_deref())
}

fn config_path_in(home: &Path, config_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    match config_dir.filter(|dir| !dir.is_empty()) {
        Some(dir) => Path::new(dir).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

/// The folder the agent keys a working folder's trust by: the root of the git checkout it is in, else the
/// folder itself, both with links resolved as the agent sees its working folder.
pub(crate) fn trust_key(folder: &Path) -> std::io::Result<PathBuf> {
    let folder = std::fs::canonicalize(folder)?;
    Ok(folder
        .ancestors()
        .find(|dir| dir.join(".git").symlink_metadata().is_ok())
        .map_or_else(|| folder.clone(), Path::to_path_buf))
}

/// Marks `folder` trusted in the agent's config the way its own trust prompt does, and returns the key it
/// wrote. Every other key is kept, and a file that is not a JSON object is left alone.
pub(crate) fn accept_folder_trust(config: &Path, folder: &Path) -> Result<PathBuf, InstallError> {
    let key = trust_key(folder)?;
    let name = key.to_string_lossy().into_owned();
    let _lock = ConfigLock::acquire(config)?;
    let mut root = read_object(config)?;
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else {
        return Err(InstallError::Unreadable(config.to_path_buf()));
    };
    let entry = projects
        .entry(name)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(entry) = entry.as_object_mut() else {
        return Err(InstallError::Unreadable(config.to_path_buf()));
    };
    if entry.get(TRUST_KEY) == Some(&Value::Bool(true)) {
        return Ok(key);
    }
    entry.insert(TRUST_KEY.into(), Value::Bool(true));
    write_object(config, root)?;
    Ok(key)
}

/// The lock directory the agent itself takes beside its config while it rewrites it.
struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    fn acquire(config: &Path) -> std::io::Result<ConfigLock> {
        let mut path = config.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        let started = std::time::Instant::now();
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(ConfigLock { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > CONFIG_LOCK_STALE);
                    if stale {
                        if let Err(error) = std::fs::remove_dir(&path) {
                            if error.kind() != std::io::ErrorKind::NotFound {
                                return Err(error);
                            }
                        }
                        continue;
                    }
                    if started.elapsed() > CONFIG_LOCK_WAIT {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            format!("{} stays locked", config.display()),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir(&self.path) {
            eprintln!("agent: unlock {}: {error}", self.path.display());
        }
    }
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

/// The command that runs pom's MCP server from `binary`, without spaces in its path.
pub(crate) fn mcp_command(state: &StateDir, binary: &Path) -> PathBuf {
    if !binary.to_string_lossy().contains(' ') {
        return binary.to_path_buf();
    }
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
        link
    } else {
        binary.to_path_buf()
    }
}

/// `--mcp-config` for an agent launched in `branch`'s workspace: the server pinned to that branch.
pub fn mcp_config_json(state: &StateDir, binary: &Path, branch: &str) -> String {
    json!({
        "mcpServers": {
            SERVER_NAME: {
                "command": mcp_command(state, binary).to_string_lossy(),
                "args": ["mcp", "--branch", branch],
            }
        }
    })
    .to_string()
}

pub(crate) const MCP_SERVER_NAME: &str = SERVER_NAME;

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

    fn trust_entry(config: &Path, key: &Path) -> Value {
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(config).expect("read")).expect("json");
        written["projects"][key.to_string_lossy().as_ref()].clone()
    }

    #[test]
    fn trust_is_written_under_the_folder_and_keeps_every_other_key() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let config = temp.path().join(".claude.json");
        let folder = temp.path().join("workspace--feat-login");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(
            &config,
            r#"{"numStartups": 3, "projects": {"/elsewhere": {"hasTrustDialogAccepted": true}}}"#,
        )
        .expect("config");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).expect("mode");

        let key = accept_folder_trust(&config, &folder).expect("trust");
        assert_eq!(key, std::fs::canonicalize(&folder).expect("real"));
        assert_eq!(trust_entry(&config, &key)["hasTrustDialogAccepted"], true);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        assert_eq!(written["numStartups"], 3);
        assert_eq!(
            written["projects"]["/elsewhere"]["hasTrustDialogAccepted"],
            true
        );
        assert_eq!(
            std::fs::metadata(&config)
                .expect("meta")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the agent's private config stays private"
        );
        assert!(
            !temp.path().join(".claude.json.lock").exists(),
            "the lock is let go"
        );

        let before = std::fs::read_to_string(&config).expect("read");
        accept_folder_trust(&config, &folder).expect("again");
        assert_eq!(
            std::fs::read_to_string(&config).expect("read"),
            before,
            "already trusted: no rewrite"
        );
    }

    #[test]
    fn trust_inside_a_checkout_goes_to_its_root_and_keeps_the_entry() {
        let temp = tempfile::tempdir().expect("temp");
        let config = temp.path().join(".claude.json");
        let repo = temp.path().join("api");
        std::fs::create_dir_all(repo.join("src")).expect("repo");
        std::fs::write(repo.join(".git"), "gitdir: /x\n").expect("worktree marker");
        let root = std::fs::canonicalize(&repo).expect("real");
        std::fs::write(
            &config,
            format!(
                r#"{{"projects": {{"{}": {{"allowedTools": ["Bash"]}}}}}}"#,
                root.display()
            ),
        )
        .expect("config");
        let key = accept_folder_trust(&config, &repo.join("src")).expect("trust");
        assert_eq!(key, root);
        let entry = trust_entry(&config, &root);
        assert_eq!(entry["hasTrustDialogAccepted"], true);
        assert_eq!(entry["allowedTools"], json!(["Bash"]));
    }

    #[test]
    fn a_config_that_is_not_an_object_or_is_locked_is_never_written() {
        let temp = tempfile::tempdir().expect("temp");
        let folder = temp.path().join("w");
        std::fs::create_dir_all(&folder).expect("folder");
        let config = temp.path().join(".claude.json");
        for text in ["{ not json", r#"{"projects": []}"#, "[1]"] {
            std::fs::write(&config, text).expect("config");
            assert!(
                accept_folder_trust(&config, &folder).is_err(),
                "{text} is refused"
            );
            assert_eq!(std::fs::read_to_string(&config).expect("read"), text);
        }
        std::fs::write(&config, "{}").expect("config");
        std::fs::create_dir(temp.path().join(".claude.json.lock")).expect("lock");
        assert!(
            accept_folder_trust(&config, &folder).is_err(),
            "a fresh lock held by the agent wins"
        );
        assert_eq!(std::fs::read_to_string(&config).expect("read"), "{}");
    }

    #[test]
    fn the_config_follows_the_agents_config_dir() {
        let home = Path::new("/home/dev");
        assert_eq!(config_path_in(home, None), home.join(".claude.json"));
        assert_eq!(
            config_path_in(home, Some(std::ffi::OsStr::new(""))),
            home.join(".claude.json")
        );
        assert_eq!(
            config_path_in(home, Some(std::ffi::OsStr::new("/cfg"))),
            Path::new("/cfg/.claude.json")
        );
    }
}
