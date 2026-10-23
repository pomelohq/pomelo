//! `pom run` and `pom commands`: a repo's commands, run in its worktree with the workspace's env.

use std::io::Write;

use crate::args::Args;
use pom_services::{RunEnv, RunProfile};

use crate::config::{dash, table};
use crate::{say, Session};

const MASKED: &str = "********";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RunCommand {
    /// A command name from the config, or a shell command.
    pub command: String,
    pub repo: Option<String>,
    /// None picks the test profile for the repo's test command and dev otherwise.
    pub profile: Option<RunProfile>,
    pub explain_env: bool,
    pub json: bool,
}

const USAGE: &str =
    "usage: pom run [--profile test|dev] [--explain-env [--json]] <command name | \"shell command\"> [repo]";

pub(crate) fn parse(words: &[&str]) -> Result<RunCommand, String> {
    let args = Args::parse(words, &["--profile", "-o", "--output"])?;
    args.allow(&["--profile", "--explain-env", "--json", "-o", "--output"])?;
    let profile = match args.value(&["--profile"]).as_deref() {
        None => None,
        Some("test") => Some(RunProfile::Test),
        Some("dev") => Some(RunProfile::Dev),
        Some(other) => return Err(format!("unknown profile {other} (test or dev)")),
    };
    let explain_env = args.has("--explain-env");
    let json = args.has("--json") || args.json()?;
    if json && !explain_env {
        return Err("--json goes with --explain-env".into());
    }
    let (command, repo) = match args.positional.as_slice() {
        [command] => (command.clone(), None),
        [command, repo] => (command.clone(), Some(repo.clone())),
        _ => return Err(USAGE.into()),
    };
    Ok(RunCommand {
        command,
        repo,
        profile,
        explain_env,
        json,
    })
}

impl Session {
    pub(crate) fn run_command(
        &self,
        request: &RunCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let looks_named = !request.command.contains(char::is_whitespace);
        let repo = match &request.repo {
            Some(name) => self
                .config
                .repos
                .iter()
                .find(|(repo, dir)| repo.as_str() == name || dir.alias == *name)
                .map(|(repo, _)| repo.clone())
                .ok_or_else(|| format!("no repo {name}"))?,
            None if looks_named => self.repo_defining(&request.command)?,
            None => self
                .config
                .repos
                .keys()
                .next()
                .cloned()
                .ok_or("the config has no repos")?,
        };
        let dir = &self.config.repos[&repo];
        let folder = self.workspace_folder(&self.branch)?;
        let worktree = folder.join(&repo);
        if !worktree.is_dir() {
            return Err(format!("{} is not checked out", worktree.display()));
        }
        let runs_tests = pom_services::runs_tests(dir.test.as_ref(), &request.command);
        let profile = request.profile.unwrap_or(if runs_tests {
            RunProfile::Test
        } else {
            RunProfile::Dev
        });
        let test_command = dir
            .test
            .as_ref()
            .map(|test| test.command.clone())
            .filter(|command| runs_tests && !command.is_empty());
        let named = looks_named
            .then(|| dir.named_command(&request.command))
            .flatten();
        let command = match (test_command, named) {
            (Some(command), _) => command,
            (None, Some(named)) => named.command,
            (None, None) if runs_tests && request.command.eq_ignore_ascii_case("test") => {
                return Err(format!(
                    "{repo} has no test command: set test.command or commands.test"
                ))
            }
            (None, None) => request.command.clone(),
        };
        let unknown_name = looks_named && command == request.command;
        let env = self.runner.workspace_env(&self.config, &self.branch);
        let run_env = env.run_env(&repo, profile, &worktree);
        if request.explain_env {
            return explain_env(out, &self.branch, &repo, &command, &run_env, request.json);
        }
        if let Err(error) = env.write_env_files() {
            eprintln!("env files: {error}");
        }
        let exports: String = run_env
            .pairs()
            .into_iter()
            .map(|(key, value)| format!("export {key}={}; ", pom_services::shell_quote(&value)))
            .collect();
        let pre_start = if dir.pre_start.trim().is_empty() {
            String::new()
        } else {
            format!("{} && ", dir.pre_start)
        };
        let script = format!(
            "export PATH={path}; {exports}cd {cwd} && {pre_start}{command}",
            path = pom_services::shell_quote(pom_services::tool_path()),
            cwd = pom_services::shell_quote(&worktree.to_string_lossy()),
        );
        let status = pom_shell::Shell::user()
            .login_command(&script)
            .current_dir(&worktree)
            .status()
            .map_err(|error| format!("run: {error}"))?;
        match status.code() {
            Some(0) => Ok(()),
            Some(127) if unknown_name => Err(format!(
                "{} is not a command of {repo} (pom commands lists them) and the shell found no program by that name",
                request.command
            )),
            Some(code) => Err(format!("the command exited with status {code}")),
            None => Err("the command was killed by a signal".into()),
        }
    }

