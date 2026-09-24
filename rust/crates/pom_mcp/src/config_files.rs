//! Reading and editing the project's config on an agent's behalf. Every write is validated first and a
//! rejected edit leaves the files untouched.

use std::path::{Path, PathBuf};

use pom_config::yaml_node::{self, Node};
use pom_config::Config;
use serde::Serialize;

#[derive(Serialize)]
pub struct ConfigFile {
    pub name: String,
    pub path: PathBuf,
    pub root: bool,
}

fn config_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn fragments(config_path: &Path) -> Vec<PathBuf> {
    pom_config::fragment_files(&config_dir(config_path)).unwrap_or_default()
}

/// The root `pom.yml` first, then every `pom.d` fragment.
pub fn list(config_path: &Path) -> Vec<ConfigFile> {
    let dir = config_dir(config_path);
    let root = ConfigFile {
        name: config_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: config_path.to_path_buf(),
        root: true,
    };
    std::iter::once(root)
        .chain(fragments(config_path).into_iter().map(|path| {
            ConfigFile {
                name: path
                    .strip_prefix(&dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
                path,
                root: false,
            }
        }))
        .collect()
}

/// The config as one text: the root file, or with a split config every file under a header naming it.
pub fn merged_text(config_path: &Path) -> Result<String, String> {
    let files = list(config_path);
    if files.len() == 1 {
        return std::fs::read_to_string(config_path).map_err(|error| error.to_string());
    }
    let mut out = String::from(
        "# The config is split: the root pom.yml plus pom.d fragments, merged in this order.\n",
    );
    for file in files {
        let body = std::fs::read_to_string(&file.path).map_err(|error| error.to_string())?;
        out.push_str(&format!("\n# --- {} ---\n{body}", file.name));
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

pub fn read(config_path: &Path, path: &Path) -> Result<String, String> {
    if !allowed(config_path, path) {
        return Err("unknown config file".into());
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

fn allowed(config_path: &Path, path: &Path) -> bool {
    list(config_path).iter().any(|file| file.path == path)
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
    if list(config_path).len() > 1 {
        return Err(
            "config is split across pom.d/*.yml - edit those files directly (use config_file_set)"
                .into(),
        );
    }
    validate_text(yaml)?;
    std::fs::write(config_path, yaml).map_err(|error| error.to_string())
}

/// Writes one file after checking the whole config as it would be with the edit. An edit is still
/// accepted while the config was already broken elsewhere, so a fix can land one file at a time.
pub fn write_file(
    config_path: &Path,
    path: &Path,
    yaml: &str,
    dry: bool,
) -> Result<String, String> {
    if !allowed(config_path, path) {
        return Err("unknown config file".into());
    }
    parse(yaml)?;
    let mut note = "Saved.".to_string();
    if let Err(edit_error) = validate_with(config_path, path, yaml) {
        if load_and_validate(config_path).is_ok() {
            return Err(edit_error);
        }
        note = format!("Saved (config still has errors elsewhere - keep fixing): {edit_error}");
    }
    if dry {
        return Ok(note.replacen("Saved", "Would save", 1));
    }
    std::fs::write(path, yaml).map_err(|error| error.to_string())?;
    Ok(note)
}

/// Loads a mirror of every config file with `target` replaced by `yaml`.
fn validate_with(config_path: &Path, target: &Path, yaml: &str) -> Result<(), String> {
    let dir = config_dir(config_path);
    let temp = tempdir()?;
    let result = (|| {
        for file in list(config_path) {
            let relative = file.path.strip_prefix(&dir).unwrap_or(&file.path);
            let destination = temp.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let body = if file.path == target {
                yaml.to_string()
            } else {
                std::fs::read_to_string(&file.path).map_err(|error| error.to_string())?
            };
            std::fs::write(&destination, body).map_err(|error| error.to_string())?;
        }
        let root = config_path.file_name().unwrap_or_default();
        load_and_validate(&temp.join(root))
    })();
    remove_dir(&temp);
    result
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
    fn split_configs_are_edited_file_by_file() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("pom.yml");
        std::fs::write(&root, "session: demo\n").expect("root");
        std::fs::create_dir_all(temp.path().join("pom.d/repos")).expect("pom.d");
        let fragment = temp.path().join("pom.d/repos/10-api.yml");
        std::fs::write(
            &fragment,
            "repos:\n  api:\n    services:\n      web: rails s\n",
        )
        .expect("fragment");

        let names: Vec<String> = list(&root).into_iter().map(|file| file.name).collect();
        assert_eq!(names, ["pom.yml", "pom.d/repos/10-api.yml"]);
        assert!(merged_text(&root)
            .expect("merged")
            .contains("# --- pom.d/repos/10-api.yml ---"));
        assert!(
            write_root(&root, VALID).is_err(),
            "a split config rejects a root rewrite"
        );

        let broken = "repos:\n  api:\n    profiles: [staging]\n    services:\n      web: rails s\n";
        assert!(write_file(&root, &fragment, broken, false).is_err());
        assert!(
            read(&root, &fragment).expect("read").contains("rails s"),
            "nothing written"
        );

        let fixed = "repos:\n  api:\n    services:\n      web: rails server\n";
        assert_eq!(
            write_file(&root, &fragment, fixed, false),
            Ok("Saved.".to_string())
        );
        assert_eq!(read(&root, &fragment).expect("read"), fixed);
        assert!(read(&root, Path::new("/etc/hosts")).is_err());
    }
}
