use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use globset::{Glob, GlobSet, GlobSetBuilder};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchLine {
    /// Zero-based line number in the file.
    pub number: usize,
    pub text: String,
    /// Byte ranges of the matches within `text`.
    pub hits: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
}
