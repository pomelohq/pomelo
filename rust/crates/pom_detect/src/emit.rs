use std::collections::BTreeMap;
use std::path::Path;

use crate::{ComposeService, ResolvedRun, ServiceKind, StackFacts};

/// One cloned repo's findings: its apps and its backing services.
#[derive(Clone, Debug, Default)]
pub struct RepoDetection {
    pub name: String,
    /// Defaults to the name.
    pub alias: String,
    pub apps: Vec<StackFacts>,
    pub shared: Vec<ComposeService>,
}

fn service_name(app: &StackFacts, run: &ResolvedRun, used: &[String]) -> String {
    let mut base = if app.dir.is_empty() {
        app.framework.clone()
    } else {
        Path::new(&app.dir)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if base.is_empty() {
        base = app.language.clone();
    }
    if base.is_empty() {
        base = "app".into();
    }
    if run.kind == "worker" {
        base.push_str("-worker");
    }
    let mut name = base.clone();
    let mut index = 2;
    while used.contains(&name) {
        name = format!("{base}-{index}");
        index += 1;
    }
    name
}

/// A YAML scalar, quoted unless it is plainly safe.
fn scalar(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _-./@".contains(c))
        && !text.starts_with([' ', '-', '@', '.'])
        && !text.ends_with(' ')
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
        )
        && text.parse::<f64>().is_err();
    if plain {
        text.to_string()
    } else {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// A draft `pom.yml` from what was detected: repos with their services, setup and shared services. It loads
/// and validates; env wiring is left to the onboarding agent.
pub fn emit(session: &str, repos: &[RepoDetection]) -> String {
    let mut out = format!("session: {}\ndefault_branch: main\n", scalar(session));
    let mut shared_types: BTreeMap<String, ()> = BTreeMap::new();
    let mut sorted: Vec<&RepoDetection> = repos.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    if !sorted.is_empty() {
        out.push_str("repos:\n");
    }
    for repo in sorted {
        out.push_str(&format!("  {}:\n", scalar(&repo.name)));
        let alias = if repo.alias.is_empty() {
            &repo.name
        } else {
            &repo.alias
        };
        out.push_str(&format!("    alias: {}\n", scalar(alias)));
        let mut used: Vec<String> = Vec::new();
        let mut setup: Vec<String> = Vec::new();
        let mut services: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
        for app in &repo.apps {
            for step in std::iter::once(&app.install).chain(app.setup.iter()) {
                if !step.is_empty() && !setup.contains(step) {
                    setup.push(step.clone());
                }
            }
            for run in &app.run {
                let name = service_name(app, run, &used);
                used.push(name.clone());
                services.insert(
                    name,
                    (
                        run.cmd.clone(),
                        app.dir.clone(),
                        run.kind == "server" && app.port > 0,
                    ),
                );
            }
        }
        if !services.is_empty() {
            out.push_str("    services:\n");
            for (name, (cmd, dir, port)) in &services {
                out.push_str(&format!(
                    "      {}:\n        cmd: {}\n",
                    scalar(name),
                    scalar(cmd)
                ));
                if !dir.is_empty() {
                    out.push_str(&format!("        dir: {}\n", scalar(dir)));
                }
                if *port {
                    out.push_str("        port: true\n");
                }
            }
        }
        if !setup.is_empty() {
            out.push_str("    setup:\n");
            for step in &setup {
                out.push_str(&format!("      - {}\n", scalar(step)));
            }
        }
        let mut shared: Vec<String> = repo
            .shared
            .iter()
            .filter(|service| {
                service.kind == ServiceKind::Shared
                    && !service.kind_name.is_empty()
                    && service.kind_name != "custom"
            })
            .map(|service| service.kind_name.clone())
            .collect();
        shared.sort();
        shared.dedup();
        if !shared.is_empty() {
            out.push_str("    shared_services:\n");
            for kind in &shared {
                out.push_str(&format!("      - {}\n", scalar(kind)));
                shared_types.insert(kind.clone(), ());
            }
        }
    }
    if !shared_types.is_empty() {
        out.push_str("shared_services:\n");
        for kind in shared_types.keys() {
            out.push_str(&format!(
                "  {}:\n    type: {}\n",
                scalar(kind),
                scalar(kind)
            ));
        }
    }
    out
}
