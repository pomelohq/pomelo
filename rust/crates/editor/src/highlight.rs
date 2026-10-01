use std::sync::Arc;

use crate::registry::{language_registry, LanguageMatcher, LanguageQueries, LanguageRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
    Python,
    Json,
    C,
    Cpp,
    Bash,
    Css,
    Html,
    Ruby,
    Java,
    Toml,
    Yaml,
    Lua,
    CSharp,
    Markdown,
    Php,
    Scala,
    Elixir,
    Haskell,
    Ocaml,
    Scss,
    Nix,
    Swift,
    Make,
    Xml,
    Zig,
    Dart,
    Sql,
    Kotlin,
    Svelte,
    Dockerfile,
    GraphQl,
    Hcl,
    Proto,
    Diff,
    GitCommit,
    Ini,
    Erlang,
    Gleam,
    R,
    Elm,
    Prisma,
    /// Only embedded: Markdown's inline syntax, regex literals, doc comments.
    MarkdownInline,
    Regex,
    JsDoc,
    PlainText,
}

/// Languages whose grammar is a package: known by name, recognized once their published package's matcher is
/// registered, and plain text until the package is installed.
pub const PACKAGED: [Lang; 21] = [
    Lang::CSharp,
    Lang::Dart,
    Lang::Elixir,
    Lang::Elm,
    Lang::Erlang,
    Lang::Gleam,
    Lang::GraphQl,
    Lang::Haskell,
    Lang::Hcl,
    Lang::Kotlin,
    Lang::Lua,
    Lang::Nix,
    Lang::Ocaml,
    Lang::Prisma,
    Lang::Proto,
    Lang::R,
    Lang::Scala,
    Lang::Svelte,
    Lang::Swift,
    Lang::Xml,
    Lang::Zig,
];

/// Files the settings say are a language, over detection: whole names, extensions, or `*` globs.
pub fn set_file_types(file_types: Vec<(Lang, Vec<String>)>) {
    if let Ok(mut registry) = language_registry().write() {
        registry.set_file_types(file_types);
    }
}

impl Lang {
    /// Every language a file can be, by name.
    pub const LANGUAGES: [Lang; 47] = [
        Lang::Rust,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::JavaScript,
        Lang::Go,
        Lang::Python,
        Lang::Json,
        Lang::C,
        Lang::Cpp,
        Lang::Bash,
        Lang::Css,
        Lang::Html,
        Lang::Ruby,
        Lang::Java,
        Lang::Toml,
        Lang::Yaml,
        Lang::Lua,
        Lang::CSharp,
        Lang::Markdown,
        Lang::Php,
        Lang::Scala,
        Lang::Elixir,
        Lang::Haskell,
        Lang::Ocaml,
        Lang::Scss,
        Lang::Nix,
        Lang::Swift,
        Lang::Make,
        Lang::Xml,
        Lang::Zig,
        Lang::Dart,
        Lang::Sql,
        Lang::Kotlin,
        Lang::Svelte,
        Lang::Dockerfile,
        Lang::GraphQl,
        Lang::Hcl,
        Lang::Proto,
        Lang::Diff,
        Lang::GitCommit,
        Lang::Ini,
        Lang::Erlang,
        Lang::Gleam,
        Lang::R,
        Lang::Elm,
        Lang::Prisma,
        Lang::PlainText,
    ];

    pub fn from_name(name: &str) -> Option<Lang> {
        language_registry().read().ok()?.language_for_name(name)
    }

