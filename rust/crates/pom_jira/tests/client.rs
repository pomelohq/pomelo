//! The client against a stand-in `curl` that records its arguments and stdin and answers canned JSON.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use pom_jira::{Client, CurlTransport, IssueCache};
use pom_paths::StateDir;

const TOKEN: &str = "s3cret-token";

fn fake_curl(dir: &Path) -> PathBuf {
    let script = dir.join("curl");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
echo "$@" >> '{args}'
cat >> '{stdin}'
for last; do :; done
case "$last" in
  */denied/*) printf '{{}}'; printf '\n401'; exit 0 ;;
  */rest/api/3/myself) printf '{{"displayName":"Sam","emailAddress":"sam@example.com","accountId":"acc-1"}}' ;;
  */rest/agile/1.0/board\?*) printf '{{"values":[{{"id":7,"name":"Team board"}}]}}' ;;
  */board/7/sprint\?*) printf '{{"values":[{{"id":42,"name":"Sprint 9"}}]}}' ;;
  */sprint/42/issue\?*) printf '{{"issues":[{{"key":"PROJ-1","fields":{{"summary":"Login page","status":{{"name":"To Do"}},"assignee":{{"displayName":"Sam","accountId":"acc-1"}}}}}},{{"key":"PROJ-2","fields":{{"summary":"Checkout","status":{{"name":"Done"}},"assignee":null}}}}]}}' ;;
  */search/jql\?*) printf '{{"issues":[{{"key":"PROJ-1","fields":{{"summary":"Login page","status":{{"name":"In Progress","statusCategory":{{"key":"indeterminate"}}}},"assignee":{{"displayName":"Sam"}}}}}}]}}' ;;
  */issue/PROJ-1/remotelink) printf '[{{"object":{{"title":"PR","url":"https://example.com/pr/1","icon":{{"url16x16":""}}}}}}]' ;;
  */issue/PROJ-1\?*) printf '{{"fields":{{"summary":"Login page","status":{{"name":"In Progress"}},"description":{{"type":"doc","content":[{{"type":"paragraph","content":[{{"type":"text","text":"Make it work"}}]}}]}},"attachment":[{{"filename":"a.png","mimeType":"image/png","content":"https://example.com/a.png"}}],"comment":{{"comments":[]}}}}}}' ;;
  *) printf '{{}}'; printf '\n404'; exit 0 ;;
esac
printf '\n200'
"#,
            args = dir.join("args.log").display(),
            stdin = dir.join("stdin.log").display(),
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn client(dir: &Path, site: &str) -> Client {
    Client::new(
        site,
        "sam@example.com",
        TOKEN,
        Box::new(CurlTransport {
            program: fake_curl(dir),
        }),
    )
}

#[test]
fn reads_boards_sprints_status_and_detail_without_exposing_the_token() {
    let temp = tempfile::tempdir().expect("temp");
    let jira = client(temp.path(), "acme.atlassian.net/");
    assert_eq!(jira.site(), "https://acme.atlassian.net");
    assert_eq!(
        jira.myself(),
        Ok(("Sam".to_string(), "sam@example.com".to_string()))
    );
    let boards = jira.boards().expect("boards");
    assert_eq!(boards[0].id, 7);
    let sprint = jira.current_sprint_issues(7).expect("sprint");
    assert_eq!(sprint.len(), 2);
    assert!(sprint[0].mine && !sprint[1].mine);
    assert_eq!(sprint[0].sprint, "Sprint 9");

    let detail = jira.issue_detail("PROJ-1").expect("detail");
    assert_eq!(
        detail.description,
        "Make it work\n\n### Attachments\n\n![a.png](https://example.com/a.png)"
    );
    assert_eq!(detail.web_links[0].title, "PR");
    assert_eq!(detail.url, "https://acme.atlassian.net/browse/PROJ-1");

    let args = std::fs::read_to_string(temp.path().join("args.log")).expect("args");
    assert!(
        !args.contains(TOKEN) && !args.contains("Authorization"),
        "{args}"
    );
    let stdin = std::fs::read_to_string(temp.path().join("stdin.log")).expect("stdin");
    assert!(
        stdin
            .contains("header = \"Authorization: Basic c2FtQGV4YW1wbGUuY29tOnMzY3JldC10b2tlbg==\""),
        "{stdin}"
    );
}

#[test]
fn the_status_cache_survives_a_restart_in_the_shared_layout() {
    let temp = tempfile::tempdir().expect("temp");
    let state = StateDir::new(temp.path().join("state"));
    let jira = client(temp.path(), "https://acme.atlassian.net");
    let keys = vec!["PROJ-1".to_string()];

    let mut cache = IssueCache::open(&state, "demo");
    assert!(cache.issue("PROJ-1").is_none());
    assert!(cache.refresh(&jira, &keys));
    assert!(!cache.refresh(&jira, &keys), "fresh for a minute");
    assert_eq!(
        cache.issue("PROJ-1").map(|i| i.status.as_str()),
        Some("In Progress")
    );

    let detail = jira.issue_detail("PROJ-1").expect("detail");
    cache.store_detail(&detail);
    let reopened = IssueCache::open(&state, "demo");
    assert_eq!(
        reopened.issue("PROJ-1").map(|i| i.category.as_str()),
        Some("indeterminate")
    );
    assert_eq!(reopened.detail("PROJ-1"), Some(detail));
    assert_eq!(reopened.stale(&keys), keys, "a restart fetches again");

    let file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("state/cache/jira-demo.json")).expect("file"),
    )
    .expect("json");
    assert_eq!(file["version"], 2);
    assert_eq!(file["detail"]["PROJ-1"]["configured"], true);
}

#[test]
fn http_failures_are_errors() {
    let temp = tempfile::tempdir().expect("temp");
    let jira = client(temp.path(), "https://acme.atlassian.net/denied");
    let error = jira.boards().expect_err("rejected");
    assert_eq!(error, pom_jira::JiraError::Http(401));
    assert_eq!(error.to_string(), "auth rejected - check email and token");
}
