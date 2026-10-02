use std::collections::HashSet;
use std::path::Path;

use pom_config::Config;
use pom_layout::WorkspaceState;
use pom_paths::StateDir;
use pom_ports::Lease;
use pom_ptyhost::SocketDir;

use crate::routing::{Machine, ProjectRoute};

/// The real lease files, holders and processes.
pub struct SystemMachine {
    pub state: StateDir,
    pub holders: SocketDir,
}

impl Machine for SystemMachine {
    fn branches(&self, project: &ProjectRoute, config: &Config) -> Vec<String> {
        let known: HashSet<String> = config.repos.keys().cloned().collect();
        pom_layout::scan(&project.root, config.global_default_branch(), Some(&known))
            .into_iter()
            .map(|workspace| workspace.branch)
            .collect()
    }

    fn leases(&self, session: &str) -> Vec<Lease> {
        pom_ports::scan_leases(&self.state.path("ports.d"))
            .into_iter()
            .filter(|lease| lease.session == session)
            .collect()
    }

    fn live_ports(&self, holder: &str) -> Vec<u16> {
        self.holders
            .holder_pid(holder)
            .map(listening_ports_in_tree)
            .unwrap_or_default()
    }

    fn holder_alive(&self, holder: &str) -> bool {
        self.holders.holder_alive(holder)
    }

    fn faults(&self) -> Vec<crate::FaultRule> {
        crate::load_faults(&self.state)
    }

    fn service_envs(&self, project_root: &Path, branch: &str) -> WorkspaceState {
        WorkspaceState::load(&pom_layout::workspace_folder(project_root, branch))
    }
}

/// Every TCP port the process or any of its descendants listens on, on any address.
pub fn listening_ports_in_tree(root_pid: i32) -> Vec<u16> {
    let pids: Vec<String> = pom_ptyhost::descendants(root_pid)
        .iter()
        .map(i32::to_string)
        .collect();
    let output = std::process::Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &pids.join(","),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ])
        .output();
    match output {
        Ok(output) => listening_ports(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => {
            eprintln!("dev-proxy: lsof: {error}");
            Vec::new()
        }
    }
}

fn listening_ports(lsof: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = Vec::new();
    for port in lsof
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter_map(|address| address.rsplit_once(':')?.1.parse().ok())
    {
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_names_yield_their_ports() {
        assert_eq!(
            listening_ports("p42\nf12\nn127.0.0.1:5173\nf13\nn[::1]:24678\nn*:8080\nn[::1]:5173\n"),
            vec![5173, 24678, 8080]
        );
    }
}