    pub fn tab_size(self) -> usize {
        match self {
            Lang::TypeScript
            | Lang::Tsx
            | Lang::JavaScript
            | Lang::Json
            | Lang::Yaml
            | Lang::Markdown
            | Lang::Dart
            | Lang::Ruby
            | Lang::Elixir
            | Lang::Gleam => 2,
            _ => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::TypeScript => "TypeScript",
            Lang::Tsx => "TSX",
            Lang::JavaScript => "JavaScript",
            Lang::Go => "Go",
            Lang::Python => "Python",
            Lang::Json => "JSON",
            Lang::C => "C",
            Lang::Cpp => "C++",
            Lang::Bash => "Shell Script",
            Lang::Css => "CSS",
            Lang::Html => "HTML",
            Lang::Ruby => "Ruby",
            Lang::Java => "Java",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Lua => "Lua",
            Lang::CSharp => "C#",
            Lang::Markdown => "Markdown",
            Lang::Php => "PHP",
            Lang::Scala => "Scala",
            Lang::Elixir => "Elixir",
            Lang::Haskell => "Haskell",
            Lang::Ocaml => "OCaml",
            Lang::Scss => "SCSS",
            Lang::Nix => "Nix",
            Lang::Swift => "Swift",
            Lang::Make => "Makefile",
            Lang::Xml => "XML",
            Lang::Zig => "Zig",
            Lang::Dart => "Dart",
            Lang::Sql => "SQL",
            Lang::Kotlin => "Kotlin",
            Lang::Svelte => "Svelte",
            Lang::Dockerfile => "Dockerfile",
            Lang::GraphQl => "GraphQL",
            Lang::Hcl => "HCL",
            Lang::Proto => "Proto",
            Lang::Diff => "Diff",
            Lang::GitCommit => "Git Commit",
            Lang::Ini => "INI",
            Lang::Erlang => "Erlang",
            Lang::Gleam => "Gleam",
            Lang::R => "R",
            Lang::Elm => "Elm",
            Lang::Prisma => "Prisma",
            Lang::MarkdownInline => "Markdown-Inline",
            Lang::Regex => "Regex",
            Lang::JsDoc => "JSDoc",
            Lang::PlainText => "Plain Text",
        }
    }

    /// The file endings the language is known by when its grammar is built in; empty otherwise.
    pub fn path_suffixes(self) -> &'static [&'static str] {
        PATH_SUFFIXES
            .iter()
            .find(|(lang, _)| *lang == self)
            .map_or(&[], |(_, suffixes)| *suffixes)
    }

    pub fn from_ext(ext: &str) -> Lang {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "go" => Lang::Go,
            "py" | "pyi" => Lang::Python,
            "json" | "jsonc" => Lang::Json,
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "sh" | "bash" | "zsh" => Lang::Bash,
            "css" => Lang::Css,
            "html" | "htm" => Lang::Html,
            "rb" | "gemspec" => Lang::Ruby,
            "java" => Lang::Java,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "lua" => Lang::Lua,
            "cs" => Lang::CSharp,
            "md" | "markdown" => Lang::Markdown,
            "php" => Lang::Php,
            "scala" | "sc" | "sbt" => Lang::Scala,
            "ex" | "exs" => Lang::Elixir,
            "hs" => Lang::Haskell,
            "ml" | "mli" => Lang::Ocaml,
            "scss" => Lang::Scss,
            "nix" => Lang::Nix,
            "swift" => Lang::Swift,
            "mk" | "makefile" => Lang::Make,
            "xml" | "svg" | "xaml" | "plist" => Lang::Xml,
            "zig" => Lang::Zig,
            "dart" => Lang::Dart,
            "sql" => Lang::Sql,
            "kt" | "kts" => Lang::Kotlin,
            "svelte" => Lang::Svelte,
            "dockerfile" | "containerfile" => Lang::Dockerfile,
            "graphql" | "gql" => Lang::GraphQl,
            "hcl" | "tf" | "tfvars" => Lang::Hcl,
            "proto" => Lang::Proto,
            "diff" | "patch" => Lang::Diff,
            "ini" => Lang::Ini,
            "erl" | "hrl" => Lang::Erlang,
            "gleam" => Lang::Gleam,
            "r" => Lang::R,
            "elm" => Lang::Elm,
            "prisma" => Lang::Prisma,
            _ => Lang::PlainText,
        }
    }

    /// The language of the file at `path`: the longest of its extension, name, or whole path that equals a
    /// language's suffix or ends in `.suffix` wins; with no match, a first-line pattern (a shebang) decides.
    pub fn detect(path: &str, first_line: Option<&str>) -> Lang {
        language_registry()
            .read()
            .map_or(Lang::PlainText, |registry| {
                registry.language_for_file(path, first_line)
            })
    }

    /// The language an injection names, by language name or file extension (`rust`, `rs`, `c++`...).
    pub fn for_injection(name: &str) -> Option<Lang> {
        let lang = match name.trim().to_ascii_lowercase().as_str() {
            "rust" => Lang::Rust,
            "typescript" => Lang::TypeScript,
            "javascript" | "jsx" => Lang::JavaScript,
            "python" | "python3" => Lang::Python,
            "golang" => Lang::Go,
            "c++" => Lang::Cpp,
            "shell" | "shellscript" | "console" => Lang::Bash,
            "ruby" => Lang::Ruby,
            "csharp" | "c#" => Lang::CSharp,
            "haskell" => Lang::Haskell,
            "ocaml" => Lang::Ocaml,
            "elixir" => Lang::Elixir,
            "kotlin" => Lang::Kotlin,
            "docker" => Lang::Dockerfile,
            "graphql" => Lang::GraphQl,
            "terraform" => Lang::Hcl,
            "protobuf" => Lang::Proto,
            "erlang" => Lang::Erlang,
            "postgresql" | "postgres" | "mysql" | "sqlite" => Lang::Sql,
            "makefile" | "make" => Lang::Make,
            "markdown_inline" | "markdown-inline" => Lang::MarkdownInline,
            "regex" => Lang::Regex,
            "jsdoc" => Lang::JsDoc,
            ext => Lang::from_ext(ext),
        };
        (lang != Lang::PlainText).then_some(lang)
    }
}

