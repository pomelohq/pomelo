//! A new project's config, checked the way someone would first run it: the config doctor, each repo's
//! setup in main, then every service booted once and stopped again. The first failure stops the run so
//! it can be repaired and verified again.

mod summary;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_services::{ServiceRunner, ServiceTarget};

pub use summary::{counts, summary_lines, Counts, Segment};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckKind {
    Doctor,
    Install { repo: String },
    Boot { repo: String, service: String },
}

impl CheckKind {
    pub fn title(&self) -> String {
        match self {
            CheckKind::Doctor => "config doctor".into(),
            CheckKind::Install { repo } => format!("install {repo}"),
            CheckKind::Boot { repo, service } if repo.is_empty() => format!("boot {service}"),
            CheckKind::Boot { repo, service } => format!("boot {repo} > {service}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Running,
    Passed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub kind: CheckKind,
    pub status: CheckStatus,
    /// One line: "0 errors, 1 warning", "bundle install - 41s", the error a boot died with.
    pub detail: String,
    /// What a repair needs to see: the doctor's findings, the tail of a failed command's output.
    pub output: String,
    /// The setup an install ran, so a later verify can skip a repo whose setup has not changed.
    pub command: String,
}

impl Check {
    fn running(kind: CheckKind) -> Check {
        Check {
            kind,
            status: CheckStatus::Running,
            detail: String::new(),
            output: String::new(),
            command: String::new(),
        }
    }

    fn passed(mut self, detail: impl Into<String>) -> Check {
        self.status = CheckStatus::Passed;
        self.detail = detail.into();
        self
    }

    fn failed(mut self, detail: impl Into<String>, output: impl Into<String>) -> Check {
        self.status = CheckStatus::Failed;
        self.detail = detail.into();
        self.output = output.into();
        self
    }
}

/// How a started service is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootState {
    Starting,
    Listening(u16),
    /// Up, with no port to answer on (a worker).
    Running,
    /// It exited: its last telling line, and the output before it.
    Exited {
        line: String,
        output: String,
    },
}

/// What verify needs from the services of main.
pub trait Services {
    /// Env files written, shared services and databases up, before anything installs or boots.
    fn prepare(&self) -> Result<(), String>;
    fn repo_env(&self, repo: &str) -> Vec<(String, String)>;
    fn start(&self, target: &ServiceTarget) -> Result<(), String>;
    fn state(&self, target: &ServiceTarget) -> BootState;
    fn has_port(&self, target: &ServiceTarget) -> bool;
    fn stop(&self, target: &ServiceTarget);
}

pub struct VerifyInput<'a> {
    pub config: &'a Config,
    pub config_path: &'a Path,
    pub root: &'a Path,
    pub secret_names: &'a [String],
    pub machine: &'a pom_doctor::Machine<'a>,
    /// Repo -> the setup it last installed with; an unchanged one is not run again.
    pub installed: &'a HashMap<String, String>,
    /// Services left out of the boot checks: (repo, service).
    pub skipped: &'a [(String, String)],
    pub boot_timeout: Duration,
    /// How long a service without a port must stay up to count as booted.
    pub worker_grace: Duration,
}

pub const BOOT_TIMEOUT: Duration = Duration::from_secs(90);
pub const WORKER_GRACE: Duration = Duration::from_secs(6);

/// Runs the checks in order, reporting each as it starts and ends; stops at the first failure or when
/// `cancel` is set.
pub fn verify(
    input: &VerifyInput<'_>,
    services: &dyn Services,
    report: &mut dyn FnMut(&Check),
    cancel: &AtomicBool,
) -> Vec<Check> {
    let mut done = Vec::new();
    let mut finish = |check: Check, report: &mut dyn FnMut(&Check)| {
        report(&check);
        let failed = check.status == CheckStatus::Failed;
        done.push(check);
        failed
    };
    let doctor = Check::running(CheckKind::Doctor);
    report(&doctor);
    if finish(doctor_check(input, doctor), report) {
        return done;
    }
    if let Err(error) = services.prepare() {
        let check = Check::running(CheckKind::Install {
            repo: "shared services".into(),
        });
        finish(check.failed(first_line(&error), error.clone()), report);
        return done;
    }
    let branch = input.config.global_default_branch();
    for (repo, dir) in &input.config.repos {
        if cancel.load(Ordering::Relaxed) {
            return done;
        }
        let checkout = pom_layout::repo_worktree(input.root, repo, branch, true);
        if !checkout.is_dir() {
            continue;
        }
        let mut steps = dir.effective_setup();
        steps.extend(dir.effective_migrate());
        if steps.is_empty() {
            continue;
        }
        let command = steps.join(" && ");
        let mut check = Check::running(CheckKind::Install { repo: repo.clone() });
        check.command = command.clone();
        if input.installed.get(repo) == Some(&command) {
            finish(check.passed(format!("{} - done before", steps[0])), report);
            continue;
        }
        report(&check);
        let started = Instant::now();
        let result = pom_workspace::run_shell(true, &command, &checkout, &services.repo_env(repo));
        let check = match result {
            Ok(()) => check.passed(format!("{} - {}", steps[0], seconds(started.elapsed()))),
            Err(output) => check.failed(telling_line(&output), output),
        };
        if finish(check, report) {
            return done;
        }
    }
    let targets = ServiceRunner::service_targets(input.config, branch, true);
    for target in targets {
        let skipped = input
            .skipped
            .iter()
            .any(|(repo, service)| *repo == target.repo && *service == target.service);
        if skipped || cancel.load(Ordering::Relaxed) {
            continue;
        }
        let check = Check::running(CheckKind::Boot {
            repo: target.repo.clone(),
            service: target.service.clone(),
        });
        report(&check);
        let check = boot(input, services, &target, check, cancel);
        services.stop(&target);
        if finish(check, report) {
            return done;
        }
    }
    done
}

fn doctor_check(input: &VerifyInput<'_>, check: Check) -> Check {
    let findings = pom_doctor::diagnose(
        Some(input.config),
        input.config_path,
        input.root,
        input.secret_names,
        input.machine,
    );
    let count = |severity| {
        findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    };
    let (errors, warnings) = (
        count(pom_doctor::Severity::Error),
        count(pom_doctor::Severity::Warn),
    );
    let detail = format!(
        "{}, {}",
        plural(errors, "error"),
        plural(warnings, "warning")
    );
    if errors == 0 {
        return check.passed(detail);
    }
    let problems: Vec<pom_doctor::Finding> = findings
        .into_iter()
        .filter(|finding| finding.severity == pom_doctor::Severity::Error)
        .collect();
    check.failed(detail, pom_doctor::fix_prompt(&problems))
}

fn boot(
    input: &VerifyInput<'_>,
    services: &dyn Services,
    target: &ServiceTarget,
    check: Check,
    cancel: &AtomicBool,
) -> Check {
    if let Err(error) = services.start(target) {
        return check.failed(first_line(&error), error.clone());
    }
    let started = Instant::now();
    let has_port = services.has_port(target);
    loop {
        match services.state(target) {
            BootState::Listening(port) => return check.passed(format!("listening on :{port}")),
            BootState::Running if !has_port && started.elapsed() >= input.worker_grace => {
                return check.passed(format!("running after {}", seconds(started.elapsed())))
            }
            BootState::Exited { line, output } => return check.failed(line, output),
            _ => {}
        }
        if cancel.load(Ordering::Relaxed) {
            return check.failed("stopped", "");
        }
        if started.elapsed() >= input.boot_timeout {
            let detail = if has_port {
                format!(
                    "did not answer on its port within {}",
                    seconds(input.boot_timeout)
                )
            } else {
                format!("did not start within {}", seconds(input.boot_timeout))
            };
            return check.failed(detail, "");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The services of main, run by the project's runner.
pub struct RunnerServices<'a> {
    pub runner: &'a ServiceRunner,
    pub config: &'a Config,
}

impl RunnerServices<'_> {
    fn branch(&self) -> &str {
        self.config.global_default_branch()
    }
}

impl Services for RunnerServices<'_> {
    fn prepare(&self) -> Result<(), String> {
        let branch = self.branch();
        self.runner
            .refresh_workspace_env(self.config, branch)
            .map_err(|error| format!("write env files: {error}"))?;
        if self.config.shared_services.is_empty() {
            return Ok(());
        }
        self.runner
            .ensure_shared(self.config)
            .map_err(|error| format!("start shared services: {error}"))?;
        let names = pom_services::database_names(self.config, branch);
        self.runner
            .create_databases_when_ready(self.config, &names)
            .map_err(|error| format!("create databases: {error}"))
    }

    fn repo_env(&self, repo: &str) -> Vec<(String, String)> {
        self.runner
            .workspace_env(self.config, self.branch())
            .repo_env(repo)
    }

    fn start(&self, target: &ServiceTarget) -> Result<(), String> {
        self.runner
            .start(self.config, target)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn state(&self, target: &ServiceTarget) -> BootState {
        let holders = self.runner.holders();
        let holder = self.runner.holder_name(target);
        if !holders.holder_alive(&holder) {
            let output = holders
                .crash_info(&holder)
                .map(|info| strip_ansi(&String::from_utf8_lossy(&info.output)))
                .unwrap_or_default();
            let line = match telling_line(&output) {
                line if line.is_empty() => "exited".to_string(),
                line => line,
            };
            return BootState::Exited { line, output };
        }
        match self.runner.port(self.config, target) {
            Some(port) if answers(port) => BootState::Listening(port),
            Some(_) => BootState::Starting,
            None => BootState::Running,
        }
    }

    fn has_port(&self, target: &ServiceTarget) -> bool {
        self.runner.port(self.config, target).is_some()
    }

    fn stop(&self, target: &ServiceTarget) {
        if let Err(error) = self.runner.stop(target) {
            eprintln!("verify: stop {}: {error}", target.service);
        }
    }
}

fn answers(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// Where a repo of the project is checked out in main.
pub fn main_checkout(root: &Path, config: &Config, repo: &str) -> PathBuf {
    pom_layout::repo_worktree(root, repo, config.global_default_branch(), true)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn seconds(elapsed: Duration) -> String {
    format!("{}s", elapsed.as_secs())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

/// The last output line that names an error, else the last line with anything on it.
pub fn telling_line(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let named = lines.iter().rev().find(|line| {
        let lower = line.to_ascii_lowercase();
        [
            "error",
            "exception",
            "panic",
            "fatal",
            "failed",
            "not found",
        ]
        .iter()
        .any(|word| lower.contains(word))
    });
    named
        .or(lines.last())
        .map(|line| line.to_string())
        .unwrap_or_default()
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
