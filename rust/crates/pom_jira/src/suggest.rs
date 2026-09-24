//! Ranking sprint tickets for the new-workspace picker: drop tickets that already have a workspace, honor "only
//! mine", fuzzy-match the query against key and summary, then mine first, best match, key.

use crate::SprintIssue;

const BOUNDARIES: [char; 6] = ['-', '_', '/', ' ', '.', ':'];

/// Every space-separated token must appear in order in `target`; consecutive and word-start matches score
/// higher, a late or spread-out match lower. `None` when a token does not match.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i64> {
    let tokens: Vec<Vec<char>> = query
        .to_lowercase()
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(|token| token.chars().collect())
        .collect();
    let haystack: Vec<char> = target.to_lowercase().chars().collect();
    tokens
        .iter()
        .map(|token| token_score(token, &haystack))
        .sum()
}

fn token_score(needle: &[char], haystack: &[char]) -> Option<i64> {
    let (mut found, mut score) = (0usize, 0i64);
    let (mut previous, mut first): (i64, i64) = (-2, -1);
    for (index, c) in haystack.iter().enumerate() {
        if found == needle.len() {
            break;
        }
        if *c != needle[found] {
            continue;
        }
        let at = index as i64;
        if first < 0 {
            first = at;
        }
        let mut bonus = 1;
        if at == previous + 1 {
            bonus += 8;
        }
        if index == 0 || BOUNDARIES.contains(&haystack[index - 1]) {
            bonus += 12;
        }
        score += bonus;
        previous = at;
        found += 1;
    }
    if found < needle.len() {
        return None;
    }
    Some(score - first / 4 - (previous - first - needle.len() as i64 + 1))
}

/// `existing` holds the (uppercase) keys that already have a workspace.
pub fn rank_suggestions(
    issues: &[SprintIssue],
    existing: &[String],
    query: &str,
    only_mine: bool,
) -> Vec<SprintIssue> {
    let query = query.trim();
    let mut scored: Vec<(&SprintIssue, i64)> = issues
        .iter()
        .filter(|issue| !existing.contains(&issue.key.to_uppercase()))
        .filter(|issue| !only_mine || issue.mine)
        .filter_map(|issue| {
            if query.is_empty() {
                return Some((issue, 0));
            }
            [
                fuzzy_score(query, &issue.key),
                fuzzy_score(query, &issue.summary),
            ]
            .into_iter()
            .flatten()
            .max()
            .map(|score| (issue, score))
        })
        .collect();
    scored.sort_by(|(a, a_score), (b, b_score)| {
        b.mine
            .cmp(&a.mine)
            .then(b_score.cmp(a_score))
            .then(a.key.cmp(&b.key))
    });
    scored.into_iter().map(|(issue, _)| issue.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(key: &str, summary: &str, mine: bool) -> SprintIssue {
        SprintIssue {
            key: key.into(),
            summary: summary.into(),
            mine,
            ..SprintIssue::default()
        }
    }

    #[test]
    fn word_starts_and_runs_score_higher() {
        assert!(fuzzy_score("log", "Fix login page") > fuzzy_score("log", "catalog"));
        assert_eq!(fuzzy_score("xyz", "login"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert!(fuzzy_score("fix page", "Fix login page").is_some());
    }

    #[test]
    fn mine_first_then_match_then_key() {
        let issues = [
            issue("PROJ-3", "Checkout total", false),
            issue("PROJ-1", "Login page", false),
            issue("PROJ-2", "Login redirect", true),
            issue("PROJ-4", "Login help", false),
        ];
        let keys = |ranked: Vec<SprintIssue>| -> Vec<String> {
            ranked.into_iter().map(|issue| issue.key).collect()
        };
        assert_eq!(
            keys(rank_suggestions(&issues, &["PROJ-4".into()], "", false)),
            ["PROJ-2", "PROJ-1", "PROJ-3"]
        );
        assert_eq!(
            keys(rank_suggestions(&issues, &[], "login", false)),
            ["PROJ-2", "PROJ-1", "PROJ-4"]
        );
        assert_eq!(keys(rank_suggestions(&issues, &[], "", true)), ["PROJ-2"]);
        assert_eq!(
            keys(rank_suggestions(&issues, &[], "proj-3", false)),
            ["PROJ-3"]
        );
    }
}
