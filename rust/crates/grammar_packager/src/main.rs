//! Builds the grammar packages the app downloads instead of compiling those grammars in.
//!
//! `build` compiles each grammar in `grammars/manifest.toml` to wasm from the crate version Cargo.lock pins,
//! with the app's own queries for its language, into `<id>-<version>.tar.gz` plus an `index.json`. `verify`
//! loads the built packages the way the app does and checks they highlight their samples exactly like the
//! compiled-in grammars. `sign` signs each package's sha256 in the index; `keygen` makes the signing key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use editor::highlight::{builtin_language, Lang};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The wasi-sdk release grammars are compiled with.
const WASI_SDK_VERSION: &str = "34";
const SIGNING_KEY_ENV: &str = "GRAMMARS_SIGNING_KEY";

#[derive(Deserialize)]
struct Entry {
    language: String,
    #[serde(rename = "crate")]
    crate_name: String,
    #[serde(default)]
    source: Option<String>,
    grammar: String,
    repository: String,
    rev: String,
    license: String,
    #[serde(default)]
    license_file: Option<String>,
    #[serde(default)]
    license_path: Option<String>,
    sample: String,
}

#[derive(Serialize, Deserialize)]
struct Index {
    version: u32,
    packages: Vec<IndexEntry>,
}

#[derive(Serialize, Deserialize)]
struct IndexEntry {
    id: String,
    language: String,
    version: String,
    file: String,
    url: Option<String>,
    size: u64,
    sha256: String,
    /// Base64 ed25519 signature of the sha256 digest; filled by `sign`.
    signature: Option<String>,
    license: String,
    repository: String,
    rev: String,
    /// How the app recognizes the language's files before the package is installed.
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

struct Paths {
    manifest: PathBuf,
    lockfile: PathBuf,
    samples: PathBuf,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("verify") => verify(&args[1..]),
        Some("sign") => sign(&args[1..]),
        Some("keygen") => keygen(),
        _ => Err(
            "usage: grammar_packager build [--out DIR] [--only id,id] [--base-url URL] [--cache DIR]\n       \
             grammar_packager verify [--out DIR]\n       grammar_packager sign [--out DIR]\n       \
             grammar_packager keygen"
                .to_string(),
        ),
    };
    if let Err(error) = result {
        eprintln!("grammar_packager: {error}");
        std::process::exit(1);
    }
}

fn workspace_paths() -> Paths {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Paths {
        manifest: workspace.join("grammars/manifest.toml"),
        lockfile: workspace.join("Cargo.lock"),
        samples: workspace.join("grammars/samples"),
    }
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn out_dir(args: &[String]) -> PathBuf {
    PathBuf::from(option(args, "--out").unwrap_or("target/grammar-packages"))
}

fn read_manifest(path: &Path) -> Result<BTreeMap<String, Entry>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn language(entry: &Entry) -> Result<Lang, String> {
    Lang::LANGUAGES
        .into_iter()
        .find(|lang| lang.name() == entry.language)
        .ok_or_else(|| format!("the editor has no language {}", entry.language))
}

/// Each locked crate's version and checksum, by name.
fn locked_crates(lockfile: &Path) -> Result<BTreeMap<String, (String, String)>, String> {
    #[derive(Deserialize)]
    struct Lock {
        package: Vec<Locked>,
    }
    #[derive(Deserialize)]
    struct Locked {
        name: String,
        version: String,
        checksum: Option<String>,
    }
    let text = std::fs::read_to_string(lockfile)
        .map_err(|error| format!("{}: {error}", lockfile.display()))?;
    let lock: Lock = toml::from_str(&text).map_err(|error| format!("Cargo.lock: {error}"))?;
    Ok(lock
        .package
        .into_iter()
        .filter_map(|locked| Some((locked.name, (locked.version, locked.checksum?))))
        .collect())
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{command:?}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Downloads `url` to `to`, giving up after `limit`.
fn download(url: &str, to: &Path, limit: Duration) -> Result<(), String> {
    run(Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--connect-timeout", "20", "--max-time"])
        .arg(limit.as_secs().to_string())
        .arg("--output")
        .arg(to)
        .arg(url))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The clang of a wasi-sdk: `$WASI_SDK_PATH`'s, else one downloaded into `cache`.
fn wasi_clang(cache: &Path) -> Result<PathBuf, String> {
    if let Some(sdk) = std::env::var_os("WASI_SDK_PATH").filter(|path| !path.is_empty()) {
        let clang = PathBuf::from(sdk).join("bin/clang");
        return clang
            .is_file()
            .then_some(clang)
            .ok_or_else(|| "WASI_SDK_PATH has no bin/clang".to_string());
    }
    let sdk = cache.join(format!("wasi-sdk-{WASI_SDK_VERSION}"));
    let clang = sdk.join("bin/clang");
    if clang.is_file() {
        return Ok(clang);
    }
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "arm64-macos",
        ("macos", "x86_64") => "x86_64-macos",
        ("linux", "x86_64") => "x86_64-linux",
        ("linux", "aarch64") => "arm64-linux",
        (os, arch) => return Err(format!("no wasi-sdk for {os} {arch}")),
    };
    let url = format!(
        "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-{WASI_SDK_VERSION}/wasi-sdk-{WASI_SDK_VERSION}.0-{platform}.tar.gz"
    );
    std::fs::create_dir_all(&sdk).map_err(|error| error.to_string())?;
    let archive = cache.join("wasi-sdk.tar.gz");
    eprintln!("downloading wasi-sdk {WASI_SDK_VERSION}");
    download(&url, &archive, Duration::from_secs(900))?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&sdk)
        .args(["--strip-components", "1"]))?;
    std::fs::remove_file(&archive).ok();
    clang
        .is_file()
        .then_some(clang)
        .ok_or_else(|| "the wasi-sdk archive has no bin/clang".to_string())
}

