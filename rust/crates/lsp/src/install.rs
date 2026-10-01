//! Finding a server's binary: the first candidate on the PATH; else the copy downloaded before, which starts
//! at once while the newest release is checked for and installed next to it when it is behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::adapters::{Candidate, NpmPackage, SourceBuild};

/// What starts a server: its program and arguments, and the candidate it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub name: &'static str,
    pub binary: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryStatus {
    CheckingForUpdate,
    Downloading,
}

pub(crate) enum Found {
    Status(&'static str, BinaryStatus),
    /// A copy downloaded before, to start with if fetching a newer one takes too long.
    Fallback(Located),
    Done(Result<Located, String>),
}

/// npm's own retries and timeouts, so a dead network fails in seconds.
const NPM_NETWORK_ARGS: [&str; 6] = [
    "--fetch-retry-mintimeout",
    "2000",
    "--fetch-retry-maxtimeout",
    "5000",
    "--fetch-timeout",
    "5000",
];

/// Where downloaded servers live, one folder each.
pub fn languages_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Pomelo/languages"))
}

/// Find a binary for the first candidate that has one, reporting along the way. Blocks on the network, so
/// call it off the UI thread.
pub(crate) fn locate(
    candidates: &[Candidate],
    env: &HashMap<String, String>,
    downloads: Option<&Path>,
    report: &dyn Fn(Found),
) {
    let path = env.get("PATH").map(String::as_str).unwrap_or_default();
    if let Some(located) = candidates
        .iter()
        .find_map(|candidate| on_path(candidate, path, env))
    {
        return report(Found::Done(Ok(located)));
    }
    let built = candidates
        .iter()
        .find_map(|candidate| Some((candidate, candidate.source.as_ref()?)));
    if let (Some((candidate, source)), Some(downloads)) = (built, downloads) {
        return report(Found::Done(build_from_source(
            candidate,
            source,
            &downloads.join(candidate.name),
            path,
            env,
            report,
        )));
    }
    let downloadable = candidates
        .iter()
        .find_map(|candidate| Some((candidate, candidate.npm.as_ref()?)));
    let (Some((candidate, package)), Some(downloads)) = (downloadable, downloads) else {
        let names: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.binary)
            .collect();
        let mut message = format!("{} is not on the PATH.", names.join(" or "));
        if let Some(command) = candidates.iter().find_map(|candidate| candidate.install) {
            message.push_str(&format!(" Install it with: {command}"));
        }
        return report(Found::Done(Err(message)));
    };
    report(Found::Done(download(
        candidate,
        package,
        &downloads.join(candidate.name),
        path,
        env,
        report,
    )));
}

fn on_path(candidate: &Candidate, path: &str, env: &HashMap<String, String>) -> Option<Located> {
    if candidate.source.is_some() {
        return None;
    }
    let found = which(candidate.binary, path)?;
    if let Some(probe) = candidate.probe {
        let works = Command::new(&found)
            .args(probe)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !works {
            eprintln!("lsp: {} is on the PATH but doesn't run", found.display());
            return None;
        }
    }
    Some(Located {
        name: candidate.name,
        binary: found,
        args: candidate.args.iter().map(|arg| arg.to_string()).collect(),
    })
}

fn download(
    candidate: &Candidate,
    package: &NpmPackage,
    dir: &Path,
    path: &str,
    env: &HashMap<String, String>,
    report: &dyn Fn(Found),
) -> Result<Located, String> {
    let (Some(node), Some(npm)) = (which("node", path), which("npm", path)) else {
        return Err(format!(
            "Node.js is not on the PATH; it is needed to download {}.",
            candidate.name
        ));
    };
    let script = dir.join(package.script);
    let located = Located {
        name: candidate.name,
        binary: node,
        args: std::iter::once(script.to_string_lossy().into_owned())
            .chain(candidate.args.iter().map(|arg| arg.to_string()))
            .collect(),
    };
    let cached = script.is_file().then(|| located.clone());
    if let Some(cached) = &cached {
        report(Found::Fallback(cached.clone()));
    }
    let fall_back = |error: String| cached.clone().ok_or(error);
    report(Found::Status(
        candidate.name,
        BinaryStatus::CheckingForUpdate,
    ));
    let latest = match latest_version(&npm, package.name, env) {
        Ok(latest) => latest,
        Err(error) => return fall_back(error),
    };
    let installed = installed_version(dir, package.name);
    if cached.is_some()
        && installed
            .as_deref()
            .is_some_and(|installed| !is_older(installed, &latest))
    {
        return Ok(located);
    }
    report(Found::Status(candidate.name, BinaryStatus::Downloading));
    if let Err(error) = std::fs::create_dir_all(dir) {
        return fall_back(format!("could not create {}: {error}", dir.display()));
    }
    let spec = format!("{}@{latest}", package.name);
    let installing = Command::new(&npm)
        .arg("install")
        .arg(&spec)
        .args(["--no-package-lock", "--save-exact"])
        .args(NPM_NETWORK_ARGS)
        .current_dir(dir)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output();
    match installing {
        Ok(output) if output.status.success() && script.is_file() => Ok(located),
        Ok(output) => fall_back(format!(
            "npm install {spec} failed: {}",
            last_lines(&String::from_utf8_lossy(&output.stderr))
        )),
        Err(error) => fall_back(format!("could not run npm: {error}")),
    }
}

