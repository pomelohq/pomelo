use std::path::{Path, PathBuf};

use pom_config::yaml_node::{self, Node};

use crate::{ResolvedRun, StackFacts};

const BUILTIN: [&str; 5] = [
    include_str!("../assets/stacks/go.yaml"),
    include_str!("../assets/stacks/java.yaml"),
    include_str!("../assets/stacks/js.yaml"),
    include_str!("../assets/stacks/python.yaml"),
    include_str!("../assets/stacks/ruby.yaml"),
];

const MANIFESTS: [&str; 9] = [
    "requirements.txt",
    "pyproject.toml",
    "Pipfile",
    "package.json",
    "go.mod",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

#[derive(Clone, Debug, Default)]
struct ManifestMatch {
    file: String,
    pattern: String,
}

#[derive(Clone, Debug, Default)]
struct Match {
    markers: Vec<String>,
    any_markers: Vec<String>,
    in_manifest: Vec<ManifestMatch>,
    any_manifest: Vec<ManifestMatch>,
}

#[derive(Clone, Debug, Default)]
struct RunRule {
    kind: String,
    cmd: String,
    when_dep: String,
    when_pm: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Rule {
    id: String,
    language: String,
    framework: String,
    priority: i64,
    detect: Match,
    package_managers: Vec<(String, String)>,
    default_pm: Option<String>,
    install: Option<Vec<(String, String)>>,
    run: Vec<RunRule>,
    setup: Vec<String>,
    default_port: u16,
    dep_cache: String,
}

fn strings(node: Option<&Node>) -> Vec<String> {
    node.and_then(Node::items)
        .map(|items| items.iter().map(|item| item.text().to_string()).collect())
        .unwrap_or_default()
}

fn manifest_matches(node: Option<&Node>) -> Vec<ManifestMatch> {
    node.and_then(Node::items)
        .map(|items| {
            items
                .iter()
                .map(|item| ManifestMatch {
                    file: item
                        .get("file")
                        .map(Node::text)
                        .unwrap_or_default()
                        .to_string(),
                    pattern: item
                        .get("pattern")
                        .map(Node::text)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text(node: &Node, key: &str) -> String {
    node.get(key)
        .map(Node::text)
        .unwrap_or_default()
        .to_string()
}

fn parse_rule(node: &Node) -> Rule {
    let detect = node.get("detect");
    Rule {
        id: text(node, "id"),
        language: text(node, "language"),
        framework: text(node, "framework"),
        priority: text(node, "priority").parse().unwrap_or(0),
        detect: Match {
            markers: strings(detect.and_then(|detect| detect.get("markers"))),
            any_markers: strings(detect.and_then(|detect| detect.get("any_markers"))),
            in_manifest: manifest_matches(detect.and_then(|detect| detect.get("in_manifest"))),
            any_manifest: manifest_matches(detect.and_then(|detect| detect.get("any_manifest"))),
        },
        package_managers: node
            .get("package_managers")
            .and_then(Node::items)
            .map(|items| {
                items
                    .iter()
                    .map(|item| (text(item, "lockfile"), text(item, "pm")))
                    .collect()
            })
            .unwrap_or_default(),
        default_pm: node
            .get("default_pm")
            .map(|value| value.text().to_string())
            .filter(|pm| !pm.is_empty()),
        install: node.get("install").and_then(Node::entries).map(|entries| {
            entries
                .iter()
                .map(|(key, value)| (key.text().to_string(), value.text().to_string()))
                .collect()
        }),
        run: node
            .get("run")
            .and_then(Node::items)
            .map(|items| {
                items
                    .iter()
                    .map(|item| RunRule {
                        kind: text(item, "kind"),
                        cmd: text(item, "cmd"),
                        when_dep: text(item, "when_dep"),
                        when_pm: text(item, "when_pm"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        setup: strings(node.get("setup")),
        default_port: text(node, "default_port").parse().unwrap_or(0),
        dep_cache: text(node, "dep_cache"),
    }
}

fn parse_rules(source: &str) -> Vec<Rule> {
    yaml_node::parse(source)
        .ok()
        .flatten()
        .and_then(|root| {
            root.items()
                .map(|items| items.iter().map(parse_rule).collect())
        })
        .unwrap_or_default()
}

fn rules_in(dir: &Path) -> Vec<Rule> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|source| parse_rules(&source))
        .collect()
}

/// The built-in rules, then the user's (`~/.config/pom/stacks`), then the project's (`.pom/stacks`).
pub(crate) fn load_rules() -> Vec<Rule> {
    let mut rules: Vec<Rule> = BUILTIN
        .iter()
        .flat_map(|source| parse_rules(source))
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        rules.extend(rules_in(&PathBuf::from(home).join(".config/pom/stacks")));
    }
    rules.extend(rules_in(Path::new(".pom/stacks")));
    rules
}

fn matcher(pattern: &str) -> Option<globset::GlobMatcher> {
    globset::Glob::new(pattern)
        .ok()
        .map(|glob| glob.compile_matcher())
}

/// Paths under `root` matching `pattern` segment by segment (`*`, `?`, `[..]`; `**` acts like `*`), sorted.
pub(crate) fn glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut current = vec![root.to_path_buf()];
    for segment in pattern.split('/').filter(|segment| !segment.is_empty()) {
        let wild = segment.contains(['*', '?', '[']);
        let mut next = Vec::new();
        for dir in &current {
            if !wild {
                let candidate = dir.join(segment);
                if candidate.exists() {
                    next.push(candidate);
                }
                continue;
            }
            let segment = segment.replace("**", "*");
            let Some(matcher) = matcher(&segment) else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .filter(|entry| matcher.is_match(entry.file_name()))
                .map(|entry| entry.path())
                .collect();
            found.sort();
            next.extend(found);
        }
        current = next;
    }
    if current.len() == 1 && current[0] == root {
        return Vec::new();
    }
    current
}

/// The first file named like `base` below `root`, skipping dependency and VCS folders.
fn walk_find(root: &Path, base: &str) -> Option<PathBuf> {
    let matcher = matcher(base)?;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        let mut subdirs = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if is_dir {
                if !matches!(name.to_str(), Some("node_modules" | ".git" | "vendor")) {
                    subdirs.push(entry.path());
                }
            } else if matcher.is_match(&name) {
                return Some(entry.path());
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    None
}

/// A plain name, a glob (`next.config.*`) or a recursive `**/name`.
pub(crate) fn has_file(root: &Path, pattern: &str) -> bool {
    match pattern.strip_prefix("**/") {
        Some(base) => walk_find(root, base).is_some(),
        None => !glob(root, pattern).is_empty(),
    }
}

fn manifest_has(root: &Path, manifest: &ManifestMatch) -> bool {
    let path = match manifest.file.strip_prefix("**/") {
        Some(base) => walk_find(root, base),
        None => glob(root, &manifest.file).into_iter().next(),
    };
    let Some(text) = path.and_then(|path| std::fs::read_to_string(path).ok()) else {
        return false;
    };
    match regex::Regex::new(&manifest.pattern) {
        Ok(pattern) => pattern.is_match(&text),
        Err(_) => text.contains(&manifest.pattern),
    }
}

fn rule_matches(root: &Path, detect: &Match) -> bool {
    if detect.markers.is_empty()
        && detect.any_markers.is_empty()
        && detect.in_manifest.is_empty()
        && detect.any_manifest.is_empty()
    {
        return false;
    }
    detect.markers.iter().all(|marker| has_file(root, marker))
        && (detect.any_markers.is_empty()
            || detect
                .any_markers
                .iter()
                .any(|marker| has_file(root, marker)))
        && detect
            .in_manifest
            .iter()
            .all(|manifest| manifest_has(root, manifest))
        && (detect.any_manifest.is_empty()
            || detect
                .any_manifest
                .iter()
                .any(|manifest| manifest_has(root, manifest)))
}

/// A framework rule without package managers of its own takes its language's base rule's.
fn inherit(winner: &mut Rule, rules: &[Rule]) {
    if !winner.package_managers.is_empty() || winner.default_pm.is_some() {
        return;
    }
    if let Some(base) = rules
        .iter()
        .find(|rule| rule.language == winner.language && rule.framework.is_empty())
    {
        winner.package_managers = base.package_managers.clone();
        winner.default_pm = base.default_pm.clone();
        if winner.install.is_none() {
            winner.install = base.install.clone();
        }
        if winner.dep_cache.is_empty() {
            winner.dep_cache = base.dep_cache.clone();
        }
    }
}

fn dep_present(root: &Path, dep: &str) -> bool {
    let pattern = regex::escape(dep);
    MANIFESTS.iter().any(|file| {
        manifest_has(
            root,
            &ManifestMatch {
                file: file.to_string(),
                pattern: pattern.clone(),
            },
        ) || manifest_has(
            root,
            &ManifestMatch {
                file: format!("**/{file}"),
                pattern: pattern.clone(),
            },
        )
    })
}

fn resolve(root: &Path, lock_root: &Path, rule: &Rule) -> StackFacts {
    let pm = rule
        .package_managers
        .iter()
        .find(|(lockfile, _)| {
            has_file(root, lockfile) || (lock_root != root && has_file(lock_root, lockfile))
        })
        .map(|(_, pm)| pm.clone())
        .unwrap_or_else(|| rule.default_pm.clone().unwrap_or_default());
    let fill = |text: &str| {
        text.replace("{{pm}}", &pm)
            .replace("{{port}}", &rule.default_port.to_string())
    };
    let install = rule
        .install
        .as_ref()
        .and_then(|install| install.iter().find(|(name, _)| *name == pm))
        .map(|(_, command)| fill(command))
        .unwrap_or_default();
    StackFacts {
        dir: String::new(),
        rule_id: rule.id.clone(),
        language: rule.language.clone(),
        framework: rule.framework.clone(),
        package_manager: pm.clone(),
        install,
        run: rule
            .run
            .iter()
            .filter(|run| run.when_dep.is_empty() || dep_present(root, &run.when_dep))
            .filter(|run| run.when_pm.is_empty() || run.when_pm == pm)
            .map(|run| ResolvedRun {
                kind: run.kind.clone(),
                cmd: fill(&run.cmd),
            })
            .collect(),
        setup: rule.setup.clone(),
        port: rule.default_port,
        dep_cache: rule.dep_cache.clone(),
    }
}

/// The best rule for `dir`: highest priority, then the longer (more specific) id. `lock_root` is where a
/// lockfile may also sit (the repo root, for a monorepo member).
pub(crate) fn detect_with(rules: &[Rule], dir: &Path, lock_root: &Path) -> Option<StackFacts> {
    let mut matched: Vec<&Rule> = rules
        .iter()
        .filter(|rule| rule_matches(dir, &rule.detect))
        .collect();
    matched.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(b.id.len().cmp(&a.id.len()))
    });
    let mut winner = (*matched.first()?).clone();
    inherit(&mut winner, rules);
    Some(resolve(dir, lock_root, &winner))
}
