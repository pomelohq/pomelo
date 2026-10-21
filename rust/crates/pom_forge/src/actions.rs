//! Changes the PR tab makes on GitHub: merging, reviewing, replying to and resolving review threads. Each is a
//! GraphQL mutation with its inputs passed as variables, never spliced into the query text.

use serde_json::{json, Value};

use crate::PullRequest;

#[derive(Clone, Debug, PartialEq)]
pub struct Mutation {
    pub query: &'static str,
    pub variables: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeMethod {
    #[default]
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    pub const ALL: [MergeMethod; 3] =
        [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];

    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Merge => "Create a merge commit",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }

    fn graphql(self) -> &'static str {
        match self {
            MergeMethod::Squash => "SQUASH",
            MergeMethod::Merge => "MERGE",
            MergeMethod::Rebase => "REBASE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    RequestChanges,
    Approve,
}

impl ReviewEvent {
    pub fn label(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "Comment",
            ReviewEvent::RequestChanges => "Request changes",
            ReviewEvent::Approve => "Approve",
        }
    }

    fn graphql(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Approve => "APPROVE",
        }
    }
}

const MERGE: &str = "mutation($pr: ID!, $method: PullRequestMergeMethod!, $head: GitObjectID) {
  mergePullRequest(input: { pullRequestId: $pr, mergeMethod: $method, expectedHeadOid: $head }) { pullRequest { state } }
}";

const REVIEW: &str = "mutation($pr: ID!, $event: PullRequestReviewEvent!, $body: String) {
  addPullRequestReview(input: { pullRequestId: $pr, event: $event, body: $body }) { pullRequestReview { state } }
}";

const REPLY: &str = "mutation($thread: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, body: $body }) { comment { id } }
}";

const RESOLVE: &str = "mutation($thread: ID!) {
  resolveReviewThread(input: { threadId: $thread }) { thread { isResolved } }
}";

const UNRESOLVE: &str = "mutation($thread: ID!) {
  unresolveReviewThread(input: { threadId: $thread }) { thread { isResolved } }
}";

/// Merges `pr` as it was shown: GitHub refuses if its head moved since.
pub fn merge(pr: &PullRequest, method: MergeMethod) -> Mutation {
    let head = (!pr.head_ref_oid.is_empty()).then_some(pr.head_ref_oid.as_str());
    Mutation {
        query: MERGE,
        variables: json!({ "pr": pr.id, "method": method.graphql(), "head": head }),
    }
}

pub fn submit_review(pr: &PullRequest, event: ReviewEvent, body: &str) -> Mutation {
    let body = body.trim();
    Mutation {
        query: REVIEW,
        variables: json!({
            "pr": pr.id,
            "event": event.graphql(),
            "body": (!body.is_empty()).then_some(body),
        }),
    }
}

pub fn reply(thread_id: &str, body: &str) -> Mutation {
    Mutation {
        query: REPLY,
        variables: json!({ "thread": thread_id, "body": body.trim() }),
    }
}

pub fn set_resolved(thread_id: &str, resolved: bool) -> Mutation {
    Mutation {
        query: if resolved { RESOLVE } else { UNRESOLVE },
        variables: json!({ "thread": thread_id }),
    }
}

