//! Marks for one test step: where each service's output had got to and what Postgres had counted, so the step's
//! own logs and queries can be read back afterwards.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_config::Config;
use serde::{Deserialize, Serialize};

use crate::control::{ServiceRunner, ServiceTarget};
use crate::shared::output_text;

const HOLDER_TIMEOUT: Duration = Duration::from_secs(5);
const TOP: usize = 10;
const FIELD_SEPARATOR: &str = "\u{1f}";
pub const STATS_UNAVAILABLE: &str = "pg_stat_statements is not available in the shared Postgres. Start it with the module loaded, e.g. in pom.yml under the postgres shared service: `command: postgres -c shared_preload_libraries=pg_stat_statements`, then restart the container (pom release, then pom start). Pomelo never recreates it for you.";

/// One row of `pg_stat_statements` for a workspace database.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatementRow {
    pub db: String,
    pub queryid: String,
    pub calls: u64,
    pub total_ms: f64,
    pub rows: u64,
    pub query: String,
}

/// `.pom/marks/<name>.json` in the workspace folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub created_ms: u64,
    /// `repo/service` -> the absolute offset its output had reached.
    #[serde(default)]
    pub logs: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statements: Option<Vec<StatementRow>>,
}

impl Mark {
    pub fn path(folder: &Path, name: &str) -> PathBuf {
        folder
            .join(".pom")
            .join("marks")
            .join(format!("{name}.json"))
    }

    pub fn load(folder: &Path, name: &str) -> Result<Mark, String> {
        crate::valid_snapshot_name(name)?;
        let text = std::fs::read_to_string(Self::path(folder, name))
            .map_err(|_| format!("no mark {name}: take it with `pom mark {name}`"))?;
        serde_json::from_str(&text).map_err(|error| format!("mark {name}: {error}"))
    }

    pub fn save(&self, folder: &Path, name: &str) -> Result<(), String> {
        crate::valid_snapshot_name(name)?;
        let path = Self::path(folder, name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())
    }

    pub fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }
}

/// A query's share of one test step.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StepQuery {
    pub db: String,
    pub queryid: String,
    pub calls: u64,
    pub total_ms: f64,
    pub rows: u64,
    pub query: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StepStats {
    /// Distinct queries run in the step.
    pub queries: usize,
    pub calls: u64,
    pub total_ms: f64,
    pub top: Vec<StepQuery>,
    /// The same query run more than the threshold: the N+1 signal.
    pub repeated: Vec<StepQuery>,
}

/// What changed in `pg_stat_statements` between a mark and now.
pub fn step_stats(before: &[StatementRow], now: &[StatementRow], repeated_over: u64) -> StepStats {
    let earlier: BTreeMap<(&str, &str), &StatementRow> = before
        .iter()
        .map(|row| ((row.db.as_str(), row.queryid.as_str()), row))
        .collect();
    let mut queries: Vec<StepQuery> = now
        .iter()
        .filter_map(|row| {
            let (calls, total_ms, rows) =
                match earlier.get(&(row.db.as_str(), row.queryid.as_str())) {
                    Some(old) => (
                        row.calls.saturating_sub(old.calls),
                        (row.total_ms - old.total_ms).max(0.0),
                        row.rows.saturating_sub(old.rows),
                    ),
                    None => (row.calls, row.total_ms, row.rows),
                };
            (calls > 0).then(|| StepQuery {
                db: row.db.clone(),
                queryid: row.queryid.clone(),
                calls,
                total_ms,
                rows,
                query: row.query.clone(),
            })
        })
        .collect();
    queries.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));
    let mut repeated: Vec<StepQuery> = queries
        .iter()
        .filter(|query| query.calls > repeated_over)
        .cloned()
        .collect();
    repeated.sort_by_key(|query| std::cmp::Reverse(query.calls));
    StepStats {
        queries: queries.len(),
        calls: queries.iter().map(|query| query.calls).sum(),
        total_ms: queries.iter().map(|query| query.total_ms).sum(),
        top: queries.into_iter().take(TOP).collect(),
        repeated,
    }
}

/// Removes terminal escape sequences, so a log reads as plain text.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A service's output since a mark.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LogSlice {
    pub service: String,
    pub since: u64,
    pub end: u64,
    /// More than the ring holds was written since the mark, so the start is missing.
    pub truncated: bool,
    pub bytes: usize,
    pub text: String,
}

