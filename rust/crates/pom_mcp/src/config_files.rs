//! Reading and editing the project's config on an agent's behalf. Every write is validated first and a
//! rejected edit leaves the files untouched.

use std::path::{Path, PathBuf};

use pom_config::yaml_node::{self, Node, NodeKind};
use pom_config::Config;

/// The config text, after folding a legacy `pom.d/` into it.
pub fn read(config_path: &Path) -> Result<String, String> {
    pom_config::migrate_fragments(config_path).map_err(|error| error.to_string())?;
    std::fs::read_to_string(config_path).map_err(|error| error.to_string())
}

fn parse(yaml: &str) -> Result<Option<Node>, String> {
    yaml_node::parse(yaml).map_err(|error| format!("yaml parse error: {error}"))
}

/// A whole `pom.yml` checked on its own: parses, loads, validates, and uses no removed keys.
pub fn validate_text(yaml: &str) -> Result<(), String> {
    let document = parse(yaml)?;
    let temp = tempdir()?;
    let path = temp.join("pom.yml");
    let result = std::fs::write(&path, yaml)
        .map_err(|error| error.to_string())
        .and_then(|_| load_and_validate(&path));
    remove_dir(&temp);
    result?;
    let removed = document
        .map(|root| pom_config::maintain::removed_keys(&root))
        .unwrap_or_default();
    if !removed.is_empty() {
        return Err(format!(
            "removed/unsupported keys - delete them (or run config_normalize): {}",
            removed.join(", ")
        ));
    }
    Ok(())
}

pub const AGENTS_KEY: &str = "agents";

/// Writes a validated `pom.yml`, refusing any change to the `agents` section: the agent policy lives there,
/// and an agent must not be able to loosen the rules it runs under.
pub fn write_root(config_path: &Path, yaml: &str) -> Result<(), String> {
    validate_text(yaml)?;
    let proposed = parse(yaml)?.and_then(|root| root.get(AGENTS_KEY).cloned());
    let current = current_agents(config_path)?;
    if !same_value(current.as_ref(), proposed.as_ref()) {
        return Err(format!(
            "the `{AGENTS_KEY}` section sets the policy agents run under, so an agent may not change it; \
             keep it exactly as it is, and ask the person to edit it in pom.yml themselves"
        ));
    }
    std::fs::write(config_path, yaml).map_err(|error| error.to_string())
}

fn current_agents(config_path: &Path) -> Result<Option<Node>, String> {
    if !config_path.exists() {
        return Ok(None);
    }
    let root = pom_config::document(config_path).map_err(|error| {
        format!(
            "the current pom.yml cannot be read, so its `{AGENTS_KEY}` policy is unknown; \
             ask the person to fix it themselves ({error})"
        )
    })?;
    Ok(root.get(AGENTS_KEY).cloned())
}

/// Equal as values: source lines and quoting do not count, and a missing section equals an empty one.
fn same_value(left: Option<&Node>, right: Option<&Node>) -> bool {
    let empty = |node: Option<&Node>| {
        node.is_none_or(|node| node.is_null() || node.entries().is_some_and(<[_]>::is_empty))
    };
    match (left, right) {
        (Some(left), Some(right)) => match (&left.kind, &right.kind) {
            (NodeKind::Scalar { .. }, NodeKind::Scalar { .. }) => {
                left.is_null() == right.is_null() && left.text() == right.text()
            }
            (NodeKind::Sequence(left), NodeKind::Sequence(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| same_value(Some(left), Some(right)))
            }
            (NodeKind::Mapping(left), NodeKind::Mapping(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.iter().any(|(other_key, other_value)| {
                            key.text() == other_key.text()
                                && same_value(Some(value), Some(other_value))
                        })
                    })
            }
            _ => empty(Some(left)) && empty(Some(right)),
        },
        _ => empty(left) && empty(right),
    }
}

fn load_and_validate(path: &Path) -> Result<(), String> {
    Config::load(path)
        .map_err(|error| error.message)?
        .validate()
}

fn tempdir() -> Result<PathBuf, String> {
    // The clock alone repeats on macOS (microsecond steps), so two checks at once shared a folder.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = format!(
        "pom-cfg-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn remove_dir(dir: &Path) {
    if let Err(error) = std::fs::remove_dir_all(dir) {
        eprintln!("mcp: remove {}: {error}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "session: demo\nrepos:\n  api:\n    services:\n      web: rails s\n";

    #[test]
    fn validation_catches_syntax_references_and_removed_keys() {
        assert!(validate_text(VALID).is_ok());
        assert!(validate_text("session: [unclosed")
            .expect_err("syntax")
            .starts_with("yaml parse error"));
        let removed = validate_text(&format!("{VALID}proxy:\n  port: 1\n")).expect_err("proxy");
        assert!(removed.contains("proxy"), "{removed}");
    }

    #[test]
    fn a_rejected_write_leaves_the_config_alone() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("pom.yml");
        std::fs::write(&root, VALID).expect("root");
        let broken = "repos:\n  api:\n    profiles: [staging]\n    services:\n      web: rails s\n";
        assert!(write_root(&root, broken).is_err());
        assert_eq!(read(&root).expect("read"), VALID);
        let fixed = VALID.replace("rails s", "rails server");
        write_root(&root, &fixed).expect("write");
        assert_eq!(read(&root).expect("read"), fixed);
    }

    #[test]
    fn an_agent_cannot_add_change_or_drop_the_agents_policy() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("pom.yml");
        std::fs::write(&root, VALID).expect("root");
        let with_policy = format!("{VALID}agents:\n  policy: ./policy.sh\n");
        let added = write_root(&root, &with_policy).expect_err("added");
        assert!(added.contains("agents"), "{added}");
        assert_eq!(read(&root).expect("read"), VALID);

        std::fs::write(&root, &with_policy).expect("root");
        let loosened = with_policy.replace("./policy.sh", "true");
        assert!(write_root(&root, &loosened).is_err());
        assert!(write_root(&root, VALID).is_err());
        assert_eq!(read(&root).expect("read"), with_policy);

        let requoted = format!(
            "agents:\n  policy: \"./policy.sh\"\n{}",
            VALID.replace("rails s", "rails server")
        );
        write_root(&root, &requoted).expect("same policy, other edits");
        assert_eq!(read(&root).expect("read"), requoted);
    }

    #[test]
    fn an_unreadable_config_cannot_be_overwritten_since_its_policy_is_unknown() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("pom.yml");
        std::fs::write(&root, "agents: [unclosed").expect("root");
        let refused = write_root(&root, VALID).expect_err("unknown policy");
        assert!(refused.contains("cannot be read"), "{refused}");
    }
}
