use std::path::{Path, PathBuf};

use crate::yaml_node::{self, Node, NodeKind};

pub const REMOVED_TOP_KEYS: [&str; 5] = [
    "schema_version",
    "plugins",
    "combinations",
    "proxy",
    "webhook",
];

/// Keys the config format dropped, anywhere they may still appear.
pub fn removed_keys(root: &Node) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (key, _) in removed_entries(root) {
        if !found.contains(&key) {
            found.push(key);
        }
    }
    found
}

pub fn removed_keys_in(config_path: &Path) -> Vec<String> {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| yaml_node::parse(&text).ok().flatten())
        .map(|root| removed_keys(&root))
        .unwrap_or_default()
}

/// Each removed key with the line (1-based) its entry starts on.
fn removed_entries(root: &Node) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    for (key, _) in root.entries().unwrap_or_default() {
        if REMOVED_TOP_KEYS.contains(&key.text()) {
            found.push((key.text().to_string(), key.line));
        }
    }
    for (_, repo) in root
        .get("repos")
        .and_then(Node::entries)
        .unwrap_or_default()
    {
        for (key, _) in repo.entries().unwrap_or_default() {
            if matches!(key.text(), "plugins" | "exposes") {
                found.push((key.text().to_string(), key.line));
            }
        }
        for (_, service) in repo
            .get("services")
            .and_then(Node::entries)
            .unwrap_or_default()
        {
            for (key, _) in service.entries().unwrap_or_default() {
                if key.text() == "exposes" {
                    found.push(("exposes".to_string(), key.line));
                }
            }
        }
    }
    found
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The lines (0-based, end exclusive) of the entry whose key is on `start`: the key line and everything more
/// indented below it, without trailing blank lines.
fn entry_lines(lines: &[&str], start: usize) -> std::ops::Range<usize> {
    let depth = lines.get(start).map_or(0, |line| indent(line));
    let mut end = start + 1;
    while end < lines.len() && (lines[end].trim().is_empty() || (indent(lines[end]) > depth)) {
        end += 1;
    }
    while end > start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    start..end
}

