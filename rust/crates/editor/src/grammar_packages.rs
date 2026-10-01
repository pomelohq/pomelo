//! Grammars installed as packages. `<dir>/<language>/<version>/` holds `grammar.wasm`, the queries it is read
//! with, and `language.toml` saying which language it is and which files are that language.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::highlight::{native_highlights, native_injections, Lang};
use crate::registry::{language_registry, LanguageMatcher, LanguageQueries, LanguageRegistry};

#[derive(Deserialize)]
struct PackageConfig {
    /// The language, as the editor names it.
    name: String,
    /// The grammar's own name, which its wasm exports `tree_sitter_<grammar>` under.
    grammar: String,
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
}

pub fn grammars_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Pomelo/grammars"))
}

/// Registers every package installed under `dir` with the editor's languages; how many were found.
pub fn register_installed_grammars(dir: &Path) -> usize {
    match language_registry().write() {
        Ok(mut registry) => register_packages(&mut registry, dir),
        Err(_) => 0,
    }
}

pub(crate) fn register_packages(registry: &mut LanguageRegistry, dir: &Path) -> usize {
    let Ok(languages) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut registered = 0;
    for language in languages.flatten() {
        let Some(package) = newest_version(&language.path()) else {
            continue;
        };
        match register_package(registry, &package) {
            Ok(()) => registered += 1,
            Err(error) => eprintln!("grammars: {}: {error}", package.display()),
        }
    }
    registered
}

/// The version folder with the highest version, compared part by part as numbers where they are numbers.
fn newest_version(language_dir: &Path) -> Option<PathBuf> {
    let versions = std::fs::read_dir(language_dir).ok()?;
    versions
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .max_by(|a, b| version_key(&a.file_name()).cmp(&version_key(&b.file_name())))
        .map(|entry| entry.path())
}

fn version_key(name: &std::ffi::OsStr) -> Vec<(u64, String)> {
    name.to_string_lossy()
        .trim_start_matches('v')
        .split('.')
        .map(|part| (part.parse().unwrap_or(0), part.to_string()))
        .collect()
}

fn register_package(registry: &mut LanguageRegistry, package: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(package.join("language.toml"))
        .map_err(|error| format!("language.toml: {error}"))?;
    let config: PackageConfig =
        toml::from_str(&text).map_err(|error| format!("language.toml: {error}"))?;
    let lang = Lang::LANGUAGES
        .into_iter()
        .find(|lang| lang.name().eq_ignore_ascii_case(&config.name))
        .ok_or_else(|| format!("unknown language {}", config.name))?;
    let wasm = package.join("grammar.wasm");
    if !wasm.is_file() {
        return Err("no grammar.wasm".into());
    }
    let first_line = config
        .first_line_pattern
        .as_deref()
        .map(regex::Regex::new)
        .transpose()
        .map_err(|error| format!("first_line_pattern: {error}"))?;
    let known = registry.matcher(lang).unwrap_or_default();
    let matcher = LanguageMatcher {
        path_suffixes: if config.path_suffixes.is_empty() {
            known.path_suffixes
        } else {
            config.path_suffixes
        },
        first_line: first_line.or(known.first_line),
    };
    let grammar: Arc<str> = config.grammar.into();
    registry.register_wasm_grammars([(grammar.clone(), wasm)]);
    let folder = package.to_path_buf();
    registry.register_language(
        lang,
        matcher,
        Some(grammar),
        false,
        Arc::new(move || package_queries(&folder, lang)),
    );
    Ok(())
}

/// The package's queries; one it doesn't ship falls back to the one compiled in for its language.
fn package_queries(folder: &Path, lang: Lang) -> LanguageQueries {
    let read = |name: &str| -> Option<Arc<str>> {
        std::fs::read_to_string(folder.join(name))
            .ok()
            .map(Into::into)
    };
    LanguageQueries {
        highlights: read("highlights.scm").unwrap_or_else(|| native_highlights(lang).into()),
        injections: read("injections.scm").or_else(|| native_injections(lang).map(Into::into)),
    }
}

