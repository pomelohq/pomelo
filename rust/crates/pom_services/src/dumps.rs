//! Databases and volumes as portable files: `pg_dump -Fc` out of the shared Postgres and `pg_restore` back in,
//! and a workspace's labelled Docker volumes as tar streams. Everything runs through `docker`, so the host needs
//! no Postgres client.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};

use pom_config::Config;

use crate::control::{ServiceError, ServiceRunner};
use crate::shared::{output_text, POSTGRES};

/// Runs the one-off `tar` that reads or fills a volume.
const VOLUME_IMAGE: &str = "alpine:3.20";
const PG_NAME_LIMIT: usize = 63;

/// A database a workspace owns, with the config entry it comes from so another machine can name its copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseTemplate {
    pub repo: String,
    /// The `databases:` key, or the shared service a `shared:` ref names.
    pub key: String,
    /// `databases:` names carry the session prefix; a shared ref's `db_name` does not.
    pub session_prefixed: bool,
    pub template: String,
    pub name: String,
}

/// The databases `branch` owns (not shared with main), each with where its name comes from.
pub fn database_templates(
    config: &Config,
    branch: &str,
    keep: impl Fn(&str) -> bool,
) -> Vec<DatabaseTemplate> {
    let owned = crate::owned_database_names(config, branch, &keep);
    let mut found: Vec<DatabaseTemplate> = Vec::new();
    for (repo, dir) in &config.repos {
        if !dir.has_worktree_config() || !keep(repo) {
            continue;
        }
        let shared = dir
            .shared_refs
            .iter()
            .filter(|shared| !shared.db_name.is_empty())
            .map(|shared| (shared.name.clone(), shared.db_name.clone(), false));
        let own = dir
            .databases
            .iter()
            .map(|(key, template)| (key.clone(), template.clone(), true));
        for (key, template, session_prefixed) in shared.chain(own) {
            let name = database_name(config, &template, session_prefixed, branch);
            if owned.contains(&name) && !found.iter().any(|known| known.name == name) {
                found.push(DatabaseTemplate {
                    repo: repo.clone(),
                    key,
                    session_prefixed,
                    template,
                    name,
                });
            }
        }
    }
    found
}

