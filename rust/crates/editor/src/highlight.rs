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
    /// Only embedded: Markdown's inline syntax, regex literals, doc comments.
    MarkdownInline,
    Regex,
    JsDoc,
    PlainText,
}

impl Lang {
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
            _ => Lang::PlainText,
        }
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
            "makefile" | "make" => Lang::Make,
            "markdown_inline" | "markdown-inline" => Lang::MarkdownInline,
            "regex" => Lang::Regex,
            "jsdoc" => Lang::JsDoc,
            ext => Lang::from_ext(ext),
        };
        (lang != Lang::PlainText).then_some(lang)
    }
}

/// The injection query a grammar ships with, where it has one.
pub fn injection_patterns(lang: Lang) -> Option<&'static str> {
    Some(match lang {
        Lang::Markdown => tree_sitter_md::INJECTION_QUERY_BLOCK,
        Lang::MarkdownInline => tree_sitter_md::INJECTION_QUERY_INLINE,
        Lang::Html => tree_sitter_html::INJECTIONS_QUERY,
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => tree_sitter_javascript::INJECTIONS_QUERY,
        Lang::Rust => tree_sitter_rust::INJECTIONS_QUERY,
        Lang::Php => tree_sitter_php::INJECTIONS_QUERY,
        Lang::Elixir => tree_sitter_elixir::INJECTIONS_QUERY,
        Lang::Nix => tree_sitter_nix::INJECTIONS_QUERY,
        Lang::Swift => tree_sitter_swift::INJECTIONS_QUERY,
        Lang::Zig => tree_sitter_zig::INJECTIONS_QUERY,
        Lang::Lua => tree_sitter_lua::INJECTIONS_QUERY,
        Lang::Haskell => tree_sitter_haskell::INJECTIONS_QUERY,
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

pub fn grammar(lang: Lang) -> Option<(tree_sitter::Language, &'static str)> {
    let (language, highlights): (tree_sitter::Language, &'static str) = match lang {
        Lang::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        ),
        Lang::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ),
        Lang::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ),
        Lang::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
        ),
        Lang::Go => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY,
        ),
        Lang::Python => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
        ),
        Lang::Json => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
        ),
        Lang::C => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY,
        ),
        Lang::Cpp => (
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY,
        ),
        Lang::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
        ),
        Lang::Css => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY,
        ),
        Lang::Html => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY,
        ),
        Lang::Ruby => (
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY,
        ),
        Lang::Java => (
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY,
        ),
        Lang::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        ),
        Lang::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
        ),
        Lang::Lua => (
            tree_sitter_lua::LANGUAGE.into(),
            tree_sitter_lua::HIGHLIGHTS_QUERY,
        ),
        Lang::CSharp => (
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        ),
        Lang::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        ),
        Lang::Php => (
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY,
        ),
        Lang::Scala => (
            tree_sitter_scala::LANGUAGE.into(),
            tree_sitter_scala::HIGHLIGHTS_QUERY,
        ),
        Lang::Elixir => (
            tree_sitter_elixir::LANGUAGE.into(),
            tree_sitter_elixir::HIGHLIGHTS_QUERY,
        ),
        Lang::Haskell => (
            tree_sitter_haskell::LANGUAGE.into(),
            tree_sitter_haskell::HIGHLIGHTS_QUERY,
        ),
        Lang::Ocaml => (
            tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            tree_sitter_ocaml::HIGHLIGHTS_QUERY,
        ),
        Lang::Scss => (
            tree_sitter_scss::language(),
            tree_sitter_scss::HIGHLIGHTS_QUERY,
        ),
        Lang::Nix => (
            tree_sitter_nix::LANGUAGE.into(),
            tree_sitter_nix::HIGHLIGHTS_QUERY,
        ),
        Lang::Swift => (
            tree_sitter_swift::LANGUAGE.into(),
            tree_sitter_swift::HIGHLIGHTS_QUERY,
        ),
        Lang::Make => (
            tree_sitter_make::LANGUAGE.into(),
            tree_sitter_make::HIGHLIGHTS_QUERY,
        ),
        Lang::Xml => (
            tree_sitter_xml::LANGUAGE_XML.into(),
            tree_sitter_xml::XML_HIGHLIGHT_QUERY,
        ),
        Lang::Zig => (
            tree_sitter_zig::LANGUAGE.into(),
            tree_sitter_zig::HIGHLIGHTS_QUERY,
        ),
        Lang::Dart => (
            tree_sitter_dart::LANGUAGE.into(),
            tree_sitter_dart::HIGHLIGHTS_QUERY,
        ),
        Lang::MarkdownInline => (
            tree_sitter_md::INLINE_LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
        ),
        Lang::Regex => (
            tree_sitter_regex::LANGUAGE.into(),
            tree_sitter_regex::HIGHLIGHTS_QUERY,
        ),
        Lang::JsDoc => (
            tree_sitter_jsdoc::LANGUAGE.into(),
            tree_sitter_jsdoc::HIGHLIGHTS_QUERY,
        ),
        Lang::PlainText => return None,
    };
    Some((language, highlights))
}
