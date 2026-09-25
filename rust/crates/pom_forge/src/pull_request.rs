use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Client, ForgeError};

const BATCH: usize = 20;

// Field names follow the previous core's cache (`cache/pr.json`) so both read each other's entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Actor {
    pub login: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Review {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Actor>,
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub submitted_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Check {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub conclusion: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub details_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workflow_name: String,
    /// pass / fail / pending / none.
    pub result: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewRequest {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub slug: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reviewer {
    pub name: String,
    /// approved / changes / commented / pending.
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// OPEN / MERGED / CLOSED.
    pub state: String,
    pub url: String,
    pub is_draft: bool,
    pub mergeable: String,
    pub merge_state_status: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub author: Option<Actor>,
    pub reviews: Vec<Review>,
    pub review_decision: String,
    pub status_check_rollup: Vec<Check>,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub review_requests: Vec<ReviewRequest>,
    /// pass / fail / pending / none.
    pub checks: String,
    /// approved / changes / review / none.
    pub review: String,
    pub conflict: bool,
    pub reviewers: Vec<Reviewer>,
    /// The conversation after the description, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<TimelineItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadComment {
    pub author: String,
    pub body: String,
    pub at: String,
}

/// Inline comments on one line of the diff.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewThread {
    pub path: String,
    pub line: Option<u64>,
    pub resolved: bool,
    pub comments: Vec<ThreadComment>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineKind {
    #[default]
    Comment,
    /// A submitted review, with the inline threads it started.
    Review,
    /// Inline threads no listed review started.
    Inline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineItem {
    pub kind: TimelineKind,
    pub author: String,
    pub body: String,
    pub at: String,
    /// A review's APPROVED / CHANGES_REQUESTED / COMMENTED.
    pub state: String,
    pub threads: Vec<ReviewThread>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Merged,
    Danger,
}

fn check_result(status: &str, conclusion: &str) -> &'static str {
    let (status, conclusion) = (status.to_uppercase(), conclusion.to_uppercase());
    match conclusion.as_str() {
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" => return "fail",
        "SUCCESS" => return "pass",
        _ => {}
    }
    if conclusion == "PENDING" || matches!(status.as_str(), "IN_PROGRESS" | "QUEUED" | "PENDING") {
        "pending"
    } else {
        "none"
    }
}

impl PullRequest {
    pub fn classify(&mut self) {
        for check in &mut self.status_check_rollup {
            check.result = check_result(&check.status, &check.conclusion).to_string();
        }
        let results: Vec<&str> = self
            .status_check_rollup
            .iter()
            .map(|check| check.result.as_str())
            .collect();
        self.checks = if results.contains(&"fail") {
            "fail"
        } else if results.contains(&"pending") {
            "pending"
        } else if results.contains(&"pass") {
            "pass"
        } else {
            "none"
        }
        .to_string();
        self.review = match self.review_decision.to_uppercase().as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes",
            "REVIEW_REQUIRED" => "review",
            _ if self.reviews.is_empty() => "none",
            _ if self
                .reviews
                .iter()
                .any(|review| review.state.eq_ignore_ascii_case("CHANGES_REQUESTED")) =>
            {
                "changes"
            }
            _ if self
                .reviews
                .iter()
                .any(|review| review.state.eq_ignore_ascii_case("APPROVED")) =>
            {
                "approved"
            }
            _ => "review",
        }
        .to_string();
        self.conflict = self.mergeable.eq_ignore_ascii_case("CONFLICTING");
        let mut latest: HashMap<String, String> = HashMap::new();
        for review in &self.reviews {
            if let Some(author) = review
                .author
                .as_ref()
                .filter(|author| !author.login.is_empty())
            {
                latest.insert(author.login.clone(), review.state.to_uppercase());
            }
        }
        let mut reviewers: Vec<Reviewer> = latest
            .iter()
            .map(|(name, state)| Reviewer {
                name: name.clone(),
                state: match state.as_str() {
                    "APPROVED" => "approved",
                    "CHANGES_REQUESTED" => "changes",
                    _ => "commented",
                }
                .to_string(),
            })
            .collect();
        for request in &self.review_requests {
            let name = if request.login.is_empty() {
                &request.name
            } else {
                &request.login
            };
            if !name.is_empty() && !latest.contains_key(name) {
                reviewers.push(Reviewer {
                    name: name.clone(),
                    state: "pending".into(),
                });
            }
        }
        reviewers.sort_by(|a, b| a.name.cmp(&b.name));
        self.reviewers = reviewers;
    }
}

/// One token for a workspace's PRs: danger over merged over warn over ok; merged PRs only count as merged.
pub fn severity<'a>(prs: impl IntoIterator<Item = &'a PullRequest>) -> Severity {
    let (mut worst, mut merged) = (Severity::Ok, false);
    for pr in prs {
        if pr.state == "MERGED" {
            merged = true;
        } else if pr.conflict || pr.checks == "fail" || pr.review == "changes" {
            worst = Severity::Danger;
        } else if worst == Severity::Ok && (pr.checks == "pending" || pr.review == "review") {
            worst = Severity::Warn;
        }
    }
    match (worst, merged) {
        (Severity::Danger, _) => Severity::Danger,
        (_, true) => Severity::Merged,
        (worst, false) => worst,
    }
}

const NODE_FIELDS: &str = "number title state url isDraft mergeable mergeStateStatus
        headRefName baseRefName additions deletions changedFiles createdAt updatedAt
        reviewDecision
        author { login avatarUrl }
        latestReviews(first: 30) { nodes { state submittedAt author { login } } }
        commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
          __typename
          ... on CheckRun { name status conclusion detailsUrl startedAt checkSuite { workflowRun { workflow { name } } } }
          ... on StatusContext { context state targetUrl }
        } } } } } }
