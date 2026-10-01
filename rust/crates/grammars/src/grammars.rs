//! Grammar packages the app downloads for languages it doesn't compile in: the index of what is published,
//! downloads checked against their sha256 and the grammars key, installs that land in one rename, and the
//! state behind the "available for this file" suggestion (what was dismissed, which languages were opened).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The public half of the key the published packages are signed with. None until it exists: downloads stay
/// off then, so nothing unverified is ever installed.
pub const GRAMMARS_PUBLIC_KEY: Option<&str> = Some("ZZTOGrqGDRRuDi/xQcREy1p1JyD8zCQpWk4APNRuM5o=");

/// The one grammars release, updated in place, so it never pushes app releases out of the updater's view.
pub const INDEX_URL: &str =
    "https://github.com/pomelohq/pomelo/releases/download/grammars/index.json";

/// How long a downloaded index is trusted before it is fetched again.
const INDEX_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const RECENT_LIMIT: usize = 50;

const EMBEDDED_INDEX: &str = include_str!("../../../grammars/index.json");

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Index {
    pub version: u32,
    #[serde(default)]
    pub packages: Vec<Package>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Package {
    /// The package's folder name under the grammars directory.
    pub id: String,
    /// The language, as the editor names it.
    pub language: String,
    pub version: String,
    pub file: String,
    pub url: Option<String>,
    pub size: u64,
    pub sha256: String,
    /// What the package was built from; a package re-published from changed inputs gets a new one.
    #[serde(default)]
    pub input_hash: Option<String>,
    /// Base64 Ed25519 signature of the archive's sha256 digest.
    pub signature: Option<String>,
    #[serde(default)]
    pub path_suffixes: Vec<String>,
    #[serde(default)]
    pub first_line_pattern: Option<String>,
}

pub fn parse_index(text: &str) -> Result<Index, String> {
    serde_json::from_str(text).map_err(|error| format!("grammar index: {error}"))
}

pub fn embedded_index() -> Index {
    parse_index(EMBEDDED_INDEX).unwrap_or_default()
}

impl Index {
    /// The package for a file: by its whole name first, then by its extension or path ending.
    pub fn package_for(&self, path: &str) -> Option<&Package> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.packages
            .iter()
            .find(|package| package.path_suffixes.iter().any(|suffix| suffix == name))
            .or_else(|| {
                self.packages.iter().find(|package| {
                    package.path_suffixes.iter().any(|suffix| {
                        path.ends_with(&format!(".{suffix}"))
                            || path.ends_with(&format!("/{suffix}"))
                    })
                })
            })
    }
}

/// The index to go by: the one downloaded last when it reads, else the copy built into the app.
pub fn load_index(grammars_dir: &Path) -> Index {
    std::fs::read_to_string(grammars_dir.join("index.json"))
        .ok()
        .and_then(|text| parse_index(&text).ok())
        .unwrap_or_else(embedded_index)
}

/// Whether the downloaded index is missing or older than a day.
pub fn index_is_stale(grammars_dir: &Path, now: SystemTime) -> bool {
    std::fs::metadata(grammars_dir.join("index.json"))
        .and_then(|meta| meta.modified())
        .ok()
        .map(|modified| now.duration_since(modified).unwrap_or_default())
        .is_none_or(|age| age >= INDEX_MAX_AGE)
}

/// Fetches the published index into the grammars directory; only while downloads are on.
pub fn refresh_index(grammars_dir: &Path) -> Result<(), String> {
    if GRAMMARS_PUBLIC_KEY.is_none() {
        return Ok(());
    }
    let bytes = download(INDEX_URL, Duration::from_secs(30))?;
    let text = String::from_utf8(bytes).map_err(|error| format!("grammar index: {error}"))?;
    parse_index(&text)?;
    std::fs::create_dir_all(grammars_dir).map_err(|error| error.to_string())?;
    let staged = grammars_dir.join(".index.json.part");
    std::fs::write(&staged, text).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, grammars_dir.join("index.json")).map_err(|error| error.to_string())
}

