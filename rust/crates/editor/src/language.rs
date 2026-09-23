//! Per-language editing config: bracket pairs (auto-close, surround, newline splitting), the chars an
//! auto-closed bracket may be typed before, and comment markers. Languages without their own entry share a
//! generic C-like config.

use crate::highlight::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub start: &'static str,
    pub end: &'static str,
    /// Insert `end` right after typing `start`.
    pub close: bool,
    /// Typing `start` over a selection wraps it.
    pub surround: bool,
    /// Enter between the pair puts the cursor on its own line.
    pub newline: bool,
    pub not_in_string: bool,
    pub not_in_comment: bool,
}

impl BracketPair {
    pub fn enabled_in(&self, scope: Scope) -> bool {
        !(self.not_in_string && scope.in_string || self.not_in_comment && scope.in_comment)
    }
}

/// The syntax context at a position, which can disable bracket pairs (no quote auto-close inside a string).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub in_string: bool,
    pub in_comment: bool,
}

pub struct LanguageConfig {
    pub brackets: &'static [BracketPair],
    /// Besides whitespace, the chars an opening bracket auto-closes in front of.
    pub autoclose_before: &'static str,
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    pub indent: IndentRules,
}

/// Line-level indentation hints layered on the syntax-tree indent query.
#[derive(Clone, Copy, Debug)]
pub struct IndentRules {
    /// A matching line indents the next one.
    pub increase: Option<&'static str>,
    /// A matching line outdents itself from the line above.
    pub decrease: Option<&'static str>,
    /// A matching line lines up with the nearest earlier block start (an `@start.<name>` capture) of one of
    /// the listed names that is not indented past it.
    pub decrease_after: &'static [(&'static str, &'static [&'static str])],
    /// Indent relative to the last non-blank line rather than the line right above.
    pub using_last_non_empty_line: bool,
}

const NO_INDENT_RULES: IndentRules = IndentRules {
    increase: None,
    decrease: None,
    decrease_after: &[],
    using_last_non_empty_line: true,
};

const PYTHON_INDENT: IndentRules = IndentRules {
    increase: Some(r"^\s*[^\s#].*:\s*(#.*)?$"),
    decrease: None,
    decrease_after: &[
        (r"^\s*elif\b.*:", &["if", "elif"]),
        (
            r"^\s*else\b.*:",
            &["if", "elif", "for", "while", "try", "except"],
        ),
        (r"^\s*except\b.*:", &["try", "except"]),
        (r"^\s*finally\b.*:", &["try", "except", "else"]),
        (r"^\s*case\b.*:", &["case"]),
    ],
    using_last_non_empty_line: false,
};

const C_INDENT: IndentRules = IndentRules {
    increase: None,
    decrease: None,
    decrease_after: &[
        (r"^\s*\{", &["if", "else", "for", "while", "do", "switch"]),
        (r"^\s*else\b", &["if"]),
    ],
    using_last_non_empty_line: true,
};

const BASH_INDENT: IndentRules = IndentRules {
    increase: Some(r"(^|;|\s)(then|do|else|in)\s*$"),
    decrease: Some(r"^\s*(fi|done|esac|else|elif)\b"),
    decrease_after: &[],
    using_last_non_empty_line: false,
};

const YAML_INDENT: IndentRules = IndentRules {
    increase: Some(r"^[^#]*:\s*[|>]?[-+]?\s*$"),
    decrease: None,
    decrease_after: &[],
    using_last_non_empty_line: false,
};

const RUBY_INDENT: IndentRules = IndentRules {
    increase: Some(
        r"^\s*(def|class|module|if|unless|else|elsif|while|until|for|case|when|begin|rescue|ensure)\b|\bdo(\s*\|[^|]*\|)?\s*$",
    ),
    decrease: Some(r"^\s*(end|else|elsif|when|rescue|ensure)\b"),
    decrease_after: &[],
    using_last_non_empty_line: true,
};

const LUA_INDENT: IndentRules = IndentRules {
    increase: Some(
        r"^\s*(local\s+)?function\b|\bfunction\s*\([^)]*\)\s*$|\b(then|do|else|repeat)\s*$",
    ),
    decrease: Some(r"^\s*(end|else|elseif|until)\b"),
    decrease_after: &[],
    using_last_non_empty_line: true,
};

