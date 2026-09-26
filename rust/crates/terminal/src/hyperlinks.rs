//! Finding the link under a grid cell: an OSC 8 hyperlink the program attached, a URL in the text, or
//! something that looks like a file path (optionally with `:line:column`).

use std::ops::Range;
use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::{Boundary, Column, Direction, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::Term;
use regex::Regex;

use crate::GridPoint;

const URL_REGEX: &str = r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^\x{27E8}\x{27E9}`']+"#;

/// Python tracebacks, then the general path shape: no leading delimiter or box-drawing character, colons and
/// parens inside only when not starting a position, no trailing punctuation, and an optional position suffix.
const PATH_REGEXES: [&str; 2] = [
    r#"File "(?<path>[^"]+)", line (?<line>[0-9]+)"#,
    r#"(?x)
    (?<path>
        (
            [^({\[<"'`\ \x{2500}-\x{257F}]
            ([^\ :(]|[:(][^0-9()\ ])*
            [^()}\]>"'`.,;:\ ]
        |
            [^(){}\[\]<>"'`.,;:\ \x{2500}-\x{257F}]
        )
        (:+[0-9]+(:[0-9]+)?|:?\([0-9]+([,:]?[0-9]+)?\))?
    )"#,
];

/// Path matching runs on every hover, so a pathological line gives up rather than stall input. Unoptimized
/// builds run the regexes many times slower, so they get more room.
const PATH_TIMEOUT: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(20)
} else {
    Duration::from_millis(1)
};

const WIDE_SPACERS: Flags = Flags::LEADING_WIDE_CHAR_SPACER.union(Flags::WIDE_CHAR_SPACER);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyperlinkMatch {
    pub text: String,
    pub is_url: bool,
    pub start: GridPoint,
    pub end: GridPoint,
}

impl HyperlinkMatch {
    pub fn contains(&self, point: GridPoint) -> bool {
        (self.start.line, self.start.column) <= (point.line, point.column)
            && (point.line, point.column) <= (self.end.line, self.end.column)
    }
}

pub(crate) struct LinkSearch {
    url: Option<RegexSearch>,
    paths: Vec<Regex>,
}

impl Default for LinkSearch {
    fn default() -> Self {
        Self {
            url: RegexSearch::new(URL_REGEX).ok(),
            paths: PATH_REGEXES
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect(),
        }
    }
}

fn grid_point(point: Point) -> GridPoint {
    GridPoint {
        line: point.line.0,
        column: point.column.0,
    }
}

fn to_match(text: String, is_url: bool, range: Match) -> HyperlinkMatch {
    HyperlinkMatch {
        text,
        is_url,
        start: grid_point(*range.start()),
        end: grid_point(*range.end()),
    }
}

impl LinkSearch {
    pub(crate) fn find<T: EventListener>(
        &mut self,
        term: &Term<T>,
        point: Point,
    ) -> Option<HyperlinkMatch> {
        let grid = term.grid();
        if let Some(link) = grid[point].hyperlink() {
            let same = |at: Point| grid[at].hyperlink().as_ref() == Some(&link);
            let mut start = point;
            loop {
                let previous = start.sub(term, Boundary::Cursor, 1);
                if previous == start || !same(previous) {
                    break;
                }
                start = previous;
            }
            let mut end = point;
            loop {
                let next = end.add(term, Boundary::Cursor, 1);
                if next == end || !same(next) {
                    break;
                }
                end = next;
            }
            return Some(normalize(to_match(
                link.uri().to_owned(),
                true,
                start..=end,
            )));
        }
        let (line_start, line_end) = (term.line_search_left(point), term.line_search_right(point));
        let url = self.url.as_mut().and_then(|regex| {
            RegexIter::new(line_start, line_end, Direction::Right, term, regex)
                .find(|found| found.contains(&point))
                .map(|found| {
                    let text = term.bounds_to_string(*found.start(), *found.end());
                    trim_url_punctuation(text, found, term)
                })
        });
        if let Some((text, range)) = url {
            return Some(normalize(to_match(text, true, range)));
        }
        path_match(term, line_start, line_end, point, &self.paths)
            .map(|(text, range)| to_match(text, false, range))
    }
}