/// The crate's unpacked source, downloaded from crates.io and checked against the checksum Cargo.lock holds.
fn crate_source(
    cache: &Path,
    name: &str,
    version: &str,
    checksum: &str,
) -> Result<PathBuf, String> {
    let unpacked = cache.join("crates").join(format!("{name}-{version}"));
    if unpacked.join("Cargo.toml").is_file() {
        return Ok(unpacked);
    }
    let crates = cache.join("crates");
    std::fs::create_dir_all(&crates).map_err(|error| error.to_string())?;
    let archive = crates.join(format!("{name}-{version}.crate"));
    let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
    download(&url, &archive, Duration::from_secs(300))?;
    let bytes = std::fs::read(&archive).map_err(|error| error.to_string())?;
    let found = sha256_hex(&bytes);
    if found != checksum {
        return Err(format!(
            "{name} {version}: checksum {found} is not Cargo.lock's {checksum}"
        ));
    }
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&crates))?;
    std::fs::remove_file(&archive).ok();
    Ok(unpacked)
}

/// The git rev the crate was published from.
fn published_rev(source: &Path) -> Result<String, String> {
    #[derive(Deserialize)]
    struct VcsInfo {
        git: VcsGit,
    }
    #[derive(Deserialize)]
    struct VcsGit {
        sha1: String,
    }
    let text = std::fs::read_to_string(source.join(".cargo_vcs_info.json"))
        .map_err(|error| format!(".cargo_vcs_info.json: {error}"))?;
    let info: VcsInfo =
        serde_json::from_str(&text).map_err(|error| format!(".cargo_vcs_info.json: {error}"))?;
    Ok(info.git.sha1)
}

/// The license text: the crate's own file, else the repository's at the published rev.
fn license_text(entry: &Entry, source: &Path, cache: &Path) -> Result<String, String> {
    if let Some(file) = &entry.license_file {
        return std::fs::read_to_string(source.join(file))
            .map_err(|error| format!("{file}: {error}"));
    }
    let path = entry
        .license_path
        .as_deref()
        .ok_or("the entry names no license_file or license_path")?;
    let repository = entry
        .repository
        .trim_end_matches(".git")
        .trim_start_matches("https://github.com/");
    let url = format!(
        "https://raw.githubusercontent.com/{repository}/{}/{path}",
        entry.rev
    );
    let to = cache.join(format!("{}-LICENSE", entry.crate_name));
    download(&url, &to, Duration::from_secs(60))?;
    std::fs::read_to_string(&to).map_err(|error| error.to_string())
}

