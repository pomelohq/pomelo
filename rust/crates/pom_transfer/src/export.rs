use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use pom_layout::{Head, WorkspaceState};
use pom_workspace::WorkspaceContext;

use crate::{
    git_bytes, git_text, pack, referenced_secrets, ticket_of, utc_timestamp, DatabaseEntry,
    Manifest, RepoEntry, SourcePaths, Staging, VolumeEntry, SCHEMA, SECRETS_FILE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub databases: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub manifest: Manifest,
    pub bytes: u64,
    pub warnings: Vec<String>,
}

/// Writes the workspace on `branch` as a `.pomws` archive to `out`. `passphrase` is asked only when the config
/// uses secrets; `progress` gets each step as (stage, line): `repo`, `database`, `volume` or `archive`.
pub fn export(
    context: &WorkspaceContext<'_>,
    branch: &str,
    options: ExportOptions,
    passphrase: &dyn Fn() -> Result<String, String>,
    progress: &dyn Fn(&str, &str),
    out: &mut dyn Write,
) -> Result<ExportReport, String> {
    let (config, runner) = (context.config, context.runner);
    let root = runner.project_root();
    if branch == config.global_default_branch() {
        return Err(format!(
            "{branch} is the main workspace; export a branch workspace"
        ));
    }
    let folder = pom_layout::workspace_folder(root, branch);
    if !folder.is_dir() {
        return Err(format!("no workspace {branch}"));
    }
    let staging = Staging::new(context.state, "export")?;
    let mut warnings = Vec::new();
    let state = WorkspaceState::load(&folder);

    let mut repos = Vec::new();
    for (name, dir) in &config.repos {
        let worktree = folder.join(name);
        if !pom_layout::is_git_repo(&worktree) {
            continue;
        }
        progress("repo", &format!("{name}: commits and changes"));
        let alias = if dir.alias.is_empty() {
            name.clone()
        } else {
            dir.alias.clone()
        };
        repos.push(export_repo(
            name,
            &alias,
            &worktree,
            config.default_branch_for(name),
            &dir.copy,
            staging.path(),
        )?);
    }
    if repos.is_empty() {
        return Err(format!("workspace {branch} has no repo checked out"));
    }

    let mut databases = Vec::new();
    let mut volumes = Vec::new();
    if options.databases {
        let templates =
            pom_services::database_templates(config, branch, |repo| folder.join(repo).is_dir());
        if !templates.is_empty() {
            runner
                .ensure_shared(config)
                .map_err(|error| format!("shared services: {error}"))?;
        }
        for template in templates {
            if !runner.database_exists(config, &template.name) {
                warnings.push(format!("{} does not exist; skipped", template.name));
                continue;
            }
            progress("database", &format!("dumping {}", template.name));
            let relative = format!("db/{}.dump", file_safe(&template.name));
            let bytes = write_file(staging.path(), &relative, |file| {
                runner
                    .dump_database(config, &template.name, file)
                    .map_err(|error| error.to_string())
            })?;
            databases.push(DatabaseEntry {
                repo: template.repo,
                key: template.key,
                session_prefixed: template.session_prefixed,
                template: template.template,
                name: template.name,
                dump: relative,
                bytes,
            });
        }
        match runner.workspace_volumes(branch) {
            Ok(names) => {
                for name in names {
                    progress("volume", &format!("saving volume {name}"));
                    let relative = format!("volumes/{}.tar", file_safe(&name));
                    let bytes = write_file(staging.path(), &relative, |file| {
                        runner
                            .export_volume(&name, file)
                            .map_err(|error| error.to_string())
                    })?;
                    volumes.push(VolumeEntry {
                        name,
                        archive: relative,
                        bytes,
                    });
                }
            }
            Err(error) => warnings.push(format!("volumes not exported: {error}")),
        }
    }

    let secrets = seal_secrets(context, root, staging.path(), passphrase)?;

    let manifest = Manifest {
        schema: SCHEMA.to_string(),
        pom_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: utc_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
        ),
        project: config.session.clone(),
        branch: branch.to_string(),
        display_name: state.display_name.clone(),
        ticket: ticket_of(branch),
        source: SourcePaths {
            project_root: root.to_string_lossy().into_owned(),
            workspace: folder.to_string_lossy().into_owned(),
        },
        repos,
        databases,
        volumes,
        service_envs: state.service_envs.clone(),
        secrets,
    };
    progress("archive", "writing the archive");
    let mut counting = Counting {
        inner: out,
        bytes: 0,
    };
    pack(&manifest, staging.path(), &mut counting)?;
    Ok(ExportReport {
        manifest,
        bytes: counting.bytes,
        warnings,
    })
}