/// `file://` links are paths (so a trailing `:line` still works); the host part of an OSC 8 file URI is
/// skipped and percent escapes decoded.
fn normalize(found: HyperlinkMatch) -> HyperlinkMatch {
    let Some(rest) = found.text.strip_prefix("file://") else {
        return found;
    };
    let path = match rest.find('/') {
        Some(slash) => &rest[slash..],
        None => rest,
    };
    HyperlinkMatch {
        text: percent_decode(path),
        is_url: false,
        ..found
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let decoded = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Trailing `.,:;(` and unbalanced `)` usually belong to the sentence around a URL, not the URL.
fn trim_url_punctuation<T: EventListener>(
    mut url: String,
    found: Match,
    term: &Term<T>,
) -> (String, Match) {
    let opens = url.chars().filter(|&c| c == '(').count();
    let mut closes = url.chars().filter(|&c| c == ')').count();
    let mut trimmed = 0;
    while let Some(last) = url.chars().last() {
        let remove = match last {
            '.' | ',' | ':' | ';' | '(' => true,
            ')' if closes > opens => {
                closes -= 1;
                true
            }
            _ => false,
        };
        if !remove {
            break;
        }
        url.pop();
        trimmed += 1;
    }
    if trimmed == 0 {
        return (url, found);
    }
    let end = found.end().sub(term, Boundary::Grid, trimmed);
    (url, *found.start()..=end)
}

/// Byte offset just past the first `(` that never closes, so `Update(src/main.rs)` yields the path while
/// `file(copy).txt` keeps its parens.
fn first_unbalanced_open_paren(text: &str) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut first = None;
    for (index, c) in text.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    first = Some(index + c.len_utf8());
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth <= 0 {
                    depth = 0;
                    first = None;
                }
            }
            _ => {}
        }
    }
    first.filter(|_| depth > 0)
}

fn path_match<T>(
    term: &Term<T>,
    line_start: Point,
    line_end: Point,
    hovered: Point,
    regexes: &[Regex],
) -> Option<(String, Match)> {
    let started = Instant::now();
    // One char per cell (tabs as a space), so byte offsets map back to cells.
    let mut line = String::new();
    let mut hovered_offset = None;
    let mut previous_len = 0;
    line.push(term.grid()[line_start].c);
    if line_start == hovered {
        hovered_offset = Some(0);
    }
    for cell in term.grid().iter_from(line_start) {
        if cell.point > line_end {
            break;
        }
        if !cell.flags.intersects(WIDE_SPACERS) {
            previous_len = line.len();
            line.push(match cell.c {
                '\t' => ' ',
                c => c,
            });
        }
        if cell.point == hovered {
            hovered_offset = Some(previous_len);
        }
    }
    let line = line.trim_ascii_end();
    let hovered_offset = hovered_offset?;
    if line.len() <= hovered_offset {
        return None;
    }
    let advance = |mut point: Point, text: &str| {
        for _ in text.chars() {
            point = term
                .expand_wide(point, Direction::Right)
                .add(term, Boundary::Grid, 1);
        }
        let flags = term.grid()[point].flags;
        if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            Point::new(point.line + 1, Column(0))
        } else if flags.contains(Flags::WIDE_CHAR_SPACER) {
            Point::new(point.line, point.column - 1)
        } else {
            point
        }
    };
    let found = |path: Range<usize>, link: Range<usize>, position: Option<(u32, Option<u32>)>| {
        let start = advance(line_start, &line[..link.start]);
        let end = advance(start, &line[link]);
        let end = term
            .expand_wide(end, Direction::Left)
            .sub(term, Boundary::Grid, 1);
        let mut text = line[path].to_string();
        if let Some((row, column)) = position {
            text.push_str(&format!(":{row}"));
            if let Some(column) = column {
                text.push_str(&format!(":{column}"));
            }
        }
        (text, start..=end)
    };
    for regex in regexes {
        let mut matched = false;
        for captures in regex.captures_iter(line) {
            matched = true;
            let Some(whole) = captures.get(0) else {
                continue;
            };
            let parse = |name: &str| {
                captures
                    .name(name)
                    .and_then(|value| value.as_str().parse().ok())
            };
            let (mut path, position) = match captures.name("path") {
                Some(path) => (
                    path.range(),
                    parse("line").map(|row| (row, parse("column"))),
                ),
                None => (whole.range(), None),
            };
            let mut link = captures
                .name("link")
                .map_or(whole.range(), |link| link.range());
            if let Some(skip) = first_unbalanced_open_paren(&line[path.clone()]) {
                path.start += skip;
                link.start = link.start.max(path.start);
            }
            if !link.contains(&hovered_offset) {
                continue;
            }
            let result = found(path, link, position);
            if result.1.contains(&hovered) {
                return Some(result);
            }
        }
        if matched || started.elapsed() > PATH_TIMEOUT {
            return None;
        }
    }
    None
}

