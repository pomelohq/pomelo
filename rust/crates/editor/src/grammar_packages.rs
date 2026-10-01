//! Grammars installed as packages. `<dir>/<language>/<version>-<hash>/` holds `grammar.wasm`, the queries it is
//! read with, `language.toml` saying which language it is and which files are that language, and the record of
//! which published package it is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::highlight::{native_queries, Lang};
use crate::language::{BracketPair, IndentRules, LanguageConfig};
use crate::registry::{language_registry, LanguageMatcher, LanguageQueries, LanguageRegistry};

/// What a package folder was installed from, written into it before it lands.
pub const INSTALL_RECORD: &str = "installed.json";

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct InstallRecord {
    /// The published package's input hash; packages from before inputs were hashed have none.
    #[serde(default)]
    pub input_hash: Option<String>,
    pub sha256: String,
    /// Seconds since the epoch: of a language's folders, the latest installed is the one in use.
    pub installed_at: u64,
}

pub fn install_record(folder: &Path) -> Option<InstallRecord> {
    let text = std::fs::read_to_string(folder.join(INSTALL_RECORD)).ok()?;
    serde_json::from_str(&text).ok()
}

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
    #[serde(flatten)]
    editing: Option<EditingConfig>,
}

/// How a package's language is edited, as `language.toml` spells it: comment markers, bracket pairs and the
/// line-based indent hints.
#[derive(Debug, Deserialize, PartialEq, serde::Serialize)]
pub struct EditingConfig {
    line_comments: Vec<String>,
    #[serde(default)]
    block_comment: Option<(String, String)>,
    autoclose_before: String,
    brackets: Vec<PackageBracket>,
    #[serde(default)]
    increase_indent_pattern: Option<String>,
    #[serde(default)]
    decrease_indent_pattern: Option<String>,
    #[serde(default)]
    decrease_indent_patterns: Vec<DecreaseAfter>,
    indent_from_last_non_blank_line: bool,
    snippet_scope: String,
}

#[derive(Debug, Deserialize, PartialEq, serde::Serialize)]
struct PackageBracket {
    start: String,
    end: String,
    close: bool,
    surround: bool,
    newline: bool,
    /// Scopes where the pair is off: `string`, `comment`.
    #[serde(default)]
    not_in: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, serde::Serialize)]
struct DecreaseAfter {
    pattern: String,
    valid_after: Vec<String>,
}

impl EditingConfig {
    pub fn from_native(config: &LanguageConfig, snippet_scope: &str) -> Self {
        let indent = config.indent;
        Self {
            line_comments: config.line_comments.iter().map(|c| c.to_string()).collect(),
            block_comment: config
                .block_comment
                .map(|(start, end)| (start.to_string(), end.to_string())),
            autoclose_before: config.autoclose_before.to_string(),
            brackets: config
                .brackets
                .iter()
                .map(|pair| PackageBracket {
                    start: pair.start.to_string(),
                    end: pair.end.to_string(),
                    close: pair.close,
                    surround: pair.surround,
                    newline: pair.newline,
                    not_in: [
                        pair.not_in_string.then_some("string"),
                        pair.not_in_comment.then_some("comment"),
                    ]
                    .into_iter()
                    .flatten()
                    .map(str::to_string)
                    .collect(),
                })
                .collect(),
            increase_indent_pattern: indent.increase.map(str::to_string),
            decrease_indent_pattern: indent.decrease.map(str::to_string),
            decrease_indent_patterns: indent
                .decrease_after
                .iter()
                .map(|(pattern, valid_after)| DecreaseAfter {
                    pattern: pattern.to_string(),
                    valid_after: valid_after.iter().map(|name| name.to_string()).collect(),
                })
                .collect(),
            indent_from_last_non_blank_line: indent.using_last_non_empty_line,
            snippet_scope: snippet_scope.to_string(),
        }
    }