fn remove_ranges(lines: &[&str], mut ranges: Vec<std::ops::Range<usize>>) -> String {
    ranges.sort_by_key(|range| range.start);
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !ranges.iter().any(|range| range.contains(&index)) {
            out.push(*line);
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

const COLON_TOKENS: [(&str, &str); 7] = [
    ("conn", "shared.{}.url"),
    ("host", "shared.{}.host"),
    ("port", "shared.{}.port"),
    ("user", "shared.{}.user"),
    ("pass", "shared.{}.pass"),
    ("slot", "shared.{}.slot"),
    ("db", "db.{}"),
];

/// Rewrites the old `{{conn:x}}`-style tokens (and `{{branch_safe}}`-style ones) into dot notation.
pub fn migrate_colon_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let replaced = match inner {
            "branch_safe" => Some("branch.safe".to_string()),
            "branch_hash" => Some("branch.hash".to_string()),
            "branch_host" => Some("branch.host".to_string()),
            _ => inner.split_once(':').and_then(|(prefix, name)| {
                let valid = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
                COLON_TOKENS
                    .iter()
                    .find(|(known, _)| *known == prefix)
                    .filter(|_| valid)
                    .map(|(_, template)| template.replace("{}", name))
            }),
        };
        match replaced {
            Some(token) => out.push_str(&format!("{{{{{token}}}}}")),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// Deletes the removed keys and moves colon tokens to dot notation. Returns the keys it removed.
pub fn normalize(config_path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let mut removed: Vec<String> = Vec::new();
    let mut next = text.clone();
    if let Ok(Some(root)) = yaml_node::parse(&text) {
        let entries = removed_entries(&root);
        if !entries.is_empty() {
            let lines: Vec<&str> = text.lines().collect();
            let ranges = entries
                .iter()
                .map(|(_, line)| entry_lines(&lines, line.saturating_sub(1)))
                .collect();
            next = remove_ranges(&lines, ranges);
            for (key, _) in entries {
                if !removed.contains(&key) {
                    removed.push(key);
                }
            }
        }
    }
    let next = migrate_colon_tokens(&next);
    if next != text {
        std::fs::write(config_path, next)
            .map_err(|error| format!("write {}: {error}", config_path.display()))?;
    }
    Ok(removed)
}

/// Sets and removes keys of a repo's `env:`, returning the file written.
/// Only the env block is rewritten (its comments go); the result must still load or the file is restored.
pub fn edit_repo_env(
    config_path: &Path,
    repo: &str,
    set: &[(String, String)],
    unset: &[String],
) -> Result<PathBuf, String> {
    for (key, _) in set {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("{key:?} is not an env var name"));
        }
    }
    let file = config_path.to_path_buf();
    let text = std::fs::read_to_string(&file)
        .map_err(|error| format!("read {}: {error}", file.display()))?;
    let root = yaml_node::parse(&text)
        .map_err(|error| format!("parse {}: {error}", file.display()))?
        .ok_or_else(|| format!("repo {repo:?} is not in the config"))?;
    let (repo_key, repo_node) = root
        .get("repos")
        .and_then(Node::entries)
        .unwrap_or_default()
        .iter()
        .find(|(key, _)| key.text() == repo)
        .ok_or_else(|| format!("repo {repo:?} is not in {}", file.display()))?;
    if !repo_node.is_mapping() && !repo_node.is_null() {
        return Err(format!("repo {repo:?} is not a mapping"));
    }
    let lines: Vec<&str> = text.lines().collect();
    let repo_line = repo_key.line.saturating_sub(1);
    let existing = repo_node
        .entries()
        .unwrap_or_default()
        .iter()
        .find(|(key, _)| key.text() == "env");
    let child_indent = repo_node
        .entries()
        .and_then(|entries| entries.first())
        .and_then(|(key, _)| lines.get(key.line.saturating_sub(1)))
        .map_or_else(
            || indent(lines.get(repo_line).unwrap_or(&"")) + 2,
            |line| indent(line),
        );
    let mut env = existing
        .map(|(_, value)| value.clone())
        .filter(Node::is_mapping)
        .unwrap_or_else(Node::empty_mapping);
    apply_env_edits(&mut env, set, unset);
    let pad = " ".repeat(child_indent);
    let mut block = vec![match &env.kind {
        NodeKind::Mapping(entries) if entries.is_empty() => format!("{pad}env: {{}}"),
        _ => format!("{pad}env:"),
    }];
    if env.entries().is_some_and(|entries| !entries.is_empty()) {
        block.extend(yaml_node::to_yaml(&env).lines().map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}  {line}")
            }
        }));
    }
    let (start, end) = match existing {
        Some((key, _)) => {
            let range = entry_lines(&lines, key.line.saturating_sub(1));
            (range.start, range.end)
        }
        None => {
            let end = entry_lines(&lines, repo_line).end;
            (end, end)
        }
    };
    let mut out: Vec<String> = lines[..start].iter().map(|line| line.to_string()).collect();
    out.extend(block);
    out.extend(lines[end..].iter().map(|line| line.to_string()));
    let mut next = out.join("\n");
    next.push('\n');
    std::fs::write(&file, &next).map_err(|error| format!("write {}: {error}", file.display()))?;
    if let Err(error) = crate::Config::load(config_path) {
        if let Err(restore) = std::fs::write(&file, &text) {
            return Err(format!(
                "{error}; restoring {} failed: {restore}",
                file.display()
            ));
        }
        return Err(format!("the edit would break the config: {error}"));
    }
    Ok(file)
}