const ELIXIR_INDENT: IndentRules = IndentRules {
    increase: Some(r"\bdo\s*$|->\s*$"),
    decrease: Some(r"^\s*(end|else|rescue|catch|after)\b"),
    decrease_after: &[],
    using_last_non_empty_line: true,
};

impl LanguageConfig {
    pub fn should_autoclose_before(&self, c: char) -> bool {
        c.is_whitespace() || self.autoclose_before.contains(c)
    }
}

const fn pair(
    start: &'static str,
    end: &'static str,
    close: bool,
    newline: bool,
    not_in_string: bool,
    not_in_comment: bool,
) -> BracketPair {
    BracketPair {
        start,
        end,
        close,
        surround: true,
        newline,
        not_in_string,
        not_in_comment,
    }
}

const BLOCKS: [BracketPair; 3] = [
    pair("{", "}", true, true, false, false),
    pair("[", "]", true, true, false, false),
    pair("(", ")", true, true, false, false),
];

const RUST: &[BracketPair] = &[
    BLOCKS[0],
    pair("r#\"", "\"#", true, true, true, true),
    pair("r##\"", "\"##", true, true, true, true),
    pair("r###\"", "\"###", true, true, true, true),
    BLOCKS[1],
    BLOCKS[2],
    pair("<", ">", false, true, true, true),
    pair("\"", "\"", true, false, true, false),
    pair("`", "`", false, false, true, false),
    pair("/*", " */", true, false, true, true),
];

const JAVASCRIPT: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("<", ">", false, true, true, true),
    pair("\"", "\"", true, false, true, true),
    pair("'", "'", true, false, true, true),
    pair("`", "`", true, false, true, true),
    pair("/*", " */", true, false, true, true),
];

const TYPESCRIPT: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("<", ">", false, true, true, true),
    pair("\"", "\"", true, false, true, false),
    pair("'", "'", true, false, true, true),
    pair("`", "`", true, false, true, false),
    pair("/*", " */", true, false, true, true),
];

const GO: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("\"", "\"", true, false, true, true),
    pair("'", "'", true, false, true, true),
    pair("`", "`", true, false, true, true),
    pair("/*", " */", true, false, true, true),
];

const C: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("\"", "\"", true, false, true, false),
    pair("`", "`", false, false, true, false),
    pair("'", "'", true, false, true, true),
    pair("/*", " */", true, false, true, true),
];

const PYTHON: &[BracketPair] = &[
    pair("f\"", "\"", true, false, true, true),
    pair("f'", "'", true, false, true, true),
    pair("b\"", "\"", true, false, true, true),
    pair("b'", "'", true, false, true, true),
    pair("u\"", "\"", true, false, true, true),
    pair("u'", "'", true, false, true, true),
    pair("r\"", "\"", true, false, true, true),
    pair("r'", "'", true, false, true, true),
    pair("rb\"", "\"", true, false, true, true),
    pair("rb'", "'", true, false, true, true),
    pair("t\"", "\"", true, false, true, true),
    pair("t'", "'", true, false, true, true),
    pair("\"\"\"", "\"\"\"", true, false, true, false),
    pair("'''", "'''", true, false, true, false),
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("\"", "\"", true, false, true, false),
    pair("'", "'", true, false, true, false),
];

const JSON: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    pair("(", ")", true, false, false, false),
    pair("\"", "\"", true, false, true, false),
];

const YAML: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    pair("\"", "\"", true, false, true, false),
    pair("'", "'", true, false, true, false),
];

const CSS: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("\"", "\"", true, false, true, true),
    pair("'", "'", true, false, true, true),
];

const BASH: &[BracketPair] = &[
    pair("[", "]", true, false, false, false),
    BLOCKS[2],
    BLOCKS[0],
    pair("\"", "\"", true, false, true, true),
    pair("'", "'", true, false, true, true),
];

const MARKDOWN: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("<", ">", true, true, false, false),
    pair("\"", "\"", false, false, false, false),
    pair("'", "'", false, false, false, false),
    pair("`", "`", false, false, false, false),
    pair("*", "*", false, false, false, false),
    pair("~", "~", false, false, false, false),
];

