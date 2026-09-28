use std::path::Path;

use pom_config::yaml_node;
use serde_json::Value;

use crate::rules::{detect_with, glob, has_file, load_rules};
use crate::{detect, ResolvedRun, StackFacts};

/// Conventional app folders scanned next to a workspace's own members, for polyglot monorepos.
const CONVENTIONAL: [&str; 11] = [
    "backend",
    "frontend",
    "api",
    "server",
    "web",
    "client",
    "worker",
    "admin",
    "apps/*",
    "services/*",
    "packages/*",
];

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn clean(globs: impl IntoIterator<Item = String>) -> Vec<String> {
    globs
        .into_iter()
        .map(|glob| glob.trim().to_string())
        .filter(|glob| !glob.is_empty() && !glob.starts_with('!'))
        .collect()
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn go_work_dirs(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join("go.work")) else {
        return Vec::new();
    };
    clean(text.lines().filter_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("use").unwrap_or(line);
        let line = line.trim_matches(|c: char| " (){}".contains(c)).trim();
        (!line.is_empty() && !line.starts_with("go ") && line != "use")
            .then(|| line.strip_prefix("./").unwrap_or(line).to_string())
    }))
}

fn java_modules(root: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(text) = std::fs::read_to_string(root.join("pom.xml")) {
        if let Ok(module) = regex::Regex::new(r"<module>\s*([^<\s]+)\s*</module>") {
            dirs.extend(
                module
                    .captures_iter(&text)
                    .map(|found| found[1].trim().to_string()),
            );
        }
    }
    let include = regex::Regex::new(r#"include\(?\s*['"]:?([^'"]+)['"]"#).ok();
    for file in ["settings.gradle", "settings.gradle.kts"] {
        let (Ok(text), Some(include)) =
            (std::fs::read_to_string(root.join(file)), include.as_ref())
        else {
            continue;
        };
        dirs.extend(include.captures_iter(&text).map(|found| {
            let dir = found[1].trim().replace(':', "/");
            dir.strip_prefix('/').unwrap_or(&dir).to_string()
        }));
    }
    clean(dirs)
}

fn pnpm_globs(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) else {
        return Vec::new();
    };
    let Ok(Some(document)) = yaml_node::parse(&text) else {
        return Vec::new();
    };
    clean(
        document
            .get("packages")
            .and_then(|packages| packages.items())
            .unwrap_or_default()
            .iter()
            .map(|item| item.text().to_string()),
    )
}

fn package_json_workspaces(root: &Path) -> Vec<String> {
    let Some(package) = read_json(&root.join("package.json")) else {
        return Vec::new();
    };
    let workspaces = package.get("workspaces");
    let listed = string_array(workspaces);
    if !listed.is_empty() {
        return clean(listed);
    }
    clean(string_array(
        workspaces.and_then(|workspaces| workspaces.get("packages")),
    ))
}

fn lerna_packages(root: &Path) -> Vec<String> {
    read_json(&root.join("lerna.json"))
        .map(|lerna| clean(string_array(lerna.get("packages"))))
        .unwrap_or_default()
}

/// Every folder holding an nx `project.json` (depth-bounded, build and dependency trees skipped).
pub(crate) fn nx_projects(root: &Path) -> Vec<String> {
    const SKIP: [&str; 7] = [
        "node_modules",
        ".git",
        "dist",
        "build",
        "coverage",
        "tmp",
        ".nx",
    ];
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if !SKIP.contains(&name.as_str()) && depth < 4 {
                    stack.push((path, depth + 1));
                }
            } else if name == "project.json" && dir != root {
                if let Ok(relative) = dir.strip_prefix(root) {
                    found.push(relative.to_string_lossy().into_owned());
                }
            }
        }
    }
    found
}

fn workspace_globs(root: &Path) -> Vec<String> {
    for found in [
        go_work_dirs(root),
        java_modules(root),
        pnpm_globs(root),
        package_json_workspaces(root),
        lerna_packages(root),
    ] {
        if !found.is_empty() {
            return found;
        }
    }
    if exists(root, "turbo.json") || exists(root, "turbo.jsonc") {
        return vec!["apps/*".into(), "packages/*".into()];
    }
    if exists(root, "nx.json") {
        let projects = nx_projects(root);
        if !projects.is_empty() {
            return projects;
        }
        return vec!["apps/*".into(), "libs/*".into(), "packages/*".into()];
    }
    Vec::new()
}

/// A package.json with an importable entrypoint (main/module/exports) is a library, not an app.
fn is_js_library(dir: &Path) -> bool {
    read_json(&dir.join("package.json")).is_some_and(|package| {
        ["main", "module"].iter().any(|key| {
            package
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        }) || package
            .get("exports")
            .is_some_and(|exports| !exports.is_null())
    })
}

