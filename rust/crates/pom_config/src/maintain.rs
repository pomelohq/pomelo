use std::path::{Path, PathBuf};

use crate::yaml_node::{self, Node};
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

#[cfg(test)]
mod tests {
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