fn apply_env_edits(env: &mut Node, set: &[(String, String)], unset: &[String]) {
    let NodeKind::Mapping(entries) = &mut env.kind else {
        return;
    };
    // Env keyed by file name (`.env: {...}`) keeps shared values under `*`.
    let file_keyed = entries.iter().any(|(_, value)| value.is_mapping());
    let base = if file_keyed {
        let at = match entries.iter().position(|(key, _)| key.text() == "*") {
            Some(at) => at,
            None => {
                entries.insert(0, (Node::scalar("*"), Node::empty_mapping()));
                0
            }
        };
        match &mut entries[at].1.kind {
            NodeKind::Mapping(base) => base,
            _ => return,
        }
    } else {
        entries
    };
    base.retain(|(key, _)| !unset.iter().any(|name| name == key.text()));
    for (name, value) in set {
        let value = Node::scalar(value.clone());
        match base.iter_mut().find(|(key, _)| key.text() == name) {
            Some(entry) => entry.1 = value,
            None => base.push((
                Node {
                    kind: NodeKind::Scalar {
                        value: name.clone(),
                        plain: true,
                    },
                    line: 0,
                },
                value,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn repo_env_edits_touch_only_the_env_block_of_the_file_with_the_repo() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("pom.yml");
        std::fs::write(
            &root,
            "session: shop\n# keep me\nrepos:\n  api:\n    alias: be # the backend\n    env:\n      A: \"1\"\n      B: two\n    services:\n      web:\n        cmd: run\n  web:\n    services: {}\n",
        )
        .expect("pom.yml");
        let edited = edit_repo_env(
            &root,
            "api",
            &[("C".into(), "x y".into()), ("A".into(), "9".into())],
            &["B".into()],
        )
        .expect("edit");
        assert_eq!(edited, root);
        let text = std::fs::read_to_string(&root).expect("read");
        assert!(text.contains("# keep me\n"), "{text}");
        assert!(text.contains("alias: be # the backend"), "{text}");
        assert!(
            text.contains("    env:\n      A: \"9\"\n      C: \"x y\"\n    services:"),
            "{text}"
        );
        let config = crate::Config::load(&root).expect("load");
        assert_eq!(
            config.repos["api"].env.get("C").map(String::as_str),
            Some("x y")
        );
        assert!(!config.repos["api"].env.contains_key("B"));

        edit_repo_env(&root, "web", &[("PORT_HINT".into(), "1".into())], &[]).expect("new env");
        let config = crate::Config::load(&root).expect("load");
        assert_eq!(
            config.repos["web"].env.get("PORT_HINT").map(String::as_str),
            Some("1")
        );
        assert!(edit_repo_env(&root, "nope", &[], &[]).is_err());
        assert!(edit_repo_env(&root, "api", &[("bad key".into(), "1".into())], &[]).is_err());
    }
    use super::*;

    const CONFIG: &str = "session: demo\n# repos below\nrepos:\n  api:\n    plugins: [x]\n    services:\n      web:\n        cmd: rails s\n        exposes: 3000\n\n  web:\n    services:\n      dev: npm run dev\nshared_services:\n  postgres:\n    type: postgres\nwebhook:\n  port: 1\nproxy: {}\n";

    #[test]
    fn colon_tokens_move_to_dot_notation() {
        assert_eq!(
            migrate_colon_tokens(
                "url: {{conn:postgres}}/{{db:main}} {{branch_safe}} {{secret.X}} {{bad:}}"
            ),
            "url: {{shared.postgres.url}}/{{db.main}} {{branch.safe}} {{secret.X}} {{bad:}}"
        );
    }

    #[test]
    fn removed_keys_are_found_at_every_level() -> Result<(), String> {
        let root = yaml_node::parse(CONFIG)
            .map_err(|error| error.to_string())?
            .ok_or("empty")?;
        assert_eq!(
            removed_keys(&root),
            ["webhook", "proxy", "plugins", "exposes"]
        );
        Ok(())
    }

    #[test]
    fn normalize_strips_removed_keys_as_written() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let config = temp.path().join("pom.yml");
        std::fs::write(&config, CONFIG).map_err(|error| error.to_string())?;
        let removed = normalize(&config)?;
        assert_eq!(removed, ["webhook", "proxy", "plugins", "exposes"]);
        let text = std::fs::read_to_string(&config).map_err(|error| error.to_string())?;
        assert_eq!(
            text,
            "session: demo\n# repos below\nrepos:\n  api:\n    services:\n      web:\n        cmd: rails s\n\n  web:\n    services:\n      dev: npm run dev\nshared_services:\n  postgres:\n    type: postgres\n"
        );
        assert_eq!(normalize(&config)?, Vec::<String>::new());
        Ok(())
    }
}

/// The config's text and the line (0-based) of `repo`'s key.
fn repo_file(config_path: &Path, repo: &str) -> Result<(PathBuf, String, usize), String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let line = yaml_node::parse(&text)
        .ok()
        .flatten()
        .and_then(|root| {
            root.get("repos")?
                .entries()?
                .iter()
                .find(|(key, _)| key.text() == repo)
                .map(|(key, _)| key.line.saturating_sub(1))
        })
        .ok_or_else(|| format!("repo {repo:?} is not in the config"))?;
    Ok((config_path.to_path_buf(), text, line))
}

/// Writes every `(file, text)`, then takes them all back if the config no longer loads and validates.
fn write_checked(config_path: &Path, edits: &[(PathBuf, String, String)]) -> Result<(), String> {
    for (file, _, next) in edits {
        std::fs::write(file, next).map_err(|error| format!("write {}: {error}", file.display()))?;
    }
    let problem = crate::Config::load(config_path)
        .map_err(|error| error.message)
        .and_then(|config| config.validate())
        .err();
    let Some(problem) = problem else {
        return Ok(());
    };
    for (file, before, _) in edits {
        if let Err(error) = std::fs::write(file, before) {
            return Err(format!(
                "{problem}; restoring {} failed: {error}",
                file.display()
            ));
        }
    }
    Err(format!("the edit would break the config: {problem}"))
}

/// Gives `repo` a new alias and rewrites every `{{<old>.` reference to it. Returns the files that changed.
pub fn rename_alias(config_path: &Path, repo: &str, alias: &str) -> Result<Vec<PathBuf>, String> {
    let alias = alias.trim();
    if alias.is_empty()
        || !alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{alias:?} is not a valid alias"));
    }
    let config = crate::Config::load(config_path).map_err(|error| error.message)?;
    let dir = config
        .repos
        .get(repo)
        .ok_or_else(|| format!("repo {repo:?} is not in the config"))?;
    let old = if dir.alias.is_empty() {
        repo.to_string()
    } else {
        dir.alias.clone()
    };
    if old == alias {
        return Ok(Vec::new());
    }
    if config
        .repos
        .iter()
        .any(|(name, other)| name != repo && (name == alias || other.alias == alias))
    {
        return Err(format!("{alias} is already used by another repo"));
    }
    let (file, text, repo_line) = repo_file(config_path, repo)?;
    let lines: Vec<&str> = text.lines().collect();
    let block = entry_lines(&lines, repo_line);
    let key_indent = indent(lines[repo_line]);
    let child_indent = lines[block.start + 1..block.end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .map_or(key_indent + 2, |line| indent(line));
    let alias_line = (block.start + 1..block.end).find(|&index| {
        indent(lines[index]) == child_indent && lines[index].trim_start().starts_with("alias:")
    });
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    let pad = " ".repeat(child_indent);
    match alias_line {
        Some(index) => {
            let range = entry_lines(&lines, index);
            out.splice(range, [format!("{pad}alias: {alias}")]);
        }
        None => {
            let key = lines[repo_line].trim_end();
            // `api: {}` becomes a mapping with the alias; `api:` gets the alias as its first entry.
            let head = key
                .strip_suffix("{}")
                .map_or(key, str::trim_end)
                .to_string();
            out.splice(
                repo_line..=repo_line,
                [head, format!("{pad}alias: {alias}")],
            );
        }
    }
    let mut renamed = out.join("\n");
    renamed.push('\n');
    let from = format!("{{{{{old}.");
    let to = format!("{{{{{alias}.");
    let next = renamed.replace(&from, &to);
    let edits = if next == text {
        Vec::new()
    } else {
        vec![(file, text, next)]
    };
    write_checked(config_path, &edits)?;
    Ok(edits.into_iter().map(|(path, _, _)| path).collect())
}

/// Takes `repo` out of the config. Refused (nothing written) when the rest of the config still refers to it.
/// Returns the file edited.
pub fn remove_repo(config_path: &Path, repo: &str) -> Result<PathBuf, String> {
    let (file, text, repo_line) = repo_file(config_path, repo)?;
    let lines: Vec<&str> = text.lines().collect();
    let next = remove_ranges(&lines, vec![entry_lines(&lines, repo_line)]);
    let alias = crate::Config::load(config_path)
        .ok()
        .and_then(|config| config.repos.get(repo).map(|dir| dir.alias.clone()))
        .filter(|alias| !alias.is_empty())
        .unwrap_or_else(|| repo.to_string());
    let needles = [format!("{{{{{alias}."), format!("{{{{{repo}.")];
    if needles.iter().any(|needle| next.contains(needle.as_str())) {
        return Err(format!(
            "{repo} is still referred to in the config; change those first"
        ));
    }
    write_checked(config_path, &[(file.clone(), text, next)])?;
    Ok(file)
}

#[cfg(test)]
mod repo_edit_tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temp");
        for (name, text) in files {
            let path = temp.path().join(name);
            std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
            std::fs::write(&path, text).expect("write");
        }
        let config = temp.path().join("pom.yml");
        (temp, config)
    }

    const REPOS: &str = "session: demo\nrepos:\n  api:\n    alias: be\n    services:\n      web:\n        cmd: rails s\n        port: true\n  web:\n    env:\n      API_URL: \"{{be.web.url}}\"\n  jobs:\n    services:\n      worker:\n        cmd: sidekiq\n";

    #[test]
    fn renaming_an_alias_rewrites_its_references() {
        let (_temp, config) = project(&[("pom.yml", REPOS)]);
        let changed = rename_alias(&config, "api", "backend").expect("rename");
        assert_eq!(changed, std::slice::from_ref(&config));
        let text = std::fs::read_to_string(&config).expect("read");
        assert!(text.contains("    alias: backend\n"), "{text}");
        assert!(text.contains("{{backend.web.url}}"), "{text}");
        assert!(
            rename_alias(&config, "api", "web").is_err(),
            "taken by another repo"
        );
    }

    #[test]
    fn removing_a_repo_refuses_while_referenced() {
        let (_temp, config) = project(&[("pom.yml", REPOS)]);
        assert!(
            remove_repo(&config, "api").is_err(),
            "web still points at api"
        );
        remove_repo(&config, "jobs").expect("remove");
        let config_now = crate::Config::load(&config).expect("loads");
        assert!(!config_now.repos.contains_key("jobs"));
        assert!(config_now.repos.contains_key("api"));
    }
}
