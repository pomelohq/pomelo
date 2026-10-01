//! Every rule about who may drive which agent session, in one place: the `pom agent` commands and the MCP
//! tools both ask here and nowhere else. A session belongs to one workspace, and an agent may only reach the
//! other sessions of its own; people and orchestrators get their own, looser limits.

use std::time::Duration;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

use crate::caller::Caller;
use crate::identity::{holder_role, workspace_prefix, HolderRole};
use crate::sessions::{now_ms, with_lock};

/// Exit status of a command the gate refused.
pub const REFUSED_EXIT: i32 = 6;
const CALLERS_DIR: &str = "agents/callers";
const HOUR_MS: u64 = 60 * 60 * 1000;

/// One workspace (one ticket) of a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub project: String,
    pub branch: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub min_interval: Duration,
    pub per_hour: u32,
    pub max_depth: u64,
    /// The longest a single wait may block; `None` for no limit.
    pub wait_cap: Option<Duration>,
}

impl Limits {
    /// What an agent inside a session may do.
    pub const AGENT: Limits = Limits {
        min_interval: Duration::from_secs(5),
        per_hour: 20,
        max_depth: 2,
        wait_cap: Some(Duration::from_secs(10 * 60)),
    };

    /// People and orchestrators: no limits unless configured.
    pub const OPERATOR: Limits = Limits {
        min_interval: Duration::ZERO,
        per_hour: 0,
        max_depth: u64::MAX,
        wait_cap: None,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApproveScope {
    Once,
    Always,
    Deny,
}

/// Why the gate said no; `code` is stable for scripts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    OtherWorkspace { allowed: String },
    NotAnAgent { holder: String },
    NotDrivable { role: String },
    SelfTarget,
    TooDeep { max: u64 },
    TooSoon { wait_ms: u64 },
    HourlyLimit { per_hour: u32 },
    Busy { state: String },
    QueueNotAllowed,
    WaitTooLong { cap_s: u64 },
    ApproveAlwaysNotAllowed,
    LeaseHeld { by: String },
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::OtherWorkspace { .. } => "other_workspace",
            Refusal::NotAnAgent { .. } => "not_an_agent",
            Refusal::NotDrivable { .. } => "not_drivable",
            Refusal::SelfTarget => "self",
            Refusal::TooDeep { .. } => "depth",
            Refusal::TooSoon { .. } => "rate",
            Refusal::HourlyLimit { .. } => "rate_hour",
            Refusal::Busy { .. } => "busy",
            Refusal::QueueNotAllowed => "queue",
            Refusal::WaitTooLong { .. } => "wait_cap",
            Refusal::ApproveAlwaysNotAllowed => "approve_always",
            Refusal::LeaseHeld { .. } => "lease",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::OtherWorkspace { allowed } => write!(
                formatter,
                "agents can only reach sessions of workspace {allowed}"
            ),
            Refusal::NotAnAgent { holder } => {
                write!(
                    formatter,
                    "{holder} is not an agent session of this workspace"
                )
            }
            Refusal::NotDrivable { role } => write!(
                formatter,
                "the {role} session cannot be driven (no driver for it, or it is the onboarder)"
            ),
            Refusal::SelfTarget => write!(formatter, "a session cannot drive itself"),
            Refusal::TooDeep { max } => write!(
                formatter,
                "too many agents in a row: a chain of agent requests stops at depth {max}"
            ),
            Refusal::TooSoon { wait_ms } => write!(
                formatter,
                "sending too fast: wait {:.1} s before the next send",
                *wait_ms as f64 / 1000.0
            ),
            Refusal::HourlyLimit { per_hour } => {
                write!(
                    formatter,
                    "the limit of {per_hour} sends per hour is reached"
                )
            }
            Refusal::Busy { state } => write!(
                formatter,
                "the session is busy ({state}); send again once it is idle"
            ),
            Refusal::QueueNotAllowed => {
                write!(
                    formatter,
                    "only a person or an orchestrator may queue a turn"
                )
            }
            Refusal::WaitTooLong { cap_s } => write!(
                formatter,
                "an agent waits at most {} min at a time; wait again to keep waiting",
                cap_s / 60
            ),
            Refusal::ApproveAlwaysNotAllowed => write!(
                formatter,
                "an agent may only approve once; \"always\" is for a person"
            ),
            Refusal::LeaseHeld { by } => {
                write!(formatter, "{by} drives this session; take over first")
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct RateLog {
    sends_ms: Vec<u64>,
}

/// The rules for one caller acting on one workspace.
pub struct Gate<'a> {
    state: &'a StateDir,
    caller: Caller,
    workspace: Workspace,
    limits: Limits,
}

impl<'a> Gate<'a> {
    /// Opens the gate for `caller` on `workspace`; an agent of another workspace is refused right away.
    pub fn open(
        state: &'a StateDir,
        caller: Caller,
        workspace: Workspace,
        operator_limits: Limits,
    ) -> Result<Gate<'a>, Refusal> {
        let limits = match &caller {
            Caller::Agent { holder } => {
                if !holder.starts_with(&workspace_prefix(&workspace.project, &workspace.branch))
                    || holder_role(holder, &workspace.project, &workspace.branch).is_none()
                {
                    return Err(Refusal::OtherWorkspace {
                        allowed: own_workspace_hint(holder),
                    });
                }
                Limits::AGENT
            }
            Caller::Operator => operator_limits,
        };
        Ok(Gate {
            state,
            caller,
            workspace,
            limits,
        })
    }

    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The role of a holder this caller may act on: an agent session of this workspace.
    pub fn target(&self, holder: &str) -> Result<HolderRole, Refusal> {
        if !holder.starts_with(&workspace_prefix(
            &self.workspace.project,
            &self.workspace.branch,
        )) {
            return Err(Refusal::OtherWorkspace {
                allowed: self.workspace.branch.clone(),
            });
        }
        holder_role(holder, &self.workspace.project, &self.workspace.branch).ok_or_else(|| {
            Refusal::NotAnAgent {
                holder: holder.to_string(),
            }
        })
    }

    /// A session this caller may send turns to or approve for.
    pub fn drivable_target(&self, holder: &str) -> Result<HolderRole, Refusal> {
        let role = self.target(holder)?;
        if !role.drivable {
            return Err(Refusal::NotDrivable { role: role.role });
        }
        if self.caller.holder() == Some(holder) {
            return Err(Refusal::SelfTarget);
        }
        Ok(role)
    }

    /// Whether a turn may go to `holder` now, and at which depth. `caller_depth` is the depth of the turn the
    /// calling agent is in (0 for one a person sent). Records the send against the caller's rate.
    pub fn send(
        &self,
        holder: &str,
        target_state: &str,
        queue: bool,
        caller_depth: u64,
    ) -> Result<u64, Refusal> {
        self.drivable_target(holder)?;
        let agent = self.caller.is_agent();
        if queue && agent {
            return Err(Refusal::QueueNotAllowed);
        }
        if target_state != "idle" && !queue {
            return Err(Refusal::Busy {
                state: target_state.to_string(),
            });
        }
        let depth = if agent { caller_depth + 1 } else { 0 };
        if depth > self.limits.max_depth {
            return Err(Refusal::TooDeep {
                max: self.limits.max_depth,
            });
        }
        self.record_send()?;
        Ok(depth)
    }

    /// How long a wait may block: an agent's is capped, a person's is what they asked.
    pub fn wait(&self, requested: Option<Duration>) -> Result<Option<Duration>, Refusal> {
        match (self.limits.wait_cap, requested) {
            (Some(cap), Some(requested)) if requested > cap => Err(Refusal::WaitTooLong {
                cap_s: cap.as_secs(),
            }),
            (Some(cap), None) => Ok(Some(cap)),
            (_, requested) => Ok(requested),
        }
    }

    /// Whether this caller may answer a permission request of `holder` with `scope`.
    pub fn approve(&self, holder: &str, scope: ApproveScope) -> Result<(), Refusal> {
        self.drivable_target(holder)?;
        if self.caller.is_agent() && scope == ApproveScope::Always {
            return Err(Refusal::ApproveAlwaysNotAllowed);
        }
        Ok(())
    }

    fn record_send(&self) -> Result<(), Refusal> {
        let limits = self.limits;
        if limits.min_interval.is_zero() && limits.per_hour == 0 {
            return Ok(());
        }
        let key = self.caller.holder().unwrap_or("operator").replace('/', "_");
        let path = self.state.path(CALLERS_DIR).join(key).join("rate.json");
        let outcome = with_lock(&path.with_extension("lock"), |_| {
            let mut log: RateLog = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            let now = now_ms();
            log.sends_ms
                .retain(|sent| now.saturating_sub(*sent) < HOUR_MS);
            let interval = limits.min_interval.as_millis() as u64;
            if let Some(last) = log.sends_ms.last() {
                let since = now.saturating_sub(*last);
                if since < interval {
                    return Ok(Err(Refusal::TooSoon {
                        wait_ms: interval - since,
                    }));
                }
            }
            if limits.per_hour > 0 && log.sends_ms.len() as u32 >= limits.per_hour {
                return Ok(Err(Refusal::HourlyLimit {
                    per_hour: limits.per_hour,
                }));
            }
            log.sends_ms.push(now);
            let blob = serde_json::to_vec(&log).map_err(std::io::Error::other)?;
            pom_paths::write_atomic(&path, &blob, 0o644)?;
            Ok(Ok(()))
        });
        match outcome {
            Ok(result) => result,
            Err(error) => {
                eprintln!("agent gate: rate log: {error}");
                Ok(())
            }
        }
    }
}

/// The workspace part of an agent holder's name, for the refusal message.
fn own_workspace_hint(holder: &str) -> String {
    let rest = holder.strip_prefix("ws-").unwrap_or(holder);
    rest.match_indices('-')
        .find(|(at, _)| crate::identity::role_of_kind(&rest[at + 1..]).is_some())
        .map_or_else(|| "its own".to_string(), |(at, _)| rest[..at].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "ws-myproject-feat-login-claude-raw";
    const REVIEWER: &str = "ws-myproject-feat-login-claude-reviewer";
    const OTHER: &str = "ws-myproject-feat-signup-claude-raw";

    fn workspace(branch: &str) -> Workspace {
        Workspace {
            project: "myproject".into(),
            branch: branch.into(),
        }
    }

    fn agent(holder: &str) -> Caller {
        Caller::Agent {
            holder: holder.into(),
        }
    }

    fn state() -> (tempfile::TempDir, StateDir) {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        (temp, state)
    }

    fn refused<T: std::fmt::Debug>(result: Result<T, Refusal>) -> &'static str {
        result.expect_err("refused").code()
    }

    #[test]
    fn an_agent_cannot_open_another_workspace() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-signup"),
            Limits::OPERATOR,
        );
        let refusal = gate.err().expect("refused");
        assert_eq!(refusal.code(), "other_workspace");
        assert_eq!(
            refusal.to_string(),
            "agents can only reach sessions of workspace myproject-feat-login"
        );
    }

    #[test]
    fn an_agent_cannot_reach_a_session_of_another_workspace() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("own workspace");
        assert_eq!(refused(gate.target(OTHER)), "other_workspace");
        assert_eq!(
            refused(gate.send(OTHER, "idle", false, 0)),
            "other_workspace"
        );
        assert_eq!(
            refused(gate.approve(OTHER, ApproveScope::Once)),
            "other_workspace"
        );
    }

    #[test]
    fn a_service_holder_is_not_an_agent_target() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            Caller::Operator,
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(
            refused(gate.target("ws-myproject-feat-login-gateway")),
            "not_an_agent"
        );
        assert_eq!(
            refused(gate.send("ws-myproject-feat-login-onboarder", "idle", false, 0)),
            "not_drivable"
        );
        assert_eq!(
            refused(gate.send("ws-myproject-feat-login-agent-codex", "idle", false, 0)),
            "not_drivable"
        );
    }

    #[test]
    fn an_agent_cannot_send_to_itself() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(refused(gate.send(MAIN, "idle", false, 0)), "self");
        assert_eq!(refused(gate.approve(MAIN, ApproveScope::Once)), "self");
    }

    #[test]
    fn a_chain_of_agents_stops_at_depth_two() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(gate.send(REVIEWER, "idle", false, 0), Ok(1));
        let (_temp, state) = self::state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(gate.send(REVIEWER, "idle", false, 1), Ok(2));
        let (_temp, state) = self::state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(refused(gate.send(REVIEWER, "idle", false, 2)), "depth");
    }

    #[test]
    fn an_agent_sends_at_most_once_per_five_seconds() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(gate.send(REVIEWER, "idle", false, 0), Ok(1));
        assert_eq!(refused(gate.send(REVIEWER, "idle", false, 0)), "rate");
    }

    #[test]
    fn an_agent_sends_at_most_twenty_times_per_hour() {
        let (_temp, state) = state();
        let path = state.path(CALLERS_DIR).join(MAIN).join("rate.json");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        let past = now_ms() - 10 * 60 * 1000;
        let log = RateLog {
            sends_ms: (0..20).map(|index| past + index).collect(),
        };
        std::fs::write(&path, serde_json::to_vec(&log).expect("json")).expect("log");
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(refused(gate.send(REVIEWER, "idle", false, 0)), "rate_hour");
    }

    #[test]
    fn a_busy_session_refuses_an_agent_and_only_an_operator_may_queue() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(refused(gate.send(REVIEWER, "thinking", false, 0)), "busy");
        assert_eq!(refused(gate.send(REVIEWER, "thinking", true, 0)), "queue");
        let operator = Gate::open(
            &state,
            Caller::Operator,
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(
            refused(operator.send(REVIEWER, "thinking", false, 0)),
            "busy"
        );
        assert_eq!(operator.send(REVIEWER, "thinking", true, 0), Ok(0));
        assert_eq!(
            operator.send(MAIN, "idle", false, 0),
            Ok(0),
            "no rate for operators by default"
        );
    }

    #[test]
    fn an_agent_waits_ten_minutes_at_most_and_an_operator_as_long_as_it_likes() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(gate.wait(None), Ok(Some(Duration::from_secs(600))));
        assert_eq!(
            refused(gate.wait(Some(Duration::from_secs(601)))),
            "wait_cap"
        );
        let operator = Gate::open(
            &state,
            Caller::Operator,
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(operator.wait(None), Ok(None));
        assert_eq!(
            operator.wait(Some(Duration::from_secs(3600))),
            Ok(Some(Duration::from_secs(3600)))
        );
    }

    #[test]
    fn an_agent_may_approve_once_but_never_always() {
        let (_temp, state) = state();
        let gate = Gate::open(
            &state,
            agent(MAIN),
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(gate.approve(REVIEWER, ApproveScope::Once), Ok(()));
        assert_eq!(gate.approve(REVIEWER, ApproveScope::Deny), Ok(()));
        assert_eq!(
            refused(gate.approve(REVIEWER, ApproveScope::Always)),
            "approve_always"
        );
        let operator = Gate::open(
            &state,
            Caller::Operator,
            workspace("feat-login"),
            Limits::OPERATOR,
        )
        .expect("gate");
        assert_eq!(operator.approve(REVIEWER, ApproveScope::Always), Ok(()));
    }
}