const PLAIN_TEXT: &[BracketPair] = &[
    pair("(", ")", true, false, false, false),
    pair("[", "]", true, false, false, false),
    pair("{", "}", true, false, false, false),
    pair("\"", "\"", true, false, false, false),
    pair("'", "'", true, false, false, false),
];

const GENERIC: &[BracketPair] = &[
    BLOCKS[0],
    BLOCKS[1],
    BLOCKS[2],
    pair("\"", "\"", true, false, true, true),
    pair("'", "'", true, false, true, true),
];

const CODE_AUTOCLOSE_BEFORE: &str = ";:.,=}])>";
const SLASH_COMMENTS: &[&str] = &["// "];
const HASH_COMMENTS: &[&str] = &["# "];
const DASH_COMMENTS: &[&str] = &["-- "];
const SLASH_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));

pub fn config(lang: Lang) -> LanguageConfig {
    let code = |brackets, line_comments, block_comment| LanguageConfig {
        brackets,
        autoclose_before: CODE_AUTOCLOSE_BEFORE,
        line_comments,
        block_comment,
        indent: NO_INDENT_RULES,
    };
    let with_indent = |config: LanguageConfig, indent| LanguageConfig { indent, ..config };
    match lang {
        Lang::Rust => code(RUST, &["// ", "/// ", "//! "], SLASH_BLOCK),
        Lang::JavaScript => code(JAVASCRIPT, SLASH_COMMENTS, SLASH_BLOCK),
        Lang::TypeScript | Lang::Tsx => code(TYPESCRIPT, SLASH_COMMENTS, SLASH_BLOCK),
        Lang::Go => code(GO, SLASH_COMMENTS, SLASH_BLOCK),
        Lang::C => with_indent(code(C, SLASH_COMMENTS, SLASH_BLOCK), C_INDENT),
        Lang::Cpp => with_indent(code(C, &["// ", "/// ", "//! "], SLASH_BLOCK), C_INDENT),
        Lang::Python => with_indent(
            code(PYTHON, HASH_COMMENTS, Some(("\"\"\"", "\"\"\""))),
            PYTHON_INDENT,
        ),
        Lang::Css | Lang::Scss => code(CSS, &[], SLASH_BLOCK),
        Lang::Json => LanguageConfig {
            brackets: JSON,
            autoclose_before: ",]}",
            line_comments: SLASH_COMMENTS,
            block_comment: None,
            indent: NO_INDENT_RULES,
        },
        Lang::Yaml => LanguageConfig {
            brackets: YAML,
            autoclose_before: ",]}",
            line_comments: HASH_COMMENTS,
            block_comment: None,
            indent: YAML_INDENT,
        },
        Lang::Bash => LanguageConfig {
            brackets: BASH,
            autoclose_before: "}])",
            line_comments: HASH_COMMENTS,
            block_comment: None,
            indent: BASH_INDENT,
        },
        Lang::Markdown => code(MARKDOWN, &[], Some(("<!--", "-->"))),
        Lang::PlainText => LanguageConfig {
            brackets: PLAIN_TEXT,
            autoclose_before: ")]}",
            line_comments: &[],
            block_comment: None,
            indent: NO_INDENT_RULES,
        },
        Lang::Ruby => with_indent(code(GENERIC, HASH_COMMENTS, None), RUBY_INDENT),
        Lang::Elixir => with_indent(code(GENERIC, HASH_COMMENTS, None), ELIXIR_INDENT),
        Lang::Toml | Lang::Nix | Lang::Make => code(GENERIC, HASH_COMMENTS, None),
        Lang::Lua => with_indent(code(GENERIC, DASH_COMMENTS, None), LUA_INDENT),
        Lang::Haskell => code(GENERIC, DASH_COMMENTS, None),
        Lang::Html | Lang::Xml => code(GENERIC, &[], Some(("<!--", "-->"))),
        Lang::Ocaml => code(GENERIC, &[], Some(("(*", "*)"))),
        Lang::Java | Lang::CSharp | Lang::Php | Lang::Scala | Lang::Swift | Lang::Dart => {
            code(GENERIC, SLASH_COMMENTS, SLASH_BLOCK)
        }
        Lang::Zig => code(GENERIC, SLASH_COMMENTS, None),
    }
}
