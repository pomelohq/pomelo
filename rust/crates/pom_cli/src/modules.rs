use std::io::Write;

use module_store::{format_size, Options, Store};
use pom_paths::StateDir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModulesCommand {
    List,
    Prune,
    Clear,
}

pub(crate) fn parse(rest: &[&str]) -> Result<ModulesCommand, String> {
    match rest {
        [] | ["list"] => Ok(ModulesCommand::List),
        ["prune"] => Ok(ModulesCommand::Prune),
        ["clear"] => Ok(ModulesCommand::Clear),
        _ => Err("usage: pom modules [list|prune|clear]".into()),
    }
}

fn say(out: &mut dyn Write, line: &str) -> Result<(), String> {
    writeln!(out, "{line}").map_err(|error| error.to_string())
}

pub(crate) fn execute(
    command: ModulesCommand,
    state: &StateDir,
    out: &mut dyn Write,
) -> Result<(), String> {
    let store = Store::new(state);
    let options = Options::from_settings_file();
    match command {
        ModulesCommand::List => {
            let entries = store.list().map_err(|error| error.to_string())?;
            if entries.is_empty() {
                return say(out, "the node_modules store is empty");
            }
            let total: u64 = entries.iter().map(|entry| entry.size).sum();
            say(
                out,
                &format!(
                    "{} copies, {} (sizes count files shared with clones in full)  {}",
                    entries.len(),
                    format_size(total),
                    store.root().display()
                ),
            )?;
            for entry in &entries {
                let manager = if entry.manager.is_empty() {
                    "-"
                } else {
                    entry.manager.as_str()
                };
                say(
                    out,
                    &format!(
                        "  {:<24} {:<5} {:>9}  {}  {} workspaces  {}",
                        entry.repo,
                        manager,
                        format_size(entry.size),
                        entry.method.label(),
                        entry.live_workspaces().len(),
                        entry.key
                    ),
                )?;
            }
            Ok(())
        }
        ModulesCommand::Prune | ModulesCommand::Clear => {
            let pruned = if command == ModulesCommand::Prune {
                store.prune(&options)
            } else {
                store.clear()
            }
            .map_err(|error| error.to_string())?;
            say(
                out,
                &format!(
                    "removed {} copies, freed {}",
                    pruned.removed,
                    format_size(pruned.freed)
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_subcommands() {
        assert_eq!(parse(&[]), Ok(ModulesCommand::List));
        assert_eq!(parse(&["prune"]), Ok(ModulesCommand::Prune));
        assert_eq!(parse(&["clear"]), Ok(ModulesCommand::Clear));
        assert!(parse(&["nuke"]).is_err());
    }

    #[test]
    fn lists_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        execute(ModulesCommand::List, &StateDir::new(dir.path()), &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "the node_modules store is empty\n"
        );
    }
}
