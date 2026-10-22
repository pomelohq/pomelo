//! The env `pom run` exports: the repo's workspace env, and under the test profile the repo's own test env
//! file on top, so a test framework sees what CI sees while still talking to this workspace's databases.

use std::path::Path;

use indexmap::IndexMap;
use pom_config::RepoTest;
use pom_layout::WorkspaceState;

use crate::env_files::{alias, is_secret_template, WorkspaceEnv, HEADER};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunProfile {
    Dev,
    Test,
}

impl RunProfile {
    pub fn name(self) -> &'static str {
        match self {
            RunProfile::Dev => "dev",
            RunProfile::Test => "test",
        }
    }
}

pub const SOURCE_WORKSPACE: &str = "workspace";
pub const SOURCE_CONNECTION: &str = "connection";
pub const SOURCE_TEST_ENV: &str = "pom.yml test.env";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEnvLine {
    pub key: String,
    pub value: String,
    /// `workspace`, `connection`, the env file's path, `pom.yml env <file>` or `pom.yml test.env`.
    pub source: String,
    pub connection: bool,
    pub secret: bool,
    /// The env file that set this connection variable too, and lost.
    pub kept_over: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEnv {
    pub profile: RunProfile,
    /// The test env files that were read, in order.
    pub env_files: Vec<String>,
    pub lines: Vec<RunEnvLine>,
}

impl RunEnv {
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.lines
            .iter()
            .map(|line| (line.key.clone(), line.value.clone()))
            .collect()
    }
}

impl WorkspaceEnv<'_> {
    /// What a command run in `repo`'s worktree gets. Under `Dev` it is exactly `repo_env`.
    pub fn run_env(&self, repo: &str, profile: RunProfile, worktree: &Path) -> RunEnv {
        let mut run = RunEnv {
            profile,
            env_files: Vec::new(),
            lines: Vec::new(),
        };
        let Some(dir) = self.config.repos.get(repo) else {
            return run;
        };
        let state = WorkspaceState::load(&self.folder());
        let env_name = state
            .service_envs
            .get(alias(repo, dir))
            .map_or("", String::as_str);
        let db_names = self.db_names(dir);
        let line = |key: String, value: String, template: &str, source: &str| {
            let connection = pom_env::is_connection_var(self.config, &key, template);
            let source = match source {
                SOURCE_WORKSPACE if connection => SOURCE_CONNECTION,
                other => other,
            };
            RunEnvLine {
                secret: is_secret_template(template),
                connection,
                source: source.to_string(),
                kept_over: None,
                key,
                value,
            }
        };
        let mut lines: IndexMap<String, RunEnvLine> = self
            .resolve(&dir.env, env_name, &db_names)
            .into_iter()
            .map(|(key, value)| {
                let template = dir.env.get(&key).map_or("", String::as_str);
                (key.clone(), line(key, value, template, SOURCE_WORKSPACE))
            })
            .collect();
        if profile == RunProfile::Test {
            let test = dir.test.clone().unwrap_or_default();
            let file = test.env_file().to_string();
            let names = [file.clone(), format!("{file}.local")];
            for name in &names {
                let Some(text) = std::fs::read_to_string(worktree.join(name)).ok() else {
                    continue;
                };
                // A file pom generated holds the dev env again; its pom.yml entry is applied below instead.
                if text.lines().next() == Some(HEADER) {
                    continue;
                }
                run.env_files.push(name.clone());
                let outer = |key: &str| lines.get(key).map(|line| line.value.clone());
                for (key, value) in crate::dotenv::parse(&text, &outer) {
                    match lines.get_mut(&key) {
                        Some(existing) if existing.connection => {
                            existing.kept_over = Some(name.clone());
                        }
                        _ => {
                            let from_file = RunEnvLine {
                                key: key.clone(),
                                value,
                                source: name.clone(),
                                connection: false,
                                secret: false,
                                kept_over: None,
                            };
                            lines.insert(key, from_file);
                        }
                    }
                }
            }
            for entry in dir
                .env_output
                .iter()
                .filter(|entry| names.contains(&entry.file))
            {
                let source = format!("pom.yml env {}", entry.file);
                self.overlay(&mut lines, &entry.env, env_name, &db_names, &source, &line);
            }
            self.overlay(
                &mut lines,
                &test.env,
                env_name,
                &db_names,
                SOURCE_TEST_ENV,
                &line,
            );
        }
        run.lines = lines.into_values().collect();
        run.lines.sort_by(|a, b| a.key.cmp(&b.key));
        run
    }

    fn overlay(
        &self,
        lines: &mut IndexMap<String, RunEnvLine>,
        templates: &IndexMap<String, String>,
        env_name: &str,
        db_names: &IndexMap<String, String>,
        source: &str,
        line: &dyn Fn(String, String, &str, &str) -> RunEnvLine,
    ) {
        for (key, value) in self.resolve(templates, env_name, db_names) {
            let template = templates.get(&key).map_or("", String::as_str);
            lines.insert(key.clone(), line(key, value, template, source));
        }
    }
}

/// Whether `pom run <command>` in `repo` runs its tests, so the test profile applies without a flag.
pub fn runs_tests(test: Option<&RepoTest>, command: &str) -> bool {
    let Some(test) = test else {
        return false;
    };
    command.eq_ignore_ascii_case("test") || (!test.command.is_empty() && command == test.command)
}
