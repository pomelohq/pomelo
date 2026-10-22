//! `sync/pushes.ndjson`: one line per push delivery that reached a workspace and per sync it led to, so
//! `pom sync status`, `pom ws ls` and the app can say what happened without the relay's process.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

pub const RECORD_SCHEMA: &str = "pom.push/v1";
const MAX_BYTES: u64 = 2 << 20;
const KEPT_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// A signed `ping`: the hook is set up.
    Pong,
    /// No workspace of a project whose secret signed it has that repo on that branch.
    NoMatch,
    /// `sync.on_push: off`.
    Off,
    /// Waiting out the debounce before the workspace syncs.
    #[default]
    Queued,
    /// The branch was deleted on GitHub; nothing changes here.
    Deleted,
    /// `sync.on_push: notify`: fetched only; `commits` are waiting.
    Notified,
    UpToDate,
    Pulled,
    /// Uncommitted changes: left as it is.
    Dirty,
    /// Local commits GitHub does not have: left as it is.
    Diverged,
    Failed,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Pong => "pong",
            Outcome::NoMatch => "no_match",
            Outcome::Off => "off",
            Outcome::Queued => "queued",
            Outcome::Deleted => "deleted",
            Outcome::Notified => "notified",
            Outcome::UpToDate => "up_to_date",
            Outcome::Pulled => "pulled",
            Outcome::Dirty => "dirty",
            Outcome::Diverged => "diverged",
            Outcome::Failed => "failed",
        }
    }

    /// What a person should hear about: something arrived and is waiting, was pulled, or could not be.
    pub fn is_notice(self) -> bool {
        matches!(
            self,
            Outcome::Notified
                | Outcome::Pulled
                | Outcome::Dirty
                | Outcome::Diverged
                | Outcome::Deleted
                | Outcome::Failed
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub schema: String,
    /// UTC, RFC 3339.
    pub time: String,
    pub ms: u64,
    /// `X-GitHub-Delivery` ids this line covers (a burst syncs once); `manual` for a pull asked for here.
    #[serde(default)]
    pub deliveries: Vec<String>,
    #[serde(default)]
    pub event: String,
    /// `owner/name` on GitHub.
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub after: String,
    /// The project's session name.
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub workspace: String,
    /// The repo key in pom.yml.
    #[serde(default)]
    pub repo: String,
    pub outcome: Outcome,
    /// Commits on GitHub the worktree did not have: pulled, or still waiting.
    #[serde(default)]
    pub commits: u64,
    /// The worktree is clean and only behind, so a pull would fast-forward it.
    #[serde(default)]
    pub can_pull: bool,
    #[serde(default)]
    pub setup: bool,
    #[serde(default)]
    pub migrated: bool,
    #[serde(default)]
    pub restarted: Vec<String>,
    #[serde(default)]
    pub detail: String,
}

impl Record {
    pub fn new(event: &str, outcome: Outcome) -> Record {
        let ms = now_ms();
        Record {
            schema: RECORD_SCHEMA.to_string(),
            time: utc_text(ms),
            ms,
            event: event.to_string(),
            outcome,
            ..Record::default()
        }
    }

    /// One line for a person: `feat-x: 3 new commits on GitHub (api)`.
    pub fn message(&self) -> String {
        let commits = match self.commits {
            1 => "1 new commit".to_string(),
            count => format!("{count} new commits"),
        };
        let what = match self.outcome {
            Outcome::Notified => format!("{commits} on GitHub"),
            Outcome::Pulled if self.restarted.is_empty() => format!("pulled {commits}"),
            Outcome::Pulled => format!("pulled {commits}, restarted {}", self.restarted.join(", ")),
            Outcome::Dirty => format!("{commits} on GitHub, not pulled: uncommitted changes"),
            Outcome::Diverged => {
                format!("{commits} on GitHub, not pulled: local commits GitHub does not have")
            }
            Outcome::Deleted => "the branch was deleted on GitHub".to_string(),
            Outcome::Failed => format!("sync failed: {}", self.detail),
            other => other.name().replace('_', " "),
        };
        format!("{}: {what} ({})", self.workspace, self.repo)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// `2026-10-22T09:15:04Z` for milliseconds since the epoch.
pub fn utc_text(ms: u64) -> String {
    let seconds = ms / 1000;
    let (days, of_day) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days to a civil date (proleptic Gregorian), after Howard Hinnant's algorithm.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

pub fn journal_path(state: &StateDir) -> PathBuf {
    state.path("sync/pushes.ndjson")
}

pub fn append(state: &StateDir, record: &Record) {
    let path = journal_path(state);
    let written = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
            let text = std::fs::read_to_string(&path)?;
            let lines: Vec<&str> = text.lines().collect();
            let kept = lines[lines.len().saturating_sub(KEPT_LINES)..].join("\n") + "\n";
            pom_paths::write_atomic(&path, kept.as_bytes(), 0o644)?;
        }
        let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
        line.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(line.as_bytes())
    })();
    if let Err(error) = written {
        eprintln!("sync on push: write {}: {error}", path.display());
    }
}

/// Every record, oldest first; lines that do not parse are skipped.
pub fn read_all(state: &StateDir) -> Vec<Record> {
    std::fs::read_to_string(journal_path(state))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The newest `limit` records, newest first.
pub fn recent(state: &StateDir, limit: usize) -> Vec<Record> {
    let mut records = read_all(state);
    records.reverse();
    records.truncate(limit);
    records
}

/// The last record of each workspace of the project `session` that is more than a queued delivery.
pub fn last_syncs(state: &StateDir, session: &str) -> HashMap<String, Record> {
    let mut latest = HashMap::new();
    for record in read_all(state)
        .into_iter()
        .filter(|record| record.project == session && record.outcome != Outcome::Queued)
    {
        latest.insert(record.workspace.clone(), record);
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_text_is_rfc_3339() {
        assert_eq!(utc_text(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_text(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(utc_text(1_792_660_504_000), "2026-10-22T09:15:04Z");
    }

    #[test]
    fn records_round_trip_and_the_latest_wins() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut record = Record::new("push", Outcome::Queued);
        record.project = "demo".into();
        record.workspace = "feat-x".into();
        append(&state, &record);
        record.outcome = Outcome::Notified;
        record.commits = 3;
        record.repo = "api".into();
        append(&state, &record);
        assert_eq!(read_all(&state).len(), 2);
        assert_eq!(recent(&state, 1)[0].outcome, Outcome::Notified);
        let last = last_syncs(&state, "demo");
        assert_eq!(last["feat-x"].commits, 3);
        assert_eq!(
            last["feat-x"].message(),
            "feat-x: 3 new commits on GitHub (api)"
        );
        assert!(last_syncs(&state, "other").is_empty());
    }
}
