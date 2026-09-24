//! Fuzzy matching of a query against short names, shared by the pickers: case-insensitive, but an upper-case
//! query char still ranks exact-case matches higher, and shorter names win ties.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const LENGTH_PENALTY: f64 = 0.01;
const SMART_CASE_PENALTY_PER_MISMATCH: f64 = 0.9;

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub candidate: usize,
    /// Char indices of the matched query chars in the name.
    pub positions: Vec<usize>,
    pub score: f64,
}

/// Fuzzy-match `query` against `names` (case-insensitive; an upper-case query char still ranks exact-case
/// matches higher), best first, with shorter names winning ties.
pub fn fuzzy_match(names: &[&str], query: &str) -> Vec<Match> {
    if query.chars().all(char::is_whitespace) {
        return (0..names.len())
            .map(|candidate| Match {
                candidate,
                positions: Vec::new(),
                score: 0.0,
            })
            .collect();
    }
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let pattern = Pattern::new(
        &normalized,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let query_chars: Option<Vec<char>> = query
        .chars()
        .any(char::is_uppercase)
        .then(|| query.chars().filter(|c| !c.is_whitespace()).collect());
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    let mut indices = Vec::new();
    let mut matches = Vec::new();
    for (candidate, name) in names.iter().enumerate() {
        indices.clear();
        let haystack = Utf32Str::new(name, &mut buffer);
        let Some(score) = pattern.indices(haystack, &mut matcher, &mut indices) else {
            continue;
        };
        let mismatches = case_mismatches(query_chars.as_deref(), &indices, name);
        indices.sort_unstable();
        indices.dedup();
        let score = score as f64 * SMART_CASE_PENALTY_PER_MISMATCH.powi(mismatches)
            - name.len() as f64 * LENGTH_PENALTY;
        matches.push(Match {
            candidate,
            positions: indices.iter().map(|&i| i as usize).collect(),
            score,
        });
    }
    matches.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.candidate.cmp(&a.candidate))
    });
    matches
}

