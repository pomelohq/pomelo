use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;
use pom_workspace::{CreateRequest, ImportRequest, RepoImport, WorkspaceContext};

use crate::{git_text, normalize_remote, Manifest, SECRETS_FILE};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Import under another branch name than the one it was exported from.
    pub branch: Option<String>,
    /// 0-based stage to resume a stopped import from.
    pub from_stage: usize,
}

/// What an unpacked archive becomes on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPlan {
    pub request: ImportRequest,
    /// Source database name -> this machine's.
    pub databases: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// Checks that this project holds the archive's repos (same origin) and maps its branch, databases, volumes
/// and secrets onto this machine. `staging` holds the unpacked archive.
pub fn plan_import(
    context: &WorkspaceContext<'_>,
    manifest: &Manifest,
    staging: &Path,
    options: &ImportOptions,
    passphrase: &dyn Fn() -> Result<String, String>,
) -> Result<ImportPlan, String> {
    let config = context.config;
    let root = context.runner.project_root();
    let branch = options
        .branch
        .clone()
        .unwrap_or_else(|| manifest.branch.clone());
    pom_workspace::validate_branch_name(&branch)?;
    let mut warnings = Vec::new();
    let mut repos = Vec::new();
    let mut names = Vec::new();
    let mut repo_branches = IndexMap::new();
    let staged = |relative: &Option<String>| relative.as_ref().map(|path| staging.join(path));
    for entry in &manifest.repos {
        if !config.repos.contains_key(&entry.name) {
            return Err(format!(
                "repo {} of the archive is not in this project's pom.yml",
                entry.name
            ));
        }
        let main =
            pom_layout::repo_worktree(root, &entry.name, config.global_default_branch(), true);
        if !pom_layout::is_git_repo(&main) {
            return Err(format!(
                "{} is not checked out at {}; clone it there first",
                entry.name,
                main.display()
            ));
        }
        let here = git_text(&main, &["remote", "get-url", "origin"])
            .map_err(|_| format!("{} has no origin remote here", entry.name))?;
        if normalize_remote(&here) != normalize_remote(&entry.remote_url) {
            return Err(format!(
                "{}: origin here is {here}, the archive's is {}; they are not the same repo",
                entry.name, entry.remote_url
            ));
        }
        if entry.branch != manifest.branch {
            repo_branches.insert(entry.name.clone(), entry.branch.clone());
        }
        names.push(entry.name.clone());
        repos.push(RepoImport {
            name: entry.name.clone(),
            head: entry.head.clone(),
            bundle: staged(&entry.bundle)
                .map(|path| (path, format!("refs/heads/{}", entry.branch))),
            patch: staged(&entry.patch),
            staged_patch: staged(&entry.staged_patch),
            untracked: staged(&entry.untracked),
        });
    }

    let owned = pom_services::owned_database_names(config, &branch, |repo| {
        names.iter().any(|name| name == repo)
    });
    let mut databases = Vec::new();
    let mut renamed = Vec::new();
    for entry in &manifest.databases {
        let template = config.repos.get(&entry.repo).and_then(|dir| {
            if entry.session_prefixed {
                dir.databases.get(&entry.key).cloned()
            } else {
                dir.shared_refs
                    .iter()
                    .find(|shared| shared.name == entry.key && !shared.db_name.is_empty())
                    .map(|shared| shared.db_name.clone())
            }
        });
        let Some(template) = template else {
            warnings.push(format!(
                "{}: {}.{} is not in this project's config; not restored",
                entry.name, entry.repo, entry.key
            ));
            continue;
        };
        let name = pom_services::database_name(config, &template, entry.session_prefixed, &branch);
        if !owned.contains(&name) {
            warnings.push(format!(
                "{}: {name} is shared with main here; not restored",
                entry.name
            ));
            continue;
        }
        databases.push((name.clone(), staging.join(&entry.dump)));
        renamed.push((entry.name.clone(), name));
    }

    let volumes = manifest
        .volumes
        .iter()
        .map(|volume| {
            let name = match volume.name.strip_prefix(&manifest.project) {
                Some(rest) => format!("{}{rest}", config.session),
                None => volume.name.clone(),
            };
            (name, staging.join(&volume.archive))
        })
        .collect();

    let sealed = staging.join(SECRETS_FILE);
    let secrets = if sealed.is_file() {
        let data = std::fs::read(&sealed).map_err(|error| error.to_string())?;
        pom_bundle::open(&data, &passphrase()?)
            .map_err(|error| format!("secrets: {error}"))?
            .secrets
    } else {
        BTreeMap::new()
    };

    Ok(ImportPlan {
        request: ImportRequest {
            create: CreateRequest {
                branch,
                repos: names,
                repo_branches,
                from_stage: options.from_stage,
                ..CreateRequest::default()
            },
            display_name: manifest.display_name.clone(),
            service_envs: manifest.service_envs.clone(),
            repos,
            databases,
            volumes,
            secrets,
        },
        databases: renamed,
        warnings,
    })
}
