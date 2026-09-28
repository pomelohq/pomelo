use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pom_detect::{RepoDetection, ServiceKind};
use pom_paths::StateDir;

const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
pub const CANCELLED: &str = "cancelled";

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
    /// Leave the local repos' gitignored `.env` values out of the secret store.
    pub skip_secrets: bool,
}

/// How a scaffold is going, repo by repo (indexes into `ScaffoldRequest::repos`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScaffoldEvent {
    Cloning {
        repo: usize,
        percent: u8,
    },
    /// `linked` for a local repo, cloned with its uncommitted work, rather than one fetched from a URL.
    Cloned {
        repo: usize,
        linked: bool,
    },
    Scanned(Vec<RepoScan>),
}

/// What detection found in one repo, before any config is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoScan {
    pub name: String,
    pub alias: String,
    /// Frameworks, or languages where no framework matched: "Rails", "Vite", "Go".
    pub stack: Vec<String>,
    /// The compose file the shared services came from.
    pub compose: Option<String>,
    /// Shared services its compose file declares: "postgres", "redis".
    pub infra: Vec<String>,
    /// Gitignored `.env` files whose values become secrets.
    pub env_files: usize,
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
    clone_remote_with(url, destination, &mut |_| {}, &AtomicBool::new(false))
}

/// `git clone --progress` reports each phase with a percentage; the whole clone is weighted towards
/// receiving objects, the phase that takes the time.
fn clone_percent(line: &str) -> Option<u8> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let value: u32 = rest.trim().split('%').next()?.trim().parse().ok()?;
    let value = value.min(100);
    let (from, span) = match phase.trim() {
        "Counting objects" | "Enumerating objects" | "Compressing objects" => (0, 10),
        "Receiving objects" => (10, 80),
        "Resolving deltas" | "Updating files" => (90, 10),
        _ => return None,
    };
    Some((from + value * span / 100) as u8)
}