fn download(url: &str, limit: Duration) -> Result<Vec<u8>, String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg(limit.as_secs().to_string())
        .arg("-H")
        .arg("User-Agent: pomelo-grammars")
        .arg(url)
        .output()
        .map_err(|error| format!("could not run curl: {error}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(match reason.trim() {
            "" => format!("could not download {url}"),
            reason => format!("could not download {url}: {reason}"),
        });
    }
    Ok(output.stdout)
}

/// Checks a downloaded archive against its sha256 and, with `public_key`, its signature; without a key
/// nothing passes.
pub fn verify(bytes: &[u8], package: &Package, public_key: Option<&str>) -> Result<(), String> {
    let Some(public_key) = public_key else {
        return Err("grammar downloads are off: the app has no grammars key".into());
    };
    let digest = Sha256::digest(bytes);
    let found: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if found != package.sha256 {
        return Err(format!(
            "{}: the download does not match its checksum",
            package.language
        ));
    }
    verify_signature(&digest, package, public_key)
}

/// Checks every package in `index` is signed by `public_key`, from the sha256 the index records: what a
/// release checks before it embeds a downloaded index.
pub fn verify_index(index: &Index, public_key: &str) -> Result<(), String> {
    if index.packages.is_empty() {
        return Err("the index lists no packages".into());
    }
    for package in &index.packages {
        let digest: Vec<u8> = (0..package.sha256.len())
            .step_by(2)
            .filter_map(|at| u8::from_str_radix(package.sha256.get(at..at + 2)?, 16).ok())
            .collect();
        if digest.len() != 32 {
            return Err(format!("{}: the sha256 is malformed", package.language));
        }
        verify_signature(&digest, package, public_key)?;
    }
    Ok(())
}

/// Checks `package`'s signature over the archive's sha256 `digest` with `public_key`.
fn verify_signature(digest: &[u8], package: &Package, public_key: &str) -> Result<(), String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let signature = package
        .signature
        .as_deref()
        .ok_or_else(|| format!("{}: the package is not signed", package.language))?;
    let signature: [u8; 64] = engine
        .decode(signature)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or_else(|| format!("{}: the signature is malformed", package.language))?;
    let key: [u8; 32] = engine
        .decode(public_key)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("the grammars key is malformed")?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&key)
        .map_err(|error| format!("grammars key: {error}"))?;
    key.verify_strict(digest, &ed25519_dalek::Signature::from_bytes(&signature))
        .map_err(|_| format!("{}: the signature does not verify", package.language))
}

/// What tells two publications of a package apart: its inputs when the index has them, else its archive.
fn package_key(package: &Package) -> &str {
    package.input_hash.as_deref().unwrap_or(&package.sha256)
}

/// Where `package` lives once installed; a re-published package lands beside the one in use.
pub fn install_dir(grammars_dir: &Path, package: &Package) -> PathBuf {
    let key: String = package_key(package).chars().take(8).collect();
    grammars_dir
        .join(&package.id)
        .join(format!("{}-{key}", package.version))
}

/// Whether the package in use for `package`'s language is an older publication than `package`. One installed
/// before installs were recorded counts as older.
pub fn is_outdated(grammars_dir: &Path, package: &Package) -> bool {
    let Some(current) = editor::grammar_packages::current_package(&grammars_dir.join(&package.id))
    else {
        return false;
    };
    match editor::grammar_packages::install_record(&current) {
        None => true,
        Some(record) => match (&record.input_hash, &package.input_hash) {
            (Some(installed), Some(published)) => installed != published,
            _ => record.sha256 != package.sha256,
        },
    }
}

/// The installed languages the index has a newer publication of.
pub fn outdated(grammars_dir: &Path, index: &Index) -> Vec<Package> {
    index
        .packages
        .iter()
        .filter(|package| is_outdated(grammars_dir, package))
        .cloned()
        .collect()
}

/// Languages with a download running, so two never install the same one at once.
static INSTALLING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Holds a language's install slot until dropped.
pub struct InstallSlot(String);

impl Drop for InstallSlot {
    fn drop(&mut self) {
        if let Ok(mut installing) = INSTALLING.lock() {
            installing.remove(&self.0);
        }
    }
}

