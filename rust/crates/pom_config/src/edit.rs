//! Checking an edit to the config before it is written, so a save never leaves the project with a config that
//! no longer loads.

use std::path::{Path, PathBuf};

use crate::Config;

/// Whether writing `text` to the config keeps it loadable. `Ok(Some(note))` lets the save through while the
/// config was already broken, so a fix can land in steps; `Err` names the problem the edit would introduce.
pub fn check_edit(config_path: &Path, text: &str) -> Result<Option<String>, String> {
    crate::yaml_node::parse(text).map_err(|error| format!("yaml parse error: {error}"))?;
    match validate_text(config_path, text) {
        Ok(()) => Ok(None),
        Err(problem) if load_and_validate(config_path).is_err() => Ok(Some(problem)),
        Err(problem) => Err(problem),
    }
}

pub fn load_and_validate(path: &Path) -> Result<(), String> {
    Config::load(path)
        .map_err(|error| error.message)?
        .validate()
}

fn validate_text(config_path: &Path, text: &str) -> Result<(), String> {
    let temp = tempdir()?;
    let name = config_path.file_name().unwrap_or_default();
    let result = std::fs::write(temp.join(name), text)
        .map_err(|error| error.to_string())
        .and_then(|()| load_and_validate(&temp.join(name)));
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

        assert_eq!(
            check_edit(
                &root,
                "repos:\n  api:\n    services:\n      web: rails s -p 3001\n"
            ),
            Ok(None)
        );
        assert!(check_edit(&root, "repos: [").is_err());
        assert!(check_edit(
            &root,
            "repos:\n  api:\n    env:\n      X: \"{{db:main}}\"\n"
        )
        .is_err());

        let still_broken = "repos:\n  api:\n    env:\n      X: \"{{db:main}}\"\n";
        std::fs::write(&root, still_broken).expect("break");
        assert!(check_edit(&root, "repos:\n  api:\n    services:\n      web: x\n") == Ok(None));
        assert!(check_edit(&root, still_broken).is_ok_and(|note| note.is_some()));
    }
}
