mod item;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_forge::{PrCache, PrTarget, PullRequest};
use pom_paths::StateDir;
use workspace::{PrSeverity, PrSummary, PrTrouble};

pub use item::PrItem;

const REFRESH_EVERY: Duration = Duration::from_secs(130);

/// A workspace's branch and its repo checkouts (name, folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRepos {
    pub branch: String,
    pub repos: Vec<(String, PathBuf)>,
}

/// Workspace branch -> (checkout folder, GitHub lookup) per repo on GitHub.
pub type Targets = HashMap<String, Vec<(PathBuf, PrTarget)>>;

struct Shared {
    state: StateDir,
    /// Sessions whose `github` secret may hold the token, first one first.
    sessions: Vec<String>,
    cache: Mutex<PrCache>,
    targets: Mutex<Targets>,
    refreshing: AtomicBool,
    changed: AtomicBool,
    last: Mutex<Option<Instant>>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

/// The session's pull requests, shared by the window, the Git panel and PR tabs.
#[derive(Clone)]
pub struct PullRequests {
    shared: Arc<Shared>,
}

fn target_for(repo: &str, root: &Path) -> Option<PrTarget> {
    target(
        repo,
        &git::working_copy::remote_url(root, "origin")?,
        &git::working_copy::head_state(root).branch?,
    )
}

/// The lookup for `repo` on branch `head` when its `origin` is on GitHub.
pub fn target(repo: &str, origin: &str, head: &str) -> Option<PrTarget> {
    let (_, owner, name) = git::working_copy::parse_github_remote(origin)?;
    Some(PrTarget {
        repo: repo.to_string(),
        owner,
        name,
        head: head.to_string(),
    })
}

impl PullRequests {
    pub fn new(state: StateDir, session: &str, waker: Arc<dyn Fn() + Send + Sync>) -> PullRequests {
        PullRequests::with_sessions(state, vec![session.to_string()], waker)
    }

    /// Looks the token up in each of `sessions` in turn.
    pub fn with_sessions(
        state: StateDir,
        sessions: Vec<String>,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> PullRequests {
        PullRequests {
            shared: Arc::new(Shared {
                cache: Mutex::new(PrCache::open(&state)),
                state,
                sessions,
                targets: Mutex::new(HashMap::new()),
                refreshing: AtomicBool::new(false),
                changed: AtomicBool::new(false),
                last: Mutex::new(None),
                waker,
            }),
        }
    }

    pub fn take_changed(&self) -> bool {
        self.shared.changed.swap(false, Ordering::SeqCst)
    }

    pub fn refresh_if_due(&self, workspaces: Vec<WorkspaceRepos>) {
        self.refresh_targets_if_due(move || {
            workspaces
                .into_iter()
                .map(|workspace| {
                    let found = workspace
                        .repos
                        .iter()
                        .filter_map(|(repo, root)| {
                            target_for(repo, root).map(|target| (root.clone(), target))
                        })
                        .collect();
                    (workspace.branch, found)
                })
                .collect()
        });
    }

    /// Like [`Self::refresh_if_due`], with the lookups found by `find` off the UI thread.
    pub fn refresh_targets_if_due(&self, find: impl FnOnce() -> Targets + Send + 'static) {
        let due = self
            .shared
            .last
            .lock()
            .map(|last| last.is_none_or(|at| at.elapsed() >= REFRESH_EVERY))
            .unwrap_or(false);
        if due {
            self.refresh(find);
        }
    }

    /// The session whose token is used: the first that has one.
    fn token_session(shared: &Shared) -> String {
        PullRequests::token_session_in(shared, &|name| std::env::var(name).ok())
    }

    fn token_session_in(shared: &Shared, env: &dyn Fn(&str) -> Option<String>) -> String {
        shared
            .sessions
            .iter()
            .find(|session| pom_forge::resolve_token(&shared.state, session, env).is_some())
            .or(shared.sessions.first())
            .cloned()
            .unwrap_or_default()
    }

    fn refresh(&self, find: impl FnOnce() -> Targets + Send + 'static) {
        if self.shared.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut last) = self.shared.last.lock() {
            *last = Some(Instant::now());
        }
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("pull-requests".into())
            .spawn(move || {
                let targets = find();
                let all: Vec<PrTarget> = targets
                    .values()
                    .flatten()
                    .map(|(_, target)| target.clone())
                    .collect();
                let targets_changed = shared
                    .targets
                    .lock()
                    .map(|mut known| {
                        let changed = *known != targets;
                        *known = targets;
                        changed
                    })
                    .unwrap_or(false);
                if targets_changed {
                    // What the cache already knows shows now; the network round can take a while.
                    shared.changed.store(true, Ordering::SeqCst);
                    (shared.waker)();
                }
                let mut prs_changed = false;
                if let Some(client) =
                    pom_forge::resolve(&shared.state, &PullRequests::token_session(&shared))
                {
                    // The UI reads the cache every frame, so it is never held across the network call.
                    let due = shared
                        .cache
                        .lock()
                        .map(|cache| cache.due(&all))
                        .unwrap_or_default();
                    if !due.is_empty() {
                        let (found, failure) = pom_forge::fetch_heads(&client, &due);
                        if let Ok(mut cache) = shared.cache.lock() {
                            match cache.absorb(found, failure) {
                                Ok(changed) => prs_changed = changed,
                                Err(error) => eprintln!("pull requests: {error}"),
                            }
                        }
                    }
                }
                shared.refreshing.store(false, Ordering::SeqCst);
                if prs_changed {
                    shared.changed.store(true, Ordering::SeqCst);
                    (shared.waker)();
                }
            });
        if spawned.is_err() {
            self.shared.refreshing.store(false, Ordering::SeqCst);
        }
    }

