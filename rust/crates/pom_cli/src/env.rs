//! `pom env ls|get|set|unset`: a repo's environment variables.

use std::io::Write;
use std::path::Path;

use crate::args::Args;
use crate::config::{dash, table};
use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnvCommand {
    List {
        target: String,
        branch: Option<String>,
        env: String,
        show_secrets: bool,
    },
    Get {
        target: String,
        key: String,
        branch: Option<String>,
        env: String,
    },
    Set {
        repo: String,
        pairs: Vec<(String, String)>,
    },
    Unset {
        repo: String,
        keys: Vec<String>,
    },
}

impl EnvCommand {
    pub(crate) fn needs_session(&self) -> bool {
        matches!(self, EnvCommand::List { .. } | EnvCommand::Get { .. })
    }
}

pub(crate) fn parse(words: &[&str]) -> Result<EnvCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("env needs a subcommand (ls, get, set, unset)")?;
    let args = Args::parse(rest, &["--branch", "--env"])?;
    let resolving = || -> Result<(Option<String>, String), String> {
        Ok((
            args.value(&["--branch"]),
            args.value(&["--env"]).unwrap_or_default(),
        ))
    };
    match (*verb, args.positional.as_slice()) {
        ("ls", [target]) => {
            args.allow(&["--branch", "--env", "--show-secrets"])?;
            let (branch, env) = resolving()?;
            Ok(EnvCommand::List {
                target: target.clone(),
                branch,
                env,
                show_secrets: args.has("--show-secrets"),
            })
        }
        ("get", [target, key]) => {
            args.allow(&["--branch", "--env"])?;
            let (branch, env) = resolving()?;
            Ok(EnvCommand::Get {
                target: target.clone(),
                key: key.clone(),
                branch,
                env,
            })
        }
        ("set", [repo, pairs @ ..]) if !pairs.is_empty() => {
            args.allow(&[])?;
            let pairs = pairs
                .iter()
                .map(|pair| {
                    pair.split_once('=')
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .ok_or_else(|| format!("expected KEY=VALUE, got {pair:?}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(EnvCommand::Set {
                repo: repo.clone(),
                pairs,
            })
        }
        ("unset", [repo, keys @ ..]) if !keys.is_empty() => {
            args.allow(&[])?;
            Ok(EnvCommand::Unset {
                repo: repo.clone(),
                keys: keys.to_vec(),
            })
        }
        ("ls", _) => Err("usage: pom env ls <repo[/service]>".into()),
        ("get", _) => Err("usage: pom env get <repo[/service]> <KEY>".into()),
        ("set", _) => Err("usage: pom env set <repo> KEY=VALUE...".into()),
        ("unset", _) => Err("usage: pom env unset <repo> KEY...".into()),
        (other, _) => Err(format!("unknown env subcommand {other}")),
    }
}

/// `set` and `unset` edit files only, so a config that does not load yet can still be fixed with them.
pub(crate) fn edit(
    command: &EnvCommand,
    config_path: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let (repo, set, unset, verb) = match command {
        EnvCommand::Set { repo, pairs } => (repo, pairs.clone(), Vec::new(), "set"),
        EnvCommand::Unset { repo, keys } => (repo, Vec::new(), keys.clone(), "unset"),
        _ => return Err("env ls and get need a project session".into()),
    };
    let repo = repo_name(config_path, repo);
    let file = pom_config::maintain::edit_repo_env(config_path, &repo, &set, &unset)?;
    let keys: Vec<&str> = set
        .iter()
        .map(|(key, _)| key.as_str())
        .chain(unset.iter().map(String::as_str))
        .collect();
    say(
        out,
        &format!("{verb} {} on {repo} in {}", keys.join(", "), file.display()),
    )
}

/// An alias names its repo too.
fn repo_name(config_path: &Path, name: &str) -> String {
    pom_config::Config::load(config_path)
        .ok()
        .and_then(|config| {
            config
                .repos
                .iter()
                .find(|(repo, dir)| repo.as_str() == name || dir.alias == name)
                .map(|(repo, _)| repo.clone())
        })
        .unwrap_or_else(|| name.to_string())
}

impl Session {
    pub(crate) fn env_command(
        &self,
        command: &EnvCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let (target, branch, env) = match command {
            EnvCommand::List {
                target,
                branch,
                env,
                ..
            }
            | EnvCommand::Get {
                target,
                branch,
                env,
                ..
            } => (target, branch, env),
            _ => return edit(command, &self.project.config_path, out),
        };
        self.config.validate_environment(env)?;
        let (repo, service) = self.repo_service(target)?;
        let branch = branch.clone().unwrap_or_else(|| self.branch.clone());
        let explained = self
            .runner
            .workspace_env(&self.config, &branch)
            .explain_service(&repo, &service, env)
            .ok_or_else(|| format!("no service {repo}/{service}"))?;
        match command {
            EnvCommand::Get { key, .. } => {
                let line = explained
                    .env
                    .iter()
                    .find(|line| &line.key == key)
                    .ok_or_else(|| format!("no env var {key} for {repo}/{service}"))?;
                say(out, &line.value)
            }
            EnvCommand::List { show_secrets, .. } => {
                let rows: Vec<Vec<String>> = explained
                    .env
                    .iter()
                    .map(|line| {
                        let value = if line.secret && !show_secrets {
                            "********".to_string()
                        } else {
                            dash(&line.value)
                        };
                        vec![line.key.clone(), value]
                    })
                    .collect();
                table(out, "", &rows)
            }
            _ => Ok(()),
        }
    }

    /// `repo/service`, or a repo (name or alias) standing for its first service.
    fn repo_service(&self, target: &str) -> Result<(String, String), String> {
        if target.contains('/') {
            return self.config.find_service_entry(target);
        }
        let (repo, dir) = self
            .config
            .repos
            .iter()
            .find(|(repo, dir)| repo.as_str() == target || dir.alias == target)
            .ok_or_else(|| format!("no repo {target}"))?;
        let service = dir
            .services
            .keys()
            .next()
            .ok_or_else(|| format!("{repo} has no services"))?;
        Ok((repo.clone(), service.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_subcommands() {
        assert_eq!(
            parse(&["set", "api", "A=1", "B=x=y"]),
            Ok(EnvCommand::Set {
                repo: "api".into(),
                pairs: vec![("A".into(), "1".into()), ("B".into(), "x=y".into())],
            })
        );
        assert_eq!(
            parse(&["get", "api/web", "PORT", "--branch", "feat"]),
            Ok(EnvCommand::Get {
                target: "api/web".into(),
                key: "PORT".into(),
                branch: Some("feat".into()),
                env: String::new(),
            })
        );
        assert!(parse(&["set", "api", "A"]).is_err());
        assert!(parse(&["unset", "api"]).is_err());
        assert!(parse(&["ls"]).is_err());
    }
}