/// A path with an optional `:row:column` (or `(row,column)`) suffix, split apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathWithPosition {
    pub path: String,
    pub row: Option<u32>,
    pub column: Option<u32>,
}

impl PathWithPosition {
    pub fn parse(text: &str) -> Self {
        let trimmed = text.trim_end_matches(':');
        if let Some(open) = trimmed.rfind('(').filter(|_| trimmed.ends_with(')')) {
            let inside = &trimmed[open + 1..trimmed.len() - 1];
            let mut numbers = inside.split([',', ':']).map(str::parse::<u32>);
            if let Some(Ok(row)) = numbers.next() {
                let column = numbers.next().and_then(Result::ok);
                return Self {
                    path: trimmed[..open].trim_end_matches(':').to_string(),
                    row: Some(row),
                    column,
                };
            }
        }
        let mut parts: Vec<&str> = trimmed.rsplitn(3, ':').collect();
        parts.reverse();
        let number = |part: &str| part.parse::<u32>().ok();
        match parts.as_slice() {
            [path, row, column] if number(row).is_some() && number(column).is_some() => Self {
                path: path.trim_end_matches(':').to_string(),
                row: number(row),
                column: number(column),
            },
            [path, row, last] if number(last).is_some() => Self {
                path: format!("{path}:{row}"),
                row: number(last),
                column: None,
            },
            [path, row] if number(row).is_some() => Self {
                path: path.trim_end_matches(':').to_string(),
                row: number(row),
                column: None,
            },
            _ => Self {
                path: trimmed.to_string(),
                row: None,
                column: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_parse_from_both_suffix_styles() {
        assert_eq!(
            PathWithPosition::parse("src/main.rs:12:5"),
            PathWithPosition {
                path: "src/main.rs".into(),
                row: Some(12),
                column: Some(5)
            }
        );
        assert_eq!(PathWithPosition::parse("src/main.rs:12").row, Some(12));
        assert_eq!(PathWithPosition::parse("a.cs(3,7)").column, Some(7));
        assert_eq!(PathWithPosition::parse("README.md").row, None);
    }

    #[test]
    fn unbalanced_parens_and_file_urls() {
        assert_eq!(first_unbalanced_open_paren("Update(src/a.rs"), Some(7));
        assert_eq!(first_unbalanced_open_paren("file(copy).txt"), None);
        let found = normalize(HyperlinkMatch {
            text: "file://host/tmp/a%20b.txt".into(),
            is_url: true,
            start: GridPoint::default(),
            end: GridPoint::default(),
        });
        assert_eq!((found.text.as_str(), found.is_url), ("/tmp/a b.txt", false));
    }
}