    fn prs_of(&self, branch: &str) -> Vec<(PathBuf, PrTarget, Option<PullRequest>)> {
        let targets = self
            .shared
            .targets
            .lock()
            .ok()
            .and_then(|targets| targets.get(branch).cloned())
            .unwrap_or_default();
        let Ok(cache) = self.shared.cache.lock() else {
            return Vec::new();
        };
        targets
            .into_iter()
            .map(|(root, target)| {
                let pr = cache.get(&target).flatten().cloned();
                (root, target, pr)
            })
            .collect()
    }

    /// How many PRs the workspace has and the worst of their states; `None` without any.
    pub fn summary(&self, branch: &str) -> Option<PrSummary> {
        let prs: Vec<PullRequest> = self
            .prs_of(branch)
            .into_iter()
            .filter_map(|(_, _, pr)| pr)
            .collect();
        if prs.is_empty() {
            return None;
        }
        let severity = match pom_forge::severity(&prs) {
            pom_forge::Severity::Ok => PrSeverity::Ok,
            pom_forge::Severity::Warn => PrSeverity::Warn,
            pom_forge::Severity::Merged => PrSeverity::Merged,
            pom_forge::Severity::Danger => PrSeverity::Danger,
        };
        let trouble = pom_forge::trouble(&prs).map(|trouble| match trouble {
            pom_forge::Trouble::Pending => PrTrouble::Pending,
            pom_forge::Trouble::ChangesRequested => PrTrouble::ChangesRequested,
            pom_forge::Trouble::ChecksFailed => PrTrouble::ChecksFailed,
            pom_forge::Trouble::Conflict => PrTrouble::Conflict,
        });
        Some(PrSummary {
            count: prs.len(),
            severity,
            trouble,
        })
    }

    /// Why the workspace is done, when every PR its repos have is merged, with the commit each was merged at.
    pub fn merged(
        &self,
        branch: &str,
    ) -> Option<(String, std::collections::BTreeMap<String, String>)> {
        let prs: Vec<(String, PullRequest)> = self
            .prs_of(branch)
            .into_iter()
            .filter_map(|(_, target, pr)| Some((target.repo, pr?)))
            .collect();
        let reason = pom_forge::finished_reason(
            prs.iter().map(|(repo, pr)| (repo.as_str(), pr)),
            true,
            false,
        )?;
        let heads = prs
            .into_iter()
            .filter(|(_, pr)| !pr.head_ref_oid.is_empty())
            .map(|(repo, pr)| (repo, pr.head_ref_oid))
            .collect();
        Some((reason, heads))
    }