fn sql_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("'{}'", name.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

impl ServiceRunner {
    /// The offset each running repo service's output has reached.
    pub fn log_offsets(&self, targets: &[ServiceTarget]) -> BTreeMap<String, u64> {
        let mut offsets = BTreeMap::new();
        for target in targets {
            let holder = self.holder_name(target);
            if !self.holders().holder_alive(&holder) {
                continue;
            }
            match pom_ptyhost::output_end(self.holders(), &holder, HOLDER_TIMEOUT) {
                Ok(end) => {
                    offsets.insert(format!("{}/{}", target.repo, target.service), end);
                }
                Err(error) => eprintln!("services: {holder} output offset: {error}"),
            }
        }
        offsets
    }

    /// `target`'s output from offset `since` on, plain text unless `raw`.
    pub fn logs_since(
        &self,
        target: &ServiceTarget,
        since: u64,
        raw: bool,
    ) -> Result<LogSlice, String> {
        let holder = self.holder_name(target);
        let label = format!("{}/{}", target.repo, target.service);
        if !self.holders().holder_alive(&holder) {
            return Err(format!("{label} is not running"));
        }
        let (bytes, end) = pom_ptyhost::read_since(self.holders(), &holder, since, HOLDER_TIMEOUT)
            .map_err(|error| format!("{label}: {error}"))?;
        let wanted = end.saturating_sub(since);
        let text = String::from_utf8_lossy(&bytes);
        Ok(LogSlice {
            service: label,
            since,
            end,
            truncated: (bytes.len() as u64) < wanted || (since > 0 && end < since),
            bytes: bytes.len(),
            text: if raw {
                text.into_owned()
            } else {
                strip_ansi(&text)
            },
        })
    }

    /// `pg_stat_statements` rows of the workspace databases `dbs`.
    pub fn statement_rows(
        &self,
        config: &Config,
        dbs: &[String],
    ) -> Result<Vec<StatementRow>, String> {
        if dbs.is_empty() {
            return Ok(Vec::new());
        }
        let postgres = self.postgres(config);
        // The view lives in each database that created the extension; the counters behind it are server-wide.
        let created = self
            .psql(
                &postgres,
                "postgres",
                "CREATE EXTENSION IF NOT EXISTS pg_stat_statements",
            )
            .map_err(|error| error.to_string())?;
        if !created.status.success() {
            return Err(output_text(&created).trim().to_string());
        }
        let sql = format!(
            "SELECT d.datname, s.queryid, s.calls, s.total_exec_time, s.rows, regexp_replace(s.query, E'\\\\s+', ' ', 'g') FROM pg_stat_statements s JOIN pg_database d ON d.oid = s.dbid WHERE d.datname IN ({})",
            sql_list(dbs)
        );
        let output = self
            .docker(&postgres.psql_args("postgres", &["-F", FIELD_SEPARATOR, "-tAc", &sql]))
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let text = output_text(&output);
            if text.contains("shared_preload_libraries") {
                return Err(STATS_UNAVAILABLE.to_string());
            }
            return Err(text.trim().to_string());
        }
        Ok(parse_statement_rows(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Live connections to the workspace databases, by `application_name` (`pom:<branch>:<repo>/<service>`).
    pub fn connections_by_app(&self, config: &Config, dbs: &[String]) -> BTreeMap<String, u64> {
        if dbs.is_empty() {
            return BTreeMap::new();
        }
        let postgres = self.postgres(config);
        let sql = format!(
            "SELECT application_name, count(*) FROM pg_stat_activity WHERE datname IN ({}) GROUP BY 1",
            sql_list(dbs)
        );
        let Ok(output) =
            self.docker(&postgres.psql_args("postgres", &["-F", FIELD_SEPARATOR, "-tAc", &sql]))
        else {
            return BTreeMap::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (name, count) = line.split_once(FIELD_SEPARATOR)?;
                Some((name.to_string(), count.trim().parse().ok()?))
            })
            .collect()
    }
}

/// `psql -tA -F <unit separator>` output of the statements query.
pub fn parse_statement_rows(text: &str) -> Vec<StatementRow> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(6, FIELD_SEPARATOR);
            Some(StatementRow {
                db: fields.next()?.to_string(),
                queryid: fields.next()?.to_string(),
                calls: fields.next()?.trim().parse().ok()?,
                total_ms: fields.next()?.trim().parse().ok()?,
                rows: fields.next()?.trim().parse().ok()?,
                query: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(queryid: &str, calls: u64, total_ms: f64) -> StatementRow {
        StatementRow {
            db: "myproject_api_feat-login".into(),
            queryid: queryid.into(),
            calls,
            total_ms,
            rows: calls,
            query: format!("SELECT {queryid}"),
        }
    }

    #[test]
    fn a_step_counts_only_what_ran_after_the_mark() {
        let before = vec![row("1", 5, 10.0), row("2", 3, 1.0)];
        let now = vec![row("1", 5, 10.0), row("2", 40, 9.0), row("3", 1, 50.0)];
        let stats = step_stats(&before, &now, 10);
        assert_eq!(stats.queries, 2, "query 1 did not run in the step");
        assert_eq!(stats.calls, 38);
        assert_eq!(stats.top[0].queryid, "3", "slowest first");
        assert_eq!(stats.repeated.len(), 1);
        assert_eq!(
            (stats.repeated[0].queryid.as_str(), stats.repeated[0].calls),
            ("2", 37)
        );
    }

    #[test]
    fn statement_rows_parse_with_the_query_text_last() {
        let text = "myproject_api_feat-login\u{1f}42\u{1f}7\u{1f}1.5\u{1f}7\u{1f}SELECT a | b FROM t WHERE x = $1\n";
        let rows = parse_statement_rows(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].calls, 7);
        assert_eq!(rows[0].query, "SELECT a | b FROM t WHERE x = $1");
        assert!(parse_statement_rows("garbage").is_empty());
    }

    #[test]
    fn escape_sequences_are_stripped_from_logs() {
        assert_eq!(
            strip_ansi("\u{1b}[32mStarted\u{1b}[0m GET /login\r\n\u{1b}]0;title\u{7}done"),
            "Started GET /login\ndone"
        );
    }
}
