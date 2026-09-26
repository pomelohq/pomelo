use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_detect::{RepoDetection, ServiceKind};
use pom_paths::StateDir;

const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoSpec {
    /// A local git repo or a git URL.
    pub path: String,
    pub alias: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScaffoldRequest {
    pub name: String,
    /// Where session folders live; empty means the default sessions root.
    pub root: String,
    pub default_branch: String,
    pub repos: Vec<RepoSpec>,
}

pub(crate) fn is_git_url(source: &str) -> bool {
    source.contains("://")
        || source
            .find(':')
            .is_some_and(|at| at > 0 && !source.starts_with('/') && !source.starts_with('.'))
}

pub(crate) fn repo_name_from_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let name = url.rsplit(['/', ':']).next().unwrap_or(url);
    name.strip_suffix(".git").unwrap_or(name).to_string()
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let output = command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub(crate) fn clone_remote(url: &str, destination: &Path) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["clone", "--", url])
        .arg(destination)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git: {error}"))?;
    let deadline = Instant::now() + CLONE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut message = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    if let Err(error) = std::io::Read::read_to_string(&mut stderr, &mut message) {
                        message = error.to_string();
                    }
                }
                return Err(message.trim().to_string());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("scaffold: stop clone: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("scaffold: reap clone: {error}");
                }
                return Err(format!("cloning {url} took over 10 minutes"));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// A local clone that keeps the source's origin and carries over its modified and untracked files.
pub(crate) fn clone_with_changes(source: &Path, destination: &Path) -> Result<(), String> {
    git(
        None,
        &[
            "clone",
            "--local",
            &source.to_string_lossy(),
            &destination.to_string_lossy(),
        ],
    )
    .map_err(|error| format!("clone: {error}"))?;
    if let Ok(origin) = git(Some(source), &["remote", "get-url", "origin"]) {
        let origin = origin.trim();
        if !origin.is_empty() {
            if let Err(error) = git(Some(destination), &["remote", "set-url", "origin", origin]) {
                eprintln!("scaffold: keep origin: {error}");
            }
        }
    }
    let Ok(changed) = git(
        Some(source),
        &["ls-files", "-m", "-o", "--exclude-standard", "-z"],
    ) else {
        return Ok(());
    };
    for relative in changed.split('\0').filter(|relative| !relative.is_empty()) {
        let target = destination.join(relative);
        let copied = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::copy(source.join(relative), &target).map(|_| ()));
        if let Err(error) = copied {
            eprintln!("scaffold: copy {relative}: {error}");
        }
    }
    Ok(())
}

/// `KEY=value` from a `.env` line (an `export ` prefix and matching quotes allowed); `None` for blanks and
/// comments.
fn parse_env_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        value = &value[1..value.len() - 1];
    }
    Some((key.to_string(), value.to_string()))
}

/// Stores the values of the source repo's gitignored `.env*` files (not examples or samples) as the session's
/// secrets, so the config can reference them by name only.
pub(crate) fn import_ignored_env(source: &Path, state: &StateDir, session: &str) {
    let Ok(listed) = git(
        Some(source),
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    ) else {
        return;
    };
    let store = pom_secrets::SecretStore::new(state.clone(), session);
    for relative in listed.split('\0') {
        let name = Path::new(relative)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if relative.is_empty()
            || !name.starts_with(".env")
            || name.contains(".example")
            || name.contains(".sample")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(source.join(relative)) else {
            continue;
        };
        for (key, value) in text.lines().filter_map(parse_env_line) {
            if let Err(error) = store.set(&key, &value) {
                eprintln!("scaffold: store {key}: {error}");
            }
        }
    }
}

