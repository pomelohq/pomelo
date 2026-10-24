//! A workspace as one `.pomws` archive another machine can open again exactly as it was: a gzip'd tar whose
//! first entry is `manifest.json`, then per repo the commits origin lacks (`git/<alias>.bundle`), the uncommitted
//! changes (`.patch`, `.staged.patch`) and untracked files (`.untracked.tar`), each database as a `pg_dump -Fc`
//! (`db/<name>.dump`), the workspace's own volumes (`volumes/<name>.tar`), and the secrets its config uses,
//! sealed with a passphrase (`secrets.sealed`). The machine key never travels.

mod export;
mod import;
mod project;

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub use export::{export, ExportOptions, ExportReport};
pub use import::{plan_import, ImportOptions, ImportPlan};
pub use pom_layout::normalize_remote;
pub use project::{
    export_project, import_project, ProjectExport, ProjectImport, ProjectManifest, ProjectRepo,
    PROJECT_EXTENSION, PROJECT_SCHEMA,
};

pub const SCHEMA: &str = "pom.wsbundle/v1";
const SCHEMA_PREFIX: &str = "pom.wsbundle/v";
const SCHEMA_MAJOR: u32 = 1;
/// The stable `--json` output of `pom ws export|import`.
pub const OUTPUT_SCHEMA: &str = "pom.ws-transfer/v1";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SECRETS_FILE: &str = "secrets.sealed";
pub const EXTENSION: &str = "pomws";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub pom_version: String,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub created_at: String,
    pub project: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    pub source: SourcePaths,
    pub repos: Vec<RepoEntry>,
    #[serde(default)]
    pub databases: Vec<DatabaseEntry>,
    #[serde(default)]
    pub volumes: Vec<VolumeEntry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub service_envs: IndexMap<String, String>,
    /// Names only; the values are in `secrets.sealed`.
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePaths {
    pub project_root: String,
    pub workspace: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub name: String,
    pub alias: String,
    pub branch: String,
    pub remote_url: String,
    pub default_branch: String,
    /// Merge-base of the branch with origin's default branch: the commit every machine already has.
    pub base: String,
    pub head: String,
    /// Commits in `base..head`, carried by the bundle.
    pub unpushed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untracked: Option<String>,
    #[serde(default)]
    pub untracked_files: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseEntry {
    pub repo: String,
    /// The `databases:` key or the shared service of a `shared:` ref.
    pub key: String,
    pub session_prefixed: bool,
    pub template: String,
    /// The name it had on the source machine.
    pub name: String,
    pub dump: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeEntry {
    pub name: String,
    pub archive: String,
    pub bytes: u64,
}

/// Refuses an archive a newer pom made: its major schema may mean things this one would get wrong.
pub fn check_schema(schema: &str) -> Result<(), String> {
    check_major(schema, SCHEMA_PREFIX, SCHEMA_MAJOR, "workspace")
}

pub(crate) fn check_major(
    schema: &str,
    prefix: &str,
    supported: u32,
    what: &str,
) -> Result<(), String> {
    let major = schema
        .strip_prefix(prefix)
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .ok_or_else(|| format!("not a {what} archive (schema {schema:?})"))?;
    if major > supported {
        return Err(format!(
            "this archive was made by a newer pom (schema {schema}); update pom here to import it"
        ));
    }
    Ok(())
}

/// Writes `manifest.json`, then every file of `staging` under its relative path, as a gzip'd tar.
pub fn pack(manifest: &Manifest, staging: &Path, out: &mut dyn Write) -> Result<(), String> {
    pack_any(manifest, staging, out)
}

pub(crate) fn pack_any(
    manifest: &impl Serialize,
    staging: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let encoder = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    let text = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(now_seconds());
    header.set_cksum();
    builder
        .append_data(&mut header, MANIFEST_FILE, text.as_slice())
        .map_err(|error| format!("write archive: {error}"))?;
    for relative in files_under(staging)? {
        builder
            .append_path_with_name(staging.join(&relative), &relative)
            .map_err(|error| format!("write archive: {}: {error}", relative.display()))?;
    }
    let encoder = builder
        .into_inner()
        .map_err(|error| format!("write archive: {error}"))?;
    let out = encoder
        .finish()
        .map_err(|error| format!("write archive: {error}"))?;
    out.flush()
        .map_err(|error| format!("write archive: {error}"))
}

/// Reads an archive: the manifest first (checked before anything else is written), then the other entries into
/// `staging`. Only the layout's own paths are accepted.
pub fn unpack(input: &mut dyn Read, staging: &Path) -> Result<Manifest, String> {
    let text = unpack_any(input, staging, &check_schema, &allowed_entry)?;
    serde_json::from_str(&text).map_err(|error| format!("manifest: {error}"))
}

/// Reads a gzip'd tar whose first entry is `manifest.json`: `check` sees its schema before anything is written,
/// and only paths `allowed` accepts are unpacked into `staging`. Returns the manifest's text.
pub(crate) fn unpack_any(
    input: &mut dyn Read,
    staging: &Path,
    check: &dyn Fn(&str) -> Result<(), String>,
    allowed: &dyn Fn(&Path) -> bool,
) -> Result<String, String> {
    let decoder = flate2::read::GzDecoder::new(input);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = archive
        .entries()
        .map_err(|error| format!("read archive: {error}"))?;
    let mut first = entries
        .next()
        .ok_or("the archive is empty")?
        .map_err(|error| format!("read archive: {error}"))?;
    let first_path = first
        .path()
        .map_err(|error| error.to_string())?
        .into_owned();
    if first_path != Path::new(MANIFEST_FILE) {
        return Err("not a pom archive (no manifest.json first)".into());
    }
    let mut text = String::new();
    first
        .read_to_string(&mut text)
        .map_err(|error| format!("read manifest: {error}"))?;
    let schema: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("manifest: {error}"))?;
    check(schema["schema"].as_str().unwrap_or_default())?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("read archive: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        if !allowed(&path) || !entry.header().entry_type().is_file() {
            return Err(format!(
                "unexpected file in the archive: {}",
                path.display()
            ));
        }
        let target = staging.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        entry
            .unpack(&target)
            .map_err(|error| format!("unpack {}: {error}", path.display()))?;
    }
    Ok(text)
}

