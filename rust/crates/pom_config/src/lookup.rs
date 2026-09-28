use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::schema::{Config, Dir, EnvFileEntry, Service, Shortcut, DEFAULT_ENV_FILE};

const SETUP_COMMAND_ORDER: [&str; 3] = ["install", "generate", "migrate"];
const SHORTCUT_COMMAND_ORDER: [&str; 7] = [
    "install", "generate", "migrate", "test", "lint", "build", "format",
];
const PREPARE_MAIN_PHASES: [&str; 3] = ["reset", "migrate", "seed"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedService {
    pub cmd: String,
    pub work_dir: PathBuf,
    pub shell_env: String,
    pub pre_start: String,
}

impl Config {
    pub fn global_default_branch(&self) -> &str {
        if self.default_branch.is_empty() {
            "main"
        } else {
            &self.default_branch
        }
    }

    pub fn default_branch_for(&self, repo: &str) -> &str {
        match self.repos.get(repo) {
            Some(dir) if !dir.default_branch.is_empty() => &dir.default_branch,
            _ => self.global_default_branch(),
        }
    }

    pub fn prepare_main_phases(&self) -> Vec<String> {
        if self.prepare_main.is_empty() {
            return PREPARE_MAIN_PHASES.map(str::to_string).to_vec();
        }
        let known: Vec<String> = self
            .prepare_main
            .iter()
            .filter(|phase| PREPARE_MAIN_PHASES.contains(&phase.as_str()))
            .cloned()
            .collect();
        if known.is_empty() {
            vec!["reset".to_string()]
        } else {
            known
        }
    }

    pub fn validate_environment(&self, name: &str) -> Result<(), String> {
        if name.is_empty() || self.environments.contains_key(name) {
            return Ok(());
        }
        if self.environments.is_empty() {
            return Err(format!(
                "environment '{name}' not found (no environments defined)"
            ));
        }
        let names: Vec<&str> = self.environments.keys().map(String::as_str).collect();
        Err(format!(
            "environment '{name}' not found (available: {})",
            names.join(", ")
        ))
    }

    /// Named service groups; with none configured, one group named after the session holds
    /// every service in repo order.
    pub fn all_workspaces(&self) -> IndexMap<String, Vec<String>> {
        let mut result = self.workspaces.clone();
        for (name, entries) in &self.combinations {
            result
                .entry(name.clone())
                .or_insert_with(|| entries.clone());
        }
        if result.is_empty() {
            let entries: Vec<String> = self
                .repos
                .values()
                .flat_map(|dir| {
                    dir.services.keys().map(move |service| {
                        if dir.alias.is_empty() {
                            service.clone()
                        } else {
                            format!("{}/{service}", dir.alias)
                        }
                    })
                })
                .collect();
            if !entries.is_empty() {
                result.insert(self.session.clone(), entries);
            }
        }
        result
    }

    /// `repo/service` (repo name or alias) or a bare service name unique across repos.
    pub fn find_service_entry(&self, entry: &str) -> Result<(String, String), String> {
        if let Some((prefix, service)) = entry.split_once('/') {
            return self
                .repos
                .iter()
                .find(|(name, dir)| {
                    (name.as_str() == prefix || dir.alias == prefix)
                        && dir.services.contains_key(service)
                })
                .map(|(name, _)| (name.clone(), service.to_string()))
                .ok_or_else(|| format!("service '{entry}' not found"));
        }
        let matches: Vec<&str> = self
            .repos
            .iter()
            .filter(|(_, dir)| dir.services.contains_key(entry))
            .map(|(name, _)| name.as_str())
            .collect();
        match matches.as_slice() {
            [] => Err(format!("service '{entry}' not found in any dir")),
            [only] => Ok((only.to_string(), entry.to_string())),
            _ => Err(format!(
                "ambiguous service '{entry}' - found in: {}",
                matches.join(", ")
            )),
        }
    }

    /// A workspace group, a repo (by name or alias), or a single service entry.
    pub fn resolve_services(&self, target: &str) -> Result<Vec<(String, String)>, String> {
        if let Some(entries) = self.all_workspaces().get(target) {
            return entries
                .iter()
                .map(|entry| self.find_service_entry(entry))
                .collect();
        }
        let repo = self
            .repos
            .get_key_value(target)
            .filter(|(_, dir)| !dir.services.is_empty())
            .or_else(|| {
                self.repos
                    .iter()
                    .find(|(_, dir)| dir.alias == target && !dir.services.is_empty())
            });
        if let Some((name, dir)) = repo {
            return Ok(dir
                .services
                .keys()
                .map(|service| (name.clone(), service.clone()))
                .collect());
        }
        self.find_service_entry(target).map(|found| vec![found])
    }

    pub fn resolve_service(
        &self,
        config_dir: &Path,
        repo: &str,
        service_name: &str,
    ) -> Result<ResolvedService, String> {
        let dir = self
            .repos
            .get(repo)
            .ok_or_else(|| format!("dir '{repo}' not found"))?;
        let service = dir
            .services
            .get(service_name)
            .ok_or_else(|| format!("service '{service_name}' not found in dir '{repo}'"))?;
        let cmd = service.active_cmd("");
        if cmd.is_empty() {
            return Err(format!("service '{repo}/{service_name}' has no 'cmd'"));
        }
        let mut work_dir = PathBuf::from(repo);
        if work_dir.is_relative() {
            let main_worktree = config_dir
                .join(format!("workspace--{}", self.global_default_branch()))
                .join(repo);
            work_dir = if main_worktree.is_dir() {
                main_worktree
            } else {
                config_dir.join(repo)
            };
        }
        if !service.dir.is_empty() {
            work_dir = work_dir.join(&service.dir);
        }
        let pick =
            |own: &str, inherited: &str| if own.is_empty() { inherited } else { own }.to_string();
        Ok(ResolvedService {
            cmd: cmd.to_string(),
            work_dir,
            shell_env: pick(&service.shell_env, &dir.shell_env),
            pre_start: pick(&service.pre_start, &dir.pre_start),
        })
    }
}

impl Dir {
    pub fn effective_setup(&self) -> Vec<String> {
        if !self.setup.is_empty() {
            return self.setup.clone();
        }
        SETUP_COMMAND_ORDER
            .iter()
            .filter_map(|key| self.commands.get(*key))
            .filter(|cmd| !cmd.is_empty())
            .cloned()
            .collect()
    }

    pub fn effective_migrate(&self) -> Vec<String> {
        if !self.migrate.is_empty() {
            return self.migrate.clone();
        }
        match self.commands.get("migrate") {
            Some(cmd) if !cmd.is_empty() => vec![cmd.clone()],
            _ => Vec::new(),
        }
    }

    /// Explicit shortcuts first, then one per named command (known lifecycle names in a fixed
    /// order, custom names sorted), skipping any command already offered.
    pub fn effective_shortcuts(&self) -> Vec<Shortcut> {
        let mut out = self.shortcuts.clone();
        let mut seen: Vec<String> = out.iter().map(|s| s.cmd.clone()).collect();
        let mut custom: Vec<&String> = self
            .commands
            .keys()
            .filter(|key| !SHORTCUT_COMMAND_ORDER.contains(&key.as_str()))
            .collect();
        custom.sort();
        let order = SHORTCUT_COMMAND_ORDER
            .iter()
            .copied()
            .chain(custom.into_iter().map(String::as_str));
        for key in order {
            let Some(cmd) = self.commands.get(key).filter(|cmd| !cmd.is_empty()) else {
                continue;
            };
            if seen.contains(cmd) {
                continue;
            }
            seen.push(cmd.clone());
            out.push(Shortcut {
                desc: lifecycle_description(key),
                cmd: cmd.clone(),
                key: key.to_string(),
            });
        }
        out
    }

    /// Environment profiles a service may switch between; `local` is always first.
    pub fn env_profiles(&self, service: Option<&Service>) -> Vec<String> {
        let list = match service {
            Some(service) if !service.profiles.is_empty() => &service.profiles,
            _ => &self.profiles,
        };
        std::iter::once("local".to_string())
            .chain(list.iter().filter(|p| p.as_str() != "local").cloned())
            .collect()
    }

    pub fn env_file_entries(&self) -> Vec<EnvFileEntry> {
        if self.env_output.is_empty() {
            return vec![EnvFileEntry {
                file: DEFAULT_ENV_FILE.to_string(),
                env: IndexMap::new(),
            }];
        }
        self.env_output.clone()
    }

    pub fn has_worktree_config(&self) -> bool {
        !self.copy.is_empty()
            || !self.setup.is_empty()
            || !self.seed.is_empty()
            || !self.databases.is_empty()
            || !self.presets.is_empty()
            || !self.commands.is_empty()
            || !self.pre_delete.is_empty()
            || !self.env.is_empty()
            || !self.env_output.is_empty()
            || !self.shared_refs.is_empty()
    }
}

fn lifecycle_description(key: &str) -> String {
    let known = match key {
        "install" => "Install dependencies",
        "generate" => "Generate types",
        "migrate" => "Migrate database",
        "test" => "Test",
        "lint" => "Lint",
        "build" => "Build",
        "format" => "Format",
        _ => {
            let mut chars = key.chars();
            return match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            };
        }
    };
    known.to_string()
}