/// How long the source archive may take to download.
const ARCHIVE_TIMEOUT_SECS: &str = "120";

/// The server built once from its pinned source release in `dir`, then reused as it is.
fn build_from_source(
    candidate: &Candidate,
    source: &SourceBuild,
    dir: &Path,
    path: &str,
    env: &HashMap<String, String>,
    report: &dyn Fn(Found),
) -> Result<Located, String> {
    let (Some(node), Some(npm)) = (which("node", path), which("npm", path)) else {
        return Err(format!(
            "Node.js is not on the PATH; it is needed to build {}.",
            candidate.name
        ));
    };
    let release = dir.join(source.folder);
    let script = release.join(source.script);
    let located = Located {
        name: candidate.name,
        binary: node,
        args: source
            .node_args
            .iter()
            .map(|arg| arg.to_string())
            .chain(std::iter::once(script.to_string_lossy().into_owned()))
            .chain(candidate.args.iter().map(|arg| arg.to_string()))
            .collect(),
    };
    if script.is_file() {
        return Ok(located);
    }
    report(Found::Status(candidate.name, BinaryStatus::Downloading));
    if dir.exists() {
        if let Err(error) = std::fs::remove_dir_all(dir) {
            return Err(format!("could not clear {}: {error}", dir.display()));
        }
    }
    if let Err(error) = std::fs::create_dir_all(&release) {
        return Err(format!("could not create {}: {error}", release.display()));
    }
    let archive = dir.join("source.tar.gz");
    let unpacked = dir.join("unpacked");
    let repo = script
        .strip_prefix(&release)
        .ok()
        .and_then(|relative| relative.components().next())
        .map(|first| release.join(first))
        .ok_or_else(|| format!("{} names no folder", source.script))?;
    run(
        Command::new("curl")
            .args(["--fail", "--location", "--silent", "--show-error"])
            .args([
                "--connect-timeout",
                "10",
                "--max-time",
                ARCHIVE_TIMEOUT_SECS,
            ])
            .arg("--output")
            .arg(&archive)
            .arg(source.url),
        "download",
    )?;
    std::fs::create_dir_all(&unpacked)
        .map_err(|error| format!("could not create {}: {error}", unpacked.display()))?;
    run(
        Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&unpacked),
        "unpack",
    )?;
    let top = std::fs::read_dir(&unpacked)
        .ok()
        .and_then(|mut entries| entries.next())
        .and_then(Result::ok)
        .ok_or_else(|| "the source archive is empty".to_string())?;
    std::fs::rename(top.path(), &repo)
        .map_err(|error| format!("could not move the source into place: {error}"))?;
    let npm_in_repo = |args: &[&str]| {
        let mut command = Command::new(&npm);
        command
            .args(args)
            .current_dir(&repo)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null());
        command
    };
    run(
        npm_in_repo(&["install"]).args(NPM_NETWORK_ARGS),
        "npm install",
    )?;
    run(
        &mut npm_in_repo(&["run-script", "compile"]),
        "npm run compile",
    )?;
    // Best effort: what is left over is only space, the server is in place either way.
    std::fs::remove_file(&archive).ok();
    std::fs::remove_dir_all(&unpacked).ok();
    if script.is_file() {
        Ok(located)
    } else {
        Err(format!(
            "building {} made no {}",
            candidate.name, source.script
        ))
    }
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    match command.stdin(Stdio::null()).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "{what} failed: {}",
            last_lines(&String::from_utf8_lossy(&output.stderr))
        )),
        Err(error) => Err(format!("could not {what}: {error}")),
    }
}