/// Compiles the grammar to wasm the way tree-sitter grammars are built for wasm runtimes: one shared object
/// exporting only `tree_sitter_<name>`, optimized for size.
fn compile(clang: &Path, src: &Path, grammar: &str, out: &Path) -> Result<(), String> {
    let parser = src.join("parser.c");
    let parser_text =
        std::fs::read_to_string(&parser).map_err(|error| format!("parser.c: {error}"))?;
    if !parser_text.contains(&format!("*tree_sitter_{grammar}(void)")) {
        return Err(format!("parser.c exports no tree_sitter_{grammar}"));
    }
    let scanner = src.join("scanner.c");
    let mut command = Command::new(clang);
    command
        .args(["-fPIC", "-shared", "-Os"])
        .arg(format!("-Wl,--export=tree_sitter_{grammar}"))
        .arg("-o")
        .arg(out)
        .arg("-I")
        .arg(src)
        .arg(&parser);
    if scanner.is_file() {
        command.arg(&scanner);
    }
    run(&mut command)
}

fn language_toml(entry: &Entry, lang: Lang, version: &str) -> String {
    let builtin = builtin_language(lang);
    let quote = |text: &str| toml::Value::String(text.to_string()).to_string();
    let mut text = format!(
        "name = {}\ngrammar = {}\nversion = {}\nrepository = {}\nrev = {}\nlicense = {}\n",
        quote(&entry.language),
        quote(&entry.grammar),
        quote(version),
        quote(&entry.repository),
        quote(&entry.rev),
        quote(&entry.license),
    );
    let suffixes: Vec<String> = builtin
        .path_suffixes
        .iter()
        .map(|suffix| quote(suffix))
        .collect();
    text.push_str(&format!("path_suffixes = [{}]\n", suffixes.join(", ")));
    if let Some(pattern) = builtin.first_line_pattern {
        text.push_str(&format!("first_line_pattern = {}\n", quote(pattern)));
    }
    text
}

fn build(args: &[String]) -> Result<(), String> {
    let paths = workspace_paths();
    let manifest = read_manifest(&paths.manifest)?;
    let locked = locked_crates(&paths.lockfile)?;
    let out = out_dir(args);
    let cache = PathBuf::from(option(args, "--cache").unwrap_or("target/grammar-cache"));
    let only: Option<Vec<&str>> = option(args, "--only").map(|ids| ids.split(',').collect());
    let base_url = option(args, "--base-url").map(|url| url.trim_end_matches('/').to_string());
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let clang = wasi_clang(&cache)?;
    let mut index = Index {
        version: 1,
        packages: Vec::new(),
    };
    for (id, entry) in &manifest {
        if only
            .as_ref()
            .is_some_and(|only| !only.contains(&id.as_str()))
        {
            continue;
        }
        let started = Instant::now();
        let lang = language(entry)?;
        let (version, checksum) = locked
            .get(&entry.crate_name)
            .ok_or_else(|| format!("{id}: {} is not in Cargo.lock", entry.crate_name))?;
        let source = crate_source(&cache, &entry.crate_name, version, checksum)?;
        let rev = published_rev(&source)?;
        if rev != entry.rev {
            return Err(format!(
                "{id}: the manifest says rev {} but {} {version} was published from {rev}",
                entry.rev, entry.crate_name
            ));
        }
        let grammar_root = entry
            .source
            .as_deref()
            .map_or(source.clone(), |folder| source.join(folder));
        let staging = tempfile::tempdir().map_err(|error| error.to_string())?;
        let package = staging.path();
        compile(
            &clang,
            &grammar_root.join("src"),
            &entry.grammar,
            &package.join("grammar.wasm"),
        )
        .map_err(|error| format!("{id}: {error}"))?;
        let builtin = builtin_language(lang);
        let write = |name: &str, text: &str| {
            std::fs::write(package.join(name), text)
                .map_err(|error| format!("{id}: {name}: {error}"))
        };
        write("highlights.scm", builtin.highlights)?;
        if let Some(injections) = builtin.injections {
            write("injections.scm", injections)?;
        }
        write("language.toml", &language_toml(entry, lang, version))?;
        write("LICENSE", &license_text(entry, &source, &cache)?)?;
        let file = format!("{id}-{version}.tar.gz");
        let archive = out.join(&file);
        run(Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(package)
            .arg("."))?;
        let bytes = std::fs::read(&archive).map_err(|error| error.to_string())?;
        eprintln!(
            "{id} {version}: {} KB in {:.1}s",
            bytes.len() / 1024,
            started.elapsed().as_secs_f32()
        );
        index.packages.push(IndexEntry {
            id: id.clone(),
            language: entry.language.clone(),
            version: version.clone(),
            url: base_url.as_ref().map(|base| format!("{base}/{file}")),
            file,
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            signature: None,
            license: entry.license.clone(),
            repository: entry.repository.clone(),
            rev: entry.rev.clone(),
            path_suffixes: builtin
                .path_suffixes
                .iter()
                .map(|suffix| suffix.to_string())
                .collect(),
            first_line_pattern: builtin.first_line_pattern.map(str::to_string),
        });
    }
    write_index(&out, &index)
}

