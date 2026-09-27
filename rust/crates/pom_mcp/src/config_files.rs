//! Reading and editing the project's config on an agent's behalf. Every write is validated first and a
//! rejected edit leaves the files untouched.

use std::path::{Path, PathBuf};

use pom_config::yaml_node::{self, Node};
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

pub fn write_root(config_path: &Path, yaml: &str) -> Result<(), String> {
    validate_text(yaml)?;
    std::fs::write(config_path, yaml).map_err(|error| error.to_string())
}

fn load_and_validate(path: &Path) -> Result<(), String> {
    Config::load(path)
        .map_err(|error| error.message)?
        .validate()
}

fn tempdir() -> Result<PathBuf, String> {
    let unique = format!(
        "pom-cfg-{}-{}",
        std::process::id(),
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
}