/// What `highlights` captures in `text` parsed with `grammar`: each capture's byte range and name, in order.
pub fn highlight_captures(
    grammar: &tree_sitter::Language,
    highlights: &str,
    text: &str,
) -> Result<Vec<(std::ops::Range<usize>, String)>, String> {
    use tree_sitter::{Query, QueryCursor, StreamingIterator};
    let tree = crate::parsers::with_parser(|parser| {
        parser
            .set_language(grammar)
            .map_err(|error| error.to_string())?;
        parser
            .parse(text, None)
            .ok_or_else(|| "the parse gave no tree".to_string())
    })?;
    let query = Query::new(grammar, highlights).map_err(|error| error.to_string())?;
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
    let mut found = Vec::new();
    while let Some((matched, index)) = captures.next() {
        let capture = matched.captures()[*index];
        found.push((
            capture.node.byte_range(),
            query.capture_names()[capture.index as usize].to_string(),
        ));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::RwLock;
    use std::time::{Duration, Instant};
    use tree_sitter::{Query, QueryCursor, StreamingIterator};

    const FIXTURE: &[u8] = include_bytes!("../fixtures/grammars/json/grammar.wasm");

    fn install(dir: &Path, version: &str, highlights: Option<&str>) {
        let package = dir.join("json").join(version);
        std::fs::create_dir_all(&package).expect("package dir");
        std::fs::write(package.join("grammar.wasm"), FIXTURE).expect("wasm");
        std::fs::write(
            package.join("language.toml"),
            "name = \"JSON\"\ngrammar = \"json\"\npath_suffixes = [\"json\", \"jsonl\"]\n",
        )
        .expect("config");
        if let Some(highlights) = highlights {
            std::fs::write(package.join("highlights.scm"), highlights).expect("query");
        }
    }

    #[test]
    fn a_package_grammar_compiles_off_thread_then_highlights() {
        static CHANGED: AtomicUsize = AtomicUsize::new(0);
        crate::registry::on_languages_changed(Arc::new(|| {
            CHANGED.fetch_add(1, Ordering::SeqCst);
        }));
        let temp = tempfile::tempdir().expect("temp");
        install(temp.path(), "0.24.0", None);
        install(
            temp.path(),
            "0.24.8",
            Some("(string) @string\n(number) @number\n"),
        );
        let registry: &'static RwLock<LanguageRegistry> =
            Box::leak(Box::new(RwLock::new(LanguageRegistry::default())));
        let registered = register_packages(&mut registry.write().expect("registry"), temp.path());
        assert_eq!(registered, 1);
        let before = registry.read().expect("registry").version();
        assert_eq!(
            registry
                .read()
                .expect("registry")
                .language_for_file("logs/today.jsonl", None),
            Lang::Json
        );
        assert!(
            crate::registry::request_grammar(registry, Lang::Json).is_none(),
            "the first ask starts the compile and returns at once"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let (grammar, highlights) = loop {
            if let Some(found) = crate::registry::request_grammar(registry, Lang::Json) {
                break found;
            }
            assert!(Instant::now() < deadline, "the grammar never loaded");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(grammar.is_wasm());
        assert!(registry.read().expect("registry").version() > before);
        assert!(CHANGED.load(Ordering::SeqCst) >= 1);
        assert!(
            highlights.contains("(number) @number"),
            "the newest version's own query"
        );
        let source = r#"{"name": "web", "port": 8080}"#;
        let tree = crate::parsers::with_parser(|parser| {
            parser.set_language(&grammar).ok()?;
            parser.parse(source, None)
        })
        .expect("parsed");
        let query = Query::new(&grammar, &highlights).expect("query");
        let mut cursor = QueryCursor::new();
        let mut captures = cursor.captures(&query, tree.root_node(), source.as_bytes());
        let mut seen = Vec::new();
        while let Some((found, index)) = captures.next() {
            let capture = found.captures()[*index];
            seen.push((
                query.capture_names()[capture.index as usize].to_string(),
                &source[capture.node.byte_range()],
            ));
        }
        assert!(seen.contains(&("number".to_string(), "8080")), "{seen:?}");
        assert!(
            seen.contains(&("string".to_string(), "\"web\"")),
            "{seen:?}"
        );
    }

    #[test]
    fn a_package_without_its_own_query_uses_the_built_in_one() {
        let temp = tempfile::tempdir().expect("temp");
        install(temp.path(), "1.0.0", None);
        let mut registry = LanguageRegistry::default();
        assert_eq!(register_packages(&mut registry, temp.path()), 1);
        let queries = registry.queries(Lang::Json).expect("queries");
        assert_eq!(&*queries.highlights, native_highlights(Lang::Json));
    }

    #[test]
    fn versions_compare_as_numbers() {
        let key = |name: &str| version_key(std::ffi::OsStr::new(name));
        assert!(key("0.10.0") > key("0.9.3"));
        assert!(key("v1.2") > key("1.1.9"));
    }
}
