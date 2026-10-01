//! Who drives a session. The holder of its lease sends the turns; everyone else attached only watches, and the
//! holder process itself drops their keystrokes. A session nobody leased belongs to the person using the app.
//! Every lease change and every message one session sends another goes to the workspace's log, which `watch`
//! streams.

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::caller::Caller;
use crate::gate::Refusal;
use crate::sessions::{now_ms, with_lock};

const LEASES_DIR: &str = "agents/leases";
const LOGS_DIR: &str = "agents/workspace-log";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseClass {
    #[default]
    Human,
    Orchestrator,
    Agent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub class: LeaseClass,
    /// `person`, `operator`, or the holder of the agent that holds it.
    pub by: String,
    #[serde(default)]
    pub token: String,
    pub since_ms: u64,
    /// Who held it before a person took over, to hand it back on release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<Lease>>,
}

impl Lease {
    pub fn person() -> Lease {
        Lease {
            class: LeaseClass::Human,
            by: "person".into(),
            token: String::new(),
            since_ms: now_ms(),
            previous: None,
        }
    }

    /// The lease a caller takes when it starts driving a session.
    pub fn for_caller(caller: &Caller) -> Lease {
        let (class, by) = match caller {
            Caller::Agent { holder } => (LeaseClass::Agent, holder.clone()),
            Caller::Operator => (LeaseClass::Orchestrator, "operator".to_string()),
        };
        Lease {
            class,
            by,
            token: new_token(),
            since_ms: now_ms(),
            previous: None,
        }
    }

    /// Whether `caller` may send turns to the session under this lease.
    pub fn allows(&self, caller: &Caller) -> Result<(), Refusal> {
        match (self.class, caller) {
            (LeaseClass::Human, _) => Err(Refusal::LeaseHeld {
                by: "a person".into(),
            }),
            (LeaseClass::Orchestrator, _) => Ok(()),
            (LeaseClass::Agent, Caller::Operator) => Ok(()),
            (LeaseClass::Agent, Caller::Agent { holder }) if *holder == self.by => Ok(()),
            (LeaseClass::Agent, Caller::Agent { .. }) => Err(Refusal::LeaseHeld {
                by: self.by.clone(),
            }),
        }
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 16];
    if let Err(error) = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
    {
        eprintln!("agent lease: random: {error}");
        bytes = now_ms()
            .to_le_bytes()
            .repeat(2)
            .try_into()
            .unwrap_or([7; 16]);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lease_path(state: &StateDir, holder: &str) -> std::path::PathBuf {
    state
        .path(LEASES_DIR)
        .join(format!("{}.json", holder.replace('/', "_")))
}

/// The session's lease; a person's when nobody took it.
pub fn read_lease(state: &StateDir, holder: &str) -> Lease {
    std::fs::read_to_string(lease_path(state, holder))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(Lease::person)
}

/// Moves the lease, tells the holder which client may type, and logs the change.
pub fn set_lease(
    state: &StateDir,
    holders: &SocketDir,
    project: &str,
    branch: &str,
    holder: &str,
    role: &str,
    lease: Lease,
) -> std::io::Result<Lease> {
    let before = read_lease(state, holder);
    let token = (lease.class != LeaseClass::Human).then_some(lease.token.as_str());
    holders.set_input_lease(holder, token)?;
    if lease.class == LeaseClass::Human
        && lease.previous.is_none()
        && before.class == LeaseClass::Human
    {
        if let Err(error) = std::fs::remove_file(lease_path(state, holder)) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    } else {
        let blob = serde_json::to_vec(&lease).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&lease_path(state, holder), &blob, 0o600)?;
    }
    if before.class != lease.class || before.by != lease.by {
        log(
            state,
            project,
            branch,
            json!({"event": "lease", "role": role, "holder": holder, "from": before.by, "from_class": before.class, "to": lease.by, "to_class": lease.class}),
        )?;
    }
    Ok(lease)
}

fn log_path(state: &StateDir, project: &str, branch: &str) -> std::path::PathBuf {
    state
        .path(LOGS_DIR)
        .join(project.replace('/', "_"))
        .join(format!("{}.ndjson", branch.replace('/', "_")))
}

/// Appends one line to the workspace's log: a lease change, a message between sessions, an approval.
pub fn log(state: &StateDir, project: &str, branch: &str, mut line: Value) -> std::io::Result<()> {
    line["t_ms"] = json!(now_ms());
    line["workspace"] = json!(branch);
    let mut text = line.to_string();
    text.push('\n');
    with_lock(&log_path(state, project, branch), |file| {
        std::io::Write::write_all(file, text.as_bytes())
    })
}

/// Every line of the workspace's log, oldest first.
pub fn log_lines(state: &StateDir, project: &str, branch: &str) -> Vec<Value> {
    std::fs::read_to_string(log_path(state, project, branch))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_says_who_may_drive_and_moving_it_is_logged_and_told_to_the_holder() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holders = SocketDir::new(temp.path().join("s"));
        let holder = "ws-myproject-feat-login-claude-reviewer";
        let agent = Caller::Agent {
            holder: "ws-myproject-feat-login-claude-raw".into(),
        };
        assert_eq!(read_lease(&state, holder).class, LeaseClass::Human);
        assert_eq!(
            read_lease(&state, holder)
                .allows(&Caller::Operator)
                .map_err(|refusal| refusal.code()),
            Err("lease")
        );
        assert_eq!(
            read_lease(&state, holder)
                .allows(&agent)
                .map_err(|refusal| refusal.code()),
            Err("lease")
        );

        let taken = set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            holder,
            "reviewer",
            Lease::for_caller(&Caller::Operator),
        )
        .expect("take");
        assert_eq!(
            holders.input_lease(holder).as_deref(),
            Some(taken.token.as_str())
        );
        assert_eq!(read_lease(&state, holder).allows(&agent), Ok(()));

        let mut person = Lease::person();
        person.previous = Some(Box::new(taken.clone()));
        set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            holder,
            "reviewer",
            person,
        )
        .expect("takeover");
        assert_eq!(holders.input_lease(holder), None, "a person types freely");
        let previous = read_lease(&state, holder).previous.expect("previous");
        set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            holder,
            "reviewer",
            *previous,
        )
        .expect("release");
        assert_eq!(read_lease(&state, holder).class, LeaseClass::Orchestrator);

        let changes: Vec<(String, String)> = log_lines(&state, "myproject", "feat-login")
            .iter()
            .map(|line| {
                (
                    line["from"].as_str().unwrap_or_default().to_string(),
                    line["to"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            changes,
            [
                ("person".to_string(), "operator".to_string()),
                ("operator".to_string(), "person".to_string()),
                ("person".to_string(), "operator".to_string()),
            ]
        );
    }

    #[test]
    fn an_agent_lease_lets_only_that_agent_or_an_operator_drive() {
        let mine = Caller::Agent {
            holder: "ws-myproject-feat-login-claude-raw".into(),
        };
        let other = Caller::Agent {
            holder: "ws-myproject-feat-login-fixer".into(),
        };
        let lease = Lease::for_caller(&mine);
        assert_eq!(lease.allows(&mine), Ok(()));
        assert_eq!(lease.allows(&Caller::Operator), Ok(()));
        assert_eq!(
            lease.allows(&other).map_err(|refusal| refusal.code()),
            Err("lease")
        );
    }
}
