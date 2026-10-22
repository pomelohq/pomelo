//! Agents of a project on another machine: their tabs in the agent dock with the lease bar, the role a new one
//! gets, and which tabs were open, kept per host and project so they come back attached.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pom_agent::SessionView;
use pom_remote::Watch;
use terminal_ui::{LeaseSource, LeaseState};

/// The lease of agent `role` of the watched workspace, read from its pushes and moved through its server.
pub struct RemoteLease {
    watch: Watch,
    role: String,
}

impl RemoteLease {
    pub fn new(watch: Watch, role: &str) -> RemoteLease {
        RemoteLease {
            watch,
            role: role.to_string(),
        }
    }

    fn holder(&self) -> Option<String> {
        self.watch
            .snapshot()
            .agents
            .into_iter()
            .find(|agent| agent.role == self.role)
            .map(|agent| agent.holder)
    }
}

impl LeaseSource for RemoteLease {
    fn state(&self) -> LeaseState {
        let presence = self
            .holder()
            .and_then(|holder| self.watch.presence(&holder));
        LeaseState {
            driver: presence.as_ref().and_then(|known| known.driven_by.clone()),
            pending: presence.and_then(|known| known.pending),
        }
    }

    fn take_over(&self) -> Result<(), String> {
        let holder = self
            .holder()
            .ok_or_else(|| format!("agent {} is not running", self.role))?;
        self.watch.take_over(&holder);
        Ok(())
    }

    fn answer(&self, request: &str, allow: bool) -> Result<(), String> {
        let holder = self
            .holder()
            .ok_or_else(|| format!("agent {} is not running", self.role))?;
        self.watch.answer(&holder, request, allow);
        Ok(())
    }
}

pub fn agent_item_id(branch: &str, role: &str) -> String {
    format!("agent:{branch}/{role}")
}

/// The tab of agent `role` running on the project's machine, attached through its server, with the lease bar.
pub fn remote_agent_item(
    watch: &Watch,
    role: &str,
    number: u64,
) -> Option<Box<dyn workspace::Item>> {
    let provider = watch.terminals()?;
    let snapshot = watch.snapshot();
    let root = snapshot
        .workspaces
        .as_ref()
        .and_then(|list| {
            list.workspaces
                .iter()
                .find(|workspace| workspace.branch == snapshot.branch)
        })
        .map(|workspace| workspace.path.clone())
        .unwrap_or_default();
    let remote = terminal::RemoteOptions {
        provider,
        target: terminal::RemoteTarget::Agent(role.to_string()),
        mode: terminal::RemoteMode::ReadWrite,
    };
    match terminal_ui::TerminalItem::remote_console(
        number,
        root,
        agent_item_id(&snapshot.branch, role),
        role.to_string(),
        remote,
        true,
        std::sync::Arc::new(ui::wake),
    ) {
        Ok(item) => Some(Box::new(
            item.with_lease(Box::new(RemoteLease::new(watch.clone(), role))),
        )),
        Err(error) => {
            eprintln!("remote agent {role}: {error}");
            None
        }
    }
}

/// The agent tabs saved for the watched workspace, each attached again to its agent; an agent that no longer
/// runs gets no tab. Item numbers come from `next_number`.
pub fn restore_agent_tabs(
    watch: &Watch,
    tabs: &RemoteAgentTabs,
    host: &str,
    project: &str,
    next_number: &mut u64,
) -> Vec<(String, Box<dyn workspace::Item>)> {
    let branch = watch.snapshot().branch;
    tabs.roles(host, project, &branch)
        .into_iter()
        .filter_map(|role| {
            let number = *next_number;
            *next_number += 1;
            let item = remote_agent_item(watch, &role, number)?;
            Some((agent_item_id(&branch, &role), item))
        })
        .collect()
}

/// The workspace's main agent when it does not run, else the first free `agent-<n>`.
pub fn next_agent_role(agents: &[SessionView]) -> String {
    let running = |role: &str| agents.iter().any(|agent| agent.alive && agent.role == role);
    if !running(pom_agent::CLAUDE_DRIVER) {
        return pom_agent::CLAUDE_DRIVER.to_string();
    }
    (2..)
        .map(|number| format!("agent-{number}"))
        .find(|role| !agents.iter().any(|agent| agent.role == *role))
        .unwrap_or_else(|| "agent".to_string())
}

/// Open agent tabs of remote projects by `host/project`, then workspace, as their roles.
pub struct RemoteAgentTabs {
    path: PathBuf,
}

type Saved = BTreeMap<String, BTreeMap<String, Vec<String>>>;

impl RemoteAgentTabs {
    pub fn new(path: PathBuf) -> RemoteAgentTabs {
        RemoteAgentTabs { path }
    }

    /// The file under the app's config dir.
    pub fn default_location() -> Option<RemoteAgentTabs> {
        pom_paths::config_dir().map(|dir| Self::new(dir.join("remote-agents.json")))
    }

    fn key(host: &str, project: &str) -> String {
        format!("{host}/{project}")
    }

    fn load(&self) -> Saved {
        read(&self.path)
    }

    pub fn roles(&self, host: &str, project: &str, branch: &str) -> Vec<String> {
        self.load()
            .get(&Self::key(host, project))
            .and_then(|branches| branches.get(branch))
            .cloned()
            .unwrap_or_default()
    }

    pub fn save(
        &self,
        host: &str,
        project: &str,
        branch: &str,
        roles: &[String],
    ) -> std::io::Result<()> {
        let mut saved = self.load();
        let branches = saved.entry(Self::key(host, project)).or_default();
        if roles.is_empty() {
            branches.remove(branch);
        } else {
            branches.insert(branch.to_string(), roles.to_vec());
        }
        saved.retain(|_, branches| !branches.is_empty());
        let bytes = serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&self.path, &bytes, 0o644)
    }
}

fn read(path: &Path) -> Saved {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_agent_tabs_are_kept_per_host_project_and_workspace() {
        let temp = tempfile::tempdir().expect("temp");
        let tabs = RemoteAgentTabs::new(temp.path().join("remote-agents.json"));
        assert!(tabs.roles("dev", "myproject", "feat-login").is_empty());
        let roles = vec!["claude".to_string(), "agent-2".to_string()];
        tabs.save("dev", "myproject", "feat-login", &roles)
            .expect("save");
        tabs.save("lab", "myproject", "feat-login", &["claude".into()])
            .expect("save");
        assert_eq!(tabs.roles("dev", "myproject", "feat-login"), roles);
        assert!(tabs.roles("dev", "myproject", "main").is_empty());
        assert_eq!(tabs.roles("lab", "myproject", "feat-login"), ["claude"]);
        tabs.save("dev", "myproject", "feat-login", &[])
            .expect("save");
        assert!(tabs.roles("dev", "myproject", "feat-login").is_empty());
        assert_eq!(tabs.roles("lab", "myproject", "feat-login"), ["claude"]);
    }

    #[test]
    fn a_new_agent_is_the_main_one_first_then_numbered() {
        let agent = |role: &str, alive: bool| SessionView {
            role: role.into(),
            alive,
            ..SessionView::default()
        };
        assert_eq!(next_agent_role(&[]), "claude");
        assert_eq!(next_agent_role(&[agent("claude", false)]), "claude");
        assert_eq!(next_agent_role(&[agent("claude", true)]), "agent-2");
        assert_eq!(
            next_agent_role(&[agent("claude", true), agent("agent-2", false)]),
            "agent-3"
        );
    }
}
