//! Jira for workspaces: the sprint tickets the new-workspace form suggests, and each workspace's ticket
//! status, refreshed in the background at most once a minute.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_jira::{Board, IssueCache, SprintIssue};
use pom_paths::StateDir;

const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// Where the new-workspace form gets its tickets from.
#[derive(Clone)]
pub struct TicketSource {
    pub boards: Arc<dyn Fn() -> Result<Vec<Board>, String> + Send + Sync>,
    pub sprint: Arc<dyn Fn(i64) -> Result<Vec<SprintIssue>, String> + Send + Sync>,
    /// The board picked last time, if any.
    pub board: Option<i64>,
    pub only_mine: bool,
}

impl TicketSource {
    /// Tickets from the session's Jira, or `None` while it is not set up.
    pub fn for_session(
        state: &StateDir,
        session: &str,
        board: Option<i64>,
        only_mine: bool,
    ) -> Option<TicketSource> {
        let client = Arc::new(pom_jira::resolve(state, session)?);
        let for_sprint = client.clone();
        Some(TicketSource {
            boards: Arc::new(move || client.boards().map_err(|error| error.to_string())),
            sprint: Arc::new(move |board| {
                for_sprint
                    .current_sprint_issues(board)
                    .map_err(|error| error.to_string())
            }),
            board,
            only_mine,
        })
    }
}

/// Each workspace's ticket status (the key comes from its branch), from the disk cache at once and from Jira
/// once a minute while asked.
pub struct TicketStatuses {
    state: StateDir,
    session: String,
    cache: Arc<Mutex<IssueCache>>,
    refreshing: Arc<AtomicBool>,
    /// A refresh brought news the rows have not shown yet.
    changed: Arc<AtomicBool>,
    last: Option<Instant>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

impl TicketStatuses {
    pub fn new(
        state: StateDir,
        session: &str,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> TicketStatuses {
        TicketStatuses {
            cache: Arc::new(Mutex::new(IssueCache::open(&state, session))),
            state,
            session: session.to_string(),
            refreshing: Arc::new(AtomicBool::new(false)),
            changed: Arc::new(AtomicBool::new(false)),
            last: None,
            waker,
        }
    }

    /// The ticket status for a branch, as last fetched.
    pub fn status(&self, branch: &str) -> Option<String> {
        let key = pom_jira::key_for_branch(branch)?;
        let cache = self.cache.lock().ok()?;
        cache
            .issue(&key)
            .map(|issue| issue.status.clone())
            .filter(|status| !status.is_empty())
    }

    /// The Jira category of the branch's ticket status: `new`, `indeterminate` or `done`.
    pub fn category(&self, branch: &str) -> Option<String> {
        let key = pom_jira::key_for_branch(branch)?;
        let cache = self.cache.lock().ok()?;
        cache
            .issue(&key)
            .map(|issue| issue.category.clone())
            .filter(|category| !category.is_empty())
    }

    /// Whether statuses changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }

    /// Starts a background refresh for these branches' tickets unless one ran in the last minute.
    pub fn refresh_if_due(&mut self, branches: &[String]) {
        if self.last.is_some_and(|at| at.elapsed() < REFRESH_EVERY)
            || self.refreshing.swap(true, Ordering::SeqCst)
        {
            return;
        }
        self.last = Some(Instant::now());
        let keys: Vec<String> = branches
            .iter()
            .filter_map(|branch| pom_jira::key_for_branch(branch))
            .collect();
        let (state, session) = (self.state.clone(), self.session.clone());
        let (cache, refreshing, changed, waker) = (
            self.cache.clone(),
            self.refreshing.clone(),
            self.changed.clone(),
            self.waker.clone(),
        );
        std::thread::spawn(move || {
            let stale = cache
                .lock()
                .map(|cache| cache.stale(&keys))
                .unwrap_or_default();
            let fetched = match pom_jira::resolve(&state, &session) {
                Some(client) if !stale.is_empty() => match client.search_by_keys(&stale) {
                    Ok(issues) => cache
                        .lock()
                        .map(|mut cache| cache.record(&stale, issues))
                        .is_ok(),
                    Err(error) => {
                        eprintln!("workspaces: ticket status: {error}");
                        false
                    }
                },
                _ => false,
            };
            refreshing.store(false, Ordering::SeqCst);
            if fetched {
                changed.store(true, Ordering::SeqCst);
                waker();
            }
        });
    }
}
