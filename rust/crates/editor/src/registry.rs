//! The languages the editor knows: how a file is recognized as one, which grammar parses it, and the queries
//! that grammar is read with. Languages and grammars are registered separately so a grammar can come from
//! somewhere other than the app binary; a language's queries are only loaded when it is first used.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

use crate::highlight::Lang;

/// How a file is recognized as a language: an ending of its path, or a pattern its first line matches.
#[derive(Clone, Debug, Default)]
pub struct LanguageMatcher {
    /// An extension, a whole file name, or a path ending.
    pub path_suffixes: Vec<String>,
    pub first_line: Option<regex::Regex>,
}

/// The queries a grammar is read with.
#[derive(Clone, Debug)]
pub struct LanguageQueries {
    pub highlights: Arc<str>,
    pub injections: Option<Arc<str>>,
}

pub type QueryLoader = Arc<dyn Fn() -> LanguageQueries + Send + Sync>;

struct AvailableLanguage {
    lang: Lang,
    matcher: LanguageMatcher,
    grammar: Option<Arc<str>>,
    /// Left out of lookups by name: only other languages embed it.
    hidden: bool,
    load: QueryLoader,
    loaded: OnceLock<LanguageQueries>,
}

/// Where a grammar stands. Compiled in for now; one read from a package on first use adds states here.
enum GrammarState {
    Native(tree_sitter::Language),
}

#[derive(Default)]
pub struct LanguageRegistry {
    languages: Vec<AvailableLanguage>,
    grammars: HashMap<Arc<str>, GrammarState>,
    /// From the settings: files that are a language whatever their name says.
    file_types: Vec<(Lang, Vec<String>)>,
    /// Bumped on every registration, so a view knows to look its files' languages up again.
    version: u64,
}

impl LanguageRegistry {
    pub fn register_native_grammars(
        &mut self,
        grammars: impl IntoIterator<Item = (Arc<str>, tree_sitter::Language)>,
    ) {
        self.grammars.extend(
            grammars
                .into_iter()
                .map(|(name, grammar)| (name, GrammarState::Native(grammar))),
        );
        self.version += 1;
    }

    /// Adds `lang`, recognized by `matcher` and parsed by the grammar named `grammar`; its queries come from
    /// `load` the first time they are needed.
    pub fn register_language(
        &mut self,
        lang: Lang,
        matcher: LanguageMatcher,
        grammar: Option<Arc<str>>,
        hidden: bool,
        load: QueryLoader,
    ) {
        self.languages.retain(|language| language.lang != lang);
        self.languages.push(AvailableLanguage {
            lang,
            matcher,
            grammar,
            hidden,
            load,
            loaded: OnceLock::new(),
        });
        self.version += 1;
    }

    pub fn set_file_types(&mut self, file_types: Vec<(Lang, Vec<String>)>) {
        self.file_types = file_types;
        self.version += 1;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn language_for_name(&self, name: &str) -> Option<Lang> {
        self.languages
            .iter()
            .find(|language| !language.hidden && language.lang.name() == name)
            .map(|language| language.lang)
    }

    /// The language of the file at `path`. The settings' file types decide first; then the longest of the
    /// file's extension, name or whole path that equals a language's suffix or ends in `.suffix`; with no
    /// match, the first language whose first-line pattern matches.
    pub fn language_for_file(&self, path: &str, first_line: Option<&str>) -> Lang {
        let name = path.rsplit('/').next().unwrap_or(path);
        let configured = self.file_types.iter().find(|(_, patterns)| {
            patterns
                .iter()
                .any(|pattern| file_type_matches(pattern, name, path))
        });
        if let Some((lang, _)) = configured {
            return *lang;
        }
        let extension = name.rsplit('.').next().unwrap_or(name);
        let candidates = [extension, name, path];
        let mut best: Option<(usize, Lang)> = None;
        for language in &self.languages {
            for suffix in &language.matcher.path_suffixes {
                let dotted = format!(".{suffix}");
                let matched = candidates
                    .iter()
                    .find(|candidate| {
                        **candidate == suffix.as_str() || candidate.ends_with(&dotted)
                    })
                    .map(|candidate| candidate.len());
                if let Some(len) = matched {
                    if best.is_none_or(|(best_len, _)| len > best_len) {
                        best = Some((len, language.lang));
                    }
                }
            }
        }
        if let Some((_, lang)) = best {
            return lang;
        }
        let first_line = first_line.unwrap_or("");
        self.languages
            .iter()
            .find(|language| {
                language
                    .matcher
                    .first_line
                    .as_ref()
                    .is_some_and(|pattern| pattern.is_match(first_line))
            })
            .map_or(Lang::PlainText, |language| language.lang)
    }

    /// `lang`'s queries, loaded on first use.
    pub fn queries(&self, lang: Lang) -> Option<LanguageQueries> {
        let language = self
            .languages
            .iter()
            .find(|language| language.lang == lang)?;
        Some(language.loaded.get_or_init(|| (language.load)()).clone())
    }

    #[cfg(test)]
    fn queries_loaded(&self, lang: Lang) -> bool {
        self.languages
            .iter()
            .any(|language| language.lang == lang && language.loaded.get().is_some())
    }

    /// The grammar that parses `lang` and its highlight query; none for a language without a grammar.
    pub fn grammar(&self, lang: Lang) -> Option<(tree_sitter::Language, Arc<str>)> {
        let language = self
            .languages
            .iter()
            .find(|language| language.lang == lang)?;
        let grammar = match self.grammars.get(language.grammar.as_ref()?)? {
            GrammarState::Native(grammar) => grammar.clone(),
        };
        Some((grammar, self.queries(lang)?.highlights))
    }
}

/// Every editor in the app reads the same languages.
pub fn language_registry() -> &'static RwLock<LanguageRegistry> {
    static REGISTRY: LazyLock<RwLock<LanguageRegistry>> =
        LazyLock::new(|| RwLock::new(crate::highlight::builtin_registry()));
    &REGISTRY
}

