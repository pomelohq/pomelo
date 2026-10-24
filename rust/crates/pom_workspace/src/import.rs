//! Bringing in a workspace exported on another machine: the create stages, plus the exported commits, the
//! uncommitted changes, the secrets and the database contents put back in between.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use pom_layout::WorkspaceState;

use crate::create::Creation;
use crate::git::{git, resolves};
use crate::{
    CreateRequest, EventSink, Operation, Outcome, PipelineError, Run, StageResult, StageScope,
    WorkspaceContext,
};

pub const IMPORT_STAGES: [&str; 11] = [
    "Validating config and hosts",
    "Provisioning workspace",
    "Storing secrets",
    "Fetching the exported commits",
    "Creating git worktrees (parallel)",
    "Applying uncommitted changes",
    "Starting shared services and databases",
    "Restoring databases and volumes",
    "Configuring repos (parallel)",
    "Running setup commands (parallel)",
    "Saving the workspace's baseline snapshot",
];

/// A repo's exported state, its files already unpacked on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoImport {
    pub name: String,
    /// The commit the workspace's branch points at.
    pub head: String,
    /// `git bundle` of the commits origin lacks, and the ref it carries them under.
    pub bundle: Option<(PathBuf, String)>,
    /// `git diff --binary HEAD`: the working tree's changes.
    pub patch: Option<PathBuf>,
    /// `git diff --binary --cached`: the part of them that was staged.
    pub staged_patch: Option<PathBuf>,
    /// Tar of the untracked files git does not ignore.
    pub untracked: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportRequest {
    /// Branch, repos and their branches; databases start empty and nothing is seeded.
    pub create: CreateRequest,
    pub display_name: String,
    pub service_envs: IndexMap<String, String>,
    pub repos: Vec<RepoImport>,
    /// This machine's database name -> its `pg_dump -Fc` file.
    pub databases: Vec<(String, PathBuf)>,
    /// Volume name -> its tar file.
    pub volumes: Vec<(String, PathBuf)>,
    pub secrets: BTreeMap<String, String>,
}

const VALIDATE: usize = 0;
const PROVISION: usize = 1;
const SECRETS: usize = 2;
const FETCH: usize = 3;
const SOURCE: usize = 4;
const CHANGES: usize = 5;
const INFRA: usize = 6;
const RESTORE: usize = 7;
const CONFIGURE: usize = 8;
const SETUP: usize = 9;
const BASELINE: usize = 10;

pub fn import(
    context: &WorkspaceContext<'_>,
    request: &ImportRequest,
    sink: EventSink<'_>,
) -> Result<Outcome, PipelineError> {
    let create = CreateRequest {
        fresh_databases: true,
        skip_seed: true,
        ..request.create.clone()
    };
    let folder = pom_layout::workspace_folder(context.runner.project_root(), &create.branch);
    let run = Run {
        operation: Operation::Import,
        branch: &create.branch,
        workspace: &folder,
        labels: &IMPORT_STAGES,
        from_stage: create.from_stage,
        state: context.state,
        sink,
        resumable: true,
    };
    let creation = Creation::plan(context, &create);
    run.execute(|index, scope| {
        let creation = creation.as_ref().map_err(Clone::clone)?;
        let import = Import { creation, request };
        match index {
            VALIDATE => import.validate(),
            PROVISION => import.provision(),
            SECRETS => import.secrets(scope),
            FETCH => import.fetch(scope),
            SOURCE => creation.source(scope),
            CHANGES => import.changes(scope),
            INFRA => creation.infra(scope),
            RESTORE => import.restore(scope),
            CONFIGURE => creation.configure(),
            SETUP => creation.setup(scope),
            BASELINE => Ok(creation.baseline(scope)),
            _ => Ok(StageResult::Skipped),
        }
    })
}

struct Import<'a> {
    creation: &'a Creation<'a>,
    request: &'a ImportRequest,
}