/// A file the manifest names, which must be one the archive could hold.
pub(crate) fn staged_file(staging: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative == SECRETS_FILE || !allowed_entry(Path::new(relative)) {
        return Err(format!("the manifest names an unexpected file: {relative}"));
    }
    Ok(staging.join(relative))
}

/// A repo's folder name: one plain path segment.
pub(crate) fn plain_name(name: &str) -> Result<&str, String> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(_)), None) if !name.starts_with('.') => Ok(name),
        _ => Err(format!("the manifest names an unexpected repo: {name}")),
    }
}

fn allowed_entry(path: &Path) -> bool {
    let parts: Vec<&str> = path
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str().unwrap_or("/"),
            _ => "/",
        })
        .collect();
    match parts.as_slice() {
        [SECRETS_FILE] => true,
        ["git" | "db" | "volumes", name] => !name.contains('/') && !name.starts_with('.'),
        _ => false,
    }
}

/// Every file below `dir`, relative and sorted, so archives come out the same for the same input.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let entries = match std::fs::read_dir(dir.join(&relative)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = relative.join(entry.file_name());
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// A scratch folder under the state dir, removed when dropped.
pub struct Staging {
    path: PathBuf,
}

impl Staging {
    pub fn new(state: &pom_paths::StateDir, what: &str) -> Result<Staging, String> {
        let unique = format!(
            "{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        );
        let path = state.path("transfer").join(unique);
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("create {}: {error}", path.display()))?;
        Ok(Staging { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("transfer: remove {}: {error}", self.path.display());
            }
        }
    }
}