pub(crate) fn file_type_matches(pattern: &str, name: &str, path: &str) -> bool {
    if !pattern.contains('*') {
        return name == pattern
            || name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension == pattern);
    }
    let subject = if pattern.contains('/') { path } else { name };
    let mut parts = pattern.split('*');
    let Some(mut rest) = subject.strip_prefix(parts.next().unwrap_or_default()) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    for (index, part) in parts.iter().enumerate() {
        if index + 1 == parts.len() {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn matcher(suffixes: &[&str], first_line: Option<&str>) -> LanguageMatcher {
        LanguageMatcher {
            path_suffixes: suffixes.iter().map(|suffix| suffix.to_string()).collect(),
            first_line: first_line.and_then(|pattern| regex::Regex::new(pattern).ok()),
        }
    }

    fn queries(highlights: &str) -> QueryLoader {
        let highlights: Arc<str> = highlights.into();
        Arc::new(move || LanguageQueries {
            highlights: highlights.clone(),
            injections: None,
        })
    }

    fn sample() -> LanguageRegistry {
        let mut registry = LanguageRegistry::default();
        registry.register_native_grammars([(Arc::from("rust"), tree_sitter_rust::LANGUAGE.into())]);
        registry.register_language(
            Lang::Rust,
            matcher(&["rs"], None),
            Some("rust".into()),
            false,
            queries("(identifier) @variable"),
        );
        registry.register_language(
            Lang::Toml,
            matcher(&["toml", "Cargo.lock"], None),
            None,
            false,
            queries(""),
        );
        registry.register_language(
            Lang::Python,
            matcher(&["py"], Some(r"^#!.*\bpython")),
            None,
            false,
            queries(""),
        );
        registry.register_language(
            Lang::Bash,
            matcher(&["sh"], Some(r"^#!.*\b(?:sh|bash)\b")),
            None,
            false,
            queries(""),
        );
        registry.register_language(Lang::Regex, matcher(&[], None), None, true, queries(""));
        registry
    }

    #[test]
    fn a_file_is_its_longest_suffix_then_its_first_line() {
        let registry = sample();
        assert_eq!(registry.language_for_file("src/main.rs", None), Lang::Rust);
        assert_eq!(registry.language_for_file("Cargo.lock", None), Lang::Toml);
        assert_eq!(registry.language_for_file("a/Cargo.lock", None), Lang::Toml);
        assert_eq!(
            registry.language_for_file("bin/tool", Some("#!/usr/bin/env python3")),
            Lang::Python
        );
        assert_eq!(
            registry.language_for_file("bin/run", Some("#!/bin/sh")),
            Lang::Bash
        );
        assert_eq!(
            registry.language_for_file("notes", Some("hello")),
            Lang::PlainText
        );
    }

    #[test]
    fn the_settings_file_types_win_over_the_suffix() {
        let mut registry = sample();
        registry.set_file_types(vec![(Lang::Python, vec!["*.rs.in".into()])]);
        assert_eq!(
            registry.language_for_file("gen/lib.rs.in", None),
            Lang::Python
        );
        assert_eq!(registry.language_for_file("src/lib.rs", None), Lang::Rust);
        registry.set_file_types(vec![(Lang::Toml, vec!["rs".into()])]);
        assert_eq!(registry.language_for_file("src/lib.rs", None), Lang::Toml);
    }

    #[test]
    fn hidden_languages_are_not_found_by_name() {
        let registry = sample();
        assert_eq!(registry.language_for_name("Rust"), Some(Lang::Rust));
        assert_eq!(registry.language_for_name("Regex"), None);
    }

    #[test]
    fn queries_load_once_on_first_use() {
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        let mut registry = sample();
        registry.register_language(
            Lang::Rust,
            matcher(&["rs"], None),
            Some("rust".into()),
            false,
            Arc::new(|| {
                LOADS.fetch_add(1, Ordering::SeqCst);
                LanguageQueries {
                    highlights: "(identifier) @variable".into(),
                    injections: None,
                }
            }),
        );
        assert!(!registry.queries_loaded(Lang::Rust));
        assert_eq!(LOADS.load(Ordering::SeqCst), 0);
        let (_, highlights) = registry.grammar(Lang::Rust).expect("rust grammar");
        assert_eq!(&*highlights, "(identifier) @variable");
        assert!(registry.grammar(Lang::Rust).is_some());
        assert!(registry.queries_loaded(Lang::Rust));
        assert_eq!(LOADS.load(Ordering::SeqCst), 1);
        assert!(
            registry.grammar(Lang::Toml).is_none(),
            "no grammar registered for it"
        );
    }

    #[test]
    fn registering_bumps_the_version() {
        let mut registry = sample();
        let before = registry.version();
        registry.set_file_types(Vec::new());
        assert!(registry.version() > before);
    }
}