/// The install slot for `id`, or none while another download of it runs.
pub fn claim_install(id: &str) -> Option<InstallSlot> {
    let mut installing = INSTALLING.lock().ok()?;
    installing
        .insert(id.to_string())
        .then(|| InstallSlot(id.to_string()))
}

/// Brings an installed language up to `package`, a newer publication: the new package installs beside the old
/// one, `activate` puts it in use, and only then the old folders go. A failure leaves the old one in use.
pub fn update(
    package: &Package,
    grammars_dir: &Path,
    public_key: Option<&str>,
    fetch: impl Fn(&Package) -> Result<Vec<u8>, String>,
    activate: impl Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let _slot = claim_install(&package.id)
        .ok_or_else(|| format!("{}: already downloading", package.language))?;
    let bytes = fetch(package)?;
    verify(&bytes, package, public_key)?;
    let folder = install(&bytes, package, grammars_dir)?;
    activate(&folder)?;
    remove_other_versions(grammars_dir, package, &folder);
    Ok(folder)
}

fn remove_other_versions(grammars_dir: &Path, package: &Package, keep: &Path) {
    let Ok(folders) = std::fs::read_dir(grammars_dir.join(&package.id)) else {
        return;
    };
    for folder in folders.flatten().map(|entry| entry.path()) {
        if folder != keep && folder.is_dir() {
            if let Err(error) = std::fs::remove_dir_all(&folder) {
                eprintln!("grammars: remove {}: {error}", folder.display());
            }
        }
    }
}

/// Whether any version of `package` is installed.
pub fn is_installed(grammars_dir: &Path, package: &Package) -> bool {
    grammars_dir.join(&package.id).is_dir()
}

/// Unpacks a verified archive next to its final place, then moves it there in one rename, so a half-written
/// package is never seen.
pub fn install(bytes: &[u8], package: &Package, grammars_dir: &Path) -> Result<PathBuf, String> {
    let target = install_dir(grammars_dir, package);
    if target.join("grammar.wasm").is_file() {
        return Ok(target);
    }
    let parent = grammars_dir.join(&package.id);
    std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let staging = parent.join(format!(".staging-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let unpacked = unpack(bytes, &staging).and_then(|()| {
        if !staging.join("grammar.wasm").is_file() || !staging.join("language.toml").is_file() {
            return Err(format!(
                "{}: the package has no grammar.wasm or language.toml",
                package.language
            ));
        }
        settle_permissions(&staging)?;
        write_record(&staging, package)?;
        std::fs::rename(&staging, &target).map_err(|error| error.to_string())
    });
    if let Err(error) = unpacked {
        if let Err(cleanup) = std::fs::remove_dir_all(&staging) {
            eprintln!("grammars: remove {}: {cleanup}", staging.display());
        }
        return Err(error);
    }
    Ok(target)
}

fn write_record(folder: &Path, package: &Package) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let installed_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let record = editor::grammar_packages::InstallRecord {
        input_hash: package.input_hash.clone(),
        sha256: package.sha256.clone(),
        installed_at,
    };
    let path = folder.join(editor::grammar_packages::INSTALL_RECORD);
    let text = serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .map_err(|error| error.to_string())
}

fn unpack(bytes: &[u8], into: &Path) -> Result<(), String> {
    let archive = into.join(".package.tar.gz");
    std::fs::write(&archive, bytes).map_err(|error| error.to_string())?;
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .map_err(|error| format!("could not run tar: {error}"))?;
    if !listing.status.success() {
        return Err("the package is not a readable archive".into());
    }
    let names = String::from_utf8_lossy(&listing.stdout);
    let escapes = names
        .lines()
        .any(|name| name.starts_with('/') || name.split('/').any(|part| part == ".."));
    if escapes {
        return Err("the package has paths outside its folder".into());
    }
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|error| format!("could not run tar: {error}"))?;
    std::fs::remove_file(&archive).map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("the package could not be unpacked".into());
    }
    Ok(())
}

