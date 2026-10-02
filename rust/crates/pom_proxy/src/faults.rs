//! Fault rules for e2e tests: make one workspace service's requests through the dev proxy fail or slow down,
//! for a while. They live in `<state>/proxy/faults.json`, so the CLI writes them and any running proxy obeys.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One rule: requests to `repo/service` of `branch` in project `session` whose path starts with `path`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FaultRule {
    pub id: String,
    pub session: String,
    pub branch: String,
    pub repo: String,
    /// The repo's alias, which hostnames and `/_pom_dev/` paths use.
    #[serde(default)]
    pub alias: String,
    pub service: String,
    /// A path prefix; empty matches every request.
    #[serde(default)]
    pub path: String,
    /// Answer with this status instead of forwarding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Wait this long first.
    #[serde(default)]
    pub delay_ms: u64,
    /// The share of matching requests it applies to, 0 to 1.
    #[serde(default = "always")]
    pub rate: f64,
    pub expires_ms: u64,
}

fn always() -> f64 {
    1.0
}

/// What the proxy does to one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub id: String,
    pub status: Option<u16>,
    pub delay_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl FaultRule {
    /// Whether the rule covers this request; `target` is `<repo or alias>/<service>`.
    pub fn matches(&self, session: &str, branch: &str, target: &str, path: &str, now: u64) -> bool {
        let Some((repo, service)) = target.split_once('/') else {
            return false;
        };
        now < self.expires_ms
            && self.session == session
            && self.branch == branch
            && self.service == service
            && (self.repo == repo || (!self.alias.is_empty() && self.alias == repo))
            && path.starts_with(&self.path)
    }

    pub fn fault(&self) -> Fault {
        Fault {
            id: self.id.clone(),
            status: self.status,
            delay_ms: self.delay_ms,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FaultFile {
    #[serde(default)]
    rules: Vec<FaultRule>,
}

pub fn faults_path(state: &StateDir) -> PathBuf {
    state.path("proxy").join("faults.json")
}

/// The rules still in force.
pub fn load_faults(state: &StateDir) -> Vec<FaultRule> {
    let now = now_ms();
    std::fs::read_to_string(faults_path(state))
        .ok()
        .and_then(|text| serde_json::from_str::<FaultFile>(&text).ok())
        .map(|file| file.rules)
        .unwrap_or_default()
        .into_iter()
        .filter(|rule| rule.expires_ms > now)
        .collect()
}

fn save_faults(state: &StateDir, rules: Vec<FaultRule>) -> Result<(), String> {
    let path = faults_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text =
        serde_json::to_string_pretty(&FaultFile { rules }).map_err(|error| error.to_string())?;
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, text).map_err(|error| error.to_string())?;
    std::fs::rename(staged, path).map_err(|error| error.to_string())
}

/// Adds `rule` under a fresh id (expired rules are dropped on the way) and returns it.
pub fn add_fault(state: &StateDir, mut rule: FaultRule) -> Result<FaultRule, String> {
    if !(0.0..=1.0).contains(&rule.rate) {
        return Err(format!("--rate {} must be between 0 and 1", rule.rate));
    }
    if rule.status.is_none() && rule.delay_ms == 0 {
        return Err("a fault needs --status, --delay or both".into());
    }
    if rule
        .status
        .is_some_and(|status| !(100..=599).contains(&status))
    {
        return Err("--status must be an HTTP status (100-599)".into());
    }
    let seed =
        now_ms() ^ (SEQUENCE.fetch_add(1, Ordering::Relaxed) << 48) ^ u64::from(std::process::id());
    rule.id = format!("f{:06x}", seed & 0xff_ffff);
    let mut rules = load_faults(state);
    rules.push(rule.clone());
    save_faults(state, rules)?;
    Ok(rule)
}

/// Removes the rule `id`; whether there was one.
pub fn remove_fault(state: &StateDir, id: &str) -> Result<bool, String> {
    let mut rules = load_faults(state);
    let before = rules.len();
    rules.retain(|rule| rule.id != id);
    let removed = rules.len() != before;
    save_faults(state, rules)?;
    Ok(removed)
}

/// Removes every rule of one workspace; how many went.
pub fn clear_faults(state: &StateDir, session: &str, branch: &str) -> Result<usize, String> {
    let mut rules = load_faults(state);
    let before = rules.len();
    rules.retain(|rule| !(rule.session == session && rule.branch == branch));
    let cleared = before - rules.len();
    save_faults(state, rules)?;
    Ok(cleared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> FaultRule {
        FaultRule {
            session: "myproject".into(),
            branch: "feat-login".into(),
            repo: "myproject-api".into(),
            alias: "api".into(),
            service: "server".into(),
            path: "/v1/orders".into(),
            status: Some(503),
            rate: 1.0,
            expires_ms: now_ms() + 60_000,
            ..FaultRule::default()
        }
    }

    #[test]
    fn a_rule_covers_only_its_workspace_service_and_path() {
        let rule = rule();
        let now = now_ms();
        assert!(rule.matches("myproject", "feat-login", "api/server", "/v1/orders/7", now));
        assert!(rule.matches(
            "myproject",
            "feat-login",
            "myproject-api/server",
            "/v1/orders",
            now
        ));
        assert!(
            !rule.matches("myproject", "main", "api/server", "/v1/orders", now),
            "another workspace"
        );
        assert!(
            !rule.matches("other", "feat-login", "api/server", "/v1/orders", now),
            "another project"
        );
        assert!(!rule.matches("myproject", "feat-login", "api/worker", "/v1/orders", now));
        assert!(!rule.matches("myproject", "feat-login", "api/server", "/v1/users", now));
        assert!(!rule.matches(
            "myproject",
            "feat-login",
            "api/server",
            "/v1/orders",
            rule.expires_ms
        ));
    }

    #[test]
    fn rules_are_added_listed_removed_and_cleared_per_workspace() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let first = add_fault(&state, rule()).expect("add");
        let second = add_fault(
            &state,
            FaultRule {
                branch: "main".into(),
                ..rule()
            },
        )
        .expect("add");
        assert_ne!(first.id, second.id);
        assert_eq!(load_faults(&state).len(), 2);
        assert!(add_fault(
            &state,
            FaultRule {
                status: None,
                delay_ms: 0,
                ..rule()
            }
        )
        .is_err());
        assert!(add_fault(
            &state,
            FaultRule {
                rate: 2.0,
                ..rule()
            }
        )
        .is_err());
        add_fault(
            &state,
            FaultRule {
                expires_ms: 1,
                ..rule()
            },
        )
        .expect("add expired");
        assert_eq!(load_faults(&state).len(), 2, "an expired rule is gone");
        assert!(remove_fault(&state, &first.id).expect("remove"));
        assert!(!remove_fault(&state, &first.id).expect("remove again"));
        assert_eq!(clear_faults(&state, "myproject", "main").expect("clear"), 1);
        assert!(load_faults(&state).is_empty());
    }
}