    /// The config as the editor holds it. Its text is kept for the rest of the run: a language's config is
    /// read once per package registration, and the editor's config borrows its strings for good.
    fn into_language(self) -> (LanguageConfig, &'static str) {
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let brackets: Vec<BracketPair> = self
            .brackets
            .into_iter()
            .map(|pair| BracketPair {
                start: leak(pair.start),
                end: leak(pair.end),
                close: pair.close,
                surround: pair.surround,
                newline: pair.newline,
                not_in_string: pair.not_in.iter().any(|scope| scope == "string"),
                not_in_comment: pair.not_in.iter().any(|scope| scope == "comment"),
            })
            .collect();
        let line_comments: Vec<&'static str> = self.line_comments.into_iter().map(leak).collect();
        let decrease_after: Vec<(&'static str, &'static [&'static str])> = self
            .decrease_indent_patterns
            .into_iter()
            .map(|after| {
                let names: Vec<&'static str> = after.valid_after.into_iter().map(leak).collect();
                (leak(after.pattern), &*Box::leak(names.into_boxed_slice()))
            })
            .collect();
        let config = LanguageConfig {
            brackets: Box::leak(brackets.into_boxed_slice()),
            autoclose_before: leak(self.autoclose_before),
            line_comments: Box::leak(line_comments.into_boxed_slice()),
            block_comment: self
                .block_comment
                .map(|(start, end)| (leak(start), leak(end))),
            indent: IndentRules {
                increase: self.increase_indent_pattern.map(leak),
                decrease: self.decrease_indent_pattern.map(leak),
                decrease_after: Box::leak(decrease_after.into_boxed_slice()),
                using_last_non_empty_line: self.indent_from_last_non_blank_line,
            },
        };
        (config, leak(self.snippet_scope))
    }

    /// The fields as `language.toml` lines.
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|error| error.to_string())
    }
}

/// The query files a package can ship, and what each one is to the editor.
pub const QUERY_FILES: [&str; 5] = [
    "highlights.scm",
    "injections.scm",
    "outline.scm",
    "indents.scm",
    "overrides.scm",
];

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

/// Registers a language a published package provides, so its files are recognized, and open as plain text,
/// before the package is installed; a language that has a grammar already is left as it is.
pub fn register_available_language(
    name: &str,
    path_suffixes: Vec<String>,
    first_line_pattern: Option<&str>,
) -> bool {
    let Some(lang) = Lang::LANGUAGES
        .into_iter()
        .find(|lang| lang.name().eq_ignore_ascii_case(name))
    else {
        return false;
    };
    let Ok(mut registry) = language_registry().write() else {
        return false;
    };
    register_available(&mut registry, lang, path_suffixes, first_line_pattern)
}

pub(crate) fn register_available(
    registry: &mut LanguageRegistry,
    lang: Lang,
    path_suffixes: Vec<String>,
    first_line_pattern: Option<&str>,
) -> bool {
    if registry.has_grammar(lang) {
        return false;
    }
    let first_line = match first_line_pattern.map(regex::Regex::new).transpose() {
        Ok(first_line) => first_line,
        Err(error) => {
            eprintln!("grammars: {}: first_line_pattern: {error}", lang.name());
            None
        }
    };
    registry.register_language(
        lang,
        LanguageMatcher {
            path_suffixes,
            first_line,
        },
        None,
        false,
        Arc::new(move || native_queries(lang)),
    );
    true
}

/// Registers one package just installed, so files of its language highlight without a restart.
pub fn register_installed_grammar(package: &Path) -> Result<(), String> {
    let registered = match language_registry().write() {
        Ok(mut registry) => register_package(&mut registry, package),
        Err(_) => Err("the language registry is unavailable".to_string()),
    };
    if registered.is_ok() {
        crate::registry::notify_languages_changed();
    }
    registered
}

pub(crate) fn register_packages(registry: &mut LanguageRegistry, dir: &Path) -> usize {
    let Ok(languages) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut registered = 0;
    for language in languages.flatten() {
        let Some(package) = current_package(&language.path()) else {
            continue;
        };
        match register_package(registry, &package) {
            Ok(()) => registered += 1,
            Err(error) => eprintln!("grammars: {}: {error}", package.display()),
        }
    }
    registered
}

