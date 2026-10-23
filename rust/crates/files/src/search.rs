//! Project search: the query (plain text with ASCII case folding and optional whole word, or regex with a case
//! flag, `\c`/`\C` inline overrides and whole word via `\b`, replacements expanding capture groups), the path
//! filters, and the walk that runs a query over a folder.

use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use fancy_regex::{Regex, RegexBuilder};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

pub enum Query {
    Text {
        query: String,
        case_sensitive: bool,
        whole_word: bool,
        replacement: Option<String>,
    },
    Regex {
        regex: Regex,
        /// Built from escaped literal text, so replacements are literal too.
        escaped: bool,
        replacement: Option<String>,
    },
}

impl Query {
    pub fn new(
        query: &str,
        options: SearchOptions,
        replacement: Option<String>,
    ) -> Result<Self, String> {
        let query = normalize_newlines(query).into_owned();
        if options.regex {
            return build_regex(query, options, false, replacement);
        }
        // ASCII case folding can't handle other scripts, so those go through an escaped regex.
        if !options.case_sensitive && !query.is_ascii() {
            return build_regex(
                fancy_regex::escape(&query).into_owned(),
                options,
                true,
                replacement,
            );
        }
        Ok(Self::Text {
            query,
            case_sensitive: options.case_sensitive,
            whole_word: options.whole_word,
            replacement,
        })
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text { query, .. } => query.is_empty(),
            Self::Regex { regex, .. } => regex.as_str().is_empty(),
        }
    }

    /// Byte ranges of the matches in `text`, in order.
    pub fn find_in(&self, text: &str) -> Vec<Range<usize>> {
        if self.is_empty() {
            return Vec::new();
        }
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        match self {
            Self::Text {
                query,
                case_sensitive,
                whole_word,
                ..
            } => {
                let hits: Vec<Range<usize>> = if *case_sensitive {
                    text.match_indices(query.as_str())
                        .map(|(at, m)| at..at + m.len())
                        .collect()
                } else {
                    let haystack = text.to_ascii_lowercase();
                    let needle = query.to_ascii_lowercase();
                    haystack
                        .match_indices(needle.as_str())
                        .map(|(at, m)| at..at + m.len())
                        .collect()
                };
                hits.into_iter()
                    .filter(|hit| {
                        !*whole_word
                            || !text[..hit.start].chars().last().is_some_and(is_word_char)
                                && !text[hit.end..].chars().next().is_some_and(is_word_char)
                    })
                    .collect()
            }
            Self::Regex { regex, .. } => regex
                .find_iter(text)
                .flatten()
                .map(|m| m.start()..m.end())
                .collect(),
        }
    }

    /// The text replacing `hit` (byte range within `line`, the lines it spans). Regex replacements expand
    /// `$1`-style groups and `\n`, `\t`, `\\` escapes.
    pub fn replacement_for(&self, line: &str, hit: Range<usize>) -> Option<String> {
        match self {
            Self::Text { replacement, .. }
            | Self::Regex {
                replacement,
                escaped: true,
                ..
            } => replacement.clone(),
            Self::Regex {
                regex,
                replacement: Some(replacement),
                escaped: false,
            } => {
                let replacement = unescape_replacement(replacement);
                let captures = regex
                    .captures_from_pos(line, hit.start)
                    .ok()
                    .flatten()
                    .filter(|c| c.get(0).is_some_and(|m| m.range() == hit));
                match captures {
                    Some(captures) => {
                        let mut out = String::new();
                        captures.expand(&replacement, &mut out);
                        Some(out)
                    }
                    None => Some(
                        regex
                            .replace(line.get(hit)?, replacement.as_str())
                            .into_owned(),
                    ),
                }
            }
            Self::Regex {
                replacement: None, ..
            } => None,
        }
    }
}

