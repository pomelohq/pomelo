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

#[cfg(test)]
mod tests {
    use super::*;

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
