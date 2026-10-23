//! Whether git on this machine signs in to each repo's origin without asking, which the Git panel of a remote
//! window needs: it runs fetch, pull and push here, with this machine's own credentials only.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Finding, Severity};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginAnswer {
    Reached,
    NoOrigin,
    SignInRefused(String),
    Unreachable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginProbe {
    pub repo: String,
    pub url: String,
    pub answer: OriginAnswer,
}

/// What a run printed, or `None` when it was stopped at the timeout.
fn run_within(command: &mut Command, timeout: Duration) -> Option<(bool, String, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out_reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        stderr.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("doctor: stop {command:?}: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("doctor: reap {command:?}: {error}");
                }
                return None;
            }
        }
    };
    let stdout = out_reader.join().ok()?.unwrap_or_default();
    let stderr = err_reader.join().ok()?.unwrap_or_default();
    Some((status.success(), stdout, stderr))
}

fn git(checkout: &Path, path_env: &str) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(checkout)
        .env("PATH", path_env)
        // A prompt nobody can answer would hang the check, and the server never has anyone to ask.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_ASKPASS");
    command
}

/// Asks the origin of the repo checked out at `checkout` for its HEAD, the way a fetch would sign in.
pub fn probe_origin(repo: &str, checkout: &Path, path_env: &str) -> OriginProbe {
    let probe = |url: String, answer| OriginProbe {
        repo: repo.to_string(),
        url,
        answer,
    };
    let url = match run_within(
        git(checkout, path_env).args(["remote", "get-url", "origin"]),
        PROBE_TIMEOUT,
    ) {
        Some((true, stdout, _)) if !stdout.trim().is_empty() => stdout.trim().to_string(),
        _ => return probe(String::new(), OriginAnswer::NoOrigin),
    };
    let mut command = git(checkout, path_env);
    let own_ssh_command = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || run_within(
            git(checkout, path_env).args(["config", "core.sshCommand"]),
            PROBE_TIMEOUT,
        )
        .is_some_and(|(set, _, _)| set);
    if !own_ssh_command {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    command.args(["ls-remote", "--quiet", "origin", "HEAD"]);
    let answer = match run_within(&mut command, PROBE_TIMEOUT) {
        Some((true, _, _)) => OriginAnswer::Reached,
        Some((false, stdout, stderr)) => {
            let said = git::working_copy::CommandOutput { stdout, stderr };
            let last = said
                .stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("git ls-remote failed")
                .trim()
                .to_string();
            if git::access::sign_in_failed(&said) {
                OriginAnswer::SignInRefused(last)
            } else {
                OriginAnswer::Unreachable(last)
            }
        }
        None => OriginAnswer::Unreachable(format!("no answer within {}s", PROBE_TIMEOUT.as_secs())),
    };
    probe(shown_url(&url), answer)
}

/// The url without a user or token written into it.
fn shown_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{path}"),
        None => url.to_string(),
    }
}

fn over_ssh(url: &str) -> bool {
    url.starts_with("ssh://") || (!url.contains("://") && url.contains('@') && url.contains(':'))
}

