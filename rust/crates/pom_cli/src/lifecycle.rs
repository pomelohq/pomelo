//! `pom refresh|release|get|describe|apply`: the project's workspaces as a whole.

use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use pom_services::ServiceTarget;
use pom_workspace::{CreateRequest, DeleteRequest};

use crate::args::Args;
use crate::config::{pretty, table};
use crate::db::confirm;
use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LifecycleCommand {
    Refresh,
    Release {
        volumes: bool,
        worktrees: bool,
        yes: bool,
    },
    Get {
        json: bool,
    },
    Describe {
        branch: String,
        json: bool,
    },
    Apply {
        branch: Option<String>,
        yes: bool,
    },
}

pub(crate) fn parse(name: &str, words: &[&str]) -> Result<LifecycleCommand, String> {
    let args = Args::parse(words, &["-o", "--output"])?;
    match name {
        "refresh" => {
            args.allow(&[])?;
            args.at_most(0, "refresh")?;
            Ok(LifecycleCommand::Refresh)
        }
        "release" => {
            args.allow(&["--disk", "--worktrees", "--yes"])?;
            args.at_most(0, "release")?;
            Ok(LifecycleCommand::Release {
                volumes: args.has("--disk"),
                worktrees: args.has("--worktrees"),
                yes: args.has("--yes"),
            })
        }
        "get" => {
            args.allow(&["-o", "--output"])?;
            match args.positional.as_slice() {
                [resource] if matches!(resource.as_str(), "workspaces" | "workspace" | "ws") => {
                    Ok(LifecycleCommand::Get { json: args.json()? })
                }
                _ => Err("usage: pom get workspaces [-o json]".into()),
            }
        }
        "describe" => {
            args.allow(&["-o", "--output"])?;
            match args.positional.as_slice() {
                [resource, branch]
                    if matches!(resource.as_str(), "workspace" | "workspaces" | "ws") =>
                {
                    Ok(LifecycleCommand::Describe {
                        branch: branch.clone(),
                        json: args.json()?,
                    })
                }
                _ => Err("usage: pom describe workspace <branch> [-o json]".into()),
            }
        }
        _ => {
            args.allow(&["--yes"])?;
            args.at_most(1, "apply")?;
            Ok(LifecycleCommand::Apply {
                branch: args.positional.first().cloned(),
                yes: args.has("--yes"),
            })
        }
    }
}

struct ServiceStatus {
    repo: String,
    name: String,
    up: bool,
    port: Option<u16>,
}

struct WorkspaceStatus {
    branch: String,
    is_main: bool,
    path: PathBuf,
    services: Vec<ServiceStatus>,
    missing_repos: Vec<String>,
}

impl WorkspaceStatus {
    fn ready(&self) -> usize {
        self.services.iter().filter(|service| service.up).count()
    }

    fn phase(&self) -> &'static str {
        match (self.ready(), self.services.len()) {
            (_, 0) => "Empty",
            (0, _) => "Stopped",
            (ready, total) if ready < total => "Partial",
            _ => "Running",
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "branch": self.branch,
            "is_main": self.is_main,
            "path": self.path,
            "phase": self.phase(),
            "ready": self.ready(),
            "total": self.services.len(),
            "missing_repos": self.missing_repos,
            "services": self.services.iter().map(|service| serde_json::json!({
                "repo": service.repo, "name": service.name, "up": service.up, "port": service.port,
            })).collect::<Vec<_>>(),
        })
    }
}

fn age(path: &std::path::Path) -> String {
    let Ok(elapsed) = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default()
        })
    else {
        return "-".into();
    };
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