fn monorepo_members(root: &Path) -> Vec<String> {
    let mut globs = workspace_globs(root);
    if globs.is_empty() {
        return Vec::new();
    }
    globs.extend(CONVENTIONAL.iter().map(|glob| glob.to_string()));
    let mut members: Vec<String> = Vec::new();
    for pattern in globs {
        for path in glob(root, &pattern) {
            if !path.is_dir() {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = relative.to_string_lossy().into_owned();
            if relative.is_empty() || members.contains(&relative) {
                continue;
            }
            if let Some(facts) = detect(&path) {
                if facts.language == "js" && (facts.framework.is_empty() || is_js_library(&path)) {
                    continue;
                }
                members.push(relative);
            }
        }
    }
    members.sort();
    members
}

fn package_dep(dir: &Path, dep: &str) -> bool {
    read_json(&dir.join("package.json")).is_some_and(|package| {
        ["dependencies", "devDependencies"].iter().any(|section| {
            package
                .get(section)
                .and_then(|deps| deps.get(dep))
                .is_some()
        })
    })
}

fn nx_package_manager(root: &Path) -> &'static str {
    [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
    ]
    .iter()
    .find(|(file, _)| exists(root, file))
    .map_or("npm", |(_, pm)| pm)
}

fn nx_framework(member: &Path, root: &Path) -> (&'static str, u16) {
    if !glob(member, "next.config.*").is_empty() {
        return ("next", 3000);
    }
    if has_file(member, "nest-cli.json") {
        return ("nest", 3000);
    }
    for dir in [member, root] {
        for (dep, framework, port) in [
            ("@nestjs/core", "nest", 3000),
            ("@angular/core", "angular", 4200),
            ("next", "next", 3000),
            ("express", "express", 3000),
            ("fastify", "fastify", 3000),
        ] {
            if package_dep(dir, dep) {
                return (framework, port);
            }
        }
    }
    if !glob(member, "vite.config.*").is_empty() {
        return ("vite", 5173);
    }
    ("", 0)
}

fn nx_serve(pm: &str, name: &str) -> String {
    match pm {
        "pnpm" => format!("pnpm exec nx serve {name}"),
        "yarn" => format!("yarn nx serve {name}"),
        "bun" => format!("bunx nx serve {name}"),
        _ => format!("npx nx serve {name}"),
    }
}

/// nx apps: deps are hoisted to the root and targets inferred, so each runnable project runs via
/// `nx serve <name>`; libraries and e2e projects are left out.
fn nx_members(root: &Path) -> Vec<StackFacts> {
    let pm = nx_package_manager(root);
    let mut dirs = nx_projects(root);
    if exists(root, "project.json") {
        dirs.push(".".into());
    }
    dirs.sort();
    let mut out = Vec::new();
    for relative in dirs {
        let member = root.join(&relative);
        let Some(project) = read_json(&member.join("project.json")) else {
            continue;
        };
        let name = project
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let project_type = project
            .get("projectType")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tags = string_array(project.get("tags"));
        let targets = project.get("targets").and_then(Value::as_object);
        let has_target = |target: &str| targets.is_some_and(|targets| targets.contains_key(target));
        let base = Path::new(&relative)
            .file_name()
            .map(|base| base.to_string_lossy().into_owned())
            .unwrap_or_default();
        let e2e = name == "e2e"
            || base == "e2e"
            || name.ends_with("-e2e")
            || base.ends_with("-e2e")
            || has_target("e2e")
            || tags.iter().any(|tag| tag == "type:e2e");
        let runnable = project_type == "application"
            || ["serve", "dev", "start"]
                .iter()
                .any(|target| has_target(target));
        if project_type == "library" || e2e || !runnable {
            continue;
        }
        let name = if name.is_empty() {
            base.clone()
        } else {
            name.to_string()
        };
        let (framework, port) = nx_framework(&member, root);
        out.push(StackFacts {
            dir: if relative == "." {
                String::new()
            } else {
                relative.clone()
            },
            rule_id: "nx".into(),
            language: "js".into(),
            framework: framework.into(),
            package_manager: pm.into(),
            install: format!("{pm} install"),
            run: vec![ResolvedRun {
                kind: "server".into(),
                cmd: nx_serve(pm, &name),
            }],
            setup: Vec::new(),
            port,
            dep_cache: "per-project".into(),
        });
    }
    out
}

/// Every runnable app in a repo: one per monorepo member (with `dir` set), plus the root when it is a real
/// app itself; a single root stack otherwise.
pub fn detect_repo(root: &Path) -> Vec<StackFacts> {
    if exists(root, "nx.json") {
        let members = nx_members(root);
        if !members.is_empty() {
            return members;
        }
    }
    let rules = load_rules();
    let members = monorepo_members(root);
    let mut out = Vec::new();
    if !members.is_empty() {
        if let Some(facts) =
            detect_with(&rules, root, root).filter(|facts| !facts.framework.is_empty())
        {
            out.push(facts);
        }
    }
    for member in members {
        if let Some(mut facts) = detect_with(&rules, &root.join(&member), root) {
            facts.dir = member;
            out.push(facts);
        }
    }
    if !out.is_empty() {
        return out;
    }
    detect(root).into_iter().collect()
}
