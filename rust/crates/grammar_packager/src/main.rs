//! Builds the grammar packages the app downloads instead of compiling those grammars in.
//!
//! `build` compiles each grammar in `grammars/manifest.toml` to wasm from the crate version it names, with the
//! queries and editing config in `grammars/languages/<id>/`, into `<id>-<version>-<input hash>.tar.gz` plus an
//! `index.json`. `verify` loads the built packages the way the app does and checks each gives for its sample
//! what `grammars/expected/<id>.json` records the compiled-in language gave. `keep-published` keeps the
//! published package of every language whose inputs did not change. `sign` signs the packages not signed yet;
//! `check-index` checks an index is signed by the app's key; `keygen` makes the signing key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use editor::highlight::Lang;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The wasi-sdk release grammars are compiled with.
const WASI_SDK_VERSION: &str = "34";
const SIGNING_KEY_ENV: &str = "GRAMMARS_SIGNING_KEY";
/// Flags every grammar compiles with; part of each package's input hash.
const COMPILE_FLAGS: [&str; 3] = ["-fPIC", "-shared", "-Os"];
/// Bumped when what goes into a package changes in a way its inputs don't show, so every package is rebuilt.
const PACKAGE_FORMAT: u32 = 1;

#[derive(Clone, Deserialize)]
struct Entry {
    language: String,
    #[serde(rename = "crate")]
    crate_name: String,
    version: String,
    checksum: String,
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
    /// What the package was built from; unchanged inputs keep the published package.
    #[serde(default)]
    input_hash: Option<String>,
    /// How the app recognizes the language's files before the package is installed.
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

struct Paths {
    manifest: PathBuf,
    samples: PathBuf,
    languages: PathBuf,
    expected: PathBuf,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("verify") => verify(&args[1..]),
        Some("sign") => sign(&args[1..]),
        Some("keep-published") => keep_published(&args[1..]),
        Some("check-index") => check_index(&args[1..]),
        Some("keygen") => keygen(),
        _ => Err(
            "usage: grammar_packager build [--out DIR] [--only id,id] [--base-url URL] [--cache DIR]\n       \
             grammar_packager verify [--out DIR]\n       grammar_packager sign [--out DIR]\n       \
             grammar_packager keep-published --published FILE [--out DIR]\n       \
             grammar_packager check-index --index FILE\n       grammar_packager keygen"
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
        samples: workspace.join("grammars/samples"),
        languages: workspace.join("grammars/languages"),
        expected: workspace.join("grammars/expected"),
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

/// The crate's unpacked source, downloaded from crates.io and checked against the manifest's checksum.
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
            "{name} {version}: checksum {found} is not the manifest's {checksum}"
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
        .args(COMPILE_FLAGS)
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

/// Everything a package is made from: its crate, how it compiles, the toolchain and the language's own
/// queries and config. The grammar's wasm bytes differ from build to build, so packages are compared by this.
fn input_hash(entry: &Entry, kept: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let fields = [
        format!("format={PACKAGE_FORMAT}"),
        format!("crate={}", entry.crate_name),
        format!("version={}", entry.version),
        format!("checksum={}", entry.checksum),
        format!("source={}", entry.source.as_deref().unwrap_or_default()),
        format!("grammar={}", entry.grammar),
        format!("language={}", entry.language),
        format!("repository={}", entry.repository),
        format!("rev={}", entry.rev),
        format!("license={}", entry.license),
        format!(
            "license_file={}",
            entry.license_file.as_deref().unwrap_or_default()
        ),
        format!(
            "license_path={}",
            entry.license_path.as_deref().unwrap_or_default()
        ),
        format!("wasi-sdk={WASI_SDK_VERSION}"),
        format!("flags={}", COMPILE_FLAGS.join(" ")),
    ];
    for field in fields {
        hasher.update(field.as_bytes());
        hasher.update(b"\n");
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(kept)
        .map_err(|error| format!("{}: {error}", kept.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    for file in files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// `language.toml`: where the package comes from, then the language's matcher and editing config as kept in
/// `languages/<id>/language.toml`.
fn language_toml(entry: &Entry, kept: &str) -> String {
    let quote = |text: &str| toml::Value::String(text.to_string()).to_string();
    format!(
        "name = {}\ngrammar = {}\nversion = {}\nrepository = {}\nrev = {}\nlicense = {}\n{kept}",
        quote(&entry.language),
        quote(&entry.grammar),
        quote(&entry.version),
        quote(&entry.repository),
        quote(&entry.rev),
        quote(&entry.license),
    )
}

/// The language's matcher as `languages/<id>/language.toml` keeps it, for the index.
#[derive(Deserialize)]
struct KeptMatcher {
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

fn build(args: &[String]) -> Result<(), String> {
    let paths = workspace_paths();
    let manifest = read_manifest(&paths.manifest)?;
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
        language(entry)?;
        let version = &entry.version;
        let source = crate_source(&cache, &entry.crate_name, version, &entry.checksum)?;
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
        let kept = paths.languages.join(id);
        for name in editor::grammar_packages::QUERY_FILES {
            if kept.join(name).is_file() {
                std::fs::copy(kept.join(name), package.join(name))
                    .map_err(|error| format!("{id}: {name}: {error}"))?;
            }
        }
        let kept_toml = std::fs::read_to_string(kept.join("language.toml"))
            .map_err(|error| format!("{id}: language.toml: {error}"))?;
        let matcher: KeptMatcher =
            toml::from_str(&kept_toml).map_err(|error| format!("{id}: language.toml: {error}"))?;
        let write = |name: &str, text: &str| {
            std::fs::write(package.join(name), text)
                .map_err(|error| format!("{id}: {name}: {error}"))
        };
        write("language.toml", &language_toml(entry, &kept_toml))?;
        write("LICENSE", &license_text(entry, &source, &cache)?)?;
        let hash = input_hash(entry, &kept)?;
        // The hash in the name keeps a rebuilt package from replacing the file an older index still points at.
        let file = format!("{id}-{version}-{}.tar.gz", &hash[..8]);
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
            input_hash: Some(hash),
            path_suffixes: matcher.path_suffixes,
            first_line_pattern: matcher.first_line_pattern,
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

/// Captures as `expected/<id>.json` keeps them: byte start, byte end, capture name.
type Captures = Vec<(usize, usize, String)>;

/// What the compiled-in language gave for a package's sample, recorded before its grammar left the app.
#[derive(Deserialize)]
struct Expected {
    captures: Captures,
    tree: String,
    /// Captures of each other query the language has, by kind.
    queried: Vec<(String, Captures)>,
    /// Its editing config as `language.toml` spells it.
    editing: String,
}

fn flat(captures: Vec<(std::ops::Range<usize>, String)>) -> Captures {
    captures
        .into_iter()
        .map(|(range, name)| (range.start, range.end, name))
        .collect()
}

/// Installs every built package as the app would, then checks each gives for its sample exactly what
/// `expected/<id>.json` records: highlight captures, syntax tree, the other queries' captures, editing config.
fn verify(args: &[String]) -> Result<(), String> {
    let paths = workspace_paths();
    let manifest = read_manifest(&paths.manifest)?;
    let out = out_dir(args);
    let index = read_index(&out)?;
    let installed = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut checks = Vec::new();
    for package in &index.packages {
        let entry = manifest
            .get(&package.id)
            .ok_or_else(|| format!("{} is not in the manifest", package.id))?;
        let lang = language(entry)?;
        let sample_path = paths.samples.join(&entry.sample);
        let sample = std::fs::read_to_string(&sample_path)
            .map_err(|error| format!("{}: {error}", sample_path.display()))?;
        let expected_path = paths.expected.join(format!("{}.json", package.id));
        let expected: Expected = std::fs::read_to_string(&expected_path)
            .map_err(|error| error.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
            .map_err(|error| format!("{}: {error}", expected_path.display()))?;
        if expected.captures.is_empty() {
            return Err(format!("{}: the sample highlights nothing", package.id));
        }
        checks.push((package.id.clone(), lang, sample, expected));
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
    for (id, lang, sample, expected) in checks {
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
            return Err(format!("{id}: the registry gives a compiled-in grammar"));
        }
        let packaged = flat(
            editor::grammar_packages::highlight_captures(&grammar, &highlights, &sample)
                .map_err(|error| format!("{id} (package): {error}"))?,
        );
        if packaged != expected.captures {
            let first = packaged
                .iter()
                .zip(&expected.captures)
                .position(|(a, b)| a != b)
                .unwrap_or(packaged.len().min(expected.captures.len()));
            return Err(format!(
                "{id}: {} captures from the package, {} expected, first difference at {first}",
                packaged.len(),
                expected.captures.len()
            ));
        }
        let tree = editor::grammar_packages::syntax_tree(&grammar, &sample)
            .map_err(|error| format!("{id} (package): {error}"))?;
        if tree != expected.tree {
            return Err(format!(
                "{id}: the package parses the sample into a different tree"
            ));
        }
        let queries = registry
            .read()
            .map_err(|_| "the language registry is unavailable".to_string())?
            .queries(lang)
            .ok_or_else(|| format!("{id}: no queries registered"))?;
        for (kind, found) in &expected.queried {
            let source = match kind.as_str() {
                "injections" => queries.injections.clone(),
                "outline" => queries.outline.clone(),
                "indents" => queries.indents.clone(),
                _ => queries.overrides.clone(),
            }
            .ok_or_else(|| format!("{id}: the package has no {kind} query"))?;
            let packaged = flat(
                editor::grammar_packages::query_captures(&grammar, &source, &sample)
                    .map_err(|error| format!("{id} {kind} (package): {error}"))?,
            );
            if &packaged != found {
                return Err(format!(
                    "{id}: {kind}: {} captures from the package, {} expected",
                    packaged.len(),
                    found.len()
                ));
            }
        }
        let editing = editor::grammar_packages::EditingConfig::from_native(
            &queries.config,
            queries.snippet_scope,
        )
        .to_toml()?;
        if editing != expected.editing {
            return Err(format!(
                "{id}: the package's editing config differs from the expected one"
            ));
        }
        let kinds: Vec<&str> = expected
            .queried
            .iter()
            .map(|(kind, _)| kind.as_str())
            .collect();
        eprintln!(
            "{id}: {} captures, the tree, the editing config{} match",
            expected.captures.len(),
            if kinds.is_empty() {
                String::new()
            } else {
                format!(" and {}", kinds.join(", "))
            }
        );
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
    let mut signed = 0;
    for package in index
        .packages
        .iter_mut()
        .filter(|package| package.signature.is_none())
    {
        signed += 1;
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
        "signed {signed} packages with public key {}",
        base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
    );
    Ok(())
}

/// Swaps each freshly built package for the published one when its inputs are the same, deleting the fresh
/// archive so only changed packages are uploaded; writes the ids still to publish to `changed.txt`.
fn keep_published(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let published_path =
        option(args, "--published").ok_or("keep-published needs --published FILE")?;
    let published: Index = match std::fs::read_to_string(published_path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|error| format!("{published_path}: {error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Index {
            version: 1,
            packages: Vec::new(),
        },
        Err(error) => return Err(format!("{published_path}: {error}")),
    };
    let mut index = read_index(&out)?;
    let mut changed = Vec::new();
    for package in &mut index.packages {
        let same = published.packages.iter().find(|old| {
            old.id == package.id
                && old.signature.is_some()
                && old.url.is_some()
                && old.input_hash.is_some()
                && old.input_hash == package.input_hash
        });
        match same {
            Some(old) => {
                let fresh = out.join(&package.file);
                if fresh.exists() {
                    std::fs::remove_file(&fresh)
                        .map_err(|error| format!("{}: {error}", fresh.display()))?;
                }
                *package = IndexEntry {
                    id: old.id.clone(),
                    language: old.language.clone(),
                    version: old.version.clone(),
                    file: old.file.clone(),
                    url: old.url.clone(),
                    size: old.size,
                    sha256: old.sha256.clone(),
                    signature: old.signature.clone(),
                    license: old.license.clone(),
                    repository: old.repository.clone(),
                    rev: old.rev.clone(),
                    input_hash: old.input_hash.clone(),
                    path_suffixes: package.path_suffixes.clone(),
                    first_line_pattern: package.first_line_pattern.clone(),
                };
            }
            None => changed.push(package.id.clone()),
        }
    }
    let removed: Vec<&str> = published
        .packages
        .iter()
        .filter(|old| !index.packages.iter().any(|package| package.id == old.id))
        .map(|old| old.id.as_str())
        .collect();
    write_index(&out, &index)?;
    std::fs::write(out.join("changed.txt"), changed.join("\n"))
        .map_err(|error| error.to_string())?;
    eprintln!(
        "{} of {} packages changed: {}{}",
        changed.len(),
        index.packages.len(),
        if changed.is_empty() {
            "none".to_string()
        } else {
            changed.join(", ")
        },
        if removed.is_empty() {
            String::new()
        } else {
            format!("; no longer listed: {}", removed.join(", "))
        }
    );
    Ok(())
}

/// Checks every package in the index at `--index` is signed by the key the app carries.
fn check_index(args: &[String]) -> Result<(), String> {
    let path = option(args, "--index").ok_or("check-index needs --index FILE")?;
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let index = grammars::parse_index(&text).map_err(|error| format!("{path}: {error}"))?;
    let key = grammars::GRAMMARS_PUBLIC_KEY.ok_or("the app carries no grammars key")?;
    grammars::verify_index(&index, key)?;
    eprintln!(
        "{path}: {} packages, every one signed by the app's key",
        index.packages.len()
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
    fn the_manifest_names_editor_languages_with_their_files() {
        let paths = workspace_paths();
        let manifest = read_manifest(&paths.manifest).expect("manifest");
        assert!(manifest.len() >= 20);
        for (id, entry) in &manifest {
            language(entry).unwrap_or_else(|error| panic!("{id}: {error}"));
            assert_eq!(entry.checksum.len(), 64, "{id}: checksum");
            assert!(
                paths.samples.join(&entry.sample).is_file(),
                "{id}: no sample"
            );
            assert!(
                paths.languages.join(id).join("highlights.scm").is_file(),
                "{id}: no highlights"
            );
            assert!(
                paths.expected.join(format!("{id}.json")).is_file(),
                "{id}: no expected results"
            );
            assert!(
                entry.license_file.is_some() != entry.license_path.is_some(),
                "{id}: exactly one of license_file and license_path"
            );
        }
    }

    #[test]
    fn a_package_input_hash_follows_its_files_and_its_crate() {
        let paths = workspace_paths();
        let manifest = read_manifest(&paths.manifest).expect("manifest");
        let entry = &manifest["kotlin"];
        let temp = tempfile::tempdir().expect("temp");
        let kept = temp.path().join("kotlin");
        std::fs::create_dir_all(&kept).expect("dir");
        std::fs::write(kept.join("highlights.scm"), "(comment) @comment").expect("write");
        let first = input_hash(entry, &kept).expect("hash");
        assert_eq!(first, input_hash(entry, &kept).expect("hash"), "stable");
        std::fs::write(kept.join("highlights.scm"), "(comment) @comment.line").expect("write");
        let edited = input_hash(entry, &kept).expect("hash");
        assert_ne!(first, edited, "a query edit changes it");
        let mut bumped = entry.clone();
        bumped.version = "9.9.9".into();
        assert_ne!(
            edited,
            input_hash(&bumped, &kept).expect("hash"),
            "a crate bump changes it"
        );
    }

    fn entry_for(id: &str, hash: &str, signature: Option<&str>) -> IndexEntry {
        IndexEntry {
            id: id.into(),
            language: id.into(),
            version: "1".into(),
            file: format!("{id}-1-{}.tar.gz", &hash[..8.min(hash.len())]),
            url: Some(format!("https://example.invalid/{id}")),
            size: 1,
            sha256: "00".repeat(32),
            signature: signature.map(str::to_string),
            license: "MIT".into(),
            repository: "https://example.invalid".into(),
            rev: "r".into(),
            input_hash: Some(hash.into()),
            path_suffixes: vec![id.into()],
            first_line_pattern: None,
        }
    }

    #[test]
    fn only_packages_whose_inputs_changed_are_published_again() {
        let temp = tempfile::tempdir().expect("temp");
        let out = temp.path().join("out");
        std::fs::create_dir_all(&out).expect("dir");
        let mut fresh = Vec::new();
        for (id, hash) in [
            ("lua", "aaaaaaaa11"),
            ("nix", "bbbbbbbb22"),
            ("zig", "cccccccc33"),
        ] {
            let mut entry = entry_for(id, hash, None);
            entry.url = Some(format!("https://example.invalid/new/{id}"));
            std::fs::write(out.join(&entry.file), id).expect("archive");
            fresh.push(entry);
        }
        write_index(
            &out,
            &Index {
                version: 1,
                packages: fresh,
            },
        )
        .expect("index");
        let published = temp.path().join("published.json");
        let old = Index {
            version: 1,
            packages: vec![
                entry_for("lua", "aaaaaaaa11", Some("signed-lua")),
                entry_for("nix", "dddddddd44", Some("signed-nix")),
                entry_for("elm", "eeeeeeee55", Some("signed-elm")),
            ],
        };
        std::fs::write(&published, serde_json::to_string(&old).expect("json")).expect("write");
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        keep_published(&args(&[
            "--published",
            published.to_str().expect("path"),
            "--out",
            out.to_str().expect("path"),
        ]))
        .expect("keep");
        let index = read_index(&out).expect("index");
        let lua = index
            .packages
            .iter()
            .find(|package| package.id == "lua")
            .expect("lua");
        assert_eq!(
            lua.signature.as_deref(),
            Some("signed-lua"),
            "kept as published"
        );
        assert!(
            !out.join("lua-1-aaaaaaaa.tar.gz").exists(),
            "its fresh archive is not uploaded"
        );
        let nix = index
            .packages
            .iter()
            .find(|package| package.id == "nix")
            .expect("nix");
        assert!(
            nix.signature.is_none() && out.join(&nix.file).exists(),
            "changed: rebuilt"
        );
        assert!(!index.packages.iter().any(|package| package.id == "elm"));
        let changed = std::fs::read_to_string(out.join("changed.txt")).expect("changed");
        assert_eq!(changed.lines().collect::<Vec<_>>(), ["nix", "zig"]);
        keep_published(&args(&[
            "--published",
            "/nonexistent/index.json",
            "--out",
            out.to_str().expect("path"),
        ]))
        .expect("nothing published yet is fine");
    }

    #[test]
    fn a_package_carries_the_languages_files_and_provenance() {
        let paths = workspace_paths();
        let manifest = read_manifest(&paths.manifest).expect("manifest");
        let entry = &manifest["kotlin"];
        let kept = std::fs::read_to_string(paths.languages.join("kotlin/language.toml"))
            .expect("kept language.toml");
        let text = language_toml(entry, &kept);
        let parsed: toml::Table = toml::from_str(&text).expect("language.toml");
        assert_eq!(parsed["name"].as_str(), Some("Kotlin"));
        assert_eq!(parsed["grammar"].as_str(), Some("kotlin"));
        assert_eq!(parsed["version"].as_str(), Some(entry.version.as_str()));
        assert_eq!(parsed["rev"].as_str(), Some(entry.rev.as_str()));
        let suffixes = parsed["path_suffixes"].as_array().expect("suffixes");
        assert!(suffixes.iter().any(|suffix| suffix.as_str() == Some("kt")));
        assert!(parsed.contains_key("brackets"), "editing config");
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