/// The folder of a language in use: the one installed last by its record, else (folders from before records)
/// the highest version, compared part by part as numbers where they are numbers.
pub fn current_package(language_dir: &Path) -> Option<PathBuf> {
    let versions = std::fs::read_dir(language_dir).ok()?;
    versions
        .flatten()
        .filter(|entry| {
            entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| {
            let installed = install_record(&entry.path()).map(|record| record.installed_at);
            (installed, version_key(&entry.file_name()), entry.path())
        })
        .max()
        .map(|(_, _, path)| path)
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
    let editing = std::sync::Mutex::new(config.editing);
    registry.register_language(
        lang,
        matcher,
        Some(grammar),
        false,
        Arc::new(move || {
            let editing = editing.lock().ok().and_then(|mut editing| editing.take());
            package_queries(&folder, lang, editing)
        }),
    );
    Ok(())
}

/// The package's queries and editing config; anything it doesn't ship falls back to what is compiled in for
/// its language.
fn package_queries(folder: &Path, lang: Lang, editing: Option<EditingConfig>) -> LanguageQueries {
    let native = native_queries(lang);
    let read = |name: &str| -> Option<Arc<str>> {
        std::fs::read_to_string(folder.join(name))
            .ok()
            .map(Into::into)
    };
    let (config, snippet_scope) = editing
        .map(EditingConfig::into_language)
        .unwrap_or((native.config, native.snippet_scope));
    LanguageQueries {
        highlights: read("highlights.scm").unwrap_or(native.highlights),
        injections: read("injections.scm").or(native.injections),
        outline: read("outline.scm").or(native.outline),
        indents: read("indents.scm").or(native.indents),
        overrides: read("overrides.scm").or(native.overrides),
        config,
        snippet_scope,
    }
}

/// What `highlights` captures in `text` parsed with `grammar`: each capture's byte range and name, in order.
pub fn highlight_captures(
    grammar: &tree_sitter::Language,
    highlights: &str,
    text: &str,
) -> Result<Vec<(std::ops::Range<usize>, String)>, String> {
    query_captures(grammar, highlights, text)
}

/// The syntax tree `grammar` gives `text`, written out: two grammars that agree on it fold, match brackets
/// and indent alike.
pub fn syntax_tree(grammar: &tree_sitter::Language, text: &str) -> Result<String, String> {
    let tree = crate::parsers::with_parser(|parser| {
        parser
            .set_language(grammar)
            .map_err(|error| error.to_string())?;
        parser
            .parse(text, None)
            .ok_or_else(|| "the parse gave no tree".to_string())
    })?;
    Ok(tree.root_node().to_sexp())
}

pub fn query_compiles(grammar: &tree_sitter::Language, query: &str) -> bool {
    tree_sitter::Query::new(grammar, query).is_ok()
}

/// What `query` captures in `text` parsed with `grammar`: each capture's byte range and name, in order.
pub fn query_captures(
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
    fn a_published_language_is_recognized_as_plain_text_until_its_package_installs() {
        let registry: &'static RwLock<LanguageRegistry> =
            Box::leak(Box::new(RwLock::new(crate::highlight::builtin_registry())));
        {
            let mut registry = registry.write().expect("registry");
            assert_eq!(registry.language_for_file("App.kt", None), Lang::PlainText);
            assert!(register_available(
                &mut registry,
                Lang::Kotlin,
                vec!["kt".into(), "kts".into()],
                None
            ));
            assert!(
                !register_available(&mut registry, Lang::Ruby, vec!["rb".into()], None),
                "a compiled-in language keeps its own"
            );
            assert_eq!(registry.language_for_file("src/App.kt", None), Lang::Kotlin);
            assert_eq!(registry.language_for_name("Kotlin"), Some(Lang::Kotlin));
            assert!(!registry.has_grammar(Lang::Kotlin));
            assert!(registry.grammar(Lang::Kotlin).is_none());
            assert!(registry
                .queries(Lang::Kotlin)
                .is_some_and(|queries| queries.config.line_comments.is_empty()));
        }
        let temp = tempfile::tempdir().expect("temp");
        let package = temp.path().join("kotlin").join("1.1.0");
        std::fs::create_dir_all(&package).expect("package dir");
        std::fs::write(package.join("grammar.wasm"), FIXTURE).expect("wasm");
        std::fs::write(
            package.join("language.toml"),
            "name = \"Kotlin\"\ngrammar = \"json\"\n",
        )
        .expect("config");
        std::fs::write(package.join("highlights.scm"), "(number) @number\n").expect("query");
        register_package(&mut registry.write().expect("registry"), &package).expect("registered");
        assert!(registry.read().expect("registry").has_grammar(Lang::Kotlin));
        assert_eq!(
            registry
                .read()
                .expect("registry")
                .language_for_file("src/App.kt", None),
            Lang::Kotlin,
            "the package keeps the matcher it was recognized by"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let (grammar, highlights) = loop {
            if let Some(found) = crate::registry::request_grammar(registry, Lang::Kotlin) {
                break found;
            }
            assert!(Instant::now() < deadline, "the grammar never loaded");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(grammar.is_wasm());
        let captures = highlight_captures(&grammar, &highlights, "[1, 2]").expect("captures");
        assert_eq!(captures.len(), 2);
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
        assert_eq!(
            &*queries.highlights,
            crate::highlight::native_highlights(Lang::Json)
        );
    }

    #[test]
    fn every_languages_editing_config_round_trips_through_language_toml() {
        let embedded = [Lang::MarkdownInline, Lang::Regex, Lang::JsDoc];
        for lang in Lang::LANGUAGES.iter().chain(&embedded).copied() {
            let native = crate::language::native_config(lang);
            let scope = crate::snippet::native_snippet_scope(lang);
            let text = format!(
                "name = \"{}\"\ngrammar = \"x\"\n{}",
                lang.name(),
                EditingConfig::from_native(&native, scope)
                    .to_toml()
                    .expect("toml")
            );
            let parsed: PackageConfig = toml::from_str(&text).expect("parse");
            let (config, snippet_scope) = parsed.editing.expect("editing fields").into_language();
            assert_eq!(config, native, "{lang:?}");
            assert_eq!(snippet_scope, scope, "{lang:?}");
        }
    }

    #[test]
    fn a_package_supplies_every_query_and_its_editing_config() {
        let temp = tempfile::tempdir().expect("temp");
        install(temp.path(), "1.0.0", Some("(string) @string\n"));
        let package = temp.path().join("json").join("1.0.0");
        std::fs::write(package.join("outline.scm"), "(pair key: (_) @name) @item\n")
            .expect("outline");
        std::fs::write(package.join("indents.scm"), "(object \"}\" @end) @indent\n")
            .expect("indents");
        std::fs::write(package.join("overrides.scm"), "(string) @string\n").expect("overrides");
        let mut text = std::fs::read_to_string(package.join("language.toml")).expect("config");
        let mut editing =
            EditingConfig::from_native(&crate::language::native_config(Lang::Json), "json");
        editing.line_comments = vec!["## ".into()];
        text.push_str(&editing.to_toml().expect("toml"));
        std::fs::write(package.join("language.toml"), text).expect("config");
        let mut registry = LanguageRegistry::default();
        assert_eq!(register_packages(&mut registry, temp.path()), 1);
        let queries = registry.queries(Lang::Json).expect("queries");
        assert_eq!(
            queries.outline.as_deref(),
            Some("(pair key: (_) @name) @item\n")
        );
        assert_eq!(
            queries.indents.as_deref(),
            Some("(object \"}\" @end) @indent\n")
        );
        assert_eq!(queries.overrides.as_deref(), Some("(string) @string\n"));
        assert_eq!(queries.config.line_comments, ["## "]);
        assert_eq!(
            queries.config.brackets,
            crate::language::native_config(Lang::Json).brackets
        );
        assert_eq!(queries.snippet_scope, "json");
    }

    #[test]
    fn a_package_without_editing_fields_edits_like_the_built_in_language() {
        let temp = tempfile::tempdir().expect("temp");
        install(temp.path(), "1.0.0", None);
        let mut registry = LanguageRegistry::default();
        assert_eq!(register_packages(&mut registry, temp.path()), 1);
        let queries = registry.queries(Lang::Json).expect("queries");
        assert_eq!(queries.config, crate::language::native_config(Lang::Json));
        assert_eq!(
            queries.outline.as_deref(),
            crate::outline::native_outline(Lang::Json)
        );
    }

    #[test]
    fn the_folder_installed_last_is_the_one_in_use() {
        let temp = tempfile::tempdir().expect("temp");
        let language = temp.path().join("json");
        for (folder, installed_at) in [
            ("2.0.0", None),
            ("1.0.0-aaaa1111", Some(10)),
            ("1.0.0-bbbb2222", Some(20)),
        ] {
            let folder = language.join(folder);
            std::fs::create_dir_all(&folder).expect("folder");
            if let Some(installed_at) = installed_at {
                let record = InstallRecord {
                    input_hash: Some(folder.display().to_string()),
                    sha256: "0".repeat(64),
                    installed_at,
                };
                std::fs::write(
                    folder.join(INSTALL_RECORD),
                    serde_json::to_string(&record).expect("record"),
                )
                .expect("write record");
            }
        }
        std::fs::create_dir_all(language.join(".staging-1")).expect("staging");
        assert_eq!(
            current_package(&language),
            Some(language.join("1.0.0-bbbb2222")),
            "a recorded install wins over a higher version without one"
        );
    }

    #[test]
    fn versions_compare_as_numbers() {
        let key = |name: &str| version_key(std::ffi::OsStr::new(name));
        assert!(key("0.10.0") > key("0.9.3"));
        assert!(key("v1.2") > key("1.1.9"));
    }
}
