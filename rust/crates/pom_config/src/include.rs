use std::path::{Path, PathBuf};

use crate::yaml_node::{Node, NodeKind};

pub const FRAGMENT_DIR: &str = "pom.d";

/// Every `*.yml`/`*.yaml` under `pom.d`, sorted by full path so `10-a.yml` merges before
/// `20-b.yml`. Dot files and dot directories are skipped.
pub fn fragment_files(config_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, out)?;
                continue;
            }
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            if matches!(extension.as_deref(), Some("yml" | "yaml")) {
                out.push(path);
            }
        }
        Ok(())
    }
    let root = config_dir.join(FRAGMENT_DIR);
    let mut files = Vec::new();
    match walk(&root, &mut files) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        result => result?,
    }
    files.sort();
    Ok(files)
}

/// Nested mappings merge key by key; any other value (scalar, list, or a type change) replaces
/// the earlier one. New keys append, so the root file's key order leads.
pub fn merge_mapping(destination: &mut Node, source: Node) {
    let NodeKind::Mapping(target) = &mut destination.kind else {
        return;
    };
    let NodeKind::Mapping(entries) = source.kind else {
        return;
    };
    for (key, value) in entries {
        match target.iter_mut().find(|(k, _)| k.text() == key.text()) {
            Some((_, current)) if current.is_mapping() && value.is_mapping() => {
                merge_mapping(current, value)
            }
            Some((_, current)) => *current = value,
            None => target.push((key, value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml_node::parse;

    fn doc(source: &str) -> Node {
        parse(source)
            .ok()
            .flatten()
            .unwrap_or_else(Node::empty_mapping)
    }

    #[test]
    fn merges_nested_maps_and_replaces_leaves() {
        let mut root = doc("a:\n  x: 1\n  y: [1]\nb: keep\n");
        merge_mapping(&mut root, doc("a:\n  y: [2]\n  z: 3\nc: new\n"));
        assert_eq!(
            root.without_lines(),
            doc("a:\n  x: 1\n  y: [2]\n  z: 3\nb: keep\nc: new\n").without_lines()
        );
    }

    #[test]
    fn fragment_files_are_sorted_and_skip_dot_entries() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let fragments = temp.path().join(FRAGMENT_DIR);
        std::fs::create_dir_all(fragments.join("repos"))?;
        std::fs::create_dir_all(fragments.join(".hidden"))?;
        for file in [
            "20-b.yml",
            "10-a.yaml",
            "notes.txt",
            ".skip.yml",
            "repos/01-api.yml",
            ".hidden/x.yml",
        ] {
            std::fs::write(fragments.join(file), "a: 1\n")?;
        }
        let names: Vec<String> = fragment_files(temp.path())?
            .iter()
            .filter_map(|p| p.strip_prefix(&fragments).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["10-a.yaml", "20-b.yml", "repos/01-api.yml"]);
        assert!(fragment_files(&temp.path().join("missing"))?.is_empty());
        Ok(())
    }
}
