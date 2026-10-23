//! The `jira.*` methods against a scratch project, with Jira itself a fake that answers from fixtures: the
//! settings and token stay on this machine, and every answer leaves the token out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pom_jira::{IssueDetail, JiraError, JiraSetup, SprintIssue, TokenOrigin, Transport};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_remote::server_api::{method, JiraAccount};
use pom_rpc::{error_code, Request};
use pom_server::{Dispatcher, JiraService, FAILED};
use serde_json::{json, Value};

const TOKEN: &str = "t0ken-on-the-host";

#[derive(Default)]
struct Asked {
    urls: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
}

struct FakeJira(Arc<Asked>);

impl Transport for FakeJira {
    fn get(&self, url: &str, authorization: &str) -> Result<(u16, String), JiraError> {
        if let Ok(mut urls) = self.0.urls.lock() {
            urls.push(url.to_string());
        }
        if let Ok(mut seen) = self.0.authorizations.lock() {
            seen.push(authorization.to_string());
        }
        let path = url
            .strip_prefix("https://example.atlassian.net")
            .unwrap_or(url);
        let body = if path.starts_with("/rest/api/3/myself") {
            json!({"displayName": "Dev", "emailAddress": "you@example.com", "accountId": "a1"})
        } else if path.starts_with("/rest/api/3/search/jql") {
            json!({"issues": [{"key": "PROJ-101", "fields": {
                "summary": "Login page", "status": {"name": "In Progress", "statusCategory": {"key": "indeterminate"}},
                "assignee": {"displayName": "Dev", "accountId": "a1"}}}]})
        } else if path.starts_with("/rest/api/3/issue/PROJ-101/remotelink") {
            json!([])
        } else if path.starts_with("/rest/api/3/issue/PROJ-101") {
            json!({"fields": {"summary": "Login page", "status": {"name": "In Progress"}}})
        } else if path.starts_with("/rest/agile/1.0/board?") {
            json!({"values": [{"id": 7, "name": "Web"}]})
        } else if path.starts_with("/rest/agile/1.0/board/7/backlog") {
            json!({"issues": [{"key": "PROJ-102", "fields": {"summary": "Signup",
                "status": {"name": "To Do", "statusCategory": {"key": "new"}}, "assignee": null}}]})
        } else {
            return Ok((404, String::new()));
        };
        Ok((200, body.to_string()))
    }
}

struct Project {
    temp: tempfile::TempDir,
    asked: Arc<Asked>,
}

impl Project {
    fn new() -> Project {
        let temp = tempfile::Builder::new()
            .prefix("pomj")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join("workspace--main/api/.git")).expect("worktree");
        std::fs::write(
            root.join("pom.yml"),
            "session: myproject\ndefault_branch: main\nrepos:\n  api: {}\n",
        )
        .expect("pom.yml");
        let project = Project {
            temp,
            asked: Arc::default(),
        };
        let mut sessions = pom_sessions::Sessions::load(&project.state());
        sessions.touch("myproject", &root.to_string_lossy(), 10);
        sessions.save(&project.state()).expect("sessions");
        project
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state"))
    }

    fn cfg(&self) -> String {
        self.temp
            .path()
            .join("project/pom.yml")
            .to_string_lossy()
            .into_owned()
    }

    fn dispatcher(&self) -> Dispatcher {
        let asked = self.asked.clone();
        Dispatcher::new(
            self.state(),
            SocketDir::new(self.temp.path().join("s")),
            PathBuf::from("/bin/false"),
            Duration::from_secs(300),
        )
        .with_jira(JiraService::new(
            Arc::new(move || Box::new(FakeJira(asked.clone()))),
            Arc::new(|_| None),
        ))
    }

    fn ask(
        &self,
        dispatcher: &Dispatcher,
        name: &str,
        params: Value,
    ) -> Result<Value, pom_rpc::RemoteError> {
        let mut params = params;
        params["cfg"] = json!(self.cfg());
        dispatcher.dispatch(&Request {
            method: name.to_string(),
            params,
        })
    }
}