/// One finding per origin git cannot sign in to or reach; one `ok` finding when every origin answered.
pub fn git_auth_findings(probes: &[OriginProbe]) -> Vec<Finding> {
    let mut out = Vec::new();
    for probe in probes {
        let (repo, url) = (&probe.repo, &probe.url);
        match &probe.answer {
            OriginAnswer::Reached | OriginAnswer::NoOrigin => {}
            OriginAnswer::SignInRefused(said) => {
                let fix = if over_ssh(url) {
                    "give this machine an SSH key your git host accepts: add ~/.ssh/id_ed25519.pub to your account or as a deploy key, or load the key in ssh-agent"
                } else {
                    "store a credential for this machine: `gh auth login` then `gh auth setup-git`, or a token in a git credential helper"
                };
                out.push(
                    Finding::new(
                        format!("git.auth:{repo}"),
                        Severity::Error,
                        format!("git cannot sign in to {repo}'s origin"),
                    )
                    .detail(format!(
                        "{url}: {said}. Fetch, pull and push from the Git panel run here with this machine's credentials"
                    ))
                    .fix(fix),
                );
            }
            OriginAnswer::Unreachable(said) => out.push(
                Finding::new(
                    format!("git.origin:{repo}"),
                    Severity::Warn,
                    format!("git cannot reach {repo}'s origin"),
                )
                .detail(format!("{url}: {said}"))
                .fix("check the network and the origin url (`git remote -v` in the repo)"),
            ),
        }
    }
    if out.is_empty()
        && probes
            .iter()
            .any(|probe| probe.answer == OriginAnswer::Reached)
    {
        out.push(Finding::new(
            "git.auth",
            Severity::Ok,
            "git signs in to every origin",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(repo: &str, url: &str, answer: OriginAnswer) -> OriginProbe {
        OriginProbe {
            repo: repo.into(),
            url: url.into(),
            answer,
        }
    }

    #[test]
    fn a_refused_sign_in_names_the_repo_and_how_to_give_this_machine_a_credential() {
        let findings = git_auth_findings(&[
            probe(
                "api",
                "git@example.com:myproject/api.git",
                OriginAnswer::Reached,
            ),
            probe(
                "web",
                "git@example.com:myproject/web.git",
                OriginAnswer::SignInRefused("Permission denied (publickey).".into()),
            ),
            probe(
                "worker",
                "https://example.com/myproject/worker.git",
                OriginAnswer::SignInRefused("terminal prompts disabled".into()),
            ),
            probe(
                "docs",
                "https://example.com/myproject/docs.git",
                OriginAnswer::Unreachable("Could not resolve host".into()),
            ),
        ]);
        let ids: Vec<&str> = findings.iter().map(|finding| finding.id.as_str()).collect();
        assert_eq!(ids, ["git.auth:web", "git.auth:worker", "git.origin:docs"]);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].fix.contains("SSH key"), "{}", findings[0].fix);
        assert!(findings[1].fix.contains("gh auth"), "{}", findings[1].fix);
        assert_eq!(findings[2].severity, Severity::Warn);
    }

    #[test]
    fn every_origin_answering_is_one_ok_finding() {
        let findings = git_auth_findings(&[
            probe(
                "api",
                "git@example.com:myproject/api.git",
                OriginAnswer::Reached,
            ),
            probe("web", "", OriginAnswer::NoOrigin),
        ]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Ok);
        assert!(git_auth_findings(&[probe("web", "", OriginAnswer::NoOrigin)]).is_empty());
    }

    #[test]
    fn a_token_in_the_url_is_not_shown() {
        assert_eq!(
            shown_url("https://user:secret@example.com/myproject/api.git"),
            "https://example.com/myproject/api.git"
        );
        assert_eq!(
            shown_url("git@example.com:myproject/api.git"),
            "git@example.com:myproject/api.git"
        );
        assert_eq!(
            shown_url("https://example.com/myproject/a@b.git"),
            "https://example.com/myproject/a@b.git"
        );
    }

    fn run(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_local_origin_answers_and_a_missing_one_does_not() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let work = dir.path().join("api");
        std::fs::create_dir(&work)?;
        run(dir.path(), &["init", "--quiet", "--bare", "origin.git"]);
        run(&work, &["init", "--quiet", "-b", "main"]);
        let path_env = std::env::var("PATH").unwrap_or_default();
        assert_eq!(
            probe_origin("api", &work, &path_env).answer,
            OriginAnswer::NoOrigin
        );
        let origin = dir.path().join("origin.git");
        run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        assert_eq!(
            probe_origin("api", &work, &path_env).answer,
            OriginAnswer::Reached
        );
        run(
            &work,
            &[
                "remote",
                "set-url",
                "origin",
                &dir.path().join("gone.git").to_string_lossy(),
            ],
        );
        assert!(matches!(
            probe_origin("api", &work, &path_env).answer,
            OriginAnswer::Unreachable(_)
        ));
        Ok(())
    }
}
