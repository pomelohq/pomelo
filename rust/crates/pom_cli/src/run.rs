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
        let repo = match &request.repo {
            Some(name) => self
                .config
                .repos
                .iter()
                .find(|(repo, dir)| repo.as_str() == name || dir.alias == *name)
                .map(|(repo, _)| repo.clone())
                .ok_or_else(|| format!("no repo {name}"))?,
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
        let named = (!request.command.contains(char::is_whitespace))
            .then(|| {
                dir.effective_shortcuts()
                    .into_iter()
                    .find(|shortcut| shortcut.key.eq_ignore_ascii_case(&request.command))
            })
            .flatten();
        let command = match (test_command, named) {
            (Some(command), _) => command,
            (None, Some(shortcut)) => shortcut.cmd,
            (None, None) if runs_tests && request.command.eq_ignore_ascii_case("test") => {
                return Err(format!(
                    "{repo} has no test command: set test.command or commands.test"
                ))
            }
            (None, None) => request.command.clone(),
        };
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
            Some(code) => Err(format!("the command exited with status {code}")),
            None => Err("the command was killed by a signal".into()),
        }
    }

    pub(crate) fn list_commands(&self, out: &mut dyn Write) -> Result<(), String> {
        let mut any = false;
        for (repo, dir) in &self.config.repos {
            let shortcuts = dir.effective_shortcuts();
            if shortcuts.is_empty() {
                continue;
            }
            any = true;
            let alias = if dir.alias.is_empty() {
                repo
            } else {
                &dir.alias
            };
            say(out, &format!("{repo} ({alias})"))?;
            let rows: Vec<Vec<String>> = shortcuts
                .iter()
                .map(|shortcut| {
                    let name = if shortcut.key.is_empty() {
                        "-"
                    } else {
                        &shortcut.key
                    };
                    vec![
                        name.to_string(),
                        shortcut.cmd.clone(),
                        shortcut.desc.clone(),
                    ]
                })
                .collect();
            table(out, "  ", &rows)?;
        }
        if !any {
            say(
                out,
                "no commands in the config (add them under lifecycle.commands)",
            )?;
        }
        say(out, "\nrun one with: pom run <name> [repo]")
    }
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