/// `git -C dir args`, its raw stdout (patches must keep every byte).
pub(crate) fn git_bytes(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(crate) fn git_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(dir, args).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

/// The ticket key a branch names (`proj-101-login` -> `PROJ-101`), as the app shows it.
pub fn ticket_of(branch: &str) -> Option<String> {
    let mut parts = branch
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty());
    let project = parts.next()?;
    let number = parts
        .next()
        .filter(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))?;
    project
        .chars()
        .all(|c| c.is_ascii_alphabetic())
        .then(|| format!("{}-{number}", project.to_uppercase()))
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time.
pub fn utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days to a civil date (proleptic Gregorian), after H. Hinnant's algorithm.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// Names of the `{{secret.NAME}}` a config text uses, in first-seen order.
pub fn referenced_secrets(config_text: &str) -> Vec<String> {
    pom_env::template::refs(config_text)
        .into_iter()
        .filter_map(|key| key.strip_prefix("secret.").map(str::to_string))
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_major_schemas_are_refused() {
        assert!(check_schema("pom.wsbundle/v1").is_ok());
        assert!(check_schema("pom.wsbundle/v1.3").is_ok());
        let newer = check_schema("pom.wsbundle/v2").expect_err("newer");
        assert!(newer.contains("newer pom"), "{newer}");
        assert!(check_schema("pom.other/v1").is_err());
    }

    #[test]
    fn archive_paths_stay_inside_the_layout() {
        for good in ["git/api.bundle", "db/shop_api.dump", "secrets.sealed"] {
            assert!(allowed_entry(Path::new(good)), "{good}");
        }
        for bad in ["../x", "git/../x", "/etc/passwd", "git/a/b", "other.txt"] {
            assert!(!allowed_entry(Path::new(bad)), "{bad}");
        }
        let staging = Path::new("/staging");
        assert_eq!(
            staged_file(staging, "git/api.patch"),
            Ok(staging.join("git/api.patch"))
        );
        for bad in [
            "../../etc/passwd",
            "/etc/passwd",
            "secrets.sealed",
            "git/.hidden",
        ] {
            assert!(staged_file(staging, bad).is_err(), "{bad}");
        }
        assert_eq!(plain_name("api"), Ok("api"));
        for bad in ["..", "../api", "a/b", ".git", "/api", ""] {
            assert!(plain_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_archive_entry_that_is_a_link_is_refused() {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let manifest = br#"{"schema":"pom.wsbundle/v1"}"#;
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, MANIFEST_FILE, &manifest[..])
            .expect("manifest");
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        builder
            .append_link(&mut link, "git/api.patch", "/etc/hosts")
            .expect("link");
        let bytes = builder.into_inner().expect("tar").finish().expect("gzip");
        let temp = tempfile::tempdir().expect("temp");
        let refused = unpack_any(
            &mut bytes.as_slice(),
            temp.path(),
            &|_| Ok(()),
            &allowed_entry,
        )
        .expect_err("link");
        assert!(refused.contains("unexpected file"), "{refused}");
        assert!(!temp.path().join("git/api.patch").exists());
    }

    #[test]
    fn small_helpers() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_792_665_296), "2026-10-22T10:34:56Z");
        assert_eq!(ticket_of("proj-101-login").as_deref(), Some("PROJ-101"));
        assert_eq!(ticket_of("feat-login"), None);
        assert_eq!(
            referenced_secrets(
                "a: \"{{secret.API_KEY}}\"\nb: \"{{ secret.TOKEN | x }}{{db.main}}\""
            ),
            ["API_KEY", "TOKEN"]
        );
    }

    #[test]
    fn an_archive_round_trips_with_its_manifest_first() {
        let temp = tempfile::tempdir().expect("temp");
        let staging = temp.path().join("in");
        std::fs::create_dir_all(staging.join("git")).expect("dir");
        std::fs::write(staging.join("git/api.patch"), "diff\n").expect("patch");
        let manifest = Manifest {
            schema: SCHEMA.into(),
            project: "shop".into(),
            branch: "feat-x".into(),
            ..Manifest::default()
        };
        let mut bytes = Vec::new();
        pack(&manifest, &staging, &mut bytes).expect("pack");
        let out = temp.path().join("out");
        let read = unpack(&mut bytes.as_slice(), &out).expect("unpack");
        assert_eq!(read, manifest);
        assert_eq!(
            std::fs::read_to_string(out.join("git/api.patch")).expect("read"),
            "diff\n"
        );
    }
}