impl Session {
    pub(crate) fn lifecycle_command(
        &self,
        command: &LifecycleCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        match command {
            LifecycleCommand::Refresh => self.refresh(out),
            LifecycleCommand::Release {
                volumes,
                worktrees,
                yes,
            } => self.release(*volumes, *worktrees, *yes, out),
            LifecycleCommand::Get { json } => {
                let statuses = self.statuses();
                if *json {
                    let list: Vec<_> = statuses.iter().map(WorkspaceStatus::json).collect();
                    return say(out, &pretty(&serde_json::Value::Array(list))?);
                }
                let mut rows = vec![vec![
                    "NAME".to_string(),
                    "READY".to_string(),
                    "PHASE".to_string(),
                    "AGE".to_string(),
                ]];
                for status in &statuses {
                    let name = if status.is_main {
                        format!("{} (main)", status.branch)
                    } else {
                        status.branch.clone()
                    };
                    rows.push(vec![
                        name,
                        format!("{}/{}", status.ready(), status.services.len()),
                        status.phase().to_string(),
                        age(&status.path),
                    ]);
                }
                table(out, "", &rows)
            }
            LifecycleCommand::Describe { branch, json } => {
                let status = self
                    .statuses()
                    .into_iter()
                    .find(|status| &status.branch == branch)
                    .ok_or_else(|| format!("no workspace {branch}"))?;
                if *json {
                    return say(out, &pretty(&status.json())?);
                }
                say(out, &format!("Workspace  {}", status.branch))?;
                say(
                    out,
                    &format!(
                        "  phase  {} ({}/{} services up)",
                        status.phase(),
                        status.ready(),
                        status.services.len()
                    ),
                )?;
                say(out, &format!("  path   {}", status.path.display()))?;
                say(out, &format!("  age    {}", age(&status.path)))?;
                if !status.missing_repos.is_empty() {
                    say(
                        out,
                        &format!(
                            "  drift  not checked out here: {} (`pom apply {}` adds them)",
                            status.missing_repos.join(", "),
                            status.branch
                        ),
                    )?;
                }
                if status.services.is_empty() {
                    return Ok(());
                }
                say(out, "\nSERVICES")?;
                let mut rows = vec![vec![
                    "REPO/SERVICE".to_string(),
                    "STATUS".to_string(),
                    "PORT".to_string(),
                ]];
                for service in &status.services {
                    rows.push(vec![
                        format!("{}/{}", service.repo, service.name),
                        if service.up { "up" } else { "stopped" }.to_string(),
                        service
                            .port
                            .map_or_else(|| "-".to_string(), |port| port.to_string()),
                    ]);
                }
                table(out, "  ", &rows)
            }
            LifecycleCommand::Apply { branch, yes } => self.apply(branch.as_deref(), *yes, out),
        }
    }

    fn statuses(&self) -> Vec<WorkspaceStatus> {
        let mut statuses: Vec<WorkspaceStatus> = self
            .project
            .workspaces
            .iter()
            .filter(|workspace| workspace.path.is_dir())
            .map(|workspace| {
                let mut status = WorkspaceStatus {
                    branch: workspace.branch.clone(),
                    is_main: workspace.is_main,
                    path: workspace.path.clone(),
                    services: Vec::new(),
                    missing_repos: Vec::new(),
                };
                for (repo, dir) in &self.config.repos {
                    if !dir.has_worktree_config() {
                        continue;
                    }
                    if !workspace.path.join(repo).is_dir() {
                        status.missing_repos.push(repo.clone());
                        continue;
                    }
                    for name in dir.services.keys() {
                        let target = ServiceTarget {
                            branch: workspace.branch.clone(),
                            is_main: workspace.is_main,
                            repo: repo.clone(),
                            service: name.clone(),
                        };
                        status.services.push(ServiceStatus {
                            repo: repo.clone(),
                            name: name.clone(),
                            up: self.runner.is_running(&target),
                            port: self.runner.port(&self.config, &target),
                        });
                    }
                }
                status
            })
            .collect();
        statuses.sort_by(|a, b| a.branch.cmp(&b.branch));
        statuses
    }

    fn refresh(&self, out: &mut dyn Write) -> Result<(), String> {
        let running = self.runner.running_holders();
        if running.is_empty() {
            return say(
                out,
                &format!("no running services for {}", self.config.session),
            );
        }
        let mut stopped = 0;
        for holder in &running {
            match self.runner.holders().kill_holder(holder) {
                Ok(()) => stopped += 1,
                Err(error) => eprintln!("stop {holder}: {error}"),
            }
        }
        say(
            out,
            &format!(
                "stopped {stopped} service(s) of {}; their ports are free",
                self.config.session
            ),
        )
    }

