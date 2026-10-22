//! A project as a `.pomproj` archive: `manifest.json` (schema `pom.projbundle/v1`, with each repo's origin and
//! default branch) and the config bundle, sealed with every secret of the session when it has any. Importing it
//! writes `pom.yml`, stores the secrets under this machine's key and clones the repos on main.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use pom_config::Config;
use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

use crate::{check_major, git_text, pack_any, unpack_any, utc_timestamp, Staging};

pub const PROJECT_SCHEMA: &str = "pom.projbundle/v1";
const PROJECT_SCHEMA_PREFIX: &str = "pom.projbundle/v";
const PROJECT_SCHEMA_MAJOR: u32 = 1;
pub const PROJECT_EXTENSION: &str = "pomproj";
const SEALED_FILE: &str = "project.pombundle";
const PLAIN_FILE: &str = "pom.yml";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub schema: String,
    pub pom_version: String,
    pub created_at: String,
    pub project: String,
    pub default_branch: String,
    pub repos: Vec<ProjectRepo>,
    /// Names only; the values are sealed in the bundle.
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepo {
    pub name: String,
    pub remote_url: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExport {
    pub manifest: ProjectManifest,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectImport {
    pub manifest: ProjectManifest,
    pub root: PathBuf,
    pub cloned: Vec<String>,
    pub secrets_stored: usize,
    pub warnings: Vec<String>,
}

/// Writes the project of `config_path` as a `.pomproj` archive; `passphrase` is asked only when it has secrets.
pub fn export_project(
    config_path: &Path,
    state: &StateDir,
    passphrase: &dyn Fn() -> Result<String, String>,
    out: &mut dyn Write,
) -> Result<ProjectExport, String> {
    let config = Config::load(config_path).map_err(|error| error.to_string())?;
    let root = config_path.parent().unwrap_or(Path::new("."));
    let mut warnings = Vec::new();
    let mut repos = Vec::new();
    for name in config.repos.keys() {
        let main = pom_layout::repo_worktree(root, name, config.global_default_branch(), true);
        if !pom_layout::is_git_repo(&main) {
            warnings.push(format!("{name} is not checked out on main; not listed"));
            continue;
        }
        match git_text(&main, &["remote", "get-url", "origin"]) {
            Ok(remote_url) => repos.push(ProjectRepo {
                name: name.clone(),
                remote_url,
                default_branch: config.default_branch_for(name).to_string(),
            }),
            Err(_) => warnings.push(format!("{name} has no origin remote; not listed")),
        }
    }
    let secrets = pom_secrets::SecretStore::new(state.clone(), &config.session)
        .names()
        .map_err(|error| error.to_string())?;
    let password = if secrets.is_empty() {
        None
    } else {
        Some(passphrase()?)
    };
    let bundle = pom_bundle::export(config_path, state, &config.session, password.as_deref())?;
    let staging = Staging::new(state, "project-export")?;
    let file = if password.is_some() {
        SEALED_FILE
    } else {
        PLAIN_FILE
    };
    std::fs::write(staging.path().join(file), &bundle.data).map_err(|error| error.to_string())?;
    let manifest = ProjectManifest {
        schema: PROJECT_SCHEMA.to_string(),
        pom_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: utc_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
        ),
        project: config.session.clone(),
        default_branch: config.global_default_branch().to_string(),
        repos,
        secrets,
    };
    pack_any(&manifest, staging.path(), out)?;
    Ok(ProjectExport { manifest, warnings })
}

/// Sets the archive's project up here: in `directory`, else `<sessions root>/<project>`.
pub fn import_project(
    input: &mut dyn Read,
    state: &StateDir,
    directory: Option<&Path>,
    passphrase: &dyn Fn() -> Result<String, String>,
    progress: &dyn Fn(&str),
) -> Result<ProjectImport, String> {
    let staging = Staging::new(state, "project-import")?;
    let text = unpack_any(
        input,
        staging.path(),
        &|schema| {
            check_major(
                schema,
                PROJECT_SCHEMA_PREFIX,
                PROJECT_SCHEMA_MAJOR,
                "project",
            )
        },
        &|path| {
            matches!(
                path.components().collect::<Vec<_>>().as_slice(),
                [Component::Normal(name)] if *name == SEALED_FILE || *name == PLAIN_FILE
            )
        },
    )?;
    let manifest: ProjectManifest =
        serde_json::from_str(&text).map_err(|error| format!("manifest: {error}"))?;
    let name = &manifest.project;
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("the archive names no usable project ({name:?})"));
    }
    if let Some(known) = pom_sessions::Projects::load(state)
        .project_dir(name)
        .filter(|dir| dir.join(pom_config::CONFIG_FILE_NAME).is_file())
    {
        return Err(format!(
            "project {name} is already on this machine at {}",
            known.display()
        ));
    }
    let root = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| pom_paths::sessions_root().join(name));
    let config_path = root.join(pom_config::CONFIG_FILE_NAME);
    if config_path.exists() {
        return Err(format!("{} already exists", config_path.display()));
    }
    let contents = if staging.path().join(SEALED_FILE).is_file() {
        let data =
            std::fs::read(staging.path().join(SEALED_FILE)).map_err(|error| error.to_string())?;
        pom_bundle::open(&data, &passphrase()?)?
    } else {
        let data =
            std::fs::read(staging.path().join(PLAIN_FILE)).map_err(|error| error.to_string())?;
        pom_bundle::open(&data, "")?
    };
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("create {}: {error}", root.display()))?;
    let applied = pom_bundle::apply(
        &config_path,
        state,
        name,
        Some(&contents.config),
        &contents.secrets,
    )?;
    let mut warnings = Vec::new();
    let mut cloned = Vec::new();
    let main = pom_layout::workspace_folder(&root, &manifest.default_branch);
    for repo in &manifest.repos {
        let destination = main.join(&repo.name);
        if pom_layout::is_git_repo(&destination) {
            continue;
        }
        progress(&format!("cloning {}", repo.name));
        match clone(&repo.remote_url, &repo.default_branch, &destination) {
            Ok(()) => cloned.push(repo.name.clone()),
            Err(error) => warnings.push(format!(
                "{}: {error}; clone {} into {} yourself",
                repo.name,
                repo.remote_url,
                destination.display()
            )),
        }
    }
    register(state, name, &root).map_err(|error| format!("register {name}: {error}"))?;
    Ok(ProjectImport {
        manifest,
        root,
        cloned,
        secrets_stored: applied.secrets_created,
        warnings,
    })
}

fn clone(url: &str, branch: &str, destination: &Path) -> Result<(), String> {
    if url.starts_with('-') {
        return Err(format!("refusing the repo URL {url:?}"));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let output = Command::new("git")
        .args(["clone", "--quiet", "--branch", branch, "--", url])
        .arg(destination)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Lists the project here without making it the current one.
fn register(state: &StateDir, name: &str, root: &Path) -> std::io::Result<()> {
    pom_sessions::Projects::register(state, name, root)?;
    let mut sessions = pom_sessions::Sessions::load(state);
    let current = sessions.current.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    sessions.touch(name, &root.to_string_lossy(), now);
    if !current.is_empty() {
        sessions.current = current;
    }
    sessions.save(state)
}