fn case_mismatches(query_chars: Option<&[char]>, matched: &[u32], name: &str) -> i32 {
    let Some(query_chars) = query_chars else {
        return 0;
    };
    if query_chars.len() != matched.len() {
        return 0;
    }
    let name_chars: Vec<char> = name.chars().collect();
    query_chars
        .iter()
        .zip(matched)
        .filter(|(&query_char, &position)| {
            name_chars
                .get(position as usize)
                .is_some_and(|&c| c != query_char && c.eq_ignore_ascii_case(&query_char))
        })
        .count() as i32
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharBag(u64);

impl CharBag {
    pub fn is_superset(self, other: CharBag) -> bool {
        self.0 & other.0 == other.0
    }

    fn insert(&mut self, c: char) {
        let c = c.to_lowercase().next().unwrap_or(c);
        if c.is_ascii_lowercase() {
            let index = u32::from(c as u8 - b'a') * 2;
            let count = ((((self.0 >> index) << 1) | 1) & 3) << index;
            self.0 |= count;
        } else if c.is_ascii_digit() {
            self.0 |= 1 << (u32::from(c as u8 - b'0') + 52);
        } else if c == '-' {
            self.0 |= 1 << 62;
        }
    }
}

impl From<&str> for CharBag {
    fn from(text: &str) -> CharBag {
        let mut bag = CharBag::default();
        for c in text.chars() {
            bag.insert(c);
        }
        bag
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathMatch {
    pub candidate: usize,
    pub positions: Vec<usize>,
    pub score: f64,
    pub distance: usize,
}

fn filename_bonus(path: &str, pattern: &Pattern, matcher: &mut Matcher) -> f64 {
    let Some(name) = path.rsplit('/').next().filter(|name| !name.is_empty()) else {
        return 0.0;
    };
    let mut buffer = Vec::new();
    let haystack = Utf32Str::new(name, &mut buffer);
    let score: u32 = pattern
        .atoms
        .iter()
        .filter_map(|atom| atom.score(haystack, matcher))
        .map(u32::from)
        .sum();
    f64::from(score) / name.len().max(1) as f64
}

fn distance_between(path: &str, relative_to: &str) -> usize {
    let mut left = path.split('/');
    let mut right = relative_to.split('/');
    while left.next().zip(right.next()).is_some_and(|(a, b)| a == b) {}
    left.count() + right.count() + 1
}

fn match_chunk(
    paths: &[(usize, &str, CharBag)],
    query_bag: CharBag,
    pattern: &Pattern,
    relative_to: Option<&str>,
) -> Vec<PathMatch> {
    let mut config = Config::DEFAULT;
    config.set_match_paths();
    let mut matcher = Matcher::new(config);
    let (mut buffer, mut indices) = (Vec::new(), Vec::new());
    let mut found = Vec::new();
    for &(candidate, path, bag) in paths {
        if !bag.is_superset(query_bag) {
            continue;
        }
        indices.clear();
        let haystack = Utf32Str::new(path, &mut buffer);
        let Some(score) = pattern.indices(haystack, &mut matcher, &mut indices) else {
            continue;
        };
        indices.sort_unstable();
        indices.dedup();
        let bonus = filename_bonus(path, pattern, &mut matcher);
        found.push(PathMatch {
            candidate,
            positions: indices.iter().map(|&index| index as usize).collect(),
            score: f64::from(score) + bonus - path.len() as f64 * LENGTH_PENALTY,
            distance: relative_to.map_or(usize::MAX, |to| distance_between(path, to)),
        });
    }
    found
}

pub fn match_paths(
    paths: &[&str],
    bags: &[CharBag],
    query: &str,
    max: usize,
    relative_to: Option<&str>,
) -> Vec<PathMatch> {
    if query.chars().all(char::is_whitespace) {
        return Vec::new();
    }
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let pattern = Pattern::new(
        &normalized,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let query_bag = CharBag::from(query);
    let candidates: Vec<(usize, &str, CharBag)> = paths
        .iter()
        .zip(bags)
        .enumerate()
        .map(|(index, (path, bag))| (index, *path, *bag))
        .collect();
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    let chunk = candidates.len().div_ceil(threads).max(2_000);
    let mut matches: Vec<PathMatch> = std::thread::scope(|scope| {
        let workers: Vec<_> = candidates
            .chunks(chunk)
            .map(|part| scope.spawn(|| match_chunk(part, query_bag, &pattern, relative_to)))
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    });
    matches.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.distance.cmp(&b.distance))
            .then_with(|| paths[a.candidate].cmp(paths[b.candidate]))
    });
    matches.truncate(max);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_bags_count_letters_up_to_two() {
        let bag = CharBag::from("src/main.rs");
        assert!(bag.is_superset(CharBag::from("mrs")));
        assert!(bag.is_superset(CharBag::from("ss")));
        assert!(!CharBag::from("src").is_superset(CharBag::from("ss")));
        assert!(!bag.is_superset(CharBag::from("x")));
    }

    #[test]
    fn paths_favour_file_name_hits_and_nearness() {
        let paths = [
            "crates/editor/src/editor.rs",
            "crates/files_ui/src/files_ui.rs",
            "docs/editing-files.md",
            "crates/files/src/files.rs",
        ];
        let bags: Vec<CharBag> = paths.iter().map(|path| CharBag::from(*path)).collect();
        let found = match_paths(&paths, &bags, "files.rs", 100, None);
        assert_eq!(
            found.first().map(|m| paths[m.candidate]),
            Some("crates/files/src/files.rs")
        );
        assert!(match_paths(&paths, &bags, "zzz", 100, None).is_empty());
        assert!(match_paths(&paths, &bags, "  ", 100, None).is_empty());
        let near = match_paths(&paths, &bags, "rs", 100, Some("crates/editor/src/lib.rs"));
        assert_eq!(near.len(), 3);
        assert_eq!(match_paths(&paths, &bags, "rs", 1, None).len(), 1);
    }

    #[test]
    fn fuzzy_ranks_tighter_and_shorter_names_first() {
        let names = [
            "editor: sort lines by length",
            "editor: select all",
            "editor: undo",
        ];
        let matches = fuzzy_match(&names, "undo");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].candidate, 2);
        assert_eq!(matches[0].positions, vec![8, 9, 10, 11]);
        let matches = fuzzy_match(&names, "sel");
        assert_eq!(matches.first().map(|m| m.candidate), Some(1));
        assert_eq!(fuzzy_match(&names, "").len(), 3);
    }
}