/// File name suffixes per language (an extension, a whole file name, or a path ending).
const PATH_SUFFIXES: &[(Lang, &[&str])] = &[
    (Lang::Rust, &["rs"]),
    (Lang::TypeScript, &["ts", "cts", "mts"]),
    (Lang::Tsx, &["tsx"]),
    (Lang::JavaScript, &["js", "jsx", "mjs", "cjs"]),
    (Lang::Go, &["go"]),
    (Lang::Python, &["py", "pyi", "mpy"]),
    (
        Lang::Json,
        &[
            "json",
            "jsonc",
            "flake.lock",
            "geojson",
            "topojson",
            "prettierrc",
            "json.dist",
            "deno.lock",
            "bun.lock",
            "babelrc",
            "eslintrc",
            "swcrc",
        ],
    ),
    (Lang::C, &["c"]),
    (
        Lang::Cpp,
        &[
            "cc", "ccm", "hh", "cpp", "cppm", "h", "hpp", "cxx", "cxxm", "hxx", "c++", "c++m",
            "h++", "hip", "ipp", "inl", "ino", "ixx", "cu", "cuh", "C", "H",
        ],
    ),
    (
        Lang::Bash,
        &[
            "sh",
            "bash",
            "bashrc",
            "bash_profile",
            "bash_aliases",
            "bash_login",
            "bash_logout",
            "bats",
            "envrc",
            "profile",
            "zsh",
            "zshrc",
            "zshenv",
            "zsh_profile",
            "zsh_aliases",
            "zlogin",
            "zprofile",
            ".env",
            "PKGBUILD",
            "APKBUILD",
            "ebuild",
        ],
    ),
    (Lang::Css, &["css", "postcss", "pcss"]),
    (Lang::Html, &["html", "htm"]),
    (
        Lang::Ruby,
        &["rb", "gemspec", "rake", "Gemfile", "Rakefile"],
    ),
    (Lang::Java, &["java"]),
    (Lang::Toml, &["toml", "Cargo.lock"]),
    (
        Lang::Yaml,
        &["yml", "yaml", "pixi.lock", "clang-format", "clangd", "bst"],
    ),
    (
        Lang::Markdown,
        &["md", "mdx", "mdwn", "mdc", "markdown", "MD"],
    ),
    (Lang::Php, &["php"]),
    (Lang::Scss, &["scss"]),
    (Lang::Make, &["mk", "Makefile", "makefile", "GNUmakefile"]),
    (Lang::Sql, &["sql"]),
    (
        Lang::Dockerfile,
        &["Dockerfile", "Containerfile", "dockerfile", "containerfile"],
    ),
    (Lang::Diff, &["diff", "patch"]),
    (
        Lang::GitCommit,
        &[
            "TAG_EDITMSG",
            "MERGE_MSG",
            "COMMIT_EDITMSG",
            "NOTES_EDITMSG",
            "EDIT_DESCRIPTION",
        ],
    ),
    (
        Lang::Ini,
        &["ini", "cfg", "editorconfig", "gitconfig", "gitmodules"],
    ),
];

/// Patterns a file's first line can match to name its language when the path doesn't.
const FIRST_LINE_PATTERNS: &[(Lang, &str)] = &[
    (Lang::Bash, r"^#!.*\b(?:ash|bash|bats|dash|sh|zsh)\b"),
    (Lang::Python, r"^#!.*((\bpython[0-9.]*\b)|(\buv run\b))"),
    (
        Lang::JavaScript,
        r"^#!.*\b(?:[/ ]node|deno run.*--ext[= ]js)\b",
    ),
    (Lang::TypeScript, r"^#!.*\b(?:deno run|ts-node|bun|tsx)\b"),
    (Lang::Go, r"^//.*\bgo run\b"),
    (Lang::Cpp, r"^//.*-\*-\s*C\+\+\s*-\*-"),
];

