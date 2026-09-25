//! `pom db create|drop|reset|clean`: a workspace's databases in the shared Postgres.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::args::Args;
use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DbCommand {
    Create(Option<String>),
    Drop(Option<String>),
    Reset(Option<String>),
    Clean { dry: bool, yes: bool },
}

pub(crate) fn parse(words: &[&str]) -> Result<DbCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("db needs a subcommand (create, drop, reset, clean)")?;
    let args = Args::parse(rest, &[])?;
    match *verb {
        "create" | "drop" | "reset" => {
            args.allow(&[])?;
            args.at_most(1, &format!("db {verb}"))?;
            let branch = args.positional.first().cloned();
            Ok(match *verb {
                "create" => DbCommand::Create(branch),
                "drop" => DbCommand::Drop(branch),
                _ => DbCommand::Reset(branch),
            })
        }
        "clean" => {
            args.allow(&["--dry-run", "--yes"])?;
            args.at_most(0, "db clean")?;
            Ok(DbCommand::Clean {
                dry: args.has("--dry-run"),
                yes: args.has("--yes"),
            })
        }
        other => Err(format!("unknown db subcommand {other}")),
    }
}

/// Postgres cuts identifiers at 63 bytes, so that is the name it lists.
const PG_NAME_LIMIT: usize = 63;

impl Session {
    pub(crate) fn db_command(
        &self,
        command: &DbCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let branch = match command {
            DbCommand::Create(branch) | DbCommand::Drop(branch) | DbCommand::Reset(branch) => {
                branch.clone().unwrap_or_else(|| self.branch.clone())
            }
            DbCommand::Clean { dry, yes } => return self.clean_databases(*dry, *yes, out),
        };
        let names = self.workspace_databases(&branch)?;
        if names.is_empty() {
            return say(out, &format!("no databases for workspace {branch}"));
        }
        let verb = match command {
            DbCommand::Create(_) => "creating",
            DbCommand::Drop(_) => "dropping",
            _ => "resetting",
        };
        say(out, &format!("{verb} databases of workspace {branch}:"))?;
        for name in &names {
            say(out, &format!("  {name}"))?;
        }
        self.runner
            .ensure_shared(&self.config)
            .map_err(|error| format!("shared services: {error}"))?;
        if matches!(command, DbCommand::Drop(_) | DbCommand::Reset(_)) {
            self.runner
                .drop_databases(&self.config, &names)
                .map_err(|error| error.to_string())?;
        }
        if matches!(command, DbCommand::Create(_) | DbCommand::Reset(_)) {
            self.runner
                .create_databases(&self.config, &names)
                .map_err(|error| error.to_string())?;
        }
        if matches!(command, DbCommand::Reset(_)) {
            say(out, "done; run the migrations to restore the schema")
        } else {
            say(out, "done")
        }
    }

    /// Where a workspace lives; main may still be the project root itself.
    pub(crate) fn workspace_folder(&self, branch: &str) -> Result<PathBuf, String> {
        if let Some(workspace) = self
            .project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
        {
            return Ok(workspace.path.clone());
        }
        let is_main = branch == self.config.global_default_branch();
        let folder = pom_layout::workspace_root(&self.project.root, branch, is_main);
        if folder.is_dir() && (is_main || folder != self.project.root) {
            return Ok(folder);
        }
        Err(format!("no workspace {branch}"))
    }

    /// Databases of the repos checked out in the workspace.
    fn workspace_databases(&self, branch: &str) -> Result<Vec<String>, String> {
        let folder = self.workspace_folder(branch)?;
        Ok(pom_services::database_names_where(
            &self.config,
            branch,
            |repo| folder.join(repo).is_dir(),
        ))
    }

    fn clean_databases(&self, dry: bool, yes: bool, out: &mut dyn Write) -> Result<(), String> {
        let existing = self
            .runner
            .list_databases(&self.config)
            .map_err(|error| format!("list databases: {error}"))?;
        let expected: BTreeSet<String> = self
            .project
            .workspaces
            .iter()
            .flat_map(|workspace| {
                self.workspace_databases(&workspace.branch)
                    .unwrap_or_default()
            })
            .map(|name| truncate_identifier(&name))
            .collect();
        let prefix = format!("{}_", self.config.session);
        let orphans: Vec<String> = existing
            .into_iter()
            .filter(|name| name.starts_with(&prefix) && !expected.contains(name))
            .collect();
        if orphans.is_empty() {
            return say(
                out,
                &format!("no orphan databases ({} expected)", expected.len()),
            );
        }
        say(out, &format!("orphan databases ({}):", orphans.len()))?;
        for name in &orphans {
            say(out, &format!("  {name}"))?;
        }
        if dry {
            return say(out, "dry run: nothing dropped");
        }
        if !yes && !confirm(out, &format!("drop {} database(s)? [y/N] ", orphans.len()))? {
            return say(out, "cancelled");
        }
        self.runner
            .drop_databases(&self.config, &orphans)
            .map_err(|error| error.to_string())?;
        say(out, &format!("dropped {}", orphans.len()))
    }
}

fn truncate_identifier(name: &str) -> String {
    let mut end = name.len().min(PG_NAME_LIMIT);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// A yes/no question on stdin; anything but y/yes is no.
pub(crate) fn confirm(out: &mut dyn Write, question: &str) -> Result<bool, String> {
    write!(out, "{question}").map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_subcommands() {
        assert_eq!(parse(&["create"]), Ok(DbCommand::Create(None)));
        assert_eq!(
            parse(&["reset", "feat"]),
            Ok(DbCommand::Reset(Some("feat".into())))
        );
        assert_eq!(
            parse(&["clean", "--dry-run"]),
            Ok(DbCommand::Clean {
                dry: true,
                yes: false
            })
        );
        assert!(parse(&["drop", "a", "b"]).is_err());
        assert_eq!(truncate_identifier(&"x".repeat(70)).len(), 63);
    }
}
