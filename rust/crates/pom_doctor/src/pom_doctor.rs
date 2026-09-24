use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_config::Config;
use serde::Serialize;

const DOCKER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
    Ok,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fix: String,
    pub agent_fixable: bool,
}

impl Finding {
    fn new(id: impl Into<String>, severity: Severity, title: impl Into<String>) -> Finding {
        Finding {
            id: id.into(),
            severity,
            title: title.into(),
            detail: String::new(),
            fix: String::new(),
            agent_fixable: false,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    fn fix(mut self, fix: impl Into<String>) -> Finding {
        self.fix = fix.into();
        self
    }

    fn agent_fixable(mut self) -> Finding {
        self.agent_fixable = true;
        self
    }
}

/// How the machine answers the tool checks; tests swap in their own.
pub struct Machine<'a> {
    pub has_tool: &'a dyn Fn(&str) -> bool,
    pub docker_running: &'a dyn Fn() -> bool,
}

pub fn on_path(path: &str, tool: &str) -> bool {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join(tool).is_file())
}

pub fn docker_answers(path: &str) -> bool {
    let Ok(mut child) = Command::new("docker")
        .arg("info")
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + DOCKER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("doctor: stop docker info: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("doctor: reap docker info: {error}");
                }
                return false;
            }
        }
    }
}

fn merged_text(config_path: &Path) -> String {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    std::iter::once(config_path.to_path_buf())
        .chain(pom_config::fragment_files(dir).unwrap_or_default())
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

fn template_refs(text: &str) -> Vec<&str> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        refs.push(after[..close].trim());
        rest = &after[close + 2..];
    }
    refs
}

/// Everything standing between the project and a first run, most blocking first; a single `ok` finding when
/// nothing does.
pub fn diagnose(
    config: Option<&Config>,
    config_path: &Path,
    project_root: &Path,
    secret_names: &[String],
    machine: &Machine<'_>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(config) = config else {
        out.push(
            Finding::new("config.load", Severity::Error, "No project config")
                .detail("pom.yml not found or failed to load")
                .fix("run `pom init` or open a project folder"),
        );
        return out;
    };
    if let Err(error) = config.validate() {
        out.push(
            Finding::new("config.validate", Severity::Error, "Config is invalid")
                .detail(error)
                .fix("fix the reported key (typo'd alias / unknown {{var}})")
                .agent_fixable(),
        );
    }
    if !(machine.has_tool)("git") {
        out.push(Finding::new("tool.git", Severity::Error, "git not found").fix("install git"));
    }
    let shared = !config.shared_services.is_empty();
    if !(machine.has_tool)("docker") {
        if shared {
            out.push(
                Finding::new("tool.docker", Severity::Error, "docker not found")
                    .detail("shared services (postgres/redis/...) need Docker")
                    .fix("install Docker Desktop / OrbStack"),
            );
        }
    } else if shared && !(machine.docker_running)() {
        out.push(
            Finding::new("docker.down", Severity::Error, "Docker not running")
                .detail("shared services can't start until Docker is up")
                .fix("start Docker"),
        );
    }
    let default_branch = config.global_default_branch();
    for name in config.repos.keys() {
        let path = pom_layout::repo_worktree(project_root, name, default_branch, true);
        if !path.is_dir() {
            out.push(
                Finding::new(
                    format!("repo.missing:{name}"),
                    Severity::Error,
                    format!("Repo not found: {name}"),
                )
                .detail(path.to_string_lossy())
                .fix("clone/prepare it (run `pom prepare-main`), or fix the repo path in config"),
            );
        }
    }
    let removed = pom_config::maintain::removed_keys_in(config_path);
    if !removed.is_empty() {
        out.push(
            Finding::new(
                "config.removed",
                Severity::Error,
                format!("Removed config keys: {}", removed.join(", ")),
            )
            .detail("these keys were removed from the schema and are ignored")
            .fix("run config_normalize (or delete them)")
            .agent_fixable(),
        );
    }
    let merged = merged_text(config_path);
    let refs = template_refs(&merged);
    let mut missing: Vec<&str> = refs
        .iter()
        .filter_map(|reference| reference.strip_prefix("secret."))
        .filter(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .filter(|name| !secret_names.iter().any(|known| known == name))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    for name in missing {
        out.push(
            Finding::new(
                format!("secret.missing:{name}"),
                Severity::Warn,
                format!("Secret not set: {name}"),
            )
            .detail(format!(
                "config uses {{{{secret.{name}}}}} but no value is stored"
            ))
            .fix("add it in Settings > Integrations > Secrets"),
        );
    }
    let mut services: Vec<&String> = config.shared_services.keys().collect();
    services.sort();
    for name in services {
        let prefix = format!("shared.{name}.");
        if !refs.iter().any(|reference| reference.starts_with(&prefix)) {
            out.push(
                Finding::new(
                    format!("shared.unwired:{name}"),
                    Severity::Warn,
                    format!("Shared service not wired: {name}"),
                )
                .detail(format!(
                    "shared_services.{name} is declared but no repo env references {{{{shared.{name}.url}}}} / {{{{shared.{name}.host}}}} / {{{{shared.{name}.port}}}} - its services will start with no connection info"
                ))
                .fix(format!(
                    "add the connection env to the repos that need it (e.g. DATABASE_URL: postgresql://{{{{shared.{name}.url}}}}/{{{{db.main}}}})"
                ))
                .agent_fixable(),
            );
        }
    }
    if out.is_empty() {
        out.push(Finding::new("ok", Severity::Ok, "Ready to run").detail("no blocking gaps found"));
    }
    out
}