/// Why `pr` cannot merge yet, most pressing first; empty when nothing the forge reported stands in the way.
pub fn merge_blockers(pr: &PullRequest) -> Vec<String> {
    let mut reasons = Vec::new();
    if pr.state != "OPEN" {
        return vec!["Only an open pull request can merge".into()];
    }
    if pr.is_draft {
        reasons.push("Draft pull requests cannot merge; mark it ready on GitHub".into());
    }
    if pr.conflict {
        reasons.push(format!(
            "Resolve the conflicts with {} first",
            pr.base_ref_name
        ));
    }
    match pr.checks.as_str() {
        "fail" => reasons.push("Checks are failing".into()),
        "pending" => reasons.push("Checks are still running".into()),
        _ => {}
    }
    if pr.review == "changes" {
        reasons.push("Changes were requested".into());
    } else if pr.review_decision.eq_ignore_ascii_case("REVIEW_REQUIRED") {
        reasons.push("An approving review is required".into());
    }
    if reasons.is_empty() && pr.merge_state_status.eq_ignore_ascii_case("BLOCKED") {
        reasons.push("Branch protection blocks merging".into());
    }
    if pr.id.is_empty() {
        reasons.push("Refresh to load the pull request first".into());
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{ok, Scripted};
    use crate::{Client, ForgeError};

    fn open() -> PullRequest {
        PullRequest {
            id: "PR_1".into(),
            state: "OPEN".into(),
            head_ref_oid: "abc123".into(),
            base_ref_name: "main".into(),
            checks: "pass".into(),
            review: "approved".into(),
            ..PullRequest::default()
        }
    }

    #[test]
    fn a_merge_names_the_method_and_pins_the_head_shown() {
        let mutation = merge(&open(), MergeMethod::Rebase);
        assert!(mutation.query.starts_with("mutation("));
        assert_eq!(
            mutation.variables,
            json!({ "pr": "PR_1", "method": "REBASE", "head": "abc123" })
        );
        let unpinned = merge(
            &PullRequest {
                head_ref_oid: String::new(),
                ..open()
            },
            MergeMethod::Squash,
        );
        assert_eq!(unpinned.variables["head"], Value::Null);
        assert_eq!(unpinned.variables["method"], "SQUASH");
    }

    #[test]
    fn user_text_travels_as_variables_never_in_the_query() {
        let hostile = "\") { evil } #";
        let review = submit_review(&open(), ReviewEvent::RequestChanges, hostile);
        assert!(!review.query.contains("evil"));
        assert_eq!(review.variables["body"], hostile);
        assert_eq!(review.variables["event"], "REQUEST_CHANGES");
        let answer = reply("PRRT_9", "  Done.  ");
        assert_eq!(
            answer.variables,
            json!({ "thread": "PRRT_9", "body": "Done." })
        );
        assert!(!answer.query.contains("PRRT_9"));
    }

    #[test]
    fn an_empty_review_body_is_left_out() {
        let approve = submit_review(&open(), ReviewEvent::Approve, "  ");
        assert_eq!(approve.variables["body"], Value::Null);
        assert_eq!(approve.variables["event"], "APPROVE");
    }

    #[test]
    fn resolving_and_unresolving_use_their_own_mutations() {
        assert!(set_resolved("T", true)
            .query
            .contains("resolveReviewThread"));
        assert!(set_resolved("T", false)
            .query
            .contains("unresolveReviewThread"));
        assert_eq!(set_resolved("T", true).variables, json!({ "thread": "T" }));
    }

    #[test]
    fn merge_blockers_list_every_reason_in_order() {
        assert!(merge_blockers(&open()).is_empty());
        let stuck = PullRequest {
            is_draft: true,
            conflict: true,
            checks: "fail".into(),
            review: "changes".into(),
            ..open()
        };
        assert_eq!(
            merge_blockers(&stuck),
            [
                "Draft pull requests cannot merge; mark it ready on GitHub",
                "Resolve the conflicts with main first",
                "Checks are failing",
                "Changes were requested",
            ]
        );
        let unreviewed = PullRequest {
            review: "review".into(),
            review_decision: "REVIEW_REQUIRED".into(),
            ..open()
        };
        assert_eq!(
            merge_blockers(&unreviewed),
            ["An approving review is required"]
        );
        let protected = PullRequest {
            merge_state_status: "BLOCKED".into(),
            ..open()
        };
        assert_eq!(
            merge_blockers(&protected),
            ["Branch protection blocks merging"]
        );
        let merged = PullRequest {
            state: "MERGED".into(),
            ..open()
        };
        assert_eq!(merge_blockers(&merged).len(), 1);
    }

    #[test]
    fn a_rejected_change_fails_even_beside_partial_data() {
        let transport = Scripted::default();
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(ok(
                r#"{"data":{"mergePullRequest":null},"errors":[{"message":"Pull Request is not mergeable"}]}"#,
            ));
            answers.push(ok(
                r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}"#,
            ));
        }
        let client = Client::new("t", Box::new(transport.clone()));
        assert_eq!(
            client.mutate(&merge(&open(), MergeMethod::Squash)),
            Err(ForgeError::Rejected("Pull Request is not mergeable".into()))
        );
        assert!(client.mutate(&set_resolved("T", true)).is_ok());
        let sent = transport
            .sent
            .lock()
            .map(|sent| sent.clone())
            .unwrap_or_default();
        let first: Value = serde_json::from_str(&sent[0]).unwrap_or_default();
        assert_eq!(first["variables"]["pr"], "PR_1");
        assert!(first["query"]
            .as_str()
            .is_some_and(|query| query.contains("mergePullRequest")));
    }
}
