//! `pom mark`, `pom logs --mark / --since` and `pom db mark / stats`: what one test step logged and queried.

use std::io::Write;
use std::path::Path;

use pom_services::{Mark, ServiceTarget};

use crate::args::Args;
use crate::{say, Session};

/// More calls than this to one query in a step counts as repeated.
const REPEATED_OVER: u64 = 10;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MarkArgs {
    pub name: String,
    pub branch: Option<String>,
    pub json: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LogsArgs {
    pub service: Option<String>,
    pub branch: Option<String>,
    /// Record a log mark under this name instead of printing.
    pub mark: Option<String>,
    /// Print only what was written since this mark.
    pub since: Option<String>,
    pub raw: bool,
    pub json: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StatsArgs {
    pub since: String,
    pub branch: Option<String>,
    pub repeated_over: u64,
    pub json: bool,
}

pub(crate) fn parse_mark(rest: &[&str]) -> Result<MarkArgs, String> {
    let args = Args::parse(rest, &["-w", "--workspace", "-o", "--output"])?;
    args.allow(&["-w", "--workspace", "-o", "--output"])?;
    let [name] = args.positional.as_slice() else {
        return Err("mark needs one name".into());
    };
    pom_services::valid_snapshot_name(name)?;
    Ok(MarkArgs {
        name: name.clone(),
        branch: args.value(&["-w", "--workspace"]),
        json: args.json()?,
    })
}

pub(crate) fn parse_logs(rest: &[&str]) -> Result<LogsArgs, String> {
    let valued = ["-w", "--workspace", "-o", "--output", "--mark", "--since"];
    let args = Args::parse(rest, &valued)?;
    let mut allowed = valued.to_vec();
    allowed.push("--raw");
    args.allow(&allowed)?;
    args.at_most(1, "logs")?;
    let parsed = LogsArgs {
        service: args.positional.first().cloned(),
        branch: args.value(&["-w", "--workspace"]),
        mark: args.value(&["--mark"]),
        since: args.value(&["--since"]),
        raw: args.has("--raw"),
        json: args.json()?,
    };
    match (&parsed.service, &parsed.mark, &parsed.since) {
        (_, Some(_), Some(_)) => Err("logs takes --mark or --since, not both".into()),
        (Some(_), Some(_), None) => {
            Err("logs --mark records every service; drop the service".into())
        }
        (None, None, _) => Err("logs needs a service (or --mark <name>)".into()),
        _ => Ok(parsed),
    }
}

pub(crate) fn parse_stats(rest: &[&str]) -> Result<StatsArgs, String> {
    let valued = [
        "-w",
        "--workspace",
        "-o",
        "--output",
        "--since",
        "--repeated",
    ];
    let args = Args::parse(rest, &valued)?;
    args.allow(&valued)?;
    args.at_most(0, "db stats")?;
    let since = args
        .value(&["--since"])
        .ok_or("db stats needs --since <mark> (take one with pom mark <name>)")?;
    let repeated_over = match args.value(&["--repeated"]) {
        Some(text) => text
            .parse()
            .map_err(|_| format!("--repeated takes a number, not {text}"))?,
        None => REPEATED_OVER,
    };
    Ok(StatsArgs {
        since,
        branch: args.value(&["-w", "--workspace"]),
        repeated_over,
        json: args.json()?,
    })
}

impl Session {
    fn branch_scope(
        &self,
        branch: &Option<String>,
    ) -> Result<(String, bool, std::path::PathBuf), String> {
        let branch = branch.clone().unwrap_or_else(|| self.branch.clone());
        let folder = self.workspace_folder(&branch)?;
        let is_main = branch == self.config.global_default_branch();
        Ok((branch, is_main, folder))
    }

    fn repo_targets(&self, branch: &str, is_main: bool) -> Vec<ServiceTarget> {
        pom_services::ServiceRunner::service_targets(&self.config, branch, is_main)
            .into_iter()
            .filter(|target| !target.is_workspace_level())
            .collect()
    }

    fn owned_databases(&self, branch: &str, folder: &Path) -> Vec<String> {
        pom_services::owned_database_names(&self.config, branch, |repo| folder.join(repo).is_dir())
    }

    /// `pom mark <name>`: the log offsets and the query counters of the workspace, as one mark.
    pub(crate) fn mark_command(&self, args: &MarkArgs, out: &mut dyn Write) -> Result<(), String> {
        let (branch, is_main, folder) = self.branch_scope(&args.branch)?;
        let logs = self
            .runner
            .log_offsets(&self.repo_targets(&branch, is_main));
        let databases = self.owned_databases(&branch, &folder);
        let (statements, warning) = match self.runner.statement_rows(&self.config, &databases) {
            Ok(rows) => (Some(rows), None),
            Err(error) => (None, Some(error)),
        };
        let mark = Mark {
            created_ms: Mark::now(),
            logs,
            statements,
        };
        mark.save(&folder, &args.name)?;
        if args.json {
            let document = serde_json::json!({
                "schema": "pom.mark/v1",
                "workspace": branch,
                "mark": args.name,
                "logs": mark.logs,
                "queries_counted": mark.statements.as_ref().map(Vec::len),
                "warning": warning,
            });
            return say(out, &document.to_string());
        }
        if let Some(warning) = warning {
            say(out, &format!("warning: {warning}"))?;
        }
        say(
            out,
            &format!(
                "marked {} in workspace {branch}: {} service log(s){}",
                args.name,
                mark.logs.len(),
                if mark.statements.is_some() {
                    " and query counters"
                } else {
                    ""
                }
            ),
        )
    }

    /// `pom logs`: everything, a log mark, or what a service printed since a mark.
    pub(crate) fn logs_command(&self, args: &LogsArgs, out: &mut dyn Write) -> Result<(), String> {
        let (branch, is_main, folder) = self.branch_scope(&args.branch)?;
        if let Some(name) = &args.mark {
            let mut mark = Mark::load(&folder, name).unwrap_or_default();
            mark.created_ms = Mark::now();
            mark.logs = self
                .runner
                .log_offsets(&self.repo_targets(&branch, is_main));
            mark.save(&folder, name)?;
            return say(
                out,
                &format!("marked logs of {} service(s) as {name}", mark.logs.len()),
            );
        }
        let Some(entry) = &args.service else {
            return Err("logs needs a service".into());
        };
        let Some(name) = &args.since else {
            return self.logs(entry, out);
        };
        let (repo, service) = self.config.find_service_entry(entry)?;
        let target = ServiceTarget {
            branch: branch.clone(),
            is_main,
            repo: repo.clone(),
            service: service.clone(),
        };
        let mark = Mark::load(&folder, name)?;
        // A service that was not running at the mark started after it, so all of its output belongs to the step.
        let since = mark
            .logs
            .get(&format!("{repo}/{service}"))
            .copied()
            .unwrap_or(0);
        let slice = self.runner.logs_since(&target, since, args.raw)?;
        if args.json {
            let text = serde_json::to_string(&slice).map_err(|error| error.to_string())?;
            return say(out, &text);
        }
        if slice.truncated {
            say(
                out,
                "warning: more was written since the mark than the log keeps; its start is missing",
            )?;
        }
        out.write_all(slice.text.as_bytes())
            .map_err(|error| error.to_string())
    }

    /// `pom db mark <name>`: only the query counters.
    pub(crate) fn db_mark(&self, args: &MarkArgs, out: &mut dyn Write) -> Result<(), String> {
        let (branch, _, folder) = self.branch_scope(&args.branch)?;
        let databases = self.owned_databases(&branch, &folder);
        let rows = self.runner.statement_rows(&self.config, &databases)?;
        let mut mark = Mark::load(&folder, &args.name).unwrap_or_default();
        mark.created_ms = Mark::now();
        mark.statements = Some(rows);
        mark.save(&folder, &args.name)?;
        say(
            out,
            &format!(
                "marked query counters of workspace {branch} as {}",
                args.name
            ),
        )
    }

    /// `pom db stats --since <mark>`: the queries the workspace ran since the mark.
    pub(crate) fn db_stats(&self, args: &StatsArgs, out: &mut dyn Write) -> Result<(), String> {
        let (branch, _, folder) = self.branch_scope(&args.branch)?;
        let mark = Mark::load(&folder, &args.since)?;
        let before = mark.statements.ok_or_else(|| {
            format!(
                "mark {} has no query counters (Postgres was not reachable when it was taken)",
                args.since
            )
        })?;
        let databases = self.owned_databases(&branch, &folder);
        let now = self.runner.statement_rows(&self.config, &databases)?;
        let stats = pom_services::step_stats(&before, &now, args.repeated_over);
        let connections = self.runner.connections_by_app(&self.config, &databases);
        if args.json {
            let document = serde_json::json!({
                "schema": "pom.db/v1",
                "workspace": branch,
                "since": args.since,
                "window_ms": Mark::now().saturating_sub(mark.created_ms),
                "queries": stats.queries,
                "calls": stats.calls,
                "total_ms": stats.total_ms,
                "top": stats.top,
                "repeated": stats.repeated,
                "connections": connections,
            });
            return say(out, &document.to_string());
        }
        say(
            out,
            &format!(
                "since {}: {} queries, {} calls, {:.1} ms",
                args.since, stats.queries, stats.calls, stats.total_ms
            ),
        )?;
        for query in &stats.top {
            say(
                out,
                &format!(
                    "  {:>6} calls {:>9.1} ms  {}",
                    query.calls, query.total_ms, query.query
                ),
            )?;
        }
        if !stats.repeated.is_empty() {
            say(
                out,
                &format!("repeated more than {} times:", args.repeated_over),
            )?;
            for query in &stats.repeated {
                say(out, &format!("  {:>6} calls  {}", query.calls, query.query))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_logs_and_stats_parse() {
        assert_eq!(
            parse_mark(&["checkout", "-w", "feat-login"]).map(|args| (args.name, args.branch)),
            Ok(("checkout".into(), Some("feat-login".into())))
        );
        assert!(parse_mark(&["bad name;"]).is_err());
        let logs = parse_logs(&["api/server", "--since", "checkout", "-o", "json"]).expect("logs");
        assert_eq!((logs.since.as_deref(), logs.json), (Some("checkout"), true));
        assert!(parse_logs(&["--mark", "checkout"]).is_ok());
        assert!(parse_logs(&["api/server", "--mark", "x"]).is_err());
        assert!(parse_logs(&[]).is_err());
        let stats = parse_stats(&["--since", "checkout", "--repeated", "5"]).expect("stats");
        assert_eq!((stats.since.as_str(), stats.repeated_over), ("checkout", 5));
        assert!(parse_stats(&[]).is_err());
    }
}
