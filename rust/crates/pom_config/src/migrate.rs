//! Folding a legacy `pom.d/` fragment tree into the single `pom.yml`. Runs once per project on load; the
//! originals are kept beside it as `pom.d.bak` and `pom.yml.bak`.

use std::path::{Path, PathBuf};

use crate::yaml_node::{self, Node, NodeKind};
use crate::LoadError;

const LEGACY_DIR: &str = "pom.d";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated {
    pub fragments_backup: PathBuf,
    pub root_backup: Option<PathBuf>,
}

/// Merges `pom.d/**.yml` into `config_path` and moves the fragments aside. `Ok(None)` when there is nothing
/// to fold, including when another process got there first.
pub fn migrate_fragments(config_path: &Path) -> Result<Option<Migrated>, LoadError> {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    let error = |path: &Path, message: String| LoadError {
        path: path.to_path_buf(),
        message,
    };
    let fragments = fragment_files(dir)
        .map_err(|e| error(&dir.join(LEGACY_DIR), format!("read failed: {e}")))?;
    if fragments.is_empty() {
        return Ok(None);
    }
    let root_text = match std::fs::read_to_string(config_path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(error(config_path, format!("read failed: {e}"))),
    };
    let mut texts = Vec::new();
    for fragment in &fragments {
        let text = std::fs::read_to_string(fragment)
            .map_err(|e| error(fragment, format!("read failed: {e}")))?;
        yaml_node::parse(&text).map_err(|e| error(fragment, format!("parse failed: {e}")))?;
        texts.push(text);
    }
    let root_source = root_text.clone().unwrap_or_default();
    let merged = merged_tree(&root_source, &texts)
        .ok_or_else(|| error(config_path, "root is not a mapping".into()))?;
    // The text merge keeps comments and layout; the tree is the fallback when the fragments overlap.
    let text = merge_text(&root_source, &texts)
        .filter(|text| {
            yaml_node::parse(text)
                .ok()
                .flatten()
                .is_some_and(|node| node.without_lines() == merged.without_lines())
        })
        .unwrap_or_else(|| yaml_node::to_yaml(&merged));

    let write_error = |path: &Path, e: std::io::Error| error(path, format!("migrate pom.d: {e}"));
    let root_backup = match &root_text {
        Some(original) => {
            let backup = free_path(dir, "pom.yml.bak");
            std::fs::write(&backup, original).map_err(|e| write_error(&backup, e))?;
            Some(backup)
        }
        None => None,
    };
    let staged = dir.join(format!(".pom.yml.migrating-{}", std::process::id()));
    std::fs::write(&staged, &text).map_err(|e| write_error(&staged, e))?;
    let fragments_backup = free_path(dir, "pom.d.bak");
    if let Err(e) = std::fs::rename(dir.join(LEGACY_DIR), &fragments_backup) {
        remove_quietly(&staged);
        if let Some(backup) = &root_backup {
            remove_quietly(backup);
        }
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(write_error(&dir.join(LEGACY_DIR), e));
    }
    std::fs::rename(&staged, config_path).map_err(|e| write_error(config_path, e))?;
    Ok(Some(Migrated {
        fragments_backup,
        root_backup,
    }))
}

/// Every `*.yml`/`*.yaml` under `pom.d`, sorted by full path (the order they used to merge in). Dot entries
/// are skipped.
fn fragment_files(config_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
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
    let mut files = Vec::new();
    match walk(&config_dir.join(LEGACY_DIR), &mut files) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        result => result?,
    }
    files.sort();
    Ok(files)
}

fn merged_tree(root: &str, fragments: &[String]) -> Option<Node> {
    let mut tree = yaml_node::parse(root)
        .ok()?
        .unwrap_or_else(Node::empty_mapping);
    if !tree.is_mapping() {
        return None;
    }
    for fragment in fragments {
        if let Ok(Some(document)) = yaml_node::parse(fragment) {
            if document.is_mapping() {
                merge_mapping(&mut tree, document);
            }
        }
    }
    Some(tree)
}