fn export_repo(
    name: &str,
    alias: &str,
    worktree: &Path,
    default_branch: &str,
    copied: &[String],
    staging: &Path,
) -> Result<RepoEntry, String> {
    let fail = |error: String| format!("{name}: {error}");
    let branch = match pom_layout::read_head(worktree) {
        Some(Head::Branch(branch)) => branch,
        Some(Head::Detached(commit)) => {
            return Err(fail(format!(
                "not on a branch (HEAD is at {commit}); check out a branch first"
            )))
        }
        None => return Err(fail("no HEAD".into())),
    };
    let remote_url = git_text(worktree, &["remote", "get-url", "origin"])
        .map_err(|_| fail("has no origin remote, so another machine cannot fetch it".into()))?;
    let origin_default = format!("refs/remotes/origin/{default_branch}");
    git_text(
        worktree,
        &["rev-parse", "--verify", "--quiet", &origin_default],
    )
    .map_err(|_| {
        fail(format!(
            "origin/{default_branch} is not known here; run git fetch origin"
        ))
    })?;
    let head = git_text(worktree, &["rev-parse", "HEAD"]).map_err(fail)?;
    let base = git_text(worktree, &["merge-base", "HEAD", &origin_default]).map_err(fail)?;
    let unpushed: usize = git_text(
        worktree,
        &["rev-list", "--count", &format!("{base}..{head}")],
    )
    .map_err(fail)?
    .parse()
    .unwrap_or(0);
    let mut entry = RepoEntry {
        name: name.to_string(),
        alias: alias.to_string(),
        branch: branch.clone(),
        remote_url,
        default_branch: default_branch.to_string(),
        base: base.clone(),
        head,
        unpushed,
        ..RepoEntry::default()
    };
    let file = file_safe(alias);
    if unpushed > 0 {
        let relative = format!("git/{file}.bundle");
        let path = staging.join(&relative);
        create_parent(&path)?;
        let path_text = path.to_string_lossy().into_owned();
        git_bytes(
            worktree,
            &[
                "bundle",
                "create",
                "--quiet",
                &path_text,
                &format!("refs/heads/{branch}"),
                &format!("^{base}"),
            ],
        )
        .map_err(fail)?;
        entry.bundle = Some(relative);
    }
    let diff = |extra: &[&str]| {
        let mut args = vec!["diff", "--binary", "--no-ext-diff", "--no-color"];
        args.extend_from_slice(extra);
        git_bytes(worktree, &args).map_err(fail)
    };
    let patch = diff(&["HEAD"])?;
    if !patch.is_empty() {
        let relative = format!("git/{file}.patch");
        write_bytes(staging, &relative, &patch)?;
        entry.patch = Some(relative);
        let staged = diff(&["--cached"])?;
        if !staged.is_empty() {
            let relative = format!("git/{file}.staged.patch");
            write_bytes(staging, &relative, &staged)?;
            entry.staged_patch = Some(relative);
        }
    }
    let listed = git_bytes(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .map_err(fail)?;
    let untracked: Vec<String> = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .filter(|path| !pom_workspace::placed_by_pom(worktree, path, copied))
        .collect();
    if !untracked.is_empty() {
        let relative = format!("git/{file}.untracked.tar");
        let path = staging.join(&relative);
        create_parent(&path)?;
        let archive = std::fs::File::create(&path).map_err(|error| error.to_string())?;
        let mut builder = tar::Builder::new(archive);
        builder.follow_symlinks(false);
        for file in &untracked {
            builder
                .append_path_with_name(worktree.join(file), file)
                .map_err(|error| fail(format!("{file}: {error}")))?;
        }
        builder.finish().map_err(|error| fail(error.to_string()))?;
        entry.untracked = Some(relative);
        entry.untracked_files = untracked.len();
    }
    Ok(entry)
}

/// Only the secrets the config uses travel, decrypted with this machine's key and sealed with the passphrase.
fn seal_secrets(
    context: &WorkspaceContext<'_>,
    root: &Path,
    staging: &Path,
    passphrase: &dyn Fn() -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let config_path = root.join(pom_config::CONFIG_FILE_NAME);
    let text = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let store = pom_secrets::SecretStore::new(context.state.clone(), &context.config.session);
    let mut secrets = BTreeMap::new();
    for name in referenced_secrets(&text) {
        if let Some(value) = store.get(&name).map_err(|error| error.to_string())? {
            secrets.insert(name, value);
        }
    }
    if secrets.is_empty() {
        return Ok(Vec::new());
    }
    let sealed = pom_bundle::seal(
        &pom_bundle::Contents {
            config: String::new(),
            secrets: secrets.clone(),
        },
        &passphrase()?,
    )?;
    write_bytes(staging, SECRETS_FILE, &sealed)?;
    Ok(secrets.into_keys().collect())
}

fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string()
}

fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display())),
        None => Ok(()),
    }
}

fn write_bytes(staging: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let path = staging.join(relative);
    create_parent(&path)?;
    std::fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn write_file(
    staging: &Path,
    relative: &str,
    fill: impl FnOnce(&mut std::fs::File) -> Result<u64, String>,
) -> Result<u64, String> {
    let path = staging.join(relative);
    create_parent(&path)?;
    let mut file = std::fs::File::create(&path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    fill(&mut file)
}

struct Counting<'a> {
    inner: &'a mut dyn Write,
    bytes: u64,
}

impl Write for Counting<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