fn settle_permissions(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            return Err("the package holds a link".into());
        }
        if file_type.is_dir() {
            settle_permissions(&path)?;
        } else {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Downloads `package`, checks it, and installs it; the folder it went to.
pub fn download_and_install(package: &Package, grammars_dir: &Path) -> Result<PathBuf, String> {
    let url = package
        .url
        .as_deref()
        .ok_or_else(|| format!("{}: the index has no download for it", package.language))?;
    let _slot = claim_install(&package.id)
        .ok_or_else(|| format!("{}: already downloading", package.language))?;
    let bytes = download(url, Duration::from_secs(120))?;
    verify(&bytes, package, GRAMMARS_PUBLIC_KEY)?;
    install(&bytes, package, grammars_dir)
}

/// What decides whether a file gets the "available for this file" suggestion.
#[derive(Clone, Copy, Debug, Default)]
pub struct SuggestionCheck {
    /// The file is highlighted already (a built-in grammar, or an installed one).
    pub has_grammar: bool,
    pub installed: bool,
    pub installing: bool,
    pub dismissed: bool,
    /// Already shown in this window.
    pub showing: bool,
    pub downloads_on: bool,
}

impl SuggestionCheck {
    pub fn suggests(&self) -> bool {
        self.downloads_on
            && !self.has_grammar
            && !self.installed
            && !self.installing
            && !self.dismissed
            && !self.showing
    }
}

pub fn downloads_on() -> bool {
    GRAMMARS_PUBLIC_KEY.is_some()
}

fn read_names(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_names(file: &Path, names: &[String]) {
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::write(
                file,
                serde_json::to_string_pretty(names).unwrap_or_default(),
            )
        });
    if let Err(error) = written {
        eprintln!("grammars: save {}: {error}", file.display());
    }
}

/// The languages whose suggestion the user turned off.
pub fn dismissed_file() -> Option<PathBuf> {
    Some(pom_paths::config_dir()?.join("dismissed-grammar-suggestions.json"))
}

pub fn dismissed(file: &Path) -> BTreeSet<String> {
    read_names(file).into_iter().collect()
}

pub fn dismiss(file: &Path, language: &str) {
    let mut names = dismissed(file);
    if names.insert(language.to_string()) {
        write_names(file, &names.into_iter().collect::<Vec<_>>());
    }
}

/// The languages opened lately, newest first: the grammars a first launch without them installs.
pub fn recent_file() -> Option<PathBuf> {
    Some(pom_paths::config_dir()?.join("recent-languages.json"))
}

pub fn recent_languages(file: &Path) -> Vec<String> {
    read_names(file)
}

pub fn record_language(file: &Path, language: &str) {
    let mut names = recent_languages(file);
    if names.first().is_some_and(|first| first == language) {
        return;
    }
    names.retain(|name| name != language);
    names.insert(0, language.to_string());
    names.truncate(RECENT_LIMIT);
    write_names(file, &names);
}

/// Languages already installed on their own because they had been opened lately, so each happens once.
pub fn auto_installed_file() -> Option<PathBuf> {
    Some(pom_paths::config_dir()?.join("auto-installed-grammars.json"))
}

/// Where the first launch without a language's grammar looks: what was opened lately, what was turned off
/// or already installed this way, and the files that keep those lists.
pub struct AutoInstall<'a> {
    pub grammars_dir: &'a Path,
    pub index: &'a Index,
    pub recent: Vec<String>,
    pub dismissed: BTreeSet<String>,
    /// Keeps the languages installed this way.
    pub done_file: &'a Path,
}