#[test]
fn settings_are_kept_here_and_answers_never_carry_the_token() {
    let project = Project::new();
    let dispatcher = project.dispatcher();

    let blank: JiraSetup = serde_json::from_value(
        project
            .ask(&dispatcher, method::JIRA_SETTINGS, json!({}))
            .expect("settings"),
    )
    .expect("setup");
    assert!(!blank.ready());
    let unset = project
        .ask(
            &dispatcher,
            method::JIRA_SEARCH,
            json!({"keys": ["PROJ-101"]}),
        )
        .expect_err("not set up");
    assert_eq!(unset.code, FAILED);
    assert!(unset.message.contains("not set up"), "{}", unset.message);

    let saved = project
        .ask(
            &dispatcher,
            method::JIRA_SAVE,
            json!({"change": {"site": "example.atlassian.net", "email": "you@example.com", "token": TOKEN}}),
        )
        .expect("save");
    assert!(!saved.to_string().contains(TOKEN));
    let saved: JiraSetup = serde_json::from_value(saved).expect("setup");
    assert!(saved.ready());
    assert_eq!(saved.token, Some(TokenOrigin::Secret));
    assert_eq!(
        pom_jira::JiraSettings::load(&project.state(), "myproject").site,
        "example.atlassian.net"
    );

    let account: JiraAccount = serde_json::from_value(
        project
            .ask(&dispatcher, method::JIRA_TEST, json!({}))
            .expect("test"),
    )
    .expect("account");
    assert_eq!(account.name, "Dev");

    let found = project
        .ask(
            &dispatcher,
            method::JIRA_SEARCH,
            json!({"keys": ["PROJ-101"]}),
        )
        .expect("search");
    assert_eq!(found[0]["status"], "In Progress");
    let detail: IssueDetail = serde_json::from_value(
        project
            .ask(&dispatcher, method::JIRA_ISSUE, json!({"key": "PROJ-101"}))
            .expect("issue"),
    )
    .expect("detail");
    assert_eq!(detail.summary, "Login page");
    let boards = project
        .ask(&dispatcher, method::JIRA_BOARDS, json!({}))
        .expect("boards");
    assert_eq!(boards[0]["name"], "Web");
    let backlog: Vec<SprintIssue> = serde_json::from_value(
        project
            .ask(&dispatcher, method::JIRA_LIST, json!({"list": "backlog:7"}))
            .expect("list"),
    )
    .expect("issues");
    assert_eq!(backlog[0].key, "PROJ-102");

    let authorizations = project.asked.authorizations.lock().expect("asked").clone();
    assert!(!authorizations.is_empty());
    assert!(authorizations
        .iter()
        .all(|value| value.starts_with("Basic ")));

    let cleared: JiraSetup = serde_json::from_value(
        project
            .ask(
                &dispatcher,
                method::JIRA_SAVE,
                json!({"change": {"clear_token": true}}),
            )
            .expect("clear"),
    )
    .expect("setup");
    assert_eq!(cleared.token, None);
    assert_eq!(cleared.site, "example.atlassian.net");
}

#[test]
fn keys_and_lists_are_checked_before_jira_is_asked() {
    let project = Project::new();
    let dispatcher = project.dispatcher();
    project
        .ask(
            &dispatcher,
            method::JIRA_SAVE,
            json!({"change": {"site": "example.atlassian.net", "email": "you@example.com", "token": TOKEN}}),
        )
        .expect("save");
    for (name, params) in [
        (method::JIRA_ISSUE, json!({"key": "../../myself"})),
        (method::JIRA_ISSUE, json!({"key": "PROJ-101?expand=all"})),
        (method::JIRA_SEARCH, json!({"keys": ["PROJ-1) OR (1=1"]})),
        (method::JIRA_LIST, json!({"list": "board:../x"})),
    ] {
        let refused = project.ask(&dispatcher, name, params).expect_err("refused");
        assert_eq!(refused.code, error_code::INVALID_PARAMS, "{name}");
    }
    let too_many: Vec<String> = (0..101).map(|index| format!("PROJ-{index}")).collect();
    let refused = project
        .ask(
            &dispatcher,
            method::JIRA_SEARCH,
            json!({ "keys": too_many }),
        )
        .expect_err("too many");
    assert_eq!(refused.code, error_code::INVALID_PARAMS);
    assert!(project.asked.urls.lock().expect("asked").is_empty());
    let elsewhere = dispatcher
        .dispatch(&Request {
            method: method::JIRA_SETTINGS.into(),
            params: json!({"cfg": "/etc/pom.yml"}),
        })
        .expect_err("not a project here");
    assert_eq!(elsewhere.code, error_code::INVALID_PARAMS);
}
