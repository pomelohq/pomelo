use std::path::{Path, PathBuf};

use crate::yaml_node::{self, Node, NodeKind};
use crate::{fragment_files, FRAGMENT_DIR};

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
    let mut found: Vec<String> = Vec::new();
    for file in config_files(config_path) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(Some(root)) = yaml_node::parse(&text) else {
            continue;
        };
        for key in removed_keys(&root) {
            if !found.contains(&key) {
                found.push(key);
            }
        }
    }
    found
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

fn config_files(config_path: &Path) -> Vec<PathBuf> {
    let dir = config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::iter::once(config_path.to_path_buf())
        .chain(fragment_files(&dir).unwrap_or_default())
        .collect()
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

/// Deletes the removed keys from every config file, moves colon tokens to dot notation, then splits the root
/// into `pom.d` fragments when it can. Returns the keys it removed.
pub fn normalize(config_path: &Path) -> Result<Vec<String>, String> {
    let mut removed: Vec<String> = Vec::new();
    for file in config_files(config_path) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
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
            std::fs::write(&file, next)
                .map_err(|error| format!("write {}: {error}", file.display()))?;
        }
    }
    if let Err(error) = split(config_path, false) {
        eprintln!("normalize: split skipped: {error}");
    }
    Ok(removed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitResult {
    pub root: PathBuf,
    pub fragments: Vec<PathBuf>,
    pub backup: PathBuf,
}

fn fragment_name(repo: &str) -> String {
    repo.replace(['/', ' '], "-")
}

fn is_split_repo_fragment(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'-'
        && (name.ends_with(".yml") || name.ends_with(".yaml"))
}

/// Moves `repos` (one fragment per repo, numbered in order), `environments`, `presets` and `shared_services`
/// out of the root `pom.yml` into `pom.d`, keeping each block's text as written. The root is backed up to
/// `pom.yml.bak`; `dry` only reports what would be written.
pub fn split(config_path: &Path, dry: bool) -> Result<SplitResult, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let root = yaml_node::parse(&text)
        .map_err(|error| format!("parse {}: {error}", config_path.display()))?
        .filter(Node::is_mapping)
        .ok_or_else(|| format!("{} is not a YAML mapping", config_path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let dir = config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let fragment_dir = dir.join(FRAGMENT_DIR);
    let mut plan: Vec<(PathBuf, String)> = Vec::new();
    let mut moved: Vec<std::ops::Range<usize>> = Vec::new();
    let top_entry = |name: &str| {
        root.entries()
            .unwrap_or_default()
            .iter()
            .find(|(key, _)| key.text() == name)
            .map(|(key, value)| (key.line.saturating_sub(1), value))
    };
    if let Some((line, repos)) = top_entry("repos") {
        let entries = repos.entries().unwrap_or_default();
        if !entries.is_empty() {
            for (index, (name, _)) in entries.iter().enumerate() {
                let block = entry_lines(&lines, name.line.saturating_sub(1));
                let body = lines[block].join("\n");
                let file = fragment_dir.join("repos").join(format!(
                    "{:02}-{}.yml",
                    index + 1,
                    fragment_name(name.text())
                ));
                plan.push((file, format!("repos:\n{body}\n")));
            }
            moved.push(entry_lines(&lines, line));
        }
    }
    for (key, file) in [
        ("environments", "environments.yml"),
        ("presets", "presets.yml"),
        ("shared_services", "shared-services.yml"),
    ] {
        if let Some((line, _)) = top_entry(key) {
            let block = entry_lines(&lines, line);
            plan.push((
                fragment_dir.join(file),
                format!("{}\n", lines[block.clone()].join("\n")),
            ));
            moved.push(block);
        }
    }
    if moved.is_empty() {
        return Err(format!(
            "nothing to split in {} (no repos/environments/presets/shared_services)",
            config_path.display()
        ));
    }
    let result = SplitResult {
        root: config_path.to_path_buf(),
        fragments: plan.iter().map(|(file, _)| file.clone()).collect(),
        backup: PathBuf::from(format!("{}.bak", config_path.display())),
    };
    if dry {
        return Ok(result);
    }
    let new_root = remove_ranges(&lines, moved);
    std::fs::write(&result.backup, &text).map_err(|error| format!("backup: {error}"))?;
    if let Ok(entries) = std::fs::read_dir(fragment_dir.join("repos")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_split_repo_fragment(&name) {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    eprintln!("split: remove {name}: {error}");
                }
            }
        }
    }
    for (file, body) in &plan {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(file, body).map_err(|error| format!("write {}: {error}", file.display()))?;
    }
    std::fs::write(config_path, new_root).map_err(|error| format!("write root: {error}"))?;
    Ok(result)
}