impl AutoInstall<'_> {
    /// The packages for recently opened languages that have no grammar in the app, in the order they were
    /// opened: in the index, not installed, not turned off, and not installed this way before.
    pub fn candidates(&self, has_grammar: impl Fn(&str) -> bool) -> Vec<Package> {
        let done: BTreeSet<String> = read_names(self.done_file).into_iter().collect();
        self.recent
            .iter()
            .filter(|language| {
                !has_grammar(language)
                    && !self.dismissed.contains(*language)
                    && !done.contains(*language)
            })
            .filter_map(|language| {
                self.index
                    .packages
                    .iter()
                    .find(|package| &package.language == language)
            })
            .filter(|package| !is_installed(self.grammars_dir, package))
            .cloned()
            .collect()
    }

    /// Installs each candidate with `fetch`, checked against `public_key`; nothing without a key. Each
    /// language is noted as done before its download, so a failing one isn't retried every launch.
    pub fn run(
        &self,
        has_grammar: impl Fn(&str) -> bool,
        public_key: Option<&str>,
        fetch: impl Fn(&Package) -> Result<Vec<u8>, String>,
    ) -> Vec<(String, Result<PathBuf, String>)> {
        if public_key.is_none() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for package in self.candidates(has_grammar) {
            let mut done = read_names(self.done_file);
            done.push(package.language.clone());
            write_names(self.done_file, &done);
            let Some(_slot) = claim_install(&package.id) else {
                continue;
            };
            let installed = fetch(&package)
                .and_then(|bytes| verify(&bytes, &package, public_key).map(|()| bytes))
                .and_then(|bytes| install(&bytes, &package, self.grammars_dir));
            results.push((package.language.clone(), installed));
        }
        results
    }
}

