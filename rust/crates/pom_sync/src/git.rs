//! Git with a deadline: network commands (fetch, push, ls-remote) must never hang a background loop.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) const LOCAL: Duration = Duration::from_secs(15);
pub(crate) const NETWORK: Duration = Duration::from_secs(90);
pub(crate) const PUSH: Duration = Duration::from_secs(30);

/// Runs `git -C dir args` with extra `env`: its stdout on success, else the command and what it printed.
pub(crate) fn git_with(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git: {error}"))?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stream) = stream {
                if let Err(error) = stream.read_to_string(&mut text) {
                    eprintln!("sync: read git output: {error}");
                }
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("sync: stop git: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("sync: reap git: {error}");
                }
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
            Err(error) => return Err(format!("git: {error}")),
        }
    };
    let (out, errors) = (
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    );
    if status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(format!(
            "git {}: {}{}",
            args.join(" "),
            out.trim(),
            errors.trim()
        ))
    }
}

pub(crate) fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    git_with(dir, args, &[], timeout)
}

pub(crate) fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"], LOCAL)
        .ok()
        .filter(|branch| !branch.is_empty() && branch != "HEAD")
}

pub(crate) fn is_dirty(dir: &Path) -> Result<bool, String> {
    git(dir, &["status", "--porcelain"], LOCAL).map(|status| !status.is_empty())
}

/// The branch `origin/HEAD` points at, if the clone knows it.
pub(crate) fn origin_default_branch(dir: &Path) -> Option<String> {
    git(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        LOCAL,
    )
    .ok()
    .and_then(|reference| reference.strip_prefix("origin/").map(str::to_string))
    .filter(|branch| !branch.is_empty())
}