/// Creates a session: its main workspace with every repo cloned (local repos keep their uncommitted work and
/// hand their ignored `.env` values to the secret store), a draft `pom.yml` from what was detected, and its
/// entry in the session list. Returns the session folder; nothing is left behind on failure.
pub fn scaffold_session(request: &ScaffoldRequest, state: &StateDir) -> Result<PathBuf, String> {
    let name = request.name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return Err("invalid session name".into());
    }
    let root = if request.root.trim().is_empty() {
        pom_paths::sessions_root()
    } else {
        PathBuf::from(request.root.trim())
    };
    let default_branch = match request.default_branch.trim() {
        "" => "main",
        branch => branch,
    };
    let session_dir = root.join(name);
    if session_dir.exists() {
        return Err(format!(
            "a session directory already exists at {}",
            session_dir.display()
        ));
    }
    let workspace = session_dir.join(format!("workspace--{default_branch}"));
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let result = (|| {
        let mut detections: Vec<(RepoDetection, PathBuf)> = Vec::new();
        for repo in &request.repos {
            let source = repo.path.trim();
            if source.is_empty() {
                continue;
            }
            if source.starts_with('-') {
                return Err(format!("invalid repo source: {source}"));
            }
            let repo_name = if is_git_url(source) {
                let repo_name = repo_name_from_url(source);
                if repo_name.is_empty() {
                    return Err(format!("cannot derive repo name from URL: {source}"));
                }
                clone_remote(source, &workspace.join(&repo_name))
                    .map_err(|error| format!("clone {repo_name}: {error}"))?;
                repo_name
            } else {
                let source_path = Path::new(source);
                if !pom_layout::is_git_repo(source_path) {
                    return Err(format!("not a git repo: {source}"));
                }
                let repo_name = source_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                clone_with_changes(source_path, &workspace.join(&repo_name))
                    .map_err(|error| format!("clone {repo_name}: {error}"))?;
                import_ignored_env(source_path, state, name);
                repo_name
            };
            let path = workspace.join(&repo_name);
            detections.push((
                RepoDetection {
                    name: repo_name,
                    alias: repo.alias.trim().to_string(),
                    apps: Vec::new(),
                    shared: Vec::new(),
                },
                path,
            ));
        }
        let repos: Vec<RepoDetection> = detections
            .into_iter()
            .map(|(mut detection, path)| {
                detection.apps = pom_detect::detect_repo(&path);
                detection.shared = pom_detect::parse_compose(&path)
                    .into_iter()
                    .filter(|service| service.kind == ServiceKind::Shared)
                    .collect();
                detection
            })
            .collect();
        let mut yaml = pom_detect::emit(name, &repos);
        if default_branch != "main" {
            yaml = yaml.replacen(
                "default_branch: main",
                &format!("default_branch: {default_branch}"),
                1,
            );
        }
        std::fs::write(session_dir.join("pom.yml"), yaml).map_err(|error| error.to_string())?;
        let mut sessions = pom_sessions::Sessions::load(state);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        sessions.touch(name, &session_dir.to_string_lossy(), now);
        sessions
            .save(state)
            .map_err(|error| format!("register the session: {error}"))?;
        Ok(session_dir.clone())
    })();
    if result.is_err() {
        if let Err(error) = std::fs::remove_dir_all(&session_dir) {
            eprintln!("scaffold: clean up {}: {error}", session_dir.display());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_lines_and_urls_parse_like_the_previous_core() {
        assert_eq!(
            parse_env_line("export API_KEY=\"abc\""),
            Some(("API_KEY".into(), "abc".into()))
        );
        assert_eq!(
            parse_env_line("TOKEN='x=y'"),
            Some(("TOKEN".into(), "x=y".into()))
        );
        assert_eq!(parse_env_line("# comment"), None);
        assert_eq!(parse_env_line("=nokey"), None);
        assert!(is_git_url("git@github.com:acme/web.git"));
        assert!(is_git_url("https://github.com/acme/web"));
        assert!(!is_git_url("/Users/dev/web"));
        assert!(!is_git_url("./web"));
        assert_eq!(repo_name_from_url("git@github.com:acme/web.git"), "web");
        assert_eq!(repo_name_from_url("https://github.com/acme/api/"), "api");
    }
}