/// Downloads a package for the first-launch install, with the same time limit as one asked for.
pub fn fetch_package(package: &Package) -> Result<Vec<u8>, String> {
    let url = package
        .url
        .as_deref()
        .ok_or_else(|| format!("{}: the index has no download for it", package.language))?;
    download(url, Duration::from_secs(120))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install slots are per process, so tests that download take turns instead of seeing each other's slot.
    fn exclusive_slots() -> std::sync::MutexGuard<'static, ()> {
        static SLOTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SLOTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The published Kotlin package, end to end: download, checksum and signature with the app's key, install,
    /// load through the editor's languages, and the highlights the compiled-in grammar gave before it left.
    #[test]
    #[ignore = "downloads from the grammars release"]
    fn a_published_package_installs_and_highlights_like_the_grammar_it_replaced() {
        let _slots = exclusive_slots();
        use editor::highlight::Lang;
        let index = embedded_index();
        let package = index.package_for("Main.kt").expect("Kotlin is published");
        let dir = tempfile::tempdir().expect("temp");
        let folder = download_and_install(package, dir.path()).expect("installed");
        editor::grammar_packages::register_installed_grammar(&folder).expect("registered");
        let registry = editor::registry::language_registry();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let (grammar, highlights) = loop {
            if let Some(found) = editor::registry::request_grammar(registry, Lang::Kotlin) {
                break found;
            }
            assert!(std::time::Instant::now() < deadline, "never loaded");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../grammars");
        let sample = std::fs::read_to_string(root.join("samples/kotlin.kt")).expect("sample");
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("expected/kotlin.json")).expect("expected"),
        )
        .expect("json");
        let captures: Vec<serde_json::Value> =
            editor::grammar_packages::highlight_captures(&grammar, &highlights, &sample)
                .expect("captures")
                .into_iter()
                .map(|(range, name)| serde_json::json!([range.start, range.end, name]))
                .collect();
        assert_eq!(serde_json::Value::Array(captures), expected["captures"]);
    }

    #[test]
    fn a_file_of_a_language_the_app_no_longer_carries_gets_the_suggestion() {
        let index = embedded_index();
        let package = index
            .package_for("app/src/Main.kt")
            .expect("Kotlin is published");
        assert_eq!(package.language, "Kotlin");
        assert!(
            index.package_for("lib/user.rb").is_none(),
            "Ruby stays built in"
        );
        let check = SuggestionCheck {
            downloads_on: downloads_on(),
            ..SuggestionCheck::default()
        };
        assert!(check.suggests());
    }

    #[test]
    fn every_package_in_the_built_in_index_is_signed_by_the_app_key() {
        let key = GRAMMARS_PUBLIC_KEY.expect("the app carries the grammars key");
        verify_index(&embedded_index(), key).unwrap_or_else(|error| panic!("{error}"));
        let other = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        assert!(verify_index(&embedded_index(), &public(&other)).is_err());
    }
    use ed25519_dalek::Signer;

    fn package(bytes: &[u8], key: &ed25519_dalek::SigningKey) -> Package {
        use base64::Engine;
        let digest = Sha256::digest(bytes);
        Package {
            id: "ocaml".into(),
            language: "OCaml".into(),
            version: "0.24.2".into(),
            file: "ocaml-0.24.2.tar.gz".into(),
            url: None,
            size: bytes.len() as u64,
            sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            input_hash: Some("inputs-one".into()),
            signature: Some(
                base64::engine::general_purpose::STANDARD.encode(key.sign(&digest).to_bytes()),
            ),
            path_suffixes: vec!["ml".into(), "mli".into(), "dune-project".into()],
            first_line_pattern: None,
        }
    }

    fn public(key: &ed25519_dalek::SigningKey) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
    }

    fn archive(dir: &Path, files: &[(&str, &str)]) -> Vec<u8> {
        let source = dir.join("source");
        std::fs::create_dir_all(&source).expect("source");
        for (name, text) in files {
            std::fs::write(source.join(name), text).expect("write");
        }
        let out = dir.join("package.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(&source)
            .arg(".")
            .status()
            .expect("tar");
        assert!(status.success());
        std::fs::read(out).expect("read")
    }

    #[test]
    fn the_index_reads_and_matches_names_before_extensions() {
        let index = parse_index(
            r#"{"version": 1, "packages": [
                {"id": "ocaml", "language": "OCaml", "version": "1", "file": "a", "url": null,
                 "size": 1, "sha256": "x", "signature": null, "path_suffixes": ["ml", "dune-project"]},
                {"id": "kotlin", "language": "Kotlin", "version": "1", "file": "b", "url": null,
                 "size": 1, "sha256": "y", "signature": null, "path_suffixes": ["kt"]}
            ]}"#,
        )
        .expect("index");
        let language = |path: &str| {
            index
                .package_for(path)
                .map(|package| package.language.as_str())
        };
        assert_eq!(language("src/main.ml"), Some("OCaml"));
        assert_eq!(language("dune-project"), Some("OCaml"));
        assert_eq!(language("app/Main.kt"), Some("Kotlin"));
        assert_eq!(language("notes.txt"), None);
        assert_eq!(language("html"), None);
        assert!(embedded_index().version >= 1);
    }

    #[test]
    fn a_download_needs_the_key_its_checksum_and_its_signature() {
        let _slots = exclusive_slots();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let bytes = b"archive".to_vec();
        let good = package(&bytes, &key);
        assert!(verify(&bytes, &good, Some(&public(&key))).is_ok());
        assert!(verify(&bytes, &good, None).is_err(), "no key, no install");
        assert!(verify(b"changed", &good, Some(&public(&key))).is_err());
        let unsigned = Package {
            signature: None,
            ..good.clone()
        };
        assert!(verify(&bytes, &unsigned, Some(&public(&key))).is_err());
        let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        assert!(verify(&bytes, &good, Some(&public(&other))).is_err());
    }

    #[test]
    fn an_install_lands_whole_in_its_version_folder() {
        let _slots = exclusive_slots();
        let temp = tempfile::tempdir().expect("temp");
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let bytes = archive(
            temp.path(),
            &[
                ("grammar.wasm", "wasm"),
                ("language.toml", "name = \"OCaml\"\n"),
            ],
        );
        let grammars = temp.path().join("grammars");
        let package = package(&bytes, &key);
        let installed = install(&bytes, &package, &grammars).expect("install");
        assert_eq!(installed, grammars.join("ocaml").join("0.24.2-inputs-o"));
        assert!(installed.join("grammar.wasm").is_file());
        assert!(is_installed(&grammars, &package));
        let record = editor::grammar_packages::install_record(&installed).expect("record");
        assert_eq!(record.input_hash.as_deref(), Some("inputs-one"));
        assert_eq!(record.sha256, package.sha256);
        let leftovers: Vec<_> = std::fs::read_dir(grammars.join("ocaml"))
            .expect("dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["0.24.2-inputs-o"]);

        let broken = archive(&temp.path().join("broken"), &[("readme", "no grammar")]);
        let mut other = package.clone();
        other.id = "kotlin".into();
        assert!(install(&broken, &other, &grammars).is_err());
        assert!(!grammars.join("kotlin").join("0.24.2-inputs-o").exists());
    }

    #[test]
    fn an_installed_package_is_outdated_only_when_its_publication_changed() {
        let temp = tempfile::tempdir().expect("temp");
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let bytes = archive(
            temp.path(),
            &[
                ("grammar.wasm", "wasm"),
                ("language.toml", "name = \"OCaml\"\n"),
            ],
        );
        let grammars = temp.path().join("grammars");
        let published = package(&bytes, &key);
        assert!(
            !is_outdated(&grammars, &published),
            "not installed is not outdated"
        );
        install(&bytes, &published, &grammars).expect("install");
        assert!(!is_outdated(&grammars, &published), "same inputs");
        let mut republished = published.clone();
        republished.input_hash = Some("inputs-two".into());
        assert!(is_outdated(&grammars, &republished));
        let index = Index {
            version: 1,
            packages: vec![republished.clone()],
        };
        assert_eq!(outdated(&grammars, &index), [republished]);

        let unrecorded = grammars.join("kotlin").join("1.0.0");
        std::fs::create_dir_all(&unrecorded).expect("old install");
        let mut kotlin = published;
        kotlin.id = "kotlin".into();
        assert!(is_outdated(&grammars, &kotlin), "installed before records");
    }

    #[test]
    fn an_update_lands_beside_the_old_package_and_replaces_it_once_in_use() {
        let _slots = exclusive_slots();
        let temp = tempfile::tempdir().expect("temp");
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let grammars = temp.path().join("grammars");
        let old_bytes = archive(
            &temp.path().join("old"),
            &[
                ("grammar.wasm", "old"),
                ("language.toml", "name = \"OCaml\"\n"),
            ],
        );
        let old = package(&old_bytes, &key);
        let old_folder = install(&old_bytes, &old, &grammars).expect("old");
        let new_bytes = archive(
            &temp.path().join("new"),
            &[
                ("grammar.wasm", "new"),
                ("language.toml", "name = \"OCaml\"\n"),
            ],
        );
        let mut new = package(&new_bytes, &key);
        new.input_hash = Some("inputs-two".into());
        let key_text = public(&key);

        let failed = update(
            &new,
            &grammars,
            Some(&key_text),
            |_| Err("offline".into()),
            |_| Ok(()),
        );
        assert!(failed.is_err());
        assert!(
            old_folder.join("grammar.wasm").is_file(),
            "a failure keeps the old one"
        );

        let refused = update(
            &new,
            &grammars,
            Some(&key_text),
            |_| Ok(new_bytes.clone()),
            |_| Err("did not load".into()),
        );
        assert!(refused.is_err());
        assert!(
            old_folder.is_dir(),
            "the old one stays until the new one is in use"
        );

        let slot = claim_install("ocaml").expect("slot");
        let busy = update(
            &new,
            &grammars,
            Some(&key_text),
            |_| Ok(new_bytes.clone()),
            |_| Ok(()),
        );
        assert!(busy.is_err(), "never two downloads of a language at once");
        drop(slot);

        let activated = std::cell::RefCell::new(Vec::new());
        let folder = update(
            &new,
            &grammars,
            Some(&key_text),
            |_| Ok(new_bytes.clone()),
            |folder| {
                activated.borrow_mut().push(folder.to_path_buf());
                Ok(())
            },
        )
        .expect("updated");
        assert_eq!(activated.into_inner(), std::slice::from_ref(&folder));
        assert!(
            !old_folder.exists(),
            "the old one goes once the new one is in use"
        );
        assert_eq!(
            editor::grammar_packages::current_package(&grammars.join("ocaml")),
            Some(folder)
        );
        assert!(!is_outdated(&grammars, &new));
    }

    #[test]
    fn a_suggestion_waits_for_downloads_and_respects_what_is_there() {
        let _slots = exclusive_slots();
        let open = SuggestionCheck {
            downloads_on: true,
            ..SuggestionCheck::default()
        };
        assert!(open.suggests());
        for blocked in [
            SuggestionCheck {
                downloads_on: false,
                ..open
            },
            SuggestionCheck {
                has_grammar: true,
                ..open
            },
            SuggestionCheck {
                installed: true,
                ..open
            },
            SuggestionCheck {
                installing: true,
                ..open
            },
            SuggestionCheck {
                dismissed: true,
                ..open
            },
            SuggestionCheck {
                showing: true,
                ..open
            },
        ] {
            assert!(!blocked.suggests(), "{blocked:?}");
        }
        assert_eq!(downloads_on(), GRAMMARS_PUBLIC_KEY.is_some());
    }

    #[test]
    fn dismissals_and_recent_languages_are_kept() {
        let temp = tempfile::tempdir().expect("temp");
        let dismissed_at = temp.path().join("state").join("dismissed.json");
        dismiss(&dismissed_at, "OCaml");
        dismiss(&dismissed_at, "OCaml");
        assert_eq!(
            dismissed(&dismissed_at).into_iter().collect::<Vec<_>>(),
            ["OCaml"]
        );

        let recent_at = temp.path().join("state").join("recent.json");
        for language in ["Rust", "OCaml", "Rust", "Kotlin"] {
            record_language(&recent_at, language);
        }
        assert_eq!(recent_languages(&recent_at), ["Kotlin", "Rust", "OCaml"]);
        for index in 0..RECENT_LIMIT + 5 {
            record_language(&recent_at, &format!("L{index}"));
        }
        assert_eq!(recent_languages(&recent_at).len(), RECENT_LIMIT);
    }

    #[test]
    fn a_fresh_index_is_kept_for_a_day() {
        let temp = tempfile::tempdir().expect("temp");
        let now = SystemTime::now();
        assert!(index_is_stale(temp.path(), now));
        std::fs::write(
            temp.path().join("index.json"),
            r#"{"version": 1, "packages": []}"#,
        )
        .expect("write");
        assert!(!index_is_stale(temp.path(), now));
        assert!(index_is_stale(
            temp.path(),
            now + INDEX_MAX_AGE + Duration::from_secs(60)
        ));
        assert_eq!(load_index(temp.path()).packages.len(), 0);
    }

    #[test]
    fn recently_opened_languages_without_a_grammar_install_once() {
        let _slots = exclusive_slots();
        let temp = tempfile::tempdir().expect("temp");
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let bytes = archive(
            temp.path(),
            &[
                ("grammar.wasm", "wasm"),
                ("language.toml", "name = \"OCaml\"\ngrammar = \"ocaml\"\n"),
            ],
        );
        let ocaml = package(&bytes, &key);
        let mut kotlin = ocaml.clone();
        kotlin.id = "kotlin".into();
        kotlin.language = "Kotlin".into();
        let index = Index {
            version: 1,
            packages: vec![ocaml.clone(), kotlin],
        };
        let grammars_dir = temp.path().join("grammars");
        let done_file = temp.path().join("auto-installed.json");
        let auto = AutoInstall {
            grammars_dir: &grammars_dir,
            index: &index,
            recent: vec!["Rust".into(), "OCaml".into(), "Kotlin".into(), "Lua".into()],
            dismissed: ["Kotlin".to_string()].into_iter().collect(),
            done_file: &done_file,
        };
        let has_grammar = |language: &str| language == "Rust";
        assert!(
            auto.run(has_grammar, None, |_| Ok(bytes.clone()))
                .is_empty(),
            "nothing without a key"
        );
        let names: Vec<String> = auto
            .candidates(has_grammar)
            .into_iter()
            .map(|package| package.language)
            .collect();
        assert_eq!(
            names,
            ["OCaml"],
            "Rust has one, Kotlin was turned off, Lua has no package"
        );
        let public = public(&key);
        let results = auto.run(has_grammar, Some(&public), |_| Ok(bytes.clone()));
        assert_eq!(results.len(), 1);
        let folder = results[0].1.clone().expect("installed");
        assert!(folder.join("grammar.wasm").is_file());
        assert!(is_installed(&grammars_dir, &ocaml));
        assert!(auto
            .run(has_grammar, Some(&public), |_| Ok(bytes.clone()))
            .is_empty());
        std::fs::remove_dir_all(&grammars_dir).expect("uninstall");
        assert!(
            auto.candidates(has_grammar).is_empty(),
            "removed by hand, not installed again"
        );
    }
}
