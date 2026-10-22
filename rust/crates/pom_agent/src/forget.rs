//! What a deleted workspace leaves of its agents: their sessions, the workspace's index, leases and log.

use std::path::{Path, PathBuf};

use pom_paths::StateDir;

use crate::identity::holder_role;
use crate::lease::{lease_path, log_path, LEASES_DIR};
use crate::sessions::{index_path, session_dir, workspace_sessions, SESSIONS_DIR, STATE_FILE};

/// Removes the agent state of workspace `branch` of `project`; `other_branches` are the project's other
/// workspaces, whose holders may share this one's name prefix (`feat` vs `feat-x`). Returns what it could not
/// remove.
pub fn forget_workspace(
    state: &StateDir,
    project: &str,
    branch: &str,
    other_branches: &[String],
) -> Vec<String> {
    let indexed = workspace_sessions(state, project, branch);
    let mut doomed: Vec<PathBuf> = Vec::new();
    let mut holders: Vec<String> = indexed.iter().map(|entry| entry.holder.clone()).collect();
    for entry in &indexed {
        doomed.push(session_dir(state, &entry.session_id));
    }
    for dir in children(&state.path(SESSIONS_DIR)) {
        // Read loosely: a state file written by an older version still names its workspace.
        let Some(session) = std::fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        else {
            continue;
        };
        let field = |name: &str| session[name].as_str().unwrap_or_default().to_string();
        if field("project") == project && field("branch") == branch {
            holders.push(field("holder"));
            doomed.push(dir);
        }
    }
    for lease in children(&state.path(LEASES_DIR)) {
        let Some(holder) = lease
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if owns_holder(project, branch, other_branches, &holder) {
            holders.push(holder);
        }
    }
    holders.sort();
    holders.dedup();
    doomed.extend(holders.iter().map(|holder| lease_path(state, holder)));
    let index = index_path(state, project, branch);
    doomed.push(index.with_extension("lock"));
    doomed.push(index);
    let log = log_path(state, project, branch);
    doomed.push(log.with_extension("ndjson.lock"));
    doomed.push(log);
    doomed.sort();
    doomed.dedup();
    doomed
        .iter()
        .filter_map(|path| remove(path).err())
        .collect()
}

fn owns_holder(project: &str, branch: &str, other_branches: &[String], holder: &str) -> bool {
    holder_role(holder, project, branch).is_some()
        && !other_branches.iter().any(|other| {
            other.len() > branch.len()
                && other.starts_with(branch)
                && holder_role(holder, project, other).is_some()
        })
}

fn children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn remove(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::AgentState;
    use crate::identity::Identity;
    use crate::lease::{log, Lease};
    use crate::sessions::{record_event, SessionEvent};

    fn record(state: &StateDir, branch: &str, session: &str, kind: &str) -> String {
        let holder = format!("ws-myproject-{branch}-{kind}");
        let identity = Identity {
            holder: holder.clone(),
            role: "claude".into(),
            project: "myproject".into(),
            branch: branch.into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: session.into(),
            event: "SessionStart".into(),
            state: Some(AgentState::Idle),
            ..SessionEvent::default()
        };
        record_event(state, &identity, &event).expect("event");
        let lease = serde_json::to_vec(&Lease::person()).expect("lease");
        let path = lease_path(state, &holder);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(path, lease).expect("lease");
        log(
            state,
            "myproject",
            branch,
            serde_json::json!({"event": "lease"}),
        )
        .expect("log");
        holder
    }

    fn files(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for path in children(&dir) {
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(
                        path.strip_prefix(root)
                            .expect("under root")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn forgetting_a_workspace_leaves_its_neighbour_with_a_longer_name_alone() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        record(&state, "feat", "one", "claude-raw");
        record(&state, "feat", "two", "claude-reviewer");
        let kept = record(&state, "feat-x", "three", "claude-raw");
        let errors = forget_workspace(&state, "myproject", "feat", &["feat-x".into()]);
        assert!(errors.is_empty(), "{errors:?}");
        let left = files(temp.path());
        assert!(
            left.iter().all(|file| !file.contains("/one/")
                && !file.contains("/two/")
                && !file.ends_with("/feat.json")
                && !file.ends_with("/feat.ndjson")
                && !file.contains("feat-claude")),
            "{left:?}"
        );
        assert!(
            left.contains(&format!("agents/leases/{kept}.json")),
            "{left:?}"
        );
        assert!(
            left.contains(&"agents/sessions/three/state.json".to_string()),
            "{left:?}"
        );
        assert!(
            left.contains(&"agents/workspace-log/myproject/feat-x.ndjson".to_string()),
            "{left:?}"
        );
    }
}