fn unescape_replacement(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\\') => {
                    out.push('\\');
                    chars.next();
                }
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `\c` in a pattern forces case-insensitive and `\C` case-sensitive matching; the flags are removed.
fn case_sensitive_from_pattern(pattern: &str) -> Option<(bool, String)> {
    if !(pattern.contains("\\c") || pattern.contains("\\C")) {
        return None;
    }
    let mut escaped = false;
    let mut out = String::new();
    let mut case_sensitive = None;
    for c in pattern.chars() {
        if escaped {
            match c {
                'c' => case_sensitive = Some(false),
                'C' => case_sensitive = Some(true),
                _ => {
                    out.push('\\');
                    out.push(c);
                }
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    case_sensitive.map(|flag| (flag, out))
}

fn build_regex(
    mut pattern: String,
    options: SearchOptions,
    escaped: bool,
    replacement: Option<String>,
) -> Result<Query, String> {
    let mut case_sensitive = options.case_sensitive;
    if let Some((flag, stripped)) = case_sensitive_from_pattern(&pattern) {
        case_sensitive = flag;
        pattern = stripped;
    }
    if options.whole_word {
        // Only anchor at edges that are word chars; `\b` next to punctuation would never match.
        let is_word = |s: &str| {
            s.chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        };
        let mut word = String::new();
        if pattern.get(0..1).is_some_and(is_word) {
            word.push_str("\\b");
        }
        word.push_str(&pattern);
        if pattern
            .get(pattern.len().saturating_sub(1)..)
            .is_some_and(is_word)
        {
            word.push_str("\\b");
        }
        pattern = word;
    }
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .multi_line(true)
        .crlf(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Query::Regex {
        regex,
        escaped,
        replacement,
    })
}

fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

pub const MAX_FILES: usize = 5_000;
pub const MAX_MATCHES: usize = 10_000;
const CONTEXT_LINES: usize = 2;
const BINARY_SNIFF: usize = 8 * 1024;

pub struct PathMatcher {
    sources: Vec<String>,
    globs: GlobSet,
}

/// Splits on commas outside `{...}`, so `*.{rs,toml}` stays one pattern.
fn split_patterns(text: &str) -> Vec<&str> {
    let mut patterns = Vec::new();
    let (mut start, mut depth, mut escaped) = (0, 0usize, false);
    for (index, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                patterns.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    patterns.push(&text[start..]);
    patterns
}

impl PathMatcher {
    pub fn new(text: &str) -> Result<PathMatcher, String> {
        let sources: Vec<String> = split_patterns(text)
            .into_iter()
            .map(|pattern| pattern.trim().trim_matches('/').to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        let mut builder = GlobSetBuilder::new();
        for source in &sources {
            builder.add(Glob::new(source).map_err(|error| error.to_string())?);
        }
        let globs = builder.build().map_err(|error| error.to_string())?;
        Ok(PathMatcher { sources, globs })
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// A path matches a pattern it starts or ends with (whole components), or a glob over it or its folder form.
    pub fn is_match(&self, path: &str) -> bool {
        let components: Vec<&str> = path.split('/').collect();
        self.sources.iter().any(|source| {
            let wanted: Vec<&str> = source.split('/').collect();
            components.starts_with(&wanted) || components.ends_with(&wanted)
        }) || self.globs.is_match(path)
            || self.globs.is_match(format!("{path}/"))
    }
}

pub struct SearchScope {
    pub include: PathMatcher,
    pub exclude: PathMatcher,
    pub include_ignored: bool,
}

/// What the project search box asks: the query as typed, its toggles, and the path filters as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchRequest {
    pub query: String,
    pub options: SearchOptions,
    pub include: String,
    pub exclude: String,
    pub include_ignored: bool,
}

impl SearchRequest {
    pub fn query(&self) -> Result<Query, String> {
        Query::new(&self.query, self.options, None)
    }

    pub fn scope(&self) -> Result<SearchScope, String> {
        match (
            PathMatcher::new(&self.include),
            PathMatcher::new(&self.exclude),
        ) {
            (Ok(include), Ok(exclude)) => Ok(SearchScope {
                include,
                exclude,
                include_ignored: self.include_ignored,
            }),
            (Err(error), _) | (_, Err(error)) => Err(format!("Invalid path pattern: {error}")),
        }
    }
}

/// Runs `request` over `root` as [`search`] does; an error says why the query or a filter cannot be used.
pub fn run(
    root: &Path,
    request: &SearchRequest,
    cancel: &AtomicBool,
    on_file: &(dyn Fn(FileResult) + Sync),
) -> Result<bool, String> {
    let query = request.query()?;
    let scope = request.scope()?;
    let find = |text: &str| query.find_in(text);
    Ok(search(root, &scope, &find, cancel, on_file))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchLine {
    /// Zero-based line number in the file.
    pub number: usize,
    pub text: String,
    /// Byte ranges of the matches within `text`.
    pub hits: Vec<Range<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileResult {
    pub path: String,
    pub excerpts: Vec<Vec<SearchLine>>,
    pub match_count: usize,
}

pub fn excerpts_for(text: &str, hits: &[Range<usize>]) -> Vec<Vec<SearchLine>> {
    let mut line_starts = vec![0];
    line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    let line_of = |offset: usize| line_starts.partition_point(|start| *start <= offset) - 1;
    let line_text = |line: usize| {
        let start = line_starts[line];
        let end = line_starts
            .get(line + 1)
            .map_or(text.len(), |next| next - 1);
        text[start..end].trim_end_matches('\r').to_string()
    };
    let mut hits_by_line: std::collections::BTreeMap<usize, Vec<Range<usize>>> = Default::default();
    for hit in hits {
        let line = line_of(hit.start);
        let start = line_starts[line];
        let end = line_text(line).len();
        let local = hit.start - start..(hit.end - start).min(end).max(hit.start - start);
        hits_by_line.entry(line).or_default().push(local);
    }
    let last_line = line_starts.len() - 1;
    let mut excerpts: Vec<Vec<SearchLine>> = Vec::new();
    let mut current: Option<Range<usize>> = None;
    let mut spans: Vec<Range<usize>> = Vec::new();
    for &line in hits_by_line.keys() {
        let span = line.saturating_sub(CONTEXT_LINES)..(line + CONTEXT_LINES).min(last_line) + 1;
        current = match current {
            Some(open) if span.start <= open.end => Some(open.start..span.end.max(open.end)),
            Some(open) => {
                spans.push(open);
                Some(span)
            }
            None => Some(span),
        };
    }
    spans.extend(current);
    for span in spans {
        excerpts.push(
            span.map(|line| SearchLine {
                number: line,
                text: line_text(line),
                hits: hits_by_line.get(&line).cloned().unwrap_or_default(),
            })
            .collect(),
        );
    }
    excerpts
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0)
}

/// Searches every file under `root` in scope with `find`, handing each file with matches to `on_file` as it is
/// found (in no particular order). Stops at `MAX_FILES` files or `MAX_MATCHES` matches, or when `cancel` is set;
/// returns whether a limit was reached.
pub fn search(
    root: &Path,
    scope: &SearchScope,
    find: &(dyn Fn(&str) -> Vec<Range<usize>> + Sync),
    cancel: &AtomicBool,
    on_file: &(dyn Fn(FileResult) + Sync),
) -> bool {
    let files = AtomicUsize::new(0);
    let matches = AtomicUsize::new(0);
    let limited = AtomicBool::new(false);
    let paths = Mutex::new(Vec::new());
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(!scope.include_ignored)
        .git_exclude(!scope.include_ignored)
        .git_global(!scope.include_ignored)
        .ignore(!scope.include_ignored)
        .parents(true)
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walker.build().filter_map(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if (!scope.include.is_empty() && !scope.include.is_match(&relative))
            || (!scope.exclude.is_empty() && scope.exclude.is_match(&relative))
        {
            continue;
        }
        if let Ok(mut paths) = paths.lock() {
            paths.push(relative);
        }
    }
    let paths = paths.into_inner().unwrap_or_default();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(2, |count| count.get().saturating_sub(1).max(1));
    std::thread::scope(|threads| {
        for _ in 0..workers {
            threads.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) || limited.load(Ordering::Relaxed) {
                    return;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(relative) = paths.get(index) else {
                    return;
                };
                let Ok(bytes) = std::fs::read(root.join(relative)) else {
                    continue;
                };
                if looks_binary(&bytes) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes);
                let hits = find(&text);
                if hits.is_empty() {
                    continue;
                }
                let total = matches.fetch_add(hits.len(), Ordering::Relaxed) + hits.len();
                let count = files.fetch_add(1, Ordering::Relaxed) + 1;
                if count > MAX_FILES || total > MAX_MATCHES {
                    limited.store(true, Ordering::Relaxed);
                }
                on_file(FileResult {
                    path: relative.clone(),
                    excerpts: excerpts_for(&text, &hits),
                    match_count: hits.len(),
                });
            });
        }
    });
    limited.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str, options: SearchOptions) -> Query {
        Query::new(text, options, None).unwrap()
    }

    #[test]
    fn text_search_folds_ascii_case_and_respects_whole_words() {
        let q = query("foo", SearchOptions::default());
        assert_eq!(q.find_in("Foo food foo"), vec![0..3, 4..7, 9..12]);
        let q = query(
            "foo",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
        );
        assert_eq!(q.find_in("Foo food foo"), vec![0..3, 9..12]);
    }

    #[test]
    fn regex_replacement_expands_groups() {
        let q = Query::new(
            r"(\w+)=(\d)",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
            Some("$2:$1".into()),
        )
        .unwrap();
        let line = "a=1 b=2";
        let hits = q.find_in(line);
        assert_eq!(hits, vec![0..3, 4..7]);
        assert_eq!(q.replacement_for(line, 4..7).as_deref(), Some("2:b"));
    }

    #[test]
    fn inline_case_flag_overrides_the_option() {
        let q = query(
            r"\CFoo",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        assert_eq!(q.find_in("foo Foo"), vec![4..7]);
    }

    #[test]
    fn non_ascii_case_insensitive_text_goes_through_regex() {
        let q = query("\u{c9}t\u{e9}", SearchOptions::default());
        assert_eq!(q.find_in("\u{e9}t\u{e9}").len(), 1);
    }

    #[test]
    fn patterns_split_outside_braces_and_match_folders() -> Result<(), String> {
        assert_eq!(
            split_patterns("src/**/*.rs, *.{toml,lock}"),
            ["src/**/*.rs", " *.{toml,lock}"]
        );
        let matcher = PathMatcher::new("vendor, *.lock")?;
        assert!(matcher.is_match("vendor/wgpu/src/lib.rs"));
        assert!(matcher.is_match("Cargo.lock"));
        assert!(matcher.is_match("crates/a/Cargo.lock"));
        assert!(!matcher.is_match("src/vendors.rs"));
        let rust = PathMatcher::new("src/**/*.rs")?;
        assert!(rust.is_match("src/a/b.rs"));
        assert!(!rust.is_match("docs/b.rs"));
        assert!(PathMatcher::new("")?.is_empty());
        assert!(PathMatcher::new("[").is_err());
        Ok(())
    }

    #[test]
    fn excerpts_merge_nearby_matches_and_keep_context() {
        let text = (1..=12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let at = |needle: &str| {
            let start = text.find(needle).unwrap_or(0);
            start..start + needle.len()
        };
        let excerpts = excerpts_for(&text, &[at("line 2"), at("line 4"), at("line 11")]);
        let numbers: Vec<Vec<usize>> = excerpts
            .iter()
            .map(|excerpt| excerpt.iter().map(|line| line.number).collect())
            .collect();
        assert_eq!(numbers, [vec![0, 1, 2, 3, 4, 5], vec![8, 9, 10, 11]]);
        assert_eq!(excerpts[0][1].hits, vec![Range { start: 0, end: 6 }]);
        assert!(excerpts[0][0].hits.is_empty());
    }

    #[test]
    fn searches_text_files_in_scope_and_skips_binaries_and_ignored() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join(".git"))?;
        std::fs::write(root.join(".gitignore"), "dist/\n")?;
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("dist"))?;
        std::fs::write(root.join("src/a.rs"), "fn needle() {}\nlet x = needle;\n")?;
        std::fs::write(root.join("src/b.txt"), "no match here\n")?;
        std::fs::write(root.join("dist/c.rs"), "needle\n")?;
        std::fs::write(root.join("blob.bin"), b"needle\0\x01")?;
        let find = |text: &str| -> Vec<Range<usize>> {
            text.match_indices("needle")
                .map(|(at, m)| at..at + m.len())
                .collect()
        };
        let found = Mutex::new(Vec::new());
        let scope = SearchScope {
            include: PathMatcher::new("").map_err(std::io::Error::other)?,
            exclude: PathMatcher::new("").map_err(std::io::Error::other)?,
            include_ignored: false,
        };
        let limited = search(root, &scope, &find, &AtomicBool::new(false), &|file| {
            if let Ok(mut found) = found.lock() {
                found.push(file);
            }
        });
        assert!(!limited);
        let found = found.into_inner().unwrap_or_default();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "src/a.rs");
        assert_eq!(found[0].match_count, 2);
        Ok(())
    }

    type OnFile<'a> = &'a (dyn Fn(FileResult) + Sync);

    #[test]
    fn a_request_finds_what_its_query_and_scope_find() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let root = temp.path();
        let write = |path: &str, text: &str| -> Result<(), String> {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(full, text).map_err(|error| error.to_string())
        };
        write(".gitignore", "dist/\n")?;
        write("src/a.rs", "fn Alpha() {}\nlet alpha_beta = alpha;\n")?;
        write("docs/b.md", "ALPHA \u{e9}t\u{e9}\n")?;
        write("dist/c.js", "alpha\n")?;
        let collect = |run: &dyn Fn(OnFile)| {
            let found = Mutex::new(Vec::new());
            run(&|file| {
                if let Ok(mut found) = found.lock() {
                    found.push(file);
                }
            });
            let mut found = found.into_inner().unwrap_or_default();
            found.sort_by(|a, b| a.path.cmp(&b.path));
            found
        };
        let requests = [
            ("alpha", SearchOptions::default(), "", "", false),
            (
                "alpha",
                SearchOptions {
                    whole_word: true,
                    ..Default::default()
                },
                "",
                "",
                false,
            ),
            (
                r"\bal\w+",
                SearchOptions {
                    regex: true,
                    ..Default::default()
                },
                "src",
                "",
                false,
            ),
            ("\u{c9}T\u{c9}", SearchOptions::default(), "", "*.rs", true),
        ];
        for (text, options, include, exclude, include_ignored) in requests {
            let request = SearchRequest {
                query: text.into(),
                options,
                include: include.into(),
                exclude: exclude.into(),
                include_ignored,
            };
            let query = Query::new(text, options, None)?;
            let scope = SearchScope {
                include: PathMatcher::new(include)?,
                exclude: PathMatcher::new(exclude)?,
                include_ignored,
            };
            let find = |text: &str| query.find_in(text);
            let direct = collect(&|on_file| {
                search(root, &scope, &find, &AtomicBool::new(false), on_file);
            });
            let requested = collect(&|on_file| {
                if let Err(error) = run(root, &request, &AtomicBool::new(false), on_file) {
                    panic!("{error}");
                }
            });
            assert!(!direct.is_empty(), "{text}");
            assert_eq!(requested, direct, "{text}");
        }
        let bad = SearchRequest {
            query: "(".into(),
            options: SearchOptions {
                regex: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(run(root, &bad, &AtomicBool::new(false), &|_| {}).is_err());
        let bad_path = SearchRequest {
            query: "alpha".into(),
            include: "[".into(),
            ..Default::default()
        };
        assert!(run(root, &bad_path, &AtomicBool::new(false), &|_| {})
            .is_err_and(|error| error.starts_with("Invalid path pattern")));
        let wire =
            serde_json::to_string(&request_round_trip()).map_err(|error| error.to_string())?;
        let back: SearchRequest = serde_json::from_str(&wire).map_err(|error| error.to_string())?;
        assert_eq!(back, request_round_trip());
        Ok(())
    }

    fn request_round_trip() -> SearchRequest {
        SearchRequest {
            query: "a".into(),
            options: SearchOptions {
                case_sensitive: true,
                whole_word: false,
                regex: true,
            },
            include: "src, *.{rs,toml}".into(),
            exclude: "vendor".into(),
            include_ignored: true,
        }
    }
}
