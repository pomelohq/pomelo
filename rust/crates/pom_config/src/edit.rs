//! Checking an edit to one config file against the whole config before it is written, so a save never leaves
//! the project with a config that no longer loads.

use std::path::{Path, PathBuf};

use crate::Config;

/// Whether writing `text` to `file` (the root `pom.yml` or a `pom.d` fragment) keeps the config loadable.
/// `Ok(Some(note))` lets the save through while the config was already broken elsewhere, so a fix can land
/// one file at a time; `Err` names the problem the edit would introduce.
pub fn check_file_edit(
    config_path: &Path,
    file: &Path,
    text: &str,
) -> Result<Option<String>, String> {
    crate::yaml_node::parse(text).map_err(|error| format!("yaml parse error: {error}"))?;
    match validate_with(config_path, file, text) {
        Ok(()) => Ok(None),
        Err(problem) if load_and_validate(config_path).is_err() => Ok(Some(problem)),
        Err(problem) => Err(problem),
    }
}

/// The root file and every fragment, in merge order.
pub fn config_files(config_path: &Path) -> Vec<PathBuf> {
    let dir = config_dir(config_path);
    std::iter::once(config_path.to_path_buf())
        .chain(crate::fragment_files(&dir).unwrap_or_default())
        .collect()
}

pub fn load_and_validate(path: &Path) -> Result<(), String> {
    Config::load(path)
        .map_err(|error| error.message)?
        .validate()
}

fn config_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Loads a mirror of every config file with `target` replaced by `text`.
fn validate_with(config_path: &Path, target: &Path, text: &str) -> Result<(), String> {
    let dir = config_dir(config_path);
    let temp = tempdir()?;
    let result = (|| {
        for file in config_files(config_path) {
            let relative = file.strip_prefix(&dir).unwrap_or(&file);
            let destination = temp.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let body = if file == target {
                text.to_string()
            } else {
                std::fs::read_to_string(&file).map_err(|error| error.to_string())?
            };
            std::fs::write(&destination, body).map_err(|error| error.to_string())?;
        }
        let root = config_path.file_name().unwrap_or_default();
        load_and_validate(&temp.join(root))
    })();
    if let Err(error) = std::fs::remove_dir_all(&temp) {
        eprintln!("config check: remove {}: {error}", temp.display());
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breaking_edit_is_refused_unless_the_config_was_already_broken() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("pom.yml");
        std::fs::write(&root, "session: demo\n").expect("root");
        let fragment = temp.path().join("pom.d/repos/api.yml");
        std::fs::create_dir_all(fragment.parent().expect("dir")).expect("pom.d");
        std::fs::write(
            &fragment,
            "repos:\n  api:\n    services:\n      web: rails s\n",
        )
        .expect("fragment");

        assert_eq!(config_files(&root), vec![root.clone(), fragment.clone()]);
        assert_eq!(
            check_file_edit(
                &root,
                &fragment,
                "repos:\n  api:\n    services:\n      web: rails s -p 3001\n"
            ),
            Ok(None)
        );
        assert!(check_file_edit(&root, &fragment, "repos: [").is_err());
        assert!(check_file_edit(
            &root,
            &fragment,
            "repos:\n  api:\n    env:\n      X: \"{{db:main}}\"\n"
        )
        .is_err());

        std::fs::write(&root, "session: demo\nbogus_key: [\n").expect("break");
        assert!(check_file_edit(
            &root,
            &fragment,
            "repos:\n  api:\n    services:\n      web: x\n"
        )
        .is_ok_and(|note| note.is_some()));
    }
}