/// The injection query `lang`'s grammar is read with, where it has one.
pub fn injection_patterns(lang: Lang) -> Option<Arc<str>> {
    language_registry().read().ok()?.queries(lang)?.injections
}

/// The injection query a compiled-in grammar ships with, where it has one.
pub(crate) fn native_injections(lang: Lang) -> Option<&'static str> {
    Some(match lang {
        Lang::Markdown => tree_sitter_md::INJECTION_QUERY_BLOCK,
        Lang::MarkdownInline => tree_sitter_md::INJECTION_QUERY_INLINE,
        Lang::Html => tree_sitter_html::INJECTIONS_QUERY,
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => tree_sitter_javascript::INJECTIONS_QUERY,
        Lang::Rust => tree_sitter_rust::INJECTIONS_QUERY,
        Lang::Php => tree_sitter_php::INJECTIONS_QUERY,
        Lang::GitCommit => tree_sitter_gitcommit::INJECTIONS_QUERY,
        _ => return None,
    })
}

// Capture names we recognize; tree-sitter maps each query capture to an index into this list. Based on the One
// Dark syntax keys so grammar captures resolve to consistent styles.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "enum",
    "function",
    "function.method",
    "keyword",
    "label",
    "namespace",
    "number",
    "operator",
    "predictive",
    "preproc",
    "primary",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.list_marker",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.literal",
    "title",
    "type",
    "type.builtin",
    "variable",
    "variable.parameter",
    "variable.special",
    "variant",
];

// These grammars extend another and ship only what they add, so their queries go after the ones they build
// on (the later pattern wins), as each grammar's own tree-sitter.json lists them.
static JAVASCRIPT_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
    ]
    .join("\n")
});
static TYPESCRIPT_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ]
    .join("\n")
});
static TSX_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ]
    .join("\n")
});
static CPP_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_c::HIGHLIGHT_QUERY,
        tree_sitter_cpp::HIGHLIGHT_QUERY,
    ]
    .join("\n")
});

/// The grammar compiled into the app for `lang`.
fn native_grammar(lang: Lang) -> Option<tree_sitter::Language> {
    Some(match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Json => tree_sitter_json::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        Lang::Css => tree_sitter_css::LANGUAGE.into(),
        Lang::Html => tree_sitter_html::LANGUAGE.into(),
        Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
        Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
        Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Lang::Scss => tree_sitter_scss::language(),
        Lang::Make => tree_sitter_make::LANGUAGE.into(),
        Lang::Sql => tree_sitter_sequel::LANGUAGE.into(),
        Lang::Dockerfile => tree_sitter_containerfile::LANGUAGE.into(),
        Lang::Diff => tree_sitter_diff::LANGUAGE.into(),
        Lang::GitCommit => tree_sitter_gitcommit::LANGUAGE.into(),
        Lang::Ini => tree_sitter_ini::LANGUAGE.into(),
        Lang::MarkdownInline => tree_sitter_md::INLINE_LANGUAGE.into(),
        Lang::Regex => tree_sitter_regex::LANGUAGE.into(),
        Lang::JsDoc => tree_sitter_jsdoc::LANGUAGE.into(),
        Lang::PlainText => return None,
        // Grammar packages, downloaded when a file needs them.
        Lang::CSharp
        | Lang::Dart
        | Lang::Elixir
        | Lang::Elm
        | Lang::Erlang
        | Lang::Gleam
        | Lang::GraphQl
        | Lang::Haskell
        | Lang::Hcl
        | Lang::Kotlin
        | Lang::Lua
        | Lang::Nix
        | Lang::Ocaml
        | Lang::Prisma
        | Lang::Proto
        | Lang::R
        | Lang::Scala
        | Lang::Svelte
        | Lang::Swift
        | Lang::Xml
        | Lang::Zig => return None,
    })
}