/// Sets and removes keys of a repo's `env:` in whichever config file declares the repo, returning that file.
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
    let (file, text, root) = config_files(config_path)
        .into_iter()
        .find_map(|file| {
            let text = std::fs::read_to_string(&file).ok()?;
            let root = yaml_node::parse(&text).ok()??;
            root.get("repos")?.get(repo)?;
            Some((file, text, root))
        })
        .ok_or_else(|| format!("repo {repo:?} is not in any config file"))?;
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
        std::fs::write(&root, "session: shop\n# keep me\n").expect("pom.yml");
        std::fs::create_dir_all(dir.path().join("pom.d/repos")).expect("pom.d");
        let fragment = dir.path().join("pom.d/repos/01-api.yml");
        std::fs::write(
            &fragment,
            "repos:\n  api:\n    alias: be # the backend\n    env:\n      A: \"1\"\n      B: two\n    services:\n      web:\n        cmd: run\n  web:\n    services: {}\n",
        )
        .expect("fragment");
        let edited = edit_repo_env(
            &root,
            "api",
            &[("C".into(), "x y".into()), ("A".into(), "9".into())],
            &["B".into()],
        )
        .expect("edit");
        assert_eq!(edited, fragment);
        let text = std::fs::read_to_string(&fragment).expect("read");
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
    fn split_moves_blocks_as_written_and_normalize_cleans_first() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!("pom-maintain-{}", std::process::id()));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let config = temp.join("pom.yml");
        std::fs::write(&config, CONFIG).map_err(|error| error.to_string())?;
        let dry = split(&config, true)?;
        assert_eq!(dry.fragments.len(), 3);
        assert_eq!(
            std::fs::read_to_string(&config).map_err(|error| error.to_string())?,
            CONFIG
        );

        let removed = normalize(&config)?;
        assert_eq!(removed, ["webhook", "proxy", "plugins", "exposes"]);
        let root = std::fs::read_to_string(&config).map_err(|error| error.to_string())?;
        assert_eq!(root, "session: demo\n# repos below\n");
        let api = std::fs::read_to_string(temp.join("pom.d/repos/01-api.yml"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            api,
            "repos:\n  api:\n    services:\n      web:\n        cmd: rails s\n"
        );
        let shared = std::fs::read_to_string(temp.join("pom.d/shared-services.yml"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            shared,
            "shared_services:\n  postgres:\n    type: postgres\n"
        );
        assert!(temp.join("pom.yml.bak").exists());
        let loaded = crate::Config::load(&config).map_err(|error| error.message)?;
        assert_eq!(loaded.repos.len(), 2);
        assert!(split(&config, false).is_err(), "nothing left to split");
        if let Err(error) = std::fs::remove_dir_all(&temp) {
            eprintln!("cleanup: {error}");
        }
        Ok(())
    }
}

/// The config file that declares `repo`, its text, and the line (0-based) of the repo's key.
fn repo_file(config_path: &Path, repo: &str) -> Result<(PathBuf, String, usize), String> {
    config_files(config_path)
        .into_iter()
        .find_map(|file| {
            let text = std::fs::read_to_string(&file).ok()?;
            let root = yaml_node::parse(&text).ok()??;
            let line = root
                .get("repos")?
                .entries()?
                .iter()
                .find(|(key, _)| key.text() == repo)
                .map(|(key, _)| key.line.saturating_sub(1))?;
            Some((file, text, line))
        })
        .ok_or_else(|| format!("repo {repo:?} is not in any config file"))
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

/// Gives `repo` a new alias and rewrites every `{{<old>.` reference to it across the config files. Returns the
/// files that changed.
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
    let mut edits = Vec::new();
    for path in config_files(config_path) {
        let before = if path == file {
            text.clone()
        } else {
            std::fs::read_to_string(&path).map_err(|error| error.to_string())?
        };
        let base = if path == file {
            renamed.clone()
        } else {
            before.clone()
        };
        let next = base.replace(&from, &to);
        if next != before {
            edits.push((path, before, next));
        }
    }
    write_checked(config_path, &edits)?;
    Ok(edits.into_iter().map(|(path, _, _)| path).collect())
}