fn write_index(out: &Path, index: &Index) -> Result<(), String> {
    let text = serde_json::to_string_pretty(index).map_err(|error| error.to_string())?;
    std::fs::write(out.join("index.json"), text + "\n").map_err(|error| error.to_string())
}

fn read_index(out: &Path) -> Result<Index, String> {
    let path = out.join("index.json");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Installs every built package as the app would, then checks each highlights its sample with exactly the
/// captures the compiled-in grammar gives.
fn verify(args: &[String]) -> Result<(), String> {
    let paths = workspace_paths();
    let manifest = read_manifest(&paths.manifest)?;
    let out = out_dir(args);
    let index = read_index(&out)?;
    let installed = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut expected = Vec::new();
    for package in &index.packages {
        let entry = manifest
            .get(&package.id)
            .ok_or_else(|| format!("{} is not in the manifest", package.id))?;
        let lang = language(entry)?;
        let sample_path = paths.samples.join(&entry.sample);
        let sample = std::fs::read_to_string(&sample_path)
            .map_err(|error| format!("{}: {error}", sample_path.display()))?;
        let builtin = builtin_language(lang);
        let native = builtin
            .grammar
            .ok_or_else(|| format!("{}: no compiled-in grammar to compare with", package.id))?;
        let captures =
            editor::grammar_packages::highlight_captures(&native, builtin.highlights, &sample)
                .map_err(|error| format!("{} (compiled in): {error}", package.id))?;
        if captures.is_empty() {
            return Err(format!("{}: the sample highlights nothing", package.id));
        }
        expected.push((package.id.clone(), lang, sample, captures));
        let folder = installed.path().join(&package.id).join(&package.version);
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        run(Command::new("tar")
            .arg("-xzf")
            .arg(out.join(&package.file))
            .arg("-C")
            .arg(&folder))?;
    }
    let registered = editor::grammar_packages::register_installed_grammars(installed.path());
    if registered != index.packages.len() {
        return Err(format!(
            "{registered} of {} packages registered",
            index.packages.len()
        ));
    }
    let registry = editor::registry::language_registry();
    for (id, lang, sample, captures) in expected {
        let deadline = Instant::now() + Duration::from_secs(60);
        let (grammar, highlights) = loop {
            if let Some(found) = editor::registry::request_grammar(registry, lang) {
                break found;
            }
            if Instant::now() > deadline {
                return Err(format!("{id}: the package grammar never loaded"));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if !grammar.is_wasm() {
            return Err(format!(
                "{id}: the registry still gives the compiled-in grammar"
            ));
        }
        let packaged = editor::grammar_packages::highlight_captures(&grammar, &highlights, &sample)
            .map_err(|error| format!("{id} (package): {error}"))?;
        if packaged != captures {
            let first = packaged
                .iter()
                .zip(&captures)
                .position(|(a, b)| a != b)
                .unwrap_or(packaged.len().min(captures.len()));
            return Err(format!(
                "{id}: {} captures from the package, {} compiled in, first difference at {first}",
                packaged.len(),
                captures.len()
            ));
        }
        eprintln!("{id}: {} captures match", captures.len());
    }
    Ok(())
}

fn signing_key(seed: &str) -> Result<SigningKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(seed.trim())
        .map_err(|error| format!("{SIGNING_KEY_ENV}: {error}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{SIGNING_KEY_ENV} must be 32 bytes, base64"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Signs each package's sha256 digest with the key in `GRAMMARS_SIGNING_KEY`; without the key, says so and
/// leaves the index unsigned.
fn sign(args: &[String]) -> Result<(), String> {
    let Some(seed) = std::env::var(SIGNING_KEY_ENV)
        .ok()
        .filter(|seed| !seed.is_empty())
    else {
        eprintln!("{SIGNING_KEY_ENV} is not set: the index stays unsigned");
        return Ok(());
    };
    let key = signing_key(&seed)?;
    let out = out_dir(args);
    let mut index = read_index(&out)?;
    for package in &mut index.packages {
        let bytes = std::fs::read(out.join(&package.file)).map_err(|error| error.to_string())?;
        if sha256_hex(&bytes) != package.sha256 {
            return Err(format!(
                "{}: the archive no longer matches its sha256",
                package.id
            ));
        }
        let digest = Sha256::digest(&bytes);
        let signature = key.sign(&digest);
        package.signature =
            Some(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
    }
    write_index(&out, &index)?;
    eprintln!(
        "signed {} packages with public key {}",
        index.packages.len(),
        base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
    );
    Ok(())
}

/// A new signing key from the system's random source: the secret for CI and the public half for the app.
fn keygen() -> Result<(), String> {
    use std::io::Read;
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut seed))
        .map_err(|error| format!("/dev/urandom: {error}"))?;
    let key = SigningKey::from_bytes(&seed);
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    println!(
        "{SIGNING_KEY_ENV} (secret, for the repository secret): {}",
        encode(&seed)
    );
    println!(
        "public key (for the app): {}",
        encode(&key.verifying_key().to_bytes())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn the_manifest_names_editor_languages_and_existing_samples() {
        let paths = workspace_paths();
        let manifest = read_manifest(&paths.manifest).expect("manifest");
        let locked = locked_crates(&paths.lockfile).expect("lockfile");
        assert!(manifest.len() >= 20);
        for (id, entry) in &manifest {
            language(entry).unwrap_or_else(|error| panic!("{id}: {error}"));
            assert!(
                locked.contains_key(&entry.crate_name),
                "{id}: {} not locked",
                entry.crate_name
            );
            assert!(
                paths.samples.join(&entry.sample).is_file(),
                "{id}: no sample"
            );
            assert!(
                entry.license_file.is_some() != entry.license_path.is_some(),
                "{id}: exactly one of license_file and license_path"
            );
        }
    }

    #[test]
    fn a_package_carries_the_languages_files_and_provenance() {
        let manifest = read_manifest(&workspace_paths().manifest).expect("manifest");
        let entry = &manifest["kotlin"];
        let text = language_toml(entry, Lang::Kotlin, "1.1.0");
        let parsed: toml::Table = toml::from_str(&text).expect("language.toml");
        assert_eq!(parsed["name"].as_str(), Some("Kotlin"));
        assert_eq!(parsed["grammar"].as_str(), Some("kotlin"));
        assert_eq!(parsed["rev"].as_str(), Some(entry.rev.as_str()));
        let suffixes = parsed["path_suffixes"].as_array().expect("suffixes");
        assert!(suffixes.iter().any(|suffix| suffix.as_str() == Some("kt")));
    }

    #[test]
    fn a_signature_checks_out_with_the_public_half() {
        let key =
            signing_key(&base64::engine::general_purpose::STANDARD.encode([7u8; 32])).expect("key");
        let digest = Sha256::digest(b"package");
        let signature = key.sign(&digest);
        assert!(key.verifying_key().verify(&digest, &signature).is_ok());
        assert!(signing_key("c2hvcnQ=").is_err());
    }
}