/// The highlight query a compiled-in grammar is read with.
pub(crate) fn native_highlights(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
        Lang::TypeScript => TYPESCRIPT_HIGHLIGHTS.as_str(),
        Lang::Tsx => TSX_HIGHLIGHTS.as_str(),
        Lang::JavaScript => JAVASCRIPT_HIGHLIGHTS.as_str(),
        Lang::Go => tree_sitter_go::HIGHLIGHTS_QUERY,
        Lang::Python => tree_sitter_python::HIGHLIGHTS_QUERY,
        Lang::Json => tree_sitter_json::HIGHLIGHTS_QUERY,
        Lang::C => tree_sitter_c::HIGHLIGHT_QUERY,
        Lang::Cpp => CPP_HIGHLIGHTS.as_str(),
        Lang::Bash => tree_sitter_bash::HIGHLIGHT_QUERY,
        Lang::Css => tree_sitter_css::HIGHLIGHTS_QUERY,
        Lang::Html => tree_sitter_html::HIGHLIGHTS_QUERY,
        Lang::Ruby => tree_sitter_ruby::HIGHLIGHTS_QUERY,
        Lang::Java => tree_sitter_java::HIGHLIGHTS_QUERY,
        Lang::Toml => tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        Lang::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY,
        Lang::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        Lang::Php => tree_sitter_php::HIGHLIGHTS_QUERY,
        Lang::Scss => tree_sitter_scss::HIGHLIGHTS_QUERY,
        Lang::Make => tree_sitter_make::HIGHLIGHTS_QUERY,
        Lang::Sql => tree_sitter_sequel::HIGHLIGHTS_QUERY,
        Lang::Dockerfile => tree_sitter_containerfile::HIGHLIGHTS_QUERY,
        Lang::Diff => tree_sitter_diff::HIGHLIGHTS_QUERY,
        Lang::GitCommit => tree_sitter_gitcommit::HIGHLIGHTS_QUERY,
        Lang::Ini => tree_sitter_ini::HIGHLIGHTS_QUERY,
        Lang::MarkdownInline => tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
        Lang::Regex => tree_sitter_regex::HIGHLIGHTS_QUERY,
        Lang::JsDoc => tree_sitter_jsdoc::HIGHLIGHTS_QUERY,
        Lang::PlainText => "",
        Lang::CSharp
        | Lang::Dart
        | Lang::Elixir
        | Lang::Elm
        | Lang::Erlang
        | Lang::Gleam
        | Lang::GraphQl
        | Lang::Haskell
        | Lang::Hcl
        | Lang::Kotlin
        | Lang::Lua
        | Lang::Nix
        | Lang::Ocaml
        | Lang::Prisma
        | Lang::Proto
        | Lang::R
        | Lang::Scala
        | Lang::Svelte
        | Lang::Swift
        | Lang::Xml
        | Lang::Zig => "",
    }
}

/// Everything compiled into the app that `lang` is edited with besides its grammar.
pub(crate) fn native_queries(lang: Lang) -> LanguageQueries {
    LanguageQueries {
        highlights: native_highlights(lang).into(),
        injections: native_injections(lang).map(Into::into),
        outline: crate::outline::native_outline(lang).map(Into::into),
        indents: Some(crate::indent::native_indents(lang))
            .filter(|patterns| !patterns.is_empty())
            .map(Into::into),
        overrides: crate::syntax::native_overrides(lang).map(Into::into),
        config: crate::language::native_config(lang),
        snippet_scope: crate::snippet::native_snippet_scope(lang),
    }
}

/// The grammar that parses `lang` and its highlight query; none for plain text.
pub fn grammar(lang: Lang) -> Option<(tree_sitter::Language, Arc<str>)> {
    crate::registry::request_grammar(language_registry(), lang)
}