impl Import<'_> {
    fn exported(&self, repo: &str) -> Result<&RepoImport, String> {
        self.request
            .repos
            .iter()
            .find(|exported| exported.name == repo)
            .ok_or_else(|| format!("{repo}: not in the export"))
    }

    fn validate(&self) -> Result<StageResult, String> {
        self.creation.validate()?;
        for repo in &self.creation.repos {
            self.exported(&repo.name)?;
        }
        Ok(StageResult::Done)
    }

    fn provision(&self) -> Result<StageResult, String> {
        self.creation.provision()?;
        let folder = &self.creation.folder;
        let mut state = WorkspaceState::load(folder);
        state.display_name = self.request.display_name.clone();
        for (key, environment) in &self.request.service_envs {
            state.service_envs.insert(key.clone(), environment.clone());
        }
        state
            .save(folder)
            .map_err(|error| format!("save workspace state: {error}"))?;
        Ok(StageResult::Done)
    }

    /// A secret this machine already has keeps its value: other workspaces of the project use it too.
    fn secrets(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        if self.request.secrets.is_empty() {
            return Ok(StageResult::Skipped);
        }
        let context = self.creation.context;
        let store = pom_secrets::SecretStore::new(context.state.clone(), &context.config.session);
        let mut stored = 0;
        for (name, value) in &self.request.secrets {
            match store.get(name).map_err(|error| error.to_string())? {
                None => {
                    store
                        .set(name, value)
                        .map_err(|error| format!("secret {name}: {error}"))?;
                    stored += 1;
                }
                Some(current) if current == *value => {}
                Some(_) => scope.warn(format!(
                    "secret {name} has another value on this machine; kept this machine's"
                )),
            }
        }
        scope.progress(format!("{stored} secret(s) stored"));
        Ok(StageResult::Done)
    }

    /// Puts each repo's branch at the exported head: origin's commits plus the bundle's.
    fn fetch(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        for repo in &self.creation.repos {
            let exported = self.exported(&repo.name)?;
            if let Err(error) = crate::fetch_origin(&repo.main) {
                scope.warn(format!("{}: {error}", repo.name));
            }
            if let Some((bundle, reference)) = &exported.bundle {
                let bundle_text = bundle.to_string_lossy().into_owned();
                git(
                    &repo.main,
                    &["fetch", "--no-tags", "--quiet", &bundle_text, reference],
                )
                .map_err(|error| format!("{}: fetch the bundle: {error}", repo.name))?;
            }
            let head = &exported.head;
            if !resolves(&repo.main, &format!("{head}^{{commit}}")) {
                return Err(format!(
                    "{}: commit {head} is neither on origin nor in the export; fetch origin and retry",
                    repo.name
                ));
            }
            let local = format!("refs/heads/{}", repo.branch);
            if resolves(&repo.main, &local) {
                let current = git(&repo.main, &["rev-parse", &local])?;
                if current != *head {
                    return Err(format!(
                        "{}: branch {} already exists here at {}; import under another name with --branch",
                        repo.name,
                        repo.branch,
                        short(&current)
                    ));
                }
                continue;
            }
            git(&repo.main, &["branch", &repo.branch, head])
                .map_err(|error| format!("{}: {error}", repo.name))?;
            let remote = format!("origin/{}", repo.branch);
            if resolves(&repo.main, &format!("refs/remotes/{remote}")) {
                if let Err(error) = git(
                    &repo.main,
                    &["branch", "--set-upstream-to", &remote, &repo.branch],
                ) {
                    scope.warn(format!("{}: track {remote}: {error}", repo.name));
                }
            }
            scope.progress(format!("{}: {} at {}", repo.name, repo.branch, short(head)));
        }
        Ok(StageResult::Done)
    }

    fn changes(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let mut any = false;
        for repo in &self.creation.repos {
            let exported = self.exported(&repo.name)?;
            let mut changed = false;
            if let Some(patch) = &exported.patch {
                apply(&repo.worktree, patch, false).map_err(|error| {
                    format!("{}: apply uncommitted changes: {error}", repo.name)
                })?;
                changed = true;
            }
            if let Some(patch) = &exported.staged_patch {
                apply(&repo.worktree, patch, true)
                    .map_err(|error| format!("{}: restore staged changes: {error}", repo.name))?;
            }
            if let Some(archive) = &exported.untracked {
                unpack(archive, &repo.worktree)
                    .map_err(|error| format!("{}: untracked files: {error}", repo.name))?;
                changed = true;
            }
            if changed {
                scope.progress(format!("{}: changes applied", repo.name));
                any = true;
            }
        }
        Ok(if any {
            StageResult::Done
        } else {
            StageResult::Skipped
        })
    }

    fn restore(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        if self.request.databases.is_empty() && self.request.volumes.is_empty() {
            return Ok(StageResult::Skipped);
        }
        let context = self.creation.context;
        let (config, runner) = (context.config, context.runner);
        if !self.request.databases.is_empty() {
            runner
                .ensure_shared(config)
                .map_err(|error| format!("shared services: {error}"))?;
        }
        for (db, dump) in &self.request.databases {
            scope.progress(format!("restoring {db}"));
            let mut file = std::fs::File::open(dump)
                .map_err(|error| format!("{}: {error}", dump.display()))?;
            runner
                .restore_database(config, db, &mut file)
                .map_err(|error| error.to_string())?;
        }
        for (volume, archive) in &self.request.volumes {
            scope.progress(format!("restoring volume {volume}"));
            let mut file = std::fs::File::open(archive)
                .map_err(|error| format!("{}: {error}", archive.display()))?;
            runner
                .import_volume(volume, self.creation.branch(), &mut file)
                .map_err(|error| error.to_string())?;
        }
        Ok(StageResult::Done)
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// `git apply`, to the working tree or (`cached`) the index only; a patch already in place is left alone so a
/// resumed run gets past it.
fn apply(worktree: &Path, patch: &Path, cached: bool) -> Result<(), String> {
    let patch = patch.to_string_lossy().into_owned();
    let mut args = vec!["apply", "--binary", "--whitespace=nowarn"];
    if cached {
        args.push("--cached");
    }
    let check = |reverse: bool| {
        let mut args = args.clone();
        args.push("--check");
        if reverse {
            args.push("--reverse");
        }
        args.push(&patch);
        git(worktree, &args)
    };
    if let Err(error) = check(false) {
        return match check(true) {
            Ok(_) => Ok(()),
            Err(_) => Err(error),
        };
    }
    args.push(&patch);
    git(worktree, &args).map(|_| ())
}

/// Writes only plain files and folders inside the worktree, never under a `.git`: a planted hook or config
/// there would run at the next git command, before anything else is started.
fn unpack(archive: &Path, worktree: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut archive = tar::Archive::new(file);
    archive.set_overwrite(true);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(format!("{} is not a plain file", path.display()));
        }
        if !untracked_path(&path) {
            return Err(format!("{} is not a path inside the repo", path.display()));
        }
        let inside = entry
            .unpack_in(worktree)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !inside {
            return Err(format!("{} is not a path inside the repo", path.display()));
        }
    }
    Ok(())
}