/// `{findings, errors, warnings}`, the shape the previous core's doctor endpoint returned.
pub fn report(findings: &[Finding]) -> serde_json::Value {
    let count = |severity: Severity| {
        findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    };
    serde_json::json!({
        "findings": findings,
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warn),
    })
}

pub fn fix_prompt(findings: &[Finding]) -> String {
    let gaps: Vec<String> = findings
        .iter()
        .filter(|finding| finding.severity != Severity::Ok)
        .map(|finding| {
            let severity = match finding.severity {
                Severity::Error => "error",
                Severity::Warn => "warn",
                Severity::Ok => "ok",
            };
            if finding.detail.is_empty() {
                format!("- [{severity}] {}", finding.title)
            } else {
                format!("- [{severity}] {} ({})", finding.title, finding.detail)
            }
        })
        .collect();
    format!(
        "Make this project runnable. Current config_doctor findings:\n{}\n\nFix them via the pom MCP config tools, then loop config_doctor until it reports no\nerrors. Ask me only for values you can't infer (a real secret value, a repo's clone URL).",
        gaps.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, yaml: &str) -> std::path::PathBuf {
        let path = root.join("pom.yml");
        if let Err(error) = std::fs::write(&path, yaml) {
            panic!("write pom.yml: {error}");
        }
        path
    }

    fn machine(docker: bool) -> (impl Fn(&str) -> bool, impl Fn() -> bool) {
        (
            move |tool: &str| tool == "git" || (docker && tool == "docker"),
            move || docker,
        )
    }

    #[test]
    fn a_clean_project_is_ready_to_run() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nrepos:\n  api:\n    services:\n      web: rails s\n",
        );
        std::fs::create_dir_all(temp.path().join("workspace--main/api"))
            .map_err(|error| error.to_string())?;
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(true);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
            },
        );
        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            ["ok"]
        );
        assert_eq!(report(&findings)["errors"], 0);
        Ok(())
    }

    #[test]
    fn gaps_are_reported_in_order_with_fixes() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nshared_services:\n  postgres:\n    type: postgres\n  redis:\n    type: redis\nrepos:\n  api:\n    env:\n      KEY: \"{{secret.API_KEY}}\"\n      TOKEN: \"{{ secret.TOKEN }}\"\n      REDIS: \"{{shared.redis.url}}\"\n    services:\n      web: rails s\nwebhook:\n  port: 1\n",
        );
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(false);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &["TOKEN".to_string()],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
            },
        );
        let ids: Vec<&str> = findings.iter().map(|finding| finding.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "tool.docker",
                "repo.missing:api",
                "config.removed",
                "secret.missing:API_KEY",
                "shared.unwired:postgres"
            ]
        );
        assert!(findings
            .iter()
            .all(|finding| finding.title.is_ascii() && finding.detail.is_ascii()));
        let prompt = fix_prompt(&findings);
        assert!(prompt.contains("- [warn] Secret not set: API_KEY"));
        assert_eq!(report(&findings)["warnings"], 2);
        Ok(())
    }

    #[test]
    fn no_config_says_so() {
        let (has_tool, docker) = machine(true);
        let findings = diagnose(
            None,
            Path::new("/nonexistent/pom.yml"),
            Path::new("/nonexistent"),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
            },
        );
        assert_eq!(findings[0].id, "config.load");
    }
}