/// The languages and grammars compiled into the app. Languages with a first-line pattern keep the order
/// those patterns are tried in; path suffixes are unique across languages, so their order doesn't matter.
pub(crate) fn builtin_registry() -> LanguageRegistry {
    let mut registry = LanguageRegistry::default();
    let embedded = [Lang::MarkdownInline, Lang::Regex, Lang::JsDoc];
    let grammars = Lang::LANGUAGES
        .iter()
        .chain(&embedded)
        .filter_map(|lang| Some((Arc::from(lang.name()), native_grammar(*lang)?)));
    registry.register_native_grammars(grammars.collect::<Vec<_>>());
    let first_line = |lang: Lang| {
        FIRST_LINE_PATTERNS
            .iter()
            .find(|(each, _)| *each == lang)
            .and_then(|(_, pattern)| regex::Regex::new(pattern).ok())
    };
    let suffixes = |lang: Lang| -> Vec<String> {
        PATH_SUFFIXES
            .iter()
            .find(|(each, _)| *each == lang)
            .map(|(_, suffixes)| suffixes.iter().map(|suffix| suffix.to_string()).collect())
            .unwrap_or_default()
    };
    let ordered = FIRST_LINE_PATTERNS
        .iter()
        .map(|(lang, _)| *lang)
        .chain(PATH_SUFFIXES.iter().map(|(lang, _)| *lang))
        .chain(PACKAGED)
        .chain([Lang::PlainText]);
    let mut registered: Vec<Lang> = Vec::new();
    for lang in ordered {
        if registered.contains(&lang) {
            continue;
        }
        registered.push(lang);
        register_builtin(&mut registry, lang, suffixes(lang), first_line(lang), false);
    }
    for lang in embedded {
        register_builtin(&mut registry, lang, Vec::new(), None, true);
    }
    registry
}

fn register_builtin(
    registry: &mut LanguageRegistry,
    lang: Lang,
    path_suffixes: Vec<String>,
    first_line: Option<regex::Regex>,
    hidden: bool,
) {
    let grammar = (lang != Lang::PlainText).then(|| Arc::from(lang.name()));
    registry.register_language(
        lang,
        LanguageMatcher {
            path_suffixes,
            first_line,
        },
        grammar,
        hidden,
        Arc::new(move || native_queries(lang)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::file_type_matches;

    #[test]
    fn file_types_match_names_extensions_and_globs() {
        assert!(file_type_matches("notjs", "app.notjs", "src/app.notjs"));
        assert!(file_type_matches(
            "Embargo.lock",
            "Embargo.lock",
            "Embargo.lock"
        ));
        assert!(file_type_matches("*.env*", ".env.local", "api/.env.local"));
        assert!(file_type_matches(
            "**/templates/*.html",
            "a.html",
            "web/templates/a.html"
        ));
        assert!(!file_type_matches("*.env*", "main.rs", "main.rs"));
        assert!(!file_type_matches("lock", "Cargo.toml", "Cargo.toml"));
    }

    #[test]
    fn every_grammar_highlight_query_compiles() {
        let embedded = [Lang::MarkdownInline, Lang::Regex, Lang::JsDoc];
        let failing: Vec<String> = Lang::LANGUAGES
            .iter()
            .chain(&embedded)
            .filter_map(|lang| {
                let (language, source) = grammar(*lang)?;
                tree_sitter::Query::new(&language, &source)
                    .err()
                    .map(|error| format!("{lang:?}: {error}"))
            })
            .collect();
        assert!(failing.is_empty(), "{failing:#?}");
    }

    #[test]
    fn detects_by_the_longest_path_match_then_the_first_line() {
        assert_eq!(Lang::detect("src/main.rs", None), Lang::Rust);
        assert_eq!(Lang::detect("Dockerfile", None), Lang::Dockerfile);
        assert_eq!(Lang::detect(".git/COMMIT_EDITMSG", None), Lang::GitCommit);
        assert_eq!(Lang::detect("include/a.h", None), Lang::Cpp);
        assert_eq!(Lang::detect("Cargo.lock", None), Lang::Toml);
        assert_eq!(Lang::detect("tsconfig.json", None), Lang::Json);
        assert_eq!(Lang::detect(".zshrc", None), Lang::Bash);
        assert_eq!(
            Lang::detect("bin/tool", Some("#!/usr/bin/env python3")),
            Lang::Python
        );
        assert_eq!(Lang::detect("bin/run", Some("#!/bin/sh")), Lang::Bash);
        assert_eq!(
            Lang::detect("bin/serve", Some("#!/usr/bin/env node")),
            Lang::JavaScript
        );
        assert_eq!(Lang::detect("notes", Some("hello")), Lang::PlainText);
    }

    #[test]
    fn first_line_patterns_are_tried_in_their_order_and_names_resolve() {
        assert_eq!(
            Lang::detect("bin/x", Some("#!/usr/bin/env -S deno run --ext=js")),
            Lang::JavaScript
        );
        assert_eq!(
            Lang::detect("bin/x", Some("#!/usr/bin/env -S deno run")),
            Lang::TypeScript
        );
        for lang in Lang::LANGUAGES {
            assert_eq!(Lang::from_name(lang.name()), Some(lang));
        }
        assert_eq!(Lang::from_name("Regex"), None);
    }
}
