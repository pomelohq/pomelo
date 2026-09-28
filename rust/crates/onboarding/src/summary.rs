use pom_config::Config;

/// A run of text; `strong` for the names (repos, services) a summary line is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub strong: bool,
}

fn plain(text: impl Into<String>) -> Segment {
    Segment {
        text: text.into(),
        strong: false,
    }
}

fn strong(text: impl Into<String>) -> Segment {
    Segment {
        text: text.into(),
        strong: true,
    }
}

const ENV_LINES_PER_REPO: usize = 3;

/// What the config sets up, a few lines per repo: its services, setup and migrations, and the env it
/// wires to shared services and other repos.
pub fn summary_lines(config: &Config) -> Vec<Vec<Segment>> {
    let mut lines = Vec::new();
    for (repo, dir) in &config.repos {
        let name = if dir.alias.is_empty() {
            repo.clone()
        } else {
            dir.alias.clone()
        };
        if !dir.services.is_empty() {
            let mut line = vec![strong(name.clone()), plain(": services ")];
            for (index, (service, spec)) in dir.services.iter().enumerate() {
                if index > 0 {
                    line.push(plain(", "));
                }
                line.push(strong(service.clone()));
                let cmd = spec.active_cmd("").trim();
                if !cmd.is_empty() {
                    line.push(plain(format!(" ({})", short_command(cmd))));
                }
            }
            lines.push(line);
        }
        let mut steps = Vec::new();
        for (label, commands) in [
            ("setup", dir.effective_setup()),
            ("migrate", dir.effective_migrate()),
            ("seed", dir.seed.clone()),
        ] {
            if !commands.is_empty() {
                steps.push(format!("{label} {}", commands.join(" && ")));
            }
        }
        if !steps.is_empty() {
            lines.push(vec![
                strong(name.clone()),
                plain(format!(": {}", steps.join(" - "))),
            ]);
        }
        let wired = dir
            .env
            .iter()
            .filter(|(_, value)| value.contains("{{"))
            .take(ENV_LINES_PER_REPO);
        for (key, value) in wired {
            lines.push(vec![
                strong(name.clone()),
                plain(format!(": {key} -> {value}")),
            ]);
        }
    }
    lines
}

fn short_command(cmd: &str) -> String {
    const LIMIT: usize = 48;
    let first = cmd.lines().next().unwrap_or_default();
    if first.chars().count() <= LIMIT {
        return first.to_string();
    }
    let cut: String = first.chars().take(LIMIT).collect();
    format!("{cut}...")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub repos: usize,
    pub services: usize,
    pub shared: usize,
    pub databases: usize,
}

pub fn counts(config: &Config) -> Counts {
    Counts {
        repos: config.repos.len(),
        services: config
            .repos
            .values()
            .map(|dir| dir.services.len())
            .sum::<usize>()
            + config.workspace_services.len(),
        shared: config.shared_services.len(),
        databases: config.repos.values().map(|dir| dir.databases.len()).sum(),
    }
}