/// Nested mappings merge key by key; any other value replaces the earlier one. New keys append.
fn merge_mapping(destination: &mut Node, source: Node) {
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

fn merge_text(root: &str, fragments: &[String]) -> Option<String> {
    let mut text = root.to_string();
    for fragment in fragments {
        text = append_fragment(&text, fragment)?;
    }
    Some(text)
}

/// Copies each top-level block of `fragment` into `text` as written: a new key goes at the end, a mapping the
/// root already has gets the fragment's children appended under it. `None` when the two would need a deep
/// merge.
fn append_fragment(text: &str, fragment: &str) -> Option<String> {
    let Some(document) = yaml_node::parse(fragment).ok()? else {
        return Some(text.to_string());
    };
    let fragment_lines: Vec<&str> = fragment.lines().collect();
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    let mut previous_end = 0;
    for (key, value) in document.entries()? {
        let start = key.line.checked_sub(1)?;
        if indent(fragment_lines.get(start)?) != 0 {
            return None;
        }
        let block = entry_lines(&fragment_lines, start);
        let leading = &fragment_lines[previous_end.min(start)..start];
        previous_end = block.end;
        let current_text = out.join("\n");
        let current = yaml_node::parse(&current_text)
            .ok()?
            .unwrap_or_else(Node::empty_mapping);
        let existing = current
            .entries()?
            .iter()
            .find(|(existing_key, _)| existing_key.text() == key.text());
        match existing {
            None => {
                if out.last().is_some_and(|line| !line.trim().is_empty()) && leading.is_empty() {
                    out.push(String::new());
                }
                out.extend(leading.iter().map(|line| line.to_string()));
                out.extend(fragment_lines[block].iter().map(|line| line.to_string()));
            }
            Some((existing_key, existing_value)) => {
                let existing_entries = existing_value.entries().filter(|e| !e.is_empty())?;
                let added = value.entries().filter(|e| !e.is_empty())?;
                if added
                    .iter()
                    .any(|(name, _)| existing_value.get(name.text()).is_some())
                {
                    return None;
                }
                if fragment_lines[block.start].trim_end() != format!("{}:", key.text()) {
                    return None;
                }
                let out_lines: Vec<&str> = out.iter().map(String::as_str).collect();
                let target_indent =
                    indent(out_lines.get(existing_entries[0].0.line.checked_sub(1)?)?);
                let insert_at = entry_lines(&out_lines, existing_key.line.checked_sub(1)?).end;
                let children = &fragment_lines[block.start + 1..block.end];
                let source_indent = indent(fragment_lines.get(added[0].0.line.checked_sub(1)?)?);
                let mut moved = Vec::new();
                for line in leading.iter().chain(children) {
                    if line.trim().is_empty() {
                        moved.push(String::new());
                    } else if indent(line) >= source_indent {
                        moved.push(format!(
                            "{}{}",
                            " ".repeat(target_indent),
                            &line[source_indent..]
                        ));
                    } else {
                        moved.push(format!(
                            "{}{}",
                            " ".repeat(target_indent),
                            line.trim_start()
                        ));
                    }
                }
                out.splice(insert_at..insert_at, moved);
            }
        }
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    Some(joined)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The key line on `start` plus everything more indented below it, without trailing blank lines.
fn entry_lines(lines: &[&str], start: usize) -> std::ops::Range<usize> {
    let depth = lines.get(start).map_or(0, |line| indent(line));
    let mut end = start + 1;
    while end < lines.len() && (lines[end].trim().is_empty() || indent(lines[end]) > depth) {
        end += 1;
    }
    while end > start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    start..end
}

fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| dir.join(format!("{name}.{n}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("migrate pom.d: remove {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp");
        for (name, body) in files {
            let path = temp.path().join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        }
        temp
    }

    #[test]
    fn split_fragments_fold_back_into_pom_yml_as_written() {
        let temp = project(&[
            ("pom.yml", "session: shop # the shop\n"),
            (
                "pom.d/repos/01-api.yml",
                "repos:\n  api:\n    # the backend\n    services:\n      web: rails s\n",
            ),
            (
                "pom.d/repos/02-web.yml",
                "repos:\n  web:\n    env:\n      API: \"{{api.web.url}}\"\n",
            ),
            (
                "pom.d/shared-services.yml",
                "# infra\nshared_services:\n  postgres:\n    type: postgres\n",
            ),
        ]);
        let config = temp.path().join("pom.yml");
        let migrated = migrate_fragments(&config)
            .expect("migrate")
            .expect("migrated");
        let text = std::fs::read_to_string(&config).expect("read");
        assert_eq!(
            text,
            "session: shop # the shop\n\nrepos:\n  api:\n    # the backend\n    services:\n      web: rails s\n  web:\n    env:\n      API: \"{{api.web.url}}\"\n# infra\nshared_services:\n  postgres:\n    type: postgres\n"
        );
        assert!(!temp.path().join("pom.d").exists());
        assert_eq!(migrated.fragments_backup, temp.path().join("pom.d.bak"));
        assert!(temp.path().join("pom.d.bak/repos/01-api.yml").exists());
        assert_eq!(
            std::fs::read_to_string(migrated.root_backup.expect("backup")).expect("backup"),
            "session: shop # the shop\n"
        );
        assert_eq!(migrate_fragments(&config).expect("again"), None);
    }

    #[test]
    fn overlapping_fragments_fall_back_to_the_merged_tree() {
        let temp = project(&[
            ("pom.yml", "repos:\n  api:\n    alias: be\n"),
            ("pom.yml.bak", "old\n"),
            (
                "pom.d/10-api.yml",
                "repos:\n  api:\n    services:\n      web: run\n",
            ),
        ]);
        let config = temp.path().join("pom.yml");
        let migrated = migrate_fragments(&config)
            .expect("migrate")
            .expect("migrated");
        let loaded = crate::Config::load(&config).expect("load");
        assert_eq!(loaded.repos["api"].alias, "be");
        assert!(loaded.repos["api"].services.contains_key("web"));
        assert_eq!(
            migrated.root_backup,
            Some(temp.path().join("pom.yml.bak.1"))
        );
    }

    #[test]
    fn a_broken_fragment_stops_the_move_and_names_the_file() {
        let temp = project(&[("pom.yml", "session: x\n"), ("pom.d/bad.yml", "a: [\n")]);
        let config = temp.path().join("pom.yml");
        let error = migrate_fragments(&config).expect_err("broken");
        assert!(error.path.ends_with("pom.d/bad.yml"));
        assert!(temp.path().join("pom.d/bad.yml").exists());
        assert_eq!(
            std::fs::read_to_string(&config).expect("read"),
            "session: x\n"
        );
    }

    #[test]
    fn dot_entries_and_other_files_are_not_fragments() -> std::io::Result<()> {
        let temp = project(&[
            ("pom.d/20-b.yml", "a: 1\n"),
            ("pom.d/10-a.yaml", "a: 1\n"),
            ("pom.d/notes.txt", "a: 1\n"),
            ("pom.d/.skip.yml", "a: 1\n"),
            ("pom.d/repos/01-api.yml", "a: 1\n"),
            ("pom.d/.hidden/x.yml", "a: 1\n"),
        ]);
        let base = temp.path().join("pom.d");
        let names: Vec<String> = fragment_files(temp.path())?
            .iter()
            .filter_map(|p| p.strip_prefix(&base).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["10-a.yaml", "20-b.yml", "repos/01-api.yml"]);
        Ok(())
    }
}