fn clone_remote_with(
    url: &str,
    destination: &Path,
    progress: &mut dyn FnMut(u8),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["clone", "--progress", "--", url])
        .arg(destination)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git: {error}"))?;
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let reader = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut all = String::new();
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = stderr.read(&mut byte) {
                if byte[0] == b'\r' || byte[0] == b'\n' {
                    let text = String::from_utf8_lossy(&line).into_owned();
                    if sender.send(text.clone()).is_err() {
                        break;
                    }
                    if byte[0] == b'\n' {
                        all.push_str(&text);
                        all.push('\n');
                    }
                    line.clear();
                } else {
                    line.push(byte[0]);
                }
            }
            all.push_str(&String::from_utf8_lossy(&line));
            all
        })
    });
    let deadline = Instant::now() + CLONE_TIMEOUT;
    let mut shown = None;
    let stop = |child: &mut std::process::Child| {
        if let Err(error) = child.kill() {
            eprintln!("scaffold: stop clone: {error}");
        }
        if let Err(error) = child.wait() {
            eprintln!("scaffold: reap clone: {error}");
        }
    };
    loop {
        while let Ok(line) = receiver.try_recv() {
            if let Some(percent) = clone_percent(&line).filter(|percent| Some(*percent) != shown) {
                shown = Some(percent);
                progress(percent);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let output = reader
                    .and_then(|reader| reader.join().ok())
                    .unwrap_or_default();
                let message = output
                    .lines()
                    .filter(|line| !line.trim().is_empty() && clone_percent(line).is_none())
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(message.trim().to_string());
            }
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                stop(&mut child);
                return Err(CANCELLED.into());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                stop(&mut child);
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

/// The repo's gitignored `.env*` files that hold real values (not examples or samples).
fn ignored_env_files(source: &Path) -> Vec<String> {
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
        return Vec::new();
    };
    listed
        .split('\0')
        .filter(|relative| {
            let name = Path::new(relative)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            !relative.is_empty()
                && name.starts_with(".env")
                && !name.contains(".example")
                && !name.contains(".sample")
        })
        .map(str::to_string)
        .collect()
}

fn stack_label(app: &pom_detect::StackFacts) -> String {
    let named = match app.framework.as_str() {
        "" => match app.language.as_str() {
            "js" => "Node",
            "ruby" => "Ruby",
            "python" => "Python",
            "go" => "Go",
            "java" => "Java",
            _ => "",
        },
        "next" => "Next.js",
        "nest" => "NestJS",
        "cra" => "Create React App",
        "spring-boot" => "Spring Boot",
        "fastapi" => "FastAPI",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let raw = if app.framework.is_empty() {
        &app.language
    } else {
        &app.framework
    };
    let mut chars = raw.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn compose_file(repo: &Path) -> Option<String> {
    [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ]
    .into_iter()
    .find(|name| repo.join(name).exists())
    .map(str::to_string)
}

fn scan_detection(detection: &RepoDetection, path: &Path, env_files: usize) -> RepoScan {
    let mut stack: Vec<String> = Vec::new();
    for label in detection.apps.iter().map(stack_label) {
        if !label.is_empty() && !stack.contains(&label) {
            stack.push(label);
        }
    }
    RepoScan {
        name: detection.name.clone(),
        alias: detection.alias.clone(),
        stack,
        compose: compose_file(path),
        infra: detection
            .shared
            .iter()
            .map(|service| service.name.clone())
            .collect(),
        env_files,
    }
}

fn detect_at(name: String, alias: String, path: &Path) -> RepoDetection {
    RepoDetection {
        name,
        alias,
        apps: pom_detect::detect_repo(path),
        shared: pom_detect::parse_compose(path)
            .into_iter()
            .filter(|service| service.kind == ServiceKind::Shared)
            .collect(),
    }
}

/// What detection finds in a local repo, shown before the project is created. `None` when the folder is
/// not a git repo.
pub fn preview_repo(path: &Path) -> Option<RepoScan> {
    if !pom_layout::is_git_repo(path) {
        return None;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let detection = detect_at(name, String::new(), path);
    Some(scan_detection(
        &detection,
        path,
        ignored_env_files(path).len(),
    ))
}

/// Stores the values of the source repo's gitignored `.env*` files (not examples or samples) as the session's
/// secrets, so the config can reference them by name only.
pub(crate) fn import_ignored_env(source: &Path, state: &StateDir, session: &str) {
    let store = pom_secrets::SecretStore::new(state.clone(), session);
    for relative in ignored_env_files(source) {
        let Ok(text) = std::fs::read_to_string(source.join(&relative)) else {
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
    scaffold_session_with(request, state, &mut |_| {}, &AtomicBool::new(false))
}

/// `scaffold_session`, reporting each repo as it is cloned and what the scan found; `cancel` stops it
/// between repos or mid-clone, and cleans up like any failure (the error is `CANCELLED`).
pub fn scaffold_session_with(
    request: &ScaffoldRequest,
    state: &StateDir,
    progress: &mut dyn FnMut(ScaffoldEvent),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
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
        let mut detections: Vec<(RepoDetection, PathBuf, usize)> = Vec::new();
        for (index, repo) in request.repos.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            let source = repo.path.trim();
            if source.is_empty() {
                continue;
            }
            progress(ScaffoldEvent::Cloning {
                repo: index,
                percent: 0,
            });
            if source.starts_with('-') {
                return Err(format!("invalid repo source: {source}"));
            }
            let remote = is_git_url(source);
            let mut env_files = 0;
            let repo_name = if remote {
                let repo_name = repo_name_from_url(source);
                if repo_name.is_empty() {
                    return Err(format!("cannot derive repo name from URL: {source}"));
                }
                clone_remote_with(
                    source,
                    &workspace.join(&repo_name),
                    &mut |percent| {
                        progress(ScaffoldEvent::Cloning {
                            repo: index,
                            percent,
                        })
                    },
                    cancel,
                )
                .map_err(|error| match error.as_str() {
                    CANCELLED => error,
                    _ => format!("clone {repo_name}: {error}"),
                })?;
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
                if !request.skip_secrets {
                    env_files = ignored_env_files(source_path).len();
                    import_ignored_env(source_path, state, name);
                }
                repo_name
            };
            progress(ScaffoldEvent::Cloned {
                repo: index,
                linked: !remote,
            });
            let path = workspace.join(&repo_name);
            detections.push((
                RepoDetection {
                    name: repo_name,
                    alias: repo.alias.trim().to_string(),
                    apps: Vec::new(),
                    shared: Vec::new(),
                },
                path,
                env_files,
            ));
        }
        let mut scans = Vec::new();
        let repos: Vec<RepoDetection> = detections
            .into_iter()
            .map(|(detection, path, env_files)| {
                let detection = detect_at(detection.name, detection.alias, &path);
                scans.push(scan_detection(&detection, &path, env_files));
                detection
            })
            .collect();
        progress(ScaffoldEvent::Scanned(scans));
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
        if let Err(error) = pom_secrets::SecretStore::new(state.clone(), name).delete_all() {
            eprintln!("scaffold: clean up secrets: {error}");
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

    #[test]
    fn clone_progress_weights_receiving_objects() {
        assert_eq!(
            clone_percent("remote: Counting objects: 100% (5/5), done."),
            Some(10)
        );
        assert_eq!(clone_percent("Receiving objects:  50% (10/20)"), Some(50));
        assert_eq!(
            clone_percent("Resolving deltas: 100% (3/3), done."),
            Some(100)
        );
        assert_eq!(clone_percent("Cloning into 'web'..."), None);
    }
}
