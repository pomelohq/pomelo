//! Who is asking to drive a session. An agent runs inside its holder's process tree, so walking a process's
//! parents until one belongs to an agent holder tells an agent from a person or an orchestrator, whatever its
//! environment claims.

use pom_ptyhost::{ancestors, descendants, SocketDir};

use crate::identity::looks_like_agent_holder;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// A process inside an agent holder: that agent session.
    Agent { holder: String },
    /// Anything else: a person at a terminal, a script, an orchestrator.
    Operator,
}

impl Caller {
    pub fn is_agent(&self) -> bool {
        matches!(self, Caller::Agent { .. })
    }

    pub fn holder(&self) -> Option<&str> {
        match self {
            Caller::Agent { holder } => Some(holder),
            Caller::Operator => None,
        }
    }

    /// The caller of `pid`, given the live holders as (name, pid).
    pub fn of_process(pid: i32, holders: &[(String, i32)]) -> Caller {
        let chain = ancestors(pid);
        holders
            .iter()
            .filter(|(name, _)| looks_like_agent_holder(name))
            .find(|(_, holder_pid)| {
                let tree = descendants(*holder_pid);
                chain.iter().any(|ancestor| tree.contains(ancestor))
            })
            .map_or(Caller::Operator, |(name, _)| Caller::Agent {
                holder: name.clone(),
            })
    }

    /// The caller of this process.
    pub fn current(holders: &SocketDir) -> Caller {
        Caller::of_process(std::process::id() as i32, &holders.holders())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_under_an_agent_holder_is_that_agent_and_anything_else_an_operator() {
        let me = std::process::id() as i32;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("sleep");
        let child_pid = child.id() as i32;
        let holders = vec![
            ("ws-myproject-main-gateway".to_string(), me),
            ("ws-myproject-feat-login-claude-raw".to_string(), child_pid),
        ];
        assert_eq!(
            Caller::of_process(child_pid, &holders),
            Caller::Agent {
                holder: "ws-myproject-feat-login-claude-raw".into()
            }
        );
        assert_eq!(
            Caller::of_process(me, &holders),
            Caller::Operator,
            "a service holder's tree does not make an agent"
        );
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn spoofed_identity_env_does_not_change_the_caller() {
        let me = std::process::id() as i32;
        std::env::set_var(
            crate::identity::HOLDER_ENV,
            "ws-myproject-feat-login-claude-raw",
        );
        std::env::set_var(crate::identity::WORKSPACE_ENV, "other");
        let caller = Caller::of_process(me, &[]);
        std::env::remove_var(crate::identity::HOLDER_ENV);
        std::env::remove_var(crate::identity::WORKSPACE_ENV);
        assert_eq!(caller, Caller::Operator);
    }
}