fn latest_version(
    npm: &Path,
    package: &str,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let output = Command::new(npm)
        .args(["info", package, "version", "--json"])
        .args(NPM_NETWORK_ARGS)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run npm: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "npm info {package} failed: {}",
            last_lines(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    serde_json::from_slice::<String>(&output.stdout)
        .map_err(|error| format!("npm info {package} gave no version: {error}"))
}

fn installed_version(dir: &Path, package: &str) -> Option<String> {
    let manifest =
        std::fs::read(dir.join("node_modules").join(package).join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    manifest.get("version")?.as_str().map(str::to_string)
}

/// Whether `installed` is an earlier release than `latest`, by their numeric parts.
fn is_older(installed: &str, latest: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parts(installed) < parts(latest)
}

fn last_lines(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

pub(crate) fn which(binary: &str, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(binary))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_is_older_by_its_numbers() {
        assert!(is_older("0.2.9", "0.2.10"));
        assert!(!is_older("1.0.0", "1.0.0"));
        assert!(!is_older("2.0.0", "1.9.9"));
        assert!(is_older("1.0.0-beta.1", "1.0.1"));
    }

    #[test]
    fn a_candidate_on_the_path_wins_without_downloading() {
        let dir = tempfile::tempdir().expect("temp");
        let binary = dir.path().join("pyright-langserver");
        std::fs::write(&binary, "#!/bin/sh\n").expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let env = HashMap::from([(
            "PATH".to_string(),
            format!("/nonexistent:{}", dir.path().display()),
        )]);
        let found = std::cell::RefCell::new(Vec::new());
        let candidates = crate::adapters::python_candidates();
        locate(&candidates, &env, None, &|event| {
            if let Found::Done(result) = event {
                found.borrow_mut().push(result);
            }
        });
        let found = found.into_inner();
        let located = found[0].as_ref().expect("found");
        assert_eq!((located.name, &located.binary), ("pyright", &binary));
        assert_eq!(located.args, ["--stdio"]);
    }

    #[test]
    fn a_downloaded_copy_is_offered_before_checking_for_a_newer_one() {
        let downloads = tempfile::tempdir().expect("temp");
        let bin = tempfile::tempdir().expect("temp");
        use std::os::unix::fs::PermissionsExt;
        for tool in ["node", "npm"] {
            let path = bin.path().join(tool);
            std::fs::write(&path, "#!/bin/sh\nexit 1\n").expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let script = downloads
            .path()
            .join("basedpyright/node_modules/basedpyright/langserver.index.js");
        std::fs::create_dir_all(script.parent().expect("parent")).expect("mkdir");
        std::fs::write(&script, "").expect("write");
        let env = HashMap::from([("PATH".to_string(), bin.path().display().to_string())]);
        let events = std::cell::RefCell::new(Vec::new());
        locate(
            &crate::adapters::python_candidates(),
            &env,
            Some(downloads.path()),
            &|event| {
                events.borrow_mut().push(match event {
                    Found::Status(_, status) => format!("{status:?}"),
                    Found::Fallback(located) => format!("fallback {}", located.name),
                    Found::Done(result) => format!("done {}", result.is_ok()),
                })
            },
        );
        assert_eq!(
            events.into_inner(),
            ["fallback basedpyright", "CheckingForUpdate", "done true"],
            "a failed check keeps the copy there is"
        );
    }

    #[test]
    #[ignore = "downloads vtsls from npm"]
    fn downloads_a_server_from_npm_and_reuses_it() {
        let downloads = tempfile::tempdir().expect("temp");
        let env = crate::capture_login_env(downloads.path()).expect("login env");
        let candidates: Vec<Candidate> = crate::adapters::adapter_for(editor::Lang::Tsx)
            .expect("tsx")
            .0
            .candidates
            .iter()
            .filter(|candidate| candidate.name == "vtsls")
            .map(|candidate| Candidate {
                binary: "pomelo-not-on-path",
                ..candidate.clone()
            })
            .collect();
        let run = || {
            let events = std::cell::RefCell::new(Vec::new());
            locate(&candidates, &env, Some(downloads.path()), &|event| {
                events.borrow_mut().push(match event {
                    Found::Status(_, status) => format!("{status:?}"),
                    Found::Fallback(_) => "fallback".to_string(),
                    Found::Done(result) => format!("done {:?}", result.map(|located| located.args)),
                })
            });
            events.into_inner()
        };
        let first = run();
        eprintln!("{first:?}");
        assert_eq!(&first[..2], ["CheckingForUpdate", "Downloading"]);
        assert!(first[2].starts_with("done Ok"), "{first:?}");
        let second = run();
        eprintln!("{second:?}");
        assert_eq!(
            &second[..2],
            ["fallback", "CheckingForUpdate"],
            "no second download"
        );
        assert!(second[2].starts_with("done Ok"));
    }
}