/// Takes `repo` out of the config: its entry goes, and a `pom.d` fragment left with nothing else is deleted.
/// Refused (nothing written) when the rest of the config still refers to it. Returns the file edited.
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
    let dir = config_path.parent().unwrap_or(Path::new("."));
    let referring: Vec<String> = config_files(config_path)
        .iter()
        .filter_map(|path| {
            let body = if *path == file {
                next.clone()
            } else {
                std::fs::read_to_string(path).ok()?
            };
            needles
                .iter()
                .any(|needle| body.contains(needle.as_str()))
                .then(|| path.strip_prefix(dir).unwrap_or(path).display().to_string())
        })
        .collect();
    if !referring.is_empty() {
        return Err(format!(
            "{repo} is still referred to in {}; change those first",
            referring.join(", ")
        ));
    }
    let emptied = file != config_path
        && yaml_node::parse(&next).ok().flatten().is_none_or(|root| {
            root.entries()
                .unwrap_or_default()
                .iter()
                .all(|(key, value)| {
                    key.text() == "repos"
                        && value.entries().is_none_or(|entries| entries.is_empty())
                })
        });
    if emptied {
        std::fs::remove_file(&file)
            .map_err(|error| format!("remove {}: {error}", file.display()))?;
        if let Err(problem) = crate::Config::load(config_path)
            .map_err(|error| error.message)
            .and_then(|config| config.validate())
        {
            if let Err(error) = std::fs::write(&file, &text) {
                return Err(format!(
                    "{problem}; restoring {} failed: {error}",
                    file.display()
                ));
            }
            return Err(format!("the edit would break the config: {problem}"));
        }
        return Ok(file);
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

    #[test]
    fn renaming_an_alias_rewrites_references_in_every_file() {
        let (temp, config) = project(&[
            ("pom.yml", "session: demo\n"),
            ("pom.d/repos/01-api.yml", "repos:\n  api:\n    alias: be\n    services:\n      web:\n        cmd: rails s\n        port: true\n"),
            ("pom.d/repos/02-web.yml", "repos:\n  web:\n    env:\n      API_URL: \"{{be.web.url}}\"\n"),
        ]);
        let changed = rename_alias(&config, "api", "backend").expect("rename");
        assert_eq!(changed.len(), 2);
        let api = std::fs::read_to_string(temp.path().join("pom.d/repos/01-api.yml")).expect("api");
        assert!(api.contains("    alias: backend\n"), "{api}");
        let web = std::fs::read_to_string(temp.path().join("pom.d/repos/02-web.yml")).expect("web");
        assert!(web.contains("{{backend.web.url}}"), "{web}");
        assert!(
            rename_alias(&config, "api", "web").is_err(),
            "taken by another repo"
        );
    }

    #[test]
    fn removing_a_repo_deletes_its_own_fragment_and_refuses_while_referenced() {
        let (temp, config) = project(&[
            ("pom.yml", "session: demo\n"),
            ("pom.d/repos/01-api.yml", "repos:\n  api:\n    services:\n      web:\n        cmd: rails s\n        port: true\n"),
            ("pom.d/repos/02-web.yml", "repos:\n  web:\n    env:\n      API_URL: \"{{api.web.url}}\"\n"),
            ("pom.d/repos/03-jobs.yml", "repos:\n  jobs:\n    services:\n      worker:\n        cmd: sidekiq\n"),
        ]);
        assert!(
            remove_repo(&config, "api").is_err(),
            "web still points at api"
        );
        assert!(temp.path().join("pom.d/repos/01-api.yml").exists());
        let removed = remove_repo(&config, "jobs").expect("remove");
        assert_eq!(removed, temp.path().join("pom.d/repos/03-jobs.yml"));
        assert!(!removed.exists());
        let config_now = crate::Config::load(&config).expect("loads");
        assert!(!config_now.repos.contains_key("jobs"));
    }
}
