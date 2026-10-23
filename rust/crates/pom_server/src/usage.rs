//! `usage.get`: the Claude account this machine's agents run under, its plan limits and what the project's
//! agents cost today, read here so the credentials never leave the machine.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_usage::{Limits, LimitsError, LimitsKeeper, Transcripts};
use pom_paths::StateDir;
use pom_remote::server_api::UsageReport;

/// A day is at most this long, whatever the asking app's timezone.
const LONGEST_DAY: u64 = 26 * 3600;

pub type FetchLimits = Arc<dyn Fn(&Path) -> Result<Limits, LimitsError> + Send + Sync>;

struct Read {
    transcripts: Transcripts,
    keeper: LimitsKeeper,
}

pub struct UsageService {
    home: PathBuf,
    fetch: FetchLimits,
    read: Mutex<Read>,
}

impl UsageService {
    pub fn new(home: PathBuf, fetch: FetchLimits) -> UsageService {
        UsageService {
            home,
            fetch,
            read: Mutex::new(Read {
                transcripts: Transcripts::default(),
                keeper: LimitsKeeper::default(),
            }),
        }
    }

    pub fn from_env() -> UsageService {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        UsageService::new(home, Arc::new(agent_usage::fetch_limits))
    }

    /// What the project at `root` used since `since`, and the account's limits as last known.
    pub fn report(&self, state: &StateDir, root: &Path, since: u64, now: u64) -> UsageReport {
        let since = since.max(now.saturating_sub(LONGEST_DAY));
        let account = agent_usage::read_account(&self.home);
        let mut read = self
            .read
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Read {
            transcripts,
            keeper,
        } = &mut *read;
        transcripts.refresh(&self.home.join(".claude/projects"), since);
        let limits = keeper.update(
            now,
            account.is_some(),
            agent_usage::reported_limits(&state.path(pom_agent::RATE_LIMITS_FILE)),
            false,
            || (self.fetch)(&self.home),
        );
        let today = agent_usage::day_summary(
            transcripts
                .turns(since)
                .into_iter()
                .filter(|turn| branch_of(root, &turn.cwd).is_some()),
            |turn| branch_of(root, &turn.cwd).unwrap_or_default(),
        );
        let (limits, limits_error) = match limits {
            Ok(limits) => (Some(limits), None),
            Err(error) => (None, Some(error)),
        };
        UsageReport {
            account,
            limits,
            limits_error,
            limits_at: keeper.kept_at(),
            today,
        }
    }
}

/// The workspace branch a transcript's working folder belongs to, when it is under `root`.
fn branch_of(root: &Path, cwd: &Path) -> Option<String> {
    let first = cwd
        .strip_prefix(root)
        .ok()?
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    first
        .strip_prefix(pom_layout::WORKSPACE_PREFIX)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW: u64 = 1_790_577_531;

    fn transcript(home: &Path, cwd: &Path, output: u64, at: &str) -> std::io::Result<()> {
        let folder = home.join(".claude/projects/project");
        std::fs::create_dir_all(&folder)?;
        let line = serde_json::json!({
            "type": "assistant",
            "timestamp": at,
            "cwd": cwd,
            "message": {
                "id": format!("message-{output}"),
                "model": "claude-sonnet",
                "usage": {"input_tokens": 0, "output_tokens": output},
            },
        });
        let path = folder.join("session.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(&line.to_string());
        text.push('\n');
        std::fs::write(path, text)
    }

    #[test]
    fn the_report_reads_this_machine_and_asks_the_endpoint_rarely() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let state = StateDir::new(temp.path().join("state"));
        let root = temp.path().join("myproject");
        std::fs::create_dir_all(&home)?;
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"dev@example.com","displayName":"dev"}}"#,
        )?;
        let workspace = root.join(format!("{}feat-login", pom_layout::WORKSPACE_PREFIX));
        transcript(
            &home,
            &workspace.join("api"),
            1_000_000,
            "2026-09-28T06:00:00Z",
        )?;
        transcript(
            &home,
            &temp.path().join("elsewhere"),
            2_000_000,
            "2026-09-28T06:10:00Z",
        )?;
        let asks = Arc::new(AtomicUsize::new(0));
        let counted = asks.clone();
        let service = UsageService::new(
            home,
            Arc::new(move |_: &Path| {
                counted.fetch_add(1, Ordering::SeqCst);
                agent_usage::parse_limits(r#"{"five_hour":{"utilization":12.0}}"#)
            }),
        );
        let report = service.report(&state, &root, NOW - 3600, NOW);
        assert_eq!(
            report.account.map(|account| account.name),
            Some("dev".into())
        );
        assert_eq!(report.limits.map(|limits| limits.session.used), Some(12.0));
        assert_eq!(report.limits_at, Some(NOW));
        let today = report.today.expect("a turn in the project today");
        assert_eq!(today.by_workspace, vec![("feat-login".to_string(), 15.0)]);
        assert_eq!(today.sessions, 1);
        let again = service.report(&state, &root, NOW - 3600, NOW + 60);
        assert_eq!(again.limits.map(|limits| limits.session.used), Some(12.0));
        assert_eq!(asks.load(Ordering::SeqCst), 1, "kept for a few minutes");
        let tomorrow = service.report(&state, &root, NOW + 86_400, NOW + 86_400);
        assert_eq!(tomorrow.today, None);
        Ok(())
    }

    #[test]
    fn signed_out_reports_no_account_and_never_asks() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let service = UsageService::new(
            temp.path().to_path_buf(),
            Arc::new(|_: &Path| panic!("signed out: nothing to ask with")),
        );
        let report = service.report(&StateDir::new(temp.path()), temp.path(), NOW, NOW);
        assert_eq!(report.account, None);
        assert_eq!(report.limits_error, Some(LimitsError::SignedOut));
        assert_eq!(report.today, None);
        Ok(())
    }
}