";

const DETAIL_FIELDS: &str = "body
        labels(first: 20) { nodes { name color } }
        reviewRequests(first: 20) { nodes { requestedReviewer { __typename ... on User { login } } } }
        comments(first: 50) { nodes { author { login } body createdAt } }
        reviews(first: 50) { nodes { databaseId state body submittedAt author { login } } }
        reviewThreads(first: 100) { nodes { isResolved path line comments(first: 50) { nodes { body createdAt author { login } pullRequestReview { databaseId } } } } }
";

/// Reviews with the inline threads they started, threads no listed review started, and plain comments, oldest
/// first. A review that only says it commented, with nothing written and no threads, is left out.
fn timeline(node: &Value) -> Vec<TimelineItem> {
    let login = |value: &Value| text(value, "/author/login");
    let review_ids: Vec<u64> = nodes(node, "/reviews/nodes")
        .filter_map(|review| review.pointer("/databaseId").and_then(Value::as_u64))
        .collect();
    let mut by_review: Vec<(u64, ReviewThread)> = Vec::new();
    let mut standalone: Vec<ReviewThread> = Vec::new();
    for thread in nodes(node, "/reviewThreads/nodes") {
        let comments: Vec<ThreadComment> = nodes(thread, "/comments/nodes")
            .map(|comment| ThreadComment {
                author: login(comment),
                body: text(comment, "/body"),
                at: text(comment, "/createdAt"),
            })
            .collect();
        if comments.is_empty() {
            continue;
        }
        let review = thread
            .pointer("/comments/nodes/0/pullRequestReview/databaseId")
            .and_then(Value::as_u64);
        let item = ReviewThread {
            path: text(thread, "/path"),
            line: thread.pointer("/line").and_then(Value::as_u64),
            resolved: thread
                .pointer("/isResolved")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            comments,
        };
        match review.filter(|id| review_ids.contains(id)) {
            Some(id) => by_review.push((id, item)),
            None => standalone.push(item),
        }
    }
    let mut items: Vec<TimelineItem> = Vec::new();
    for review in nodes(node, "/reviews/nodes") {
        let id = review.pointer("/databaseId").and_then(Value::as_u64);
        let threads: Vec<ReviewThread> = by_review
            .iter()
            .filter(|(owner, _)| Some(*owner) == id)
            .map(|(_, thread)| thread.clone())
            .collect();
        let state = text(review, "/state").to_uppercase();
        let body = text(review, "/body");
        if body.trim().is_empty() && threads.is_empty() && state == "COMMENTED" {
            continue;
        }
        items.push(TimelineItem {
            kind: TimelineKind::Review,
            author: login(review),
            body,
            at: text(review, "/submittedAt"),
            state,
            threads,
        });
    }
    for thread in standalone {
        items.push(TimelineItem {
            kind: TimelineKind::Inline,
            author: thread.comments[0].author.clone(),
            body: String::new(),
            at: thread.comments[0].at.clone(),
            state: String::new(),
            threads: vec![thread],
        });
    }
    for comment in nodes(node, "/comments/nodes") {
        let body = text(comment, "/body");
        if body.trim().is_empty() {
            continue;
        }
        items.push(TimelineItem {
            kind: TimelineKind::Comment,
            author: login(comment),
            body,
            at: text(comment, "/createdAt"),
            ..TimelineItem::default()
        });
    }
    items.sort_by(|a, b| a.at.cmp(&b.at));
    items
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn actor(value: &Value) -> Option<Actor> {
    value.as_object().map(|_| Actor {
        login: text(value, "/login"),
        avatar_url: text(value, "/avatarUrl"),
    })
}

fn nodes<'a>(value: &'a Value, pointer: &str) -> impl Iterator<Item = &'a Value> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn from_node(node: &Value) -> PullRequest {
    let number = |pointer: &str| node.pointer(pointer).and_then(Value::as_u64).unwrap_or(0);
    let checks = nodes(
        node,
        "/commits/nodes/0/commit/statusCheckRollup/contexts/nodes",
    )
    .map(|context| {
        if text(context, "/__typename") == "StatusContext" {
            // A commit status has one state; map it onto a check run's status and conclusion.
            let (status, conclusion) = match text(context, "/state").as_str() {
                "SUCCESS" => ("COMPLETED", "SUCCESS"),
                "FAILURE" | "ERROR" => ("COMPLETED", "FAILURE"),
                _ => ("PENDING", ""),
            };
            Check {
                name: text(context, "/context"),
                status: status.into(),
                conclusion: conclusion.into(),
                details_url: text(context, "/targetUrl"),
                ..Check::default()
            }
        } else {
            Check {
                name: text(context, "/name"),
                status: text(context, "/status"),
                conclusion: text(context, "/conclusion"),
                details_url: text(context, "/detailsUrl"),
                started_at: text(context, "/startedAt"),
                workflow_name: text(context, "/checkSuite/workflowRun/workflow/name"),
                result: String::new(),
            }
        }
    })
    .collect();
    let mut pr = PullRequest {
        number: number("/number"),
        title: text(node, "/title"),
        state: text(node, "/state"),
        url: text(node, "/url"),
        is_draft: node
            .pointer("/isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        mergeable: text(node, "/mergeable"),
        merge_state_status: text(node, "/mergeStateStatus"),
        head_ref_name: text(node, "/headRefName"),
        base_ref_name: text(node, "/baseRefName"),
        author: node.get("author").and_then(actor),
        reviews: nodes(node, "/latestReviews/nodes")
            .map(|review| Review {
                author: review.get("author").and_then(actor),
                state: text(review, "/state"),
                submitted_at: text(review, "/submittedAt"),
            })
            .collect(),
        review_decision: text(node, "/reviewDecision"),
        status_check_rollup: checks,
        additions: number("/additions"),
        deletions: number("/deletions"),
        changed_files: number("/changedFiles"),
        created_at: text(node, "/createdAt"),
        updated_at: text(node, "/updatedAt"),
        body: text(node, "/body"),
        labels: nodes(node, "/labels/nodes")
            .map(|label| Label {
                name: text(label, "/name"),
                color: text(label, "/color"),
            })
            .collect(),
        review_requests: nodes(node, "/reviewRequests/nodes")
            .filter_map(|request| {
                request
                    .get("requestedReviewer")
                    .filter(|reviewer| !reviewer.is_null())
            })
            .map(|reviewer| ReviewRequest {
                login: text(reviewer, "/login"),
                name: text(reviewer, "/name"),
                slug: text(reviewer, "/slug"),
            })
            .collect(),
        timeline: timeline(node),
        ..PullRequest::default()
    };
    pr.classify();
    pr
}

