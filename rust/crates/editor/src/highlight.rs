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
        let name = path.rsplit('/').next().unwrap_or(path);
        let extension = name.rsplit('.').next().unwrap_or(name);
        let candidates = [extension, name, path];
        let mut best: Option<(usize, Lang)> = None;
        for (lang, suffixes) in PATH_SUFFIXES {
            for suffix in *suffixes {
                let dotted = format!(".{suffix}");
                let matched = candidates
                    .iter()
                    .find(|candidate| **candidate == *suffix || candidate.ends_with(&dotted))
                    .map(|candidate| candidate.len());
                if let Some(len) = matched {
                    if best.is_none_or(|(best_len, _)| len > best_len) {
                        best = Some((len, *lang));
                    }
                }
            }
        }
        if let Some((_, lang)) = best {
            return lang;
        }
        let first_line = first_line.unwrap_or("");
        FIRST_LINE_PATTERNS
            .iter()
            .find(|(_, pattern)| {
                regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(first_line))
            })
            .map_or(Lang::PlainText, |(lang, _)| *lang)
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
    (Lang::Lua, &["lua"]),
    (Lang::CSharp, &["cs"]),
    (
        Lang::Markdown,
        &["md", "mdx", "mdwn", "mdc", "markdown", "MD"],
    ),
    (Lang::Php, &["php"]),
    (Lang::Scala, &["scala", "sc", "sbt"]),
    (Lang::Elixir, &["ex", "exs"]),
    (Lang::Haskell, &["hs"]),
    (Lang::Ocaml, &["ml", "mli"]),
    (Lang::Scss, &["scss"]),
    (Lang::Nix, &["nix"]),
    (Lang::Swift, &["swift"]),
    (Lang::Make, &["mk", "Makefile", "makefile", "GNUmakefile"]),
    (Lang::Xml, &["xml", "svg", "xaml", "plist"]),
    (Lang::Zig, &["zig"]),
    (Lang::Dart, &["dart"]),
    (Lang::Sql, &["sql"]),
    (Lang::Kotlin, &["kt", "kts"]),
    (Lang::Svelte, &["svelte"]),
    (
        Lang::Dockerfile,
        &["Dockerfile", "Containerfile", "dockerfile", "containerfile"],
    ),
    (Lang::GraphQl, &["graphql", "gql", "graphqls"]),
    (Lang::Hcl, &["hcl", "tf", "tfvars"]),
    (Lang::Proto, &["proto"]),
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
    (
        Lang::Erlang,
        &["erl", "hrl", "app.src", "escript", "rebar.config"],
    ),
    (Lang::Gleam, &["gleam"]),
    (Lang::R, &["r", "R"]),
    (Lang::Elm, &["elm"]),
    (Lang::Prisma, &["prisma"]),
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
        Lang::Svelte => tree_sitter_svelte_ng::INJECTIONS_QUERY,
        Lang::GitCommit => tree_sitter_gitcommit::INJECTIONS_QUERY,
        Lang::Elm => tree_sitter_elm::INJECTIONS_QUERY,
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
        Lang::Sql => (
            tree_sitter_sequel::LANGUAGE.into(),
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
        ),
        Lang::Kotlin => (
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            include_str!("../queries/kotlin/highlights.scm"),
        ),
        Lang::Svelte => (
            tree_sitter_svelte_ng::LANGUAGE.into(),
            tree_sitter_svelte_ng::HIGHLIGHTS_QUERY,
        ),
        Lang::Dockerfile => (
            tree_sitter_containerfile::LANGUAGE.into(),
            tree_sitter_containerfile::HIGHLIGHTS_QUERY,
        ),
        Lang::GraphQl => (
            tree_sitter_graphql::LANGUAGE.into(),
            include_str!("../queries/graphql/highlights.scm"),
        ),
        Lang::Hcl => (
            tree_sitter_hcl::LANGUAGE.into(),
            include_str!("../queries/hcl/highlights.scm"),
        ),
        Lang::Proto => (
            tree_sitter_proto::LANGUAGE.into(),
            include_str!("../queries/proto/highlights.scm"),
        ),
        Lang::Diff => (
            tree_sitter_diff::LANGUAGE.into(),
            tree_sitter_diff::HIGHLIGHTS_QUERY,
        ),
        Lang::GitCommit => (
            tree_sitter_gitcommit::LANGUAGE.into(),
            tree_sitter_gitcommit::HIGHLIGHTS_QUERY,
        ),
        Lang::Ini => (
            tree_sitter_ini::LANGUAGE.into(),
            tree_sitter_ini::HIGHLIGHTS_QUERY,
        ),
        Lang::Erlang => (
            tree_sitter_erlang::LANGUAGE.into(),
            tree_sitter_erlang::HIGHLIGHTS_QUERY,
        ),
        Lang::Gleam => (
            tree_sitter_gleam::LANGUAGE.into(),
            tree_sitter_gleam::HIGHLIGHT_QUERY,
        ),
        Lang::R => (
            tree_sitter_r::LANGUAGE.into(),
            tree_sitter_r::HIGHLIGHTS_QUERY,
        ),
        Lang::Elm => (
            tree_sitter_elm::LANGUAGE.into(),
            tree_sitter_elm::HIGHLIGHTS_QUERY,
        ),
        Lang::Prisma => (
            tree_sitter_prisma_io::LANGUAGE.into(),
            include_str!("../queries/prisma/highlights.scm"),
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

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Lang; 50] = [
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
        Lang::MarkdownInline,
        Lang::Regex,
        Lang::JsDoc,
        Lang::PlainText,
    ];

    #[test]
    fn every_grammar_highlight_query_compiles() {
        let failing: Vec<String> = ALL
            .iter()
            .filter_map(|lang| {
                let (language, source) = grammar(*lang)?;
                tree_sitter::Query::new(&language, source)
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
        assert_eq!(Lang::detect("infra/main.tf", None), Lang::Hcl);
        assert_eq!(Lang::detect(".git/COMMIT_EDITMSG", None), Lang::GitCommit);
        assert_eq!(Lang::detect("include/a.h", None), Lang::Cpp);
        assert_eq!(Lang::detect("Cargo.lock", None), Lang::Toml);
        assert_eq!(Lang::detect("tsconfig.json", None), Lang::Json);
        assert_eq!(Lang::detect(".zshrc", None), Lang::Bash);
        assert_eq!(Lang::detect("schema.graphql", None), Lang::GraphQl);
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
}
