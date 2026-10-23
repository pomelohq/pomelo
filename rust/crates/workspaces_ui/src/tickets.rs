//! Jira for workspaces: the sprint tickets the new-workspace form suggests, and each workspace's ticket
//! status, refreshed in the background at most once a minute.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use pom_jira::TicketList;
use pom_jira::{Board, Connect, IssueCache, SprintIssue};
use pom_paths::StateDir;

const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// The list the form showed last in a project, opened again next time.
fn list_file(state: &StateDir, session: &str) -> std::path::PathBuf {
    state.path("cache").join(format!("jira-list-{session}"))
}

fn load_list(state: &StateDir, session: &str) -> Option<TicketList> {
    TicketList::decode(&std::fs::read_to_string(list_file(state, session)).ok()?)
}

fn save_list(path: &std::path::Path, list: TicketList) {
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| pom_paths::write_atomic(path, list.encode().as_bytes(), 0o644));
    if let Err(error) = written {
        eprintln!("jira: remember the ticket list: {error}");
    }
}

type Issues = Arc<dyn Fn(TicketList) -> Result<Vec<SprintIssue>, String> + Send + Sync>;

/// Where the new-workspace form gets its tickets from.
#[derive(Clone)]
pub struct TicketSource {
    pub boards: Arc<dyn Fn() -> Result<Vec<Board>, String> + Send + Sync>,
    pub issues: Issues,
    /// The board picked last time, if any.
    pub board: Option<i64>,
    /// The list picked last time in this project, if any.
    pub start: Option<TicketList>,
    pub remember: Arc<dyn Fn(TicketList) + Send + Sync>,
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
        let client = pom_jira::resolve(state, session)?;
        let jira: Arc<dyn pom_jira::Jira> = Arc::new(client);
        Some(TicketSource::reaching(
            state,
            session,
            Arc::new(move || Some(jira.clone())),
            board,
            only_mine,
        ))
    }

    /// Tickets from whatever `connect` reaches when asked; `session` keeps the list picked last.
    pub fn reaching(
        state: &StateDir,
        session: &str,
        connect: Connect,
        board: Option<i64>,
        only_mine: bool,
    ) -> TicketSource {
        let for_issues = connect.clone();
        let not_set_up = || "Jira is not set up for this project".to_string();
        TicketSource {
            boards: Arc::new(move || {
                connect()
                    .ok_or_else(not_set_up)?
                    .boards()
                    .map_err(|error| error.to_string())
            }),
            issues: Arc::new(move |list| {
                for_issues()
                    .ok_or_else(not_set_up)?
                    .issues(list)
                    .map_err(|error| error.to_string())
            }),
            board,
            start: load_list(state, session),
            remember: {
                let path = list_file(state, session);
                Arc::new(move |list| save_list(&path, list))
            },
            only_mine,
        }
    }
}

/// Each workspace's ticket status (the key comes from its branch), from the disk cache at once and from Jira
/// once a minute while asked.
pub struct TicketStatuses {
    jira: Connect,
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
            jira: pom_jira::session_connect(&state, session),
            refreshing: Arc::new(AtomicBool::new(false)),
            changed: Arc::new(AtomicBool::new(false)),
            last: None,
            waker,
        }
    }

    /// Asks `jira` for the statuses instead of this Mac's settings for the session.
    pub fn with_jira(mut self, jira: Connect) -> TicketStatuses {
        self.jira = jira;
        self
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
        let jira = self.jira.clone();
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
            let fetched = match (!stale.is_empty()).then(|| jira()).flatten() {
                Some(client) => match client.search_by_keys(&stale) {
                    Ok(issues) => cache
                        .lock()
                        .map(|mut cache| cache.record(&stale, issues))
                        .is_ok(),
                    Err(error) => {
                        eprintln!("workspaces: ticket status: {error}");
                        false
                    }
                },
                None => false,
            };
            refreshing.store(false, Ordering::SeqCst);
            if fetched {
                changed.store(true, Ordering::SeqCst);
                waker();
            }
        });
    }
}
