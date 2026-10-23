use serde::{Deserialize, Serialize};

use crate::Turn;

/// What the agents cost in a day, at API prices.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaySummary {
    pub cost: f64,
    pub sessions: usize,
    /// (workspace, cost), the costliest first.
    pub by_workspace: Vec<(String, f64)>,
}

/// The day of `turns`, each counted under the workspace `workspace` names for it; `None` without turns.
pub fn day_summary<'a>(
    turns: impl IntoIterator<Item = &'a Turn>,
    workspace: impl Fn(&Turn) -> String,
) -> Option<DaySummary> {
    let mut summary = DaySummary::default();
    let mut sessions: Vec<&str> = Vec::new();
    for turn in turns {
        let cost = crate::cost(turn);
        summary.cost += cost;
        sessions.push(&turn.session);
        let key = workspace(turn);
        match summary.by_workspace.iter_mut().find(|entry| entry.0 == key) {
            Some(entry) => entry.1 += cost,
            None => summary.by_workspace.push((key, cost)),
        }
    }
    if sessions.is_empty() {
        return None;
    }
    sessions.sort_unstable();
    sessions.dedup();
    summary.sessions = sessions.len();
    summary.by_workspace.sort_by(|a, b| b.1.total_cmp(&a.1));
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_adds_up_by_workspace_and_session() {
        let turn = |session: &str, cwd: &str, output: u64| Turn {
            session: session.into(),
            cwd: cwd.into(),
            model: "claude-sonnet".into(),
            output,
            ..Turn::default()
        };
        let turns = [
            turn("a", "web", 1_000_000),
            turn("a", "api", 2_000_000),
            turn("b", "api", 1_000_000),
        ];
        let summary = day_summary(&turns, |turn| turn.cwd.display().to_string()).expect("turns");
        assert_eq!(summary.sessions, 2);
        assert_eq!(summary.cost, 60.0);
        assert_eq!(
            summary.by_workspace,
            vec![("api".to_string(), 45.0), ("web".to_string(), 15.0)]
        );
        assert_eq!(day_summary(&[], |_| String::new()), None);
    }
}