fn untracked_path(path: &Path) -> bool {
    use std::path::Component;
    path.components().all(|part| match part {
        Component::Normal(name) => !name.eq_ignore_ascii_case(".git"),
        Component::CurDir => true,
        _ => false,
    }) && path
        .components()
        .any(|part| matches!(part, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_with(dir: &Path, build: impl FnOnce(&mut tar::Builder<std::fs::File>)) -> PathBuf {
        let path = dir.join("untracked.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&path).expect("create"));
        build(&mut builder);
        builder.finish().expect("finish");
        path
    }

    fn file(builder: &mut tar::Builder<std::fs::File>, path: &str, body: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o4755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body)
            .expect("append");
    }

    fn link(builder: &mut tar::Builder<std::fs::File>, kind: tar::EntryType, path: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(0);
        builder
            .append_link(&mut header, path, "/etc/hosts")
            .expect("link");
    }

    #[test]
    fn plain_files_unpack_without_their_setuid_bit() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let worktree = temp.path().join("api");
        std::fs::create_dir_all(&worktree).expect("dir");
        let archive = archive_with(temp.path(), |builder| {
            file(builder, "notes/todo.txt", b"hi")
        });
        unpack(&archive, &worktree).expect("unpack");
        let written = worktree.join("notes/todo.txt");
        assert_eq!(std::fs::read(&written).expect("read"), b"hi");
        let mode = std::fs::metadata(&written)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o4000, 0);
    }

    #[test]
    fn git_folders_links_and_escapes_are_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let worktree = temp.path().join("api");
        std::fs::create_dir_all(&worktree).expect("dir");
        let refused = |archive: PathBuf| unpack(&archive, &worktree).is_err();
        assert!(refused(archive_with(temp.path(), |builder| file(
            builder,
            ".git/hooks/pre-commit",
            b"#!/bin/sh\n"
        ))));
        assert!(refused(archive_with(temp.path(), |builder| file(
            builder,
            "vendor/lib/.GIT/config",
            b"[core]\n"
        ))));
        assert!(refused(archive_with(temp.path(), |builder| link(
            builder,
            tar::EntryType::Symlink,
            "hosts"
        ))));
        assert!(refused(archive_with(temp.path(), |builder| link(
            builder,
            tar::EntryType::Link,
            "hosts"
        ))));
        let escape = temp.path().join("escape.tar");
        {
            let mut bytes = Vec::new();
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            let name = b"../outside.txt";
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            bytes.extend_from_slice(header.as_bytes());
            let mut block = [0u8; 512];
            block[0] = b'x';
            bytes.extend_from_slice(&block);
            bytes.extend_from_slice(&[0u8; 1024]);
            std::fs::write(&escape, bytes).expect("write");
        }
        assert!(refused(escape));
        assert!(!temp.path().join("outside.txt").exists());
        assert!(!worktree.join(".git").exists());
    }
}