    /// The repo that declares command `name`; the first repo when none does, so a bare program still runs.
    fn repo_defining(&self, name: &str) -> Result<String, String> {
        let defining: Vec<&String> = self
            .config
            .repos
            .iter()
            .filter(|(_, dir)| dir.named_command(name).is_some())
            .map(|(repo, _)| repo)
            .collect();
        match defining.as_slice() {
            [only] => Ok((*only).clone()),
            [] => self
                .config
                .repos
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| "the config has no repos".to_string()),
            several => Err(format!(
                "{name} is a command of {}: name the repo (pom run {name} <repo>)",
                several
                    .iter()
                    .map(|repo| repo.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub(crate) fn list_commands(&self, json: bool, out: &mut dyn Write) -> Result<(), String> {
        let listed = command_list(&self.config);
        if json {
            let commands: Vec<serde_json::Value> = listed
                .iter()
                .map(|(repo, named)| {
                    serde_json::json!({
                        "name": named.name,
                        "repo": repo,
                        "command": named.command,
                        "description": named.description,
                        "source": named.source,
                    })
                })
                .collect();
            let document = serde_json::json!({
                "schema": COMMANDS_SCHEMA,
                "commands": commands,
            });
            return say(out, &document.to_string());
        }
        if listed.is_empty() {
            say(
                out,
                "no commands in the config (add them under lifecycle.commands)",
            )?;
        }
        for (repo, dir) in &self.config.repos {
            let rows: Vec<Vec<String>> = listed
                .iter()
                .filter(|(owner, _)| owner == repo)
                .map(|(_, named)| {
                    vec![
                        named.name.clone(),
                        named.command.clone(),
                        named.description.clone(),
                    ]
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let alias = if dir.alias.is_empty() {
                repo
            } else {
                &dir.alias
            };
            say(out, &format!("{repo} ({alias})"))?;
            table(out, "  ", &rows)?;
        }
        say(out, "\nrun one with: pom run <name> [repo]")
    }
}

pub(crate) const COMMANDS_SCHEMA: &str = "pom.commands/v1";

/// Every repo's named commands, in config order.
pub(crate) fn command_list(config: &pom_config::Config) -> Vec<(String, pom_config::NamedCommand)> {
    config
        .repos
        .iter()
        .flat_map(|(repo, dir)| {
            dir.named_commands()
                .into_iter()
                .map(move |named| (repo.clone(), named))
        })
        .collect()
}

/// `pom commands [-o json | --json]`.
pub(crate) fn parse_commands(words: &[&str]) -> Result<bool, String> {
    let args = Args::parse(words, &["-o", "--output"])?;
    args.allow(&["--json", "-o", "--output"])?;
    args.at_most(0, "commands")?;
    Ok(args.has("--json") || args.json()?)
}

/// Each variable's value (secrets masked) and where it came from, without running anything.
fn explain_env(
    out: &mut dyn Write,
    branch: &str,
    repo: &str,
    command: &str,
    run_env: &RunEnv,
    json: bool,
) -> Result<(), String> {
    let shown = |line: &pom_services::RunEnvLine| {
        if line.secret {
            MASKED.to_string()
        } else {
            line.value.clone()
        }
    };
    if json {
        let env: Vec<serde_json::Value> = run_env
            .lines
            .iter()
            .map(|line| {
                serde_json::json!({
                    "key": line.key,
                    "value": shown(line),
                    "source": line.source,
                    "connection": line.connection,
                    "secret": line.secret,
                    "kept_over": line.kept_over,
                })
            })
            .collect();
        let document = serde_json::json!({
            "schema": "pom.run/v1",
            "workspace": branch,
            "repo": repo,
            "command": command,
            "profile": run_env.profile.name(),
            "env_files": run_env.env_files,
            "env": env,
        });
        return say(out, &document.to_string());
    }
    let files = if run_env.env_files.is_empty() {
        "-".to_string()
    } else {
        run_env.env_files.join(", ")
    };
    say(
        out,
        &format!(
            "{repo}: {command}\nprofile {}, env files read: {files}\n",
            run_env.profile.name()
        ),
    )?;
    let rows: Vec<Vec<String>> = run_env
        .lines
        .iter()
        .map(|line| {
            let source = match &line.kept_over {
                Some(file) => format!("{} (kept over {file})", line.source),
                None => line.source.clone(),
            };
            vec![line.key.clone(), dash(&shown(line)), source]
        })
        .collect();
    table(out, "", &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_arguments() {
        assert_eq!(
            parse(&["migrate", "api"]),
            Ok(RunCommand {
                command: "migrate".into(),
                repo: Some("api".into()),
                profile: None,
                explain_env: false,
                json: false,
            })
        );
        let explained =
            parse(&["--explain-env", "--profile", "test", "bin/rspec", "--json"]).expect("explain");
        assert_eq!(explained.profile, Some(RunProfile::Test));
        assert!(explained.explain_env && explained.json);
        assert!(parse(&["--profile", "prod", "x"]).is_err());
        assert!(parse(&["--json", "x"]).is_err());
        assert!(parse(&[]).is_err());
        assert!(parse(&["a", "b", "c"]).is_err());
    }
}
