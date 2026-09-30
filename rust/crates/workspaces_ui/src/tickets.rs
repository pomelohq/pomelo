//! Jira for workspaces: the sprint tickets the new-workspace form suggests, and each workspace's ticket
//! status, refreshed in the background at most once a minute.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_jira::{Board, IssueCache, SprintIssue};
use pom_paths::StateDir;

const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// Which tickets the new-workspace form lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketList {
    Sprint(i64),
    Backlog(i64),
    Assigned,
}

impl TicketList {
    fn encode(self) -> String {
        match self {
            TicketList::Sprint(board) => format!("sprint:{board}"),
            TicketList::Backlog(board) => format!("backlog:{board}"),
            TicketList::Assigned => "assigned".into(),
        }
    }

    fn decode(text: &str) -> Option<TicketList> {
        let text = text.trim();
        if text == "assigned" {
            return Some(TicketList::Assigned);
        }
        let (kind, board) = text.split_once(':')?;
        let board = board.parse().ok()?;
        match kind {
            "sprint" => Some(TicketList::Sprint(board)),
            "backlog" => Some(TicketList::Backlog(board)),
            _ => None,
        }
    }
}

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
        let client = Arc::new(pom_jira::resolve(state, session)?);
        let for_issues = client.clone();
        Some(TicketSource {
            boards: Arc::new(move || client.boards().map_err(|error| error.to_string())),
            issues: Arc::new(move |list| {
                match list {
                    TicketList::Sprint(board) => for_issues.current_sprint_issues(board),
                    TicketList::Backlog(board) => for_issues.backlog_issues(board),
                    TicketList::Assigned => for_issues.assigned_issues(),
                }
                .map_err(|error| error.to_string())
            }),
            board,
            start: load_list(state, session),
            remember: {
                let path = list_file(state, session);
                Arc::new(move |list| save_list(&path, list))
            },
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
