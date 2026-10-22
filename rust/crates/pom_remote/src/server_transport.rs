//! One ssh session running `pom server proxy` on the host: its stdin and stdout are the byte pipe of a
//! [`pom_rpc::Transport`], and its stderr is kept for why it ended.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{Host, Ssh};

const STDERR_LINES: usize = 64;

/// How a `pom server proxy` session ended before the server answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyFailure {
    /// ssh could not connect (its own exit 255).
    Unreachable(String),
    /// The host's pom predates `pom server`.
    NoServer(String),
    /// pom is not installed there.
    NotInstalled,
    /// Anything else, with the last thing it said.
    Other(String),
}

/// The ssh child behind a transport; dropping it kills and reaps the child.
pub struct ProxyProcess {
    child: Arc<Mutex<Child>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn stop(child: &Mutex<Child>) -> std::io::Result<()> {
    let mut child = locked(child);
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait().map(|_| ())
}

impl ProxyProcess {
    /// What ssh and the remote pom printed on stderr, oldest first.
    pub fn stderr(&self) -> Vec<String> {
        locked(&self.stderr).iter().cloned().collect()
    }

    /// The exit status once the child ends, waiting up to `timeout` for it.
    pub fn wait_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            match locked(&self.child).try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Why the session ended, from its exit status and stderr.
    pub fn failure(&self, timeout: Duration) -> ProxyFailure {
        let status = self.wait_exit(timeout);
        classify(status.and_then(|status| status.code()), &self.stderr())
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        if let Err(error) = stop(&self.child) {
            eprintln!("pom server proxy: stopping ssh: {error}");
        }
    }
}

/// Reads an ended proxy session: ssh's 255, the shell's 127, an older pom's usage error, or the proxy's
/// own JSON error line.
pub fn classify(code: Option<i32>, stderr: &[String]) -> ProxyFailure {
    let last = stderr
        .iter()
        .map(|line| line.trim())
        .rfind(|line| !line.is_empty())
        .map(|line| line.strip_prefix("error: ").unwrap_or(line).to_string());
    let unknown_command = stderr
        .iter()
        .any(|line| line.contains("unknown command server"));
    match code {
        Some(255) | Some(crate::EXIT_UNREACHABLE) => {
            ProxyFailure::Unreachable(last.unwrap_or_else(|| "ssh could not connect".to_string()))
        }
        Some(127) => ProxyFailure::NotInstalled,
        _ if unknown_command => ProxyFailure::NoServer(
            "its pom is older than pom server; update pom on the VM to get terminals".to_string(),
        ),
        Some(code) => {
            let said = last
                .as_deref()
                .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .and_then(|value| value["error"].as_str().map(str::to_string));
            ProxyFailure::Other(said.or(last).unwrap_or_else(|| format!("exit {code}")))
        }
        None => ProxyFailure::Other(last.unwrap_or_else(|| "the session ended".to_string())),
    }
}

/// Starts `ssh ... pom server proxy` on `host` and wraps its pipes.
pub fn open(ssh: &Ssh, host: &Host) -> std::io::Result<(pom_rpc::Transport, ProxyProcess)> {
    let args = ["server".to_string(), "proxy".to_string()];
    let mut command = ssh.command_with(host, &args, &["IPQoS=lowdelay"]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let missing = |what: &str| std::io::Error::other(format!("ssh has no {what}"));
    let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr_pipe = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    let stderr = Arc::new(Mutex::new(VecDeque::new()));
    let kept = stderr.clone();
    std::thread::Builder::new()
        .name("pom-server-ssh-stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let Ok(line) = line else { break };
                let mut lines = locked(&kept);
                if lines.len() == STDERR_LINES {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
        })?;
    let child = Arc::new(Mutex::new(child));
    let closing = child.clone();
    let transport = pom_rpc::Transport::new(stdout, stdin).with_closer(move || stop(&closing));
    Ok((transport, ProxyProcess { child, stderr }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn an_ended_session_says_why() {
        assert_eq!(
            classify(Some(255), &lines(&["ssh: connect: Connection refused"])),
            ProxyFailure::Unreachable("ssh: connect: Connection refused".into())
        );
        assert_eq!(classify(Some(127), &[]), ProxyFailure::NotInstalled);
        assert!(matches!(
            classify(
                Some(2),
                &lines(&["unknown command server", "", "usage: pom ..."])
            ),
            ProxyFailure::NoServer(_)
        ));
        assert_eq!(
            classify(Some(70), &lines(&[r#"{"error":"server_gone"}"#])),
            ProxyFailure::Other("server_gone".into())
        );
        assert_eq!(
            classify(Some(1), &lines(&["error: no socket dir"])),
            ProxyFailure::Other("no socket dir".into())
        );
    }
}