    fn release(
        &self,
        volumes: bool,
        worktrees: bool,
        yes: bool,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        if (volumes || worktrees) && !yes {
            let what = match (volumes, worktrees) {
                (true, true) => "branch workspaces and docker volumes (database data)",
                (true, false) => "docker volumes (database data)",
                _ => "branch workspaces",
            };
            let question = format!(
                "This removes the {what} of {}. Continue? [y/N] ",
                self.config.session
            );
            if !confirm(out, &question)? {
                return say(out, "cancelled");
            }
        }
        if worktrees {
            let context = pom_workspace::WorkspaceContext {
                config: &self.config,
                runner: &self.runner,
                state: &self.state,
            };
            for workspace in self.project.workspaces.iter().filter(|w| !w.is_main) {
                say(out, &format!("- deleting workspace {}", workspace.branch))?;
                let request = DeleteRequest {
                    branch: workspace.branch.clone(),
                    from_stage: 0,
                };
                let result = crate::workspaces::stream(out, |sink| {
                    pom_workspace::delete(&context, &request, sink)
                })?;
                if let Err(error) = result {
                    say(out, &format!("  {}: {}", workspace.branch, error.message))?;
                }
            }
        }
        say(out, "- stopping services")?;
        self.refresh(out)?;
        let compose = self.runner.compose_file();
        if compose.is_file() {
            say(out, "- removing the shared containers")?;
            let mut args = vec![
                "compose".to_string(),
                "-f".to_string(),
                compose.to_string_lossy().into_owned(),
                "-p".to_string(),
                self.runner.compose_project(),
                "down".to_string(),
            ];
            if volumes {
                args.push("-v".into());
            }
            let status = std::process::Command::new("docker")
                .args(&args)
                .env("PATH", pom_services::tool_path())
                .status()
                .map_err(|error| format!("docker: {error}"))?;
            if !status.success() {
                say(out, "  docker compose down failed")?;
            }
        }
        say(
            out,
            &format!("released the local footprint of {}", self.config.session),
        )
    }

    fn apply(&self, branch: Option<&str>, yes: bool, out: &mut dyn Write) -> Result<(), String> {
        let statuses = self.statuses();
        if let Some(branch) = branch {
            if !statuses.iter().any(|status| status.branch == branch) {
                return Err(format!("no workspace {branch}"));
            }
        }
        let drifted: Vec<&WorkspaceStatus> = statuses
            .iter()
            .filter(|status| branch.is_none_or(|branch| status.branch == branch))
            .filter(|status| !status.is_main && !status.missing_repos.is_empty())
            .collect();
        if drifted.is_empty() {
            return say(out, "every workspace matches the config; nothing to apply");
        }
        say(out, "plan (adds only, never deletes):")?;
        for status in &drifted {
            say(
                out,
                &format!("  {}: + {}", status.branch, status.missing_repos.join(", ")),
            )?;
        }
        if !yes {
            return say(out, "\ndry run: re-run with --yes to check them out");
        }
        let context = pom_workspace::WorkspaceContext {
            config: &self.config,
            runner: &self.runner,
            state: &self.state,
        };
        for status in drifted {
            say(out, &format!("\n>>> {}", status.branch))?;
            let request = CreateRequest {
                branch: status.branch.clone(),
                repos: status.missing_repos.clone(),
                environment: String::new(),
                skip_seed: false,
                from_stage: 0,
            };
            crate::workspaces::stream(out, |sink| pom_workspace::create(&context, &request, sink))?
                .map_err(|error| format!("{}: {}", status.branch, error.message))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_arguments() {
        assert_eq!(
            parse("release", &["--disk", "--yes"]),
            Ok(LifecycleCommand::Release {
                volumes: true,
                worktrees: false,
                yes: true
            })
        );
        assert_eq!(
            parse("get", &["ws", "-o", "json"]),
            Ok(LifecycleCommand::Get { json: true })
        );
        assert_eq!(
            parse("describe", &["workspace", "feat"]),
            Ok(LifecycleCommand::Describe {
                branch: "feat".into(),
                json: false
            })
        );
        assert_eq!(
            parse("apply", &["feat", "--yes"]),
            Ok(LifecycleCommand::Apply {
                branch: Some("feat".into()),
                yes: true
            })
        );
        assert!(parse("get", &["pods"]).is_err());
    }
}