/// Which repo's branch to look up: `repo` is the workspace's name for it, `owner/name` the GitHub repo.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrTarget {
    pub repo: String,
    pub owner: String,
    pub name: String,
    pub head: String,
}

impl PrTarget {
    pub fn key(&self) -> String {
        format!("{}\u{0}{}", self.repo, self.head)
    }
}

fn quoted(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

fn lookup(target: &PrTarget, states: &str, extra: &str) -> String {
    format!(
        "repository(owner: {}, name: {}) {{ pullRequests(headRefName: {}, first: 1, states: [{states}], orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{\n{NODE_FIELDS}{extra}      }} }} }}",
        quoted(&target.owner),
        quoted(&target.name),
        quoted(&target.head),
    )
}

/// The newest open or merged PR of each target's branch (`None` when it has none), in batches of 20. A batch that
/// fails leaves its targets out and makes the whole call an error once the rest are in.
pub fn fetch_heads(
    client: &Client,
    targets: &[PrTarget],
) -> (HashMap<String, Option<PullRequest>>, Option<ForgeError>) {
    let mut found = HashMap::new();
    let mut failure = None;
    for chunk in targets.chunks(BATCH) {
        let mut query = String::from("query {\n");
        for (index, target) in chunk.iter().enumerate() {
            query.push_str(&format!(
                "  p{index}: {}\n",
                lookup(target, "OPEN, MERGED", "")
            ));
        }
        query.push_str("}\n");
        match client.graphql(&query) {
            Ok(data) => {
                for (index, target) in chunk.iter().enumerate() {
                    let pr = data
                        .pointer(&format!("/p{index}/pullRequests/nodes/0"))
                        .filter(|node| node.is_object())
                        .map(from_node);
                    found.insert(target.key(), pr);
                }
            }
            Err(error) => failure = Some(error),
        }
    }
    (found, failure)
}

/// The branch's newest PR in any state, with its description, labels and requested reviewers.
pub fn fetch_detail(client: &Client, target: &PrTarget) -> Result<Option<PullRequest>, ForgeError> {
    let query = format!(
        "query {{ {} }}",
        lookup(target, "OPEN, MERGED, CLOSED", DETAIL_FIELDS)
    );
    let data = client.graphql(&query)?;
    Ok(data
        .pointer("/repository/pullRequests/nodes/0")
        .filter(|node| node.is_object())
        .map(from_node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{ok, Scripted};

    fn node() -> Value {
        serde_json::json!({
            "number": 42, "title": "Add login", "state": "OPEN", "url": "https://github.com/acme/web/pull/42",
            "isDraft": false, "mergeable": "MERGEABLE", "headRefName": "feat-login", "baseRefName": "main",
            "additions": 10, "deletions": 2, "changedFiles": 3, "reviewDecision": "",
            "author": { "login": "dev", "avatarUrl": "" },
            "latestReviews": { "nodes": [
                { "state": "COMMENTED", "author": { "login": "bea" } },
                { "state": "APPROVED", "author": { "login": "cy" } }
            ] },
            "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "contexts": { "nodes": [
                { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS",
                  "checkSuite": { "workflowRun": { "workflow": { "name": "CI" } } } },
                { "__typename": "StatusContext", "context": "deploy", "state": "PENDING", "targetUrl": "https://ci" }
            ] } } } } ] },
            "reviewRequests": { "nodes": [ { "requestedReviewer": { "__typename": "User", "login": "ann" } } ] }
        })
    }

    #[test]
    fn the_timeline_groups_threads_under_their_review_and_sorts_by_time() {
        let node = serde_json::json!({
            "comments": { "nodes": [
                { "author": { "login": "ann" }, "body": "Looks close", "createdAt": "2026-09-02T10:00:00Z" },
                { "author": { "login": "bot" }, "body": "", "createdAt": "2026-09-02T11:00:00Z" }
            ] },
            "reviews": { "nodes": [
                { "databaseId": 7, "state": "CHANGES_REQUESTED", "body": "Fix the query",
                  "submittedAt": "2026-09-01T09:00:00Z", "author": { "login": "bea" } },
                { "databaseId": 8, "state": "COMMENTED", "body": "",
                  "submittedAt": "2026-09-03T09:00:00Z", "author": { "login": "cy" } }
            ] },
            "reviewThreads": { "nodes": [
                { "isResolved": true, "path": "src/db.rs", "line": 12, "comments": { "nodes": [
                    { "body": "N+1 here", "createdAt": "2026-09-01T09:00:00Z", "author": { "login": "bea" },
                      "pullRequestReview": { "databaseId": 7 } },
                    { "body": "Fixed", "createdAt": "2026-09-01T12:00:00Z", "author": { "login": "dev" },
                      "pullRequestReview": { "databaseId": 99 } }
                ] } },
                { "isResolved": false, "path": "src/ui.rs", "line": null, "comments": { "nodes": [
                    { "body": "Rename?", "createdAt": "2026-09-04T09:00:00Z", "author": { "login": "dan" },
                      "pullRequestReview": null }
                ] } }
            ] }
        });
        let items = timeline(&node);
        let kinds: Vec<(TimelineKind, &str)> = items
            .iter()
            .map(|item| (item.kind, item.author.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (TimelineKind::Review, "bea"),
                (TimelineKind::Comment, "ann"),
                (TimelineKind::Inline, "dan")
            ],
            "an empty comment and a bare COMMENTED review drop out"
        );
        assert_eq!(items[0].state, "CHANGES_REQUESTED");
        assert_eq!(items[0].threads.len(), 1);
        assert!(items[0].threads[0].resolved);
        assert_eq!(items[0].threads[0].line, Some(12));
        assert_eq!(items[0].threads[0].comments[1].body, "Fixed");
        assert_eq!(items[2].threads[0].line, None);
    }

    #[test]
    fn a_node_becomes_a_classified_pull_request() {
        let pr = from_node(&node());
        assert_eq!(pr.number, 42);
        assert_eq!(pr.status_check_rollup[0].workflow_name, "CI");
        assert_eq!(pr.status_check_rollup[0].result, "pass");
        assert_eq!(pr.status_check_rollup[1].result, "pending");
        assert_eq!(pr.checks, "pending");
        assert_eq!(pr.review, "approved");
        assert!(!pr.conflict);
        let reviewers: Vec<(&str, &str)> = pr
            .reviewers
            .iter()
            .map(|reviewer| (reviewer.name.as_str(), reviewer.state.as_str()))
            .collect();
        assert_eq!(
            reviewers,
            [("ann", "pending"), ("bea", "commented"), ("cy", "approved")]
        );
    }

    #[test]
    fn checks_and_review_follow_the_worst_signal() {
        let mut pr = from_node(&node());
        pr.status_check_rollup[1].conclusion = "TIMED_OUT".into();
        pr.review_decision = "CHANGES_REQUESTED".into();
        pr.mergeable = "CONFLICTING".into();
        pr.classify();
        assert_eq!(
            (pr.checks.as_str(), pr.review.as_str(), pr.conflict),
            ("fail", "changes", true)
        );
        assert_eq!(severity([&pr]), Severity::Danger);
    }

    #[test]
    fn severity_ranks_danger_then_merged_then_warn() {
        let pr = |state: &str, checks: &str, review: &str| PullRequest {
            state: state.into(),
            checks: checks.into(),
            review: review.into(),
            ..PullRequest::default()
        };
        assert_eq!(severity([&pr("OPEN", "pass", "approved")]), Severity::Ok);
        assert_eq!(severity([&pr("OPEN", "pending", "none")]), Severity::Warn);
        assert_eq!(
            severity([
                &pr("OPEN", "pending", "none"),
                &pr("MERGED", "fail", "changes")
            ]),
            Severity::Merged
        );
        assert_eq!(
            severity([&pr("OPEN", "fail", "none"), &pr("MERGED", "pass", "none")]),
            Severity::Danger
        );
        assert_eq!(severity([]), Severity::Ok);
    }

    #[test]
    fn heads_are_asked_in_one_aliased_query() {
        let transport = Scripted::default();
        let answer = serde_json::json!({ "data": {
            "p0": { "pullRequests": { "nodes": [ node() ] } },
            "p1": { "pullRequests": { "nodes": [] } }
        } });
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(ok(&answer.to_string()));
        }
        let client = Client::new("t", Box::new(transport.clone()));
        let target = |repo: &str, head: &str| PrTarget {
            repo: repo.into(),
            owner: "acme".into(),
            name: repo.into(),
            head: head.into(),
        };
        let targets = [
            target("web", "feat-login"),
            target("api", "feat \"quoted\""),
        ];
        let (found, failure) = fetch_heads(&client, &targets);
        assert!(failure.is_none());
        assert_eq!(
            found[&targets[0].key()].as_ref().map(|pr| pr.number),
            Some(42)
        );
        assert_eq!(found[&targets[1].key()], None);
        let sent = transport
            .sent
            .lock()
            .map(|sent| sent[0].clone())
            .unwrap_or_default();
        let query: Value = serde_json::from_str(&sent).unwrap_or_default();
        let query = query["query"].as_str().unwrap_or_default();
        assert!(query.contains("p1: repository(owner: \"acme\", name: \"api\") { pullRequests(headRefName: \"feat \\\"quoted\\\"\""));
        assert!(query.contains("states: [OPEN, MERGED]"));
    }

    #[test]
    fn the_cache_format_matches_the_previous_core() {
        let pr = from_node(&node());
        let json = serde_json::to_value(&pr).unwrap_or_default();
        for key in [
            "isDraft",
            "headRefName",
            "statusCheckRollup",
            "checks",
            "review",
            "conflict",
            "reviewers",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(
            json.get("body").is_none(),
            "empty detail fields are left out"
        );
        let back: PullRequest = serde_json::from_value(json).unwrap_or_default();
        assert_eq!(back, pr);
    }
}