/// The Postgres name a database template resolves to for `branch` in this project.
pub fn database_name(
    config: &Config,
    template: &str,
    session_prefixed: bool,
    branch: &str,
) -> String {
    let resolved = pom_env::resolve_branch_tokens(template, branch);
    let name = if session_prefixed {
        format!("{}_{resolved}", config.session)
    } else {
        resolved
    };
    let mut end = name.len().min(PG_NAME_LIMIT);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

impl ServiceRunner {
    /// Streams `pg_dump -Fc` of `db` into `out`; returns the bytes written.
    pub fn dump_database(
        &self,
        config: &Config,
        db: &str,
        out: &mut dyn Write,
    ) -> Result<u64, ServiceError> {
        let postgres = self.postgres(config);
        let args = postgres.tool_args("pg_dump", db, false, &["-Fc", "--no-owner"]);
        let mut child = self.spawn_docker(&args, Stdio::null())?;
        let copied = match child.stdout.take() {
            Some(mut stdout) => std::io::copy(&mut stdout, out),
            None => Ok(0),
        };
        finish(child, &format!("pg_dump {db}"))?;
        copied.map_err(ServiceError::Io)
    }

    /// Replaces `db` with the contents of a `pg_dump -Fc` archive read from `input`.
    pub fn restore_database(
        &self,
        config: &Config,
        db: &str,
        input: &mut dyn Read,
    ) -> Result<(), ServiceError> {
        let postgres = self.postgres(config);
        self.terminate(&postgres, db);
        self.drop_databases(config, &[db.to_string()])?;
        // Created bare: the dump brings its own extensions, and one made here would collide with them.
        let output = self.psql(&postgres, POSTGRES, &format!("CREATE DATABASE \"{db}\""))?;
        if !output.status.success() {
            return Err(ServiceError::Io(std::io::Error::other(format!(
                "create {db}: {}",
                output_text(&output).trim()
            ))));
        }
        let args = postgres.tool_args(
            "pg_restore",
            db,
            true,
            &["--no-owner", "--no-privileges", "--exit-on-error"],
        );
        let mut child = self.spawn_docker(&args, Stdio::piped())?;
        let fed = match child.stdin.take() {
            Some(mut stdin) => std::io::copy(input, &mut stdin).map(|_| ()),
            None => Ok(()),
        };
        finish(child, &format!("pg_restore {db}"))?;
        fed.map_err(ServiceError::Io)
    }

    /// Docker volumes labelled as this workspace's (see `workspace_volume_label`).
    pub fn workspace_volumes(&self, branch: &str) -> Result<Vec<String>, ServiceError> {
        let args = [
            "volume".to_string(),
            "ls".into(),
            "-q".into(),
            "--filter".into(),
            format!("label={}", self.workspace_volume_label(branch)),
        ];
        let output = self.docker(&args)?;
        if !output.status.success() {
            return Err(ServiceError::Io(std::io::Error::other(format!(
                "docker volume ls: {}",
                output_text(&output).trim()
            ))));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Streams the files of volume `name` as a tar archive into `out`.
    pub fn export_volume(&self, name: &str, out: &mut dyn Write) -> Result<u64, ServiceError> {
        let mount = format!("{name}:/volume:ro");
        let args = [
            "run",
            "--rm",
            "-v",
            &mount,
            VOLUME_IMAGE,
            "tar",
            "-C",
            "/volume",
            "-cf",
            "-",
            ".",
        ]
        .map(str::to_string);
        let mut child = self.spawn_docker(&args, Stdio::null())?;
        let copied = match child.stdout.take() {
            Some(mut stdout) => std::io::copy(&mut stdout, out),
            None => Ok(0),
        };
        finish(child, &format!("export volume {name}"))?;
        copied.map_err(ServiceError::Io)
    }

    /// Creates volume `name` labelled as `branch`'s and fills it from a tar archive read from `input`.
    pub fn import_volume(
        &self,
        name: &str,
        branch: &str,
        input: &mut dyn Read,
    ) -> Result<(), ServiceError> {
        let create = [
            "volume".to_string(),
            "create".into(),
            "--label".into(),
            self.workspace_volume_label(branch),
            name.into(),
        ];
        let output = self.docker(&create)?;
        if !output.status.success() {
            return Err(ServiceError::Io(std::io::Error::other(format!(
                "create volume {name}: {}",
                output_text(&output).trim()
            ))));
        }
        let mount = format!("{name}:/volume");
        let args = [
            "run",
            "--rm",
            "-i",
            "-v",
            &mount,
            VOLUME_IMAGE,
            "tar",
            "-C",
            "/volume",
            "-xf",
            "-",
        ]
        .map(str::to_string);
        let mut child = self.spawn_docker(&args, Stdio::piped())?;
        let fed = match child.stdin.take() {
            Some(mut stdin) => std::io::copy(input, &mut stdin).map(|_| ()),
            None => Ok(()),
        };
        finish(child, &format!("import volume {name}"))?;
        fed.map_err(ServiceError::Io)
    }

    fn spawn_docker(&self, args: &[String], stdin: Stdio) -> Result<Child, ServiceError> {
        Command::new(&self.docker)
            .args(args)
            .current_dir(&self.project_root)
            .env("PATH", crate::tool_path())
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                ServiceError::Io(std::io::Error::new(
                    error.kind(),
                    format!("docker: {error}"),
                ))
            })
    }
}

/// Waits for a streaming docker command; its stderr becomes the error when it fails.
fn finish(mut child: Child, what: &str) -> Result<(), ServiceError> {
    let mut errors = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        if let Err(error) = stderr.read_to_string(&mut errors) {
            errors.push_str(&error.to_string());
        }
    }
    let status = child.wait().map_err(ServiceError::Io)?;
    if status.success() {
        return Ok(());
    }
    let tail: Vec<&str> = errors
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = tail[tail.len().saturating_sub(10)..].join("\n");
    Err(ServiceError::Io(std::io::Error::other(format!(
        "{what}: {}",
        if tail.is_empty() {
            format!("exit {}", status.code().unwrap_or(-1))
        } else {
            tail
        }
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_name_only_the_databases_a_branch_owns() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("pom.yml");
        std::fs::write(
            &path,
            "session: shop\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n      cache: \"cache\"\n",
        )
        .expect("pom.yml");
        let config = Config::load(&path).expect("config");
        let found = database_templates(&config, "feat-x", |_| true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "shop_api_feat-x");
        assert_eq!(found[0].key, "main");
        assert!(found[0].session_prefixed);
        assert_eq!(
            database_name(&config, &found[0].template, true, "other"),
            "shop_api_other"
        );
    }
}
