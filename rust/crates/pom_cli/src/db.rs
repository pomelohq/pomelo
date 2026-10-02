//! `pom db create|drop|reset|clean|snapshot|restore|snapshots`: a workspace's databases in the shared Postgres.

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
    Snapshot(SnapshotArgs),
    Restore(SnapshotArgs),
    Snapshots(SnapshotArgs),
    SnapshotDrop(SnapshotArgs),
    Baseline(SnapshotArgs),
    Reseed(SnapshotArgs),
    Mark(crate::marks::MarkArgs),
    Stats(crate::marks::StatsArgs),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotArgs {
    pub name: String,
    pub branch: Option<String>,
    pub replace: bool,
    pub main: bool,
    pub no_restart: bool,
    pub json: bool,
    /// For reseed: one of the workspace's own snapshots instead of main's baseline.
    pub from_snapshot: Option<String>,
}

const SNAPSHOT_VALUED: &[&str] = &[
    "-w",
    "--workspace",
    "-o",
    "--output",
    "--from",
    "--snapshot",
];

fn snapshot_args(rest: &[&str], known: &[&str], what: &str) -> Result<SnapshotArgs, String> {
    let args = Args::parse(rest, SNAPSHOT_VALUED)?;
    let mut allowed: Vec<&str> = vec!["-w", "--workspace", "-o", "--output"];
    allowed.extend_from_slice(known);
    args.allow(&allowed)?;
    args.at_most(1, what)?;
    let from_snapshot = match (args.value(&["--from"]), args.value(&["--snapshot"])) {
        (Some(_), Some(_)) => return Err(format!("{what}: pass --from or --snapshot, not both")),
        (Some(from), None) if from == pom_services::MAIN_BASELINE => None,
        (Some(from), None) => {
            return Err(format!(
                "{what} --from takes {} (or pass --snapshot <name>), not {from}",
                pom_services::MAIN_BASELINE
            ))
        }
        (None, snapshot) => snapshot,
    };
    Ok(SnapshotArgs {
        name: args.positional.first().cloned().unwrap_or_default(),
        branch: args.value(&["-w", "--workspace"]),
        replace: args.has("--replace"),
        main: args.has("--main"),
        no_restart: args.has("--no-restart"),
        json: args.json()?,
        from_snapshot,
    })
}

fn named(args: SnapshotArgs, what: &str) -> Result<SnapshotArgs, String> {
    if args.name.is_empty() {
        return Err(format!("{what} needs a snapshot name"));
    }
    Ok(args)
}

pub(crate) fn parse(words: &[&str]) -> Result<DbCommand, String> {
    let (verb, rest) = words.split_first().ok_or(
        "db needs a subcommand (create, drop, reset, clean, snapshot, restore, snapshots)",
    )?;
    match *verb {
        "snapshot" if rest.first() == Some(&"drop") => Ok(DbCommand::SnapshotDrop(named(
            snapshot_args(&rest[1..], &[], "db snapshot drop")?,
            "db snapshot drop",
        )?)),
        "snapshot" => Ok(DbCommand::Snapshot(named(
            snapshot_args(rest, &["--replace"], "db snapshot")?,
            "db snapshot",
        )?)),
        "restore" => Ok(DbCommand::Restore(named(
            snapshot_args(rest, &["--main", "--no-restart"], "db restore")?,
            "db restore",
        )?)),
        "snapshots" => Ok(DbCommand::Snapshots(snapshot_args(
            rest,
            &[],
            "db snapshots",
        )?)),
        "mark" => Ok(DbCommand::Mark(crate::marks::parse_mark(rest)?)),
        "stats" => Ok(DbCommand::Stats(crate::marks::parse_stats(rest)?)),
        "baseline" => {
            let args = snapshot_args(rest, &[], "db baseline")?;
            if !args.name.is_empty() {
                return Err("db baseline takes no arguments".into());
            }
            Ok(DbCommand::Baseline(args))
        }
        "reseed" => {
            let args = snapshot_args(rest, &["--from", "--snapshot", "--main"], "db reseed")?;
            if !args.name.is_empty() {
                return Err("db reseed takes no arguments".into());
            }
            Ok(DbCommand::Reseed(args))
        }
        _ => parse_basic(verb, rest),
    }
}