    /// The lookup and PR (if any) for the checkout at `root`; `None` while its GitHub repo is unknown.
    pub fn for_checkout(&self, root: &Path) -> Option<(PrTarget, Option<PullRequest>)> {
        let targets = self.shared.targets.lock().ok()?;
        let target = targets
            .values()
            .flatten()
            .find(|(checkout, _)| checkout == root)
            .map(|(_, target)| target.clone())?;
        drop(targets);
        let pr = self
            .shared
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&target).flatten().cloned());
        Some((target, pr))
    }

    pub fn item(&self, target: PrTarget) -> Box<dyn workspace::Item> {
        let cached = self
            .shared
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&target).flatten().cloned());
        Box::new(PrItem::new(
            self.shared.state.clone(),
            PullRequests::token_session(&self.shared),
            self.shared.waker.clone(),
            target,
            cached,
        ))
    }

    pub fn item_id(target: &PrTarget) -> String {
        item::item_id(target)
    }

    /// Makes `branch` show `prs` as if they had been looked up (previews and tests).
    pub fn show(&self, branch: &str, prs: Vec<(PathBuf, PrTarget, PullRequest)>) {
        if let Ok(mut targets) = self.shared.targets.lock() {
            targets.insert(
                branch.to_string(),
                prs.iter()
                    .map(|(root, target, _)| (root.clone(), target.clone()))
                    .collect(),
            );
        }
        if let Ok(mut cache) = self.shared.cache.lock() {
            cache.remember(
                prs.into_iter()
                    .map(|(_, target, pr)| (target, pr))
                    .collect(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(repo: &str) -> PrTarget {
        PrTarget {
            repo: repo.into(),
            owner: "acme".into(),
            name: repo.into(),
            head: "feat-login".into(),
        }
    }

    fn pr(number: u64, state: &str, checks: &str) -> PullRequest {
        PullRequest {
            number,
            state: state.into(),
            checks: checks.into(),
            ..PullRequest::default()
        }
    }

    #[test]
    fn a_workspace_sums_its_repos_prs_and_checkouts_find_theirs() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let prs = PullRequests::new(StateDir::new(temp.path()), "myproject", Arc::new(|| {}));
        assert_eq!(prs.summary("feat-login"), None);
        prs.show(
            "feat-login",
            vec![
                (
                    temp.path().join("web"),
                    target("web"),
                    pr(1, "OPEN", "pending"),
                ),
                (
                    temp.path().join("api"),
                    target("api"),
                    pr(2, "OPEN", "pass"),
                ),
            ],
        );
        assert_eq!(
            prs.summary("feat-login"),
            Some(PrSummary {
                count: 2,
                severity: PrSeverity::Warn,
                trouble: Some(PrTrouble::Pending),
            })
        );
        let found = prs.for_checkout(&temp.path().join("api"));
        assert_eq!(found.and_then(|(_, pr)| pr).map(|pr| pr.number), Some(2));
        assert!(prs.for_checkout(&temp.path().join("other")).is_none());
        let item = prs.item(target("api"));
        assert_eq!(item.title(), "#2 ");
        assert_eq!(item.id().as_deref(), Some("pr:api:feat-login"));
        assert_eq!(prs.merged("feat-login"), None);
        let mut merged = pr(3, "MERGED", "pass");
        merged.head_ref_oid = "abc123".into();
        prs.show(
            "feat-login",
            vec![(temp.path().join("api"), target("api"), merged)],
        );
        let (reason, heads) = prs.merged("feat-login").ok_or("merged")?;
        assert_eq!(reason, "api #3 merged");
        assert_eq!(heads.get("api").map(String::as_str), Some("abc123"));
        Ok(())
    }

    #[test]
    fn a_remote_window_finds_the_token_stored_for_the_project_here() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        let prs = PullRequests::with_sessions(
            state.clone(),
            vec!["myproject@dev".into(), "myproject".into()],
            Arc::new(|| {}),
        );
        let no_env = |_: &str| None;
        assert_eq!(
            PullRequests::token_session_in(&prs.shared, &no_env),
            "myproject@dev"
        );
        pom_secrets::SecretStore::new(state, "myproject")
            .set("github", "token")
            .map_err(|error| error.to_string())?;
        assert_eq!(
            PullRequests::token_session_in(&prs.shared, &no_env),
            "myproject"
        );
        Ok(())
    }

    #[test]
    fn a_github_origin_and_a_head_make_a_lookup() {
        assert_eq!(
            super::target("api", "git@github.com:acme/api.git", "feat-login"),
            Some(target("api"))
        );
        assert_eq!(
            super::target("api", "https://example.com/acme/api.git", "feat-login"),
            None
        );
    }
}
