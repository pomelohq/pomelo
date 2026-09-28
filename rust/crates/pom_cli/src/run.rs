//! `pom run` and `pom commands`: a repo's commands, run in its worktree with the workspace's env.

use std::io::Write;

use crate::args::Args;
use crate::config::table;
use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RunCommand {
    /// A command name from the config, or a shell command.
    pub command: String,
    pub repo: Option<String>,
}

pub(crate) fn parse(words: &[&str]) -> Result<RunCommand, String> {
    let args = Args::parse(words, &[])?;
    args.allow(&[])?;
    match args.positional.as_slice() {
        [command] => Ok(RunCommand {
            command: command.clone(),
            repo: None,
        }),
        [command, repo] => Ok(RunCommand {
            command: command.clone(),
            repo: Some(repo.clone()),
        }),
        _ => Err("usage: pom run <command name | \"shell command\"> [repo]".into()),
    }
}

impl Session {
    pub(crate) fn run_command(&self, request: &RunCommand) -> Result<(), String> {
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
        let named = (!request.command.contains(char::is_whitespace))
            .then(|| {
                dir.effective_shortcuts()
                    .into_iter()
                    .find(|shortcut| shortcut.key.eq_ignore_ascii_case(&request.command))
            })
            .flatten();
        let command = named.map_or_else(|| request.command.clone(), |shortcut| shortcut.cmd);
        let env = self.runner.workspace_env(&self.config, &self.branch);
        if let Err(error) = env.write_env_files() {
            eprintln!("env files: {error}");
        }
        let exports: String = env
            .repo_env(&repo)
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
        let status = std::process::Command::new("zsh")
            .args(["-lc", &script])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_arguments() {
        assert_eq!(
            parse(&["migrate", "api"]),
            Ok(RunCommand {
                command: "migrate".into(),
                repo: Some("api".into())
            })
        );
        assert!(parse(&[]).is_err());
        assert!(parse(&["a", "b", "c"]).is_err());
    }
}