fn parse_basic(verb: &str, rest: &[&str]) -> Result<DbCommand, String> {
    let args = Args::parse(rest, &[])?;
    match verb {
        "create" | "drop" | "reset" => {
            args.allow(&[])?;
            args.at_most(1, &format!("db {verb}"))?;
            let branch = args.positional.first().cloned();
            Ok(match verb {
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
            DbCommand::Snapshot(args)
            | DbCommand::Restore(args)
            | DbCommand::Snapshots(args)
            | DbCommand::SnapshotDrop(args) => return self.snapshot_command(command, args, out),
            DbCommand::Baseline(args) | DbCommand::Reseed(args) => {
                return self.baseline_command(command, args, out)
            }
            DbCommand::Mark(args) => return self.db_mark(args, out),
            DbCommand::Stats(args) => return self.db_stats(args, out),
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
        let mut orphans: Vec<String> = existing
            .into_iter()
            .filter(|name| name.starts_with(&prefix) && !expected.contains(name))
            .collect();
        let kept_snapshots: BTreeSet<String> = self
            .project
            .workspaces
            .iter()
            .flat_map(|workspace| {
                pom_services::SnapshotIndex::load(&workspace.path)
                    .snapshots
                    .into_values()
                    .flat_map(|entry| entry.databases)
                    .map(|saved| saved.snapshot_db)
            })
            .collect();
        let orphan_snapshots: Vec<String> = self
            .runner
            .snapshot_databases_present(&self.config)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| name.starts_with(&prefix) && !kept_snapshots.contains(name))
            .collect();
        orphans.extend(orphan_snapshots.iter().cloned());
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
        let plain: Vec<String> = orphans
            .iter()
            .filter(|name| !orphan_snapshots.contains(name))
            .cloned()
            .collect();
        self.runner
            .drop_databases(&self.config, &plain)
            .map_err(|error| error.to_string())?;
        self.runner
            .drop_snapshot_databases(&self.config, &orphan_snapshots)?;
        say(out, &format!("dropped {}", orphans.len()))
    }
}

impl Session {
    fn snapshot_command(
        &self,
        command: &DbCommand,
        args: &SnapshotArgs,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let branch = args.branch.clone().unwrap_or_else(|| self.branch.clone());
        let folder = self.workspace_folder(&branch)?;
        let is_main = branch == self.config.global_default_branch();
        if let DbCommand::Snapshots(_) = command {
            return self.list_snapshots(&branch, &folder, args.json, out);
        }
        if matches!(command, DbCommand::Restore(_)) && is_main && !args.main {
            return Err(
                "restoring main replaces the data every new workspace starts from; pass --main to do it anyway"
                    .into(),
            );
        }
        let names = pom_services::owned_database_names(&self.config, &branch, |repo| {
            folder.join(repo).is_dir()
        });
        if names.is_empty() && !matches!(command, DbCommand::SnapshotDrop(_)) {
            return Err(format!("workspace {branch} has no databases"));
        }
        self.runner
            .ensure_shared(&self.config)
            .map_err(|error| format!("shared services: {error}"))?;
        let report = match command {
            DbCommand::Snapshot(_) => self.runner.snapshot_workspace(
                &self.config,
                &branch,
                &folder,
                &names,
                &args.name,
                args.replace,
            )?,
            DbCommand::Restore(_) => self.runner.restore_workspace(
                &self.config,
                &branch,
                is_main,
                &folder,
                &names,
                &args.name,
                !args.no_restart,
            )?,
            _ => self
                .runner
                .drop_workspace_snapshot(&self.config, &branch, &folder, &args.name)?,
        };
        print_report(&report, args.json, out)?;
        if report.ok() {
            Ok(())
        } else {
            Err(format!(
                "{} {} failed for some databases",
                report.action, report.snapshot
            ))
        }
    }

    fn list_snapshots(
        &self,
        branch: &str,
        folder: &std::path::Path,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let index = pom_services::SnapshotIndex::load(folder);
        let total: u64 = index
            .snapshots
            .values()
            .flat_map(|entry| &entry.databases)
            .map(|saved| saved.bytes)
            .sum();
        if json {
            let snapshots: Vec<serde_json::Value> = index
                .snapshots
                .iter()
                .map(|(name, entry)| {
                    serde_json::json!({
                        "name": name,
                        "created_ms": entry.created_ms,
                        "bytes": entry.databases.iter().map(|saved| saved.bytes).sum::<u64>(),
                        "databases": entry.databases,
                    })
                })
                .collect();
            let document = serde_json::json!({
                "schema": "pom.db/v1",
                "workspace": branch,
                "snapshots": snapshots,
                "total_bytes": total,
            });
            return say(out, &document.to_string());
        }
        if index.snapshots.is_empty() {
            return say(out, &format!("workspace {branch} has no snapshots"));
        }
        for (name, entry) in &index.snapshots {
            let bytes: u64 = entry.databases.iter().map(|saved| saved.bytes).sum();
            say(
                out,
                &format!(
                    "{name}  {} database(s)  {}",
                    entry.databases.len(),
                    megabytes(bytes)
                ),
            )?;
        }
        say(out, &format!("total {}", megabytes(total)))
    }
}

impl Session {
    fn baseline_command(
        &self,
        command: &DbCommand,
        args: &SnapshotArgs,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let branch = args.branch.clone().unwrap_or_else(|| self.branch.clone());
        let folder = self.workspace_folder(&branch)?;
        let is_main = branch == self.config.global_default_branch();
        if matches!(command, DbCommand::Reseed(_)) && is_main && !args.main {
            return Err(
                "reseeding main replaces the data every new workspace starts from; pass --main to do it anyway"
                    .into(),
            );
        }
        if matches!(command, DbCommand::Baseline(_)) && is_main {
            return Err("main's baseline is main__baseline: run `pom prepare-main`".into());
        }
        self.runner
            .ensure_shared(&self.config)
            .map_err(|error| format!("shared services: {error}"))?;
        let context = self.context();
        let report = match command {
            DbCommand::Baseline(_) => pom_workspace::rebaseline(&context, &branch, &folder)?,
            _ => {
                let source = match &args.from_snapshot {
                    Some(name) => pom_workspace::ReseedSource::Snapshot(name.clone()),
                    None => pom_workspace::ReseedSource::MainBaseline,
                };
                pom_workspace::reseed(&context, &branch, is_main, &folder, &source)?
            }
        };
        if args.json {
            let text = serde_json::to_string(&report).map_err(|error| error.to_string())?;
            say(out, &text)?;
        } else {
            for outcome in &report.databases {
                let status = match &outcome.error {
                    Some(error) => format!("failed: {error}"),
                    None => format!("copied in {} ms", outcome.ms),
                };
                say(out, &format!("  {}  {status}", outcome.db))?;
            }
            for migration in &report.migrations {
                let status = match &migration.error {
                    Some(error) => format!("migrations failed: {error}"),
                    None => "migrated".to_string(),
                };
                say(out, &format!("  {}  {status}", migration.repo))?;
            }
            if let Some(baseline) = &report.baseline {
                print_report(baseline, false, out)?;
            }
            for service in &report.services_restarted {
                say(out, &format!("  restarted {service}"))?;
            }
            for warning in &report.warnings {
                say(out, &format!("warning: {warning}"))?;
            }
            say(
                out,
                &format!(
                    "{} of workspace {}: {} ms",
                    report.action, report.workspace, report.total_ms
                ),
            )?;
        }
        if report.ok() {
            Ok(())
        } else {
            Err(format!("{} failed for some databases", report.action))
        }
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn print_report(
    report: &pom_services::SnapshotReport,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    if json {
        let text = serde_json::to_string(report).map_err(|error| error.to_string())?;
        return say(out, &text);
    }
    for outcome in &report.databases {
        let status = match &outcome.error {
            Some(error) => format!("failed: {error}"),
            None => format!("{}  {} ms", megabytes(outcome.bytes), outcome.ms),
        };
        say(out, &format!("  {}  {status}", outcome.db))?;
    }
    for service in &report.services_restarted {
        say(out, &format!("  restarted {service}"))?;
    }
    for warning in &report.warnings {
        say(out, &format!("warning: {warning}"))?;
    }
    say(
        out,
        &format!(
            "{} {} of workspace {}: {} ms",
            report.action, report.snapshot, report.workspace, report.total_ms
        ),
    )
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
        let DbCommand::Snapshot(args) = parse(&[
            "snapshot",
            "before-checkout",
            "-w",
            "feat-login",
            "--replace",
            "-o",
            "json",
        ])
        .expect("snapshot") else {
            panic!("not a snapshot");
        };
        assert_eq!(
            (
                args.name.as_str(),
                args.branch.as_deref(),
                args.replace,
                args.json
            ),
            ("before-checkout", Some("feat-login"), true, true)
        );
        assert!(matches!(
            parse(&["snapshot", "drop", "s"]),
            Ok(DbCommand::SnapshotDrop(SnapshotArgs { ref name, .. })) if name == "s"
        ));
        assert!(matches!(
            parse(&["restore", "s", "--main", "--no-restart"]),
            Ok(DbCommand::Restore(SnapshotArgs {
                main: true,
                no_restart: true,
                ..
            }))
        ));
        assert!(parse(&["restore"]).is_err(), "restore needs a name");
        assert!(
            matches!(parse(&["baseline", "-w", "feat-login"]), Ok(DbCommand::Baseline(SnapshotArgs { ref branch, .. })) if branch.as_deref() == Some("feat-login"))
        );
        assert!(matches!(
            parse(&["reseed"]),
            Ok(DbCommand::Reseed(SnapshotArgs {
                from_snapshot: None,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["reseed", "--from", "main__baseline"]),
            Ok(DbCommand::Reseed(SnapshotArgs {
                from_snapshot: None,
                ..
            }))
        ));
        assert!(
            matches!(parse(&["reseed", "--snapshot", "seeded"]), Ok(DbCommand::Reseed(SnapshotArgs { from_snapshot: Some(ref name), .. })) if name == "seeded")
        );
        assert!(parse(&["reseed", "--from", "elsewhere"]).is_err());
        assert!(parse(&["reseed", "--from", "main__baseline", "--snapshot", "s"]).is_err());
        assert!(
            parse(&["snapshot", "s", "--main"]).is_err(),
            "--main only restores"
        );
        assert!(matches!(
            parse(&["snapshots", "-o", "json"]),
            Ok(DbCommand::Snapshots(SnapshotArgs { json: true, .. }))
        ));
        assert_eq!(truncate_identifier(&"x".repeat(70)).len(), 63);
    }
}
