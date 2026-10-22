//! The methods against a scratch project registered in a temp state dir: answers equal what `LocalSource`
//! reads, and parameters are refused before anything runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_remote::server_api::{method, op_state, AgentListing, OpResult};
use pom_remote::{LocalSource, ProjectSource, StatusReport, WorkspaceList};
use pom_rpc::{error_code, Request};
use pom_server::{check_branch, check_name, check_op_id, registered_config, Dispatcher, Ops};
use serde_json::{json, Value};

struct Project {
    temp: tempfile::TempDir,
}

impl Project {
    fn new() -> Project {
        let temp = tempfile::Builder::new()
            .prefix("poms")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web:\n        type: backend\n        cmd: \"exec sleep 60\"\n",
        )
        .expect("pom.yml");
        let project = Project { temp };
        let mut sessions = pom_sessions::Sessions::load(&project.state());
        sessions.touch("demo", &root.to_string_lossy(), 10);
        sessions.save(&project.state()).expect("sessions");
        project
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state"))
    }

    fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    fn config(&self) -> String {
        self.temp
            .path()
            .join("project/pom.yml")
            .to_string_lossy()
            .into_owned()
    }

    fn dispatcher(&self) -> Dispatcher {
        Dispatcher::new(
            self.state(),
            self.holders(),
            PathBuf::from("/bin/false"),
            Duration::from_secs(300),
        )
    }

    fn local(&self) -> LocalSource {
        LocalSource::new(
            PathBuf::from(self.config()),
            self.state(),
            self.holders(),
            PathBuf::from("/bin/false"),
        )
    }
}

fn request(name: &str, params: Value) -> Request {
    Request {
        method: name.to_string(),
        params,
    }
}

#[test]
fn reads_answer_what_the_local_source_reads() {
    let project = Project::new();
    let dispatcher = project.dispatcher();
    let local = project.local();
    let cfg = project.config();

    let listed: WorkspaceList = serde_json::from_value(
        dispatcher
            .dispatch(&request(method::WS_LIST, json!({ "cfg": cfg })))
            .expect("ws.list"),
    )
    .expect("list");
    assert_eq!(listed, local.workspaces().expect("local"));
    assert_eq!(listed.workspaces.len(), 2);

    let status: StatusReport = serde_json::from_value(
        dispatcher
            .dispatch(&request(
                method::WS_STATUS,
                json!({ "cfg": cfg, "branch": "feat" }),
            ))
            .expect("ws.status"),
    )
    .expect("status");
    assert_eq!(status, local.status("feat").expect("local status"));

    let agents: AgentListing = serde_json::from_value(
        dispatcher
            .dispatch(&request(
                method::AGENT_LIST,
                json!({ "cfg": cfg, "branch": "feat" }),
            ))
            .expect("agent.list"),
    )
    .expect("agents");
    assert_eq!(agents.schema, "pom.agent/v1");
    assert_eq!(agents.sessions, local.agents("feat").expect("local agents"));

    let projects = dispatcher
        .dispatch(&request(method::PROJECT_LIST, Value::Null))
        .expect("project.list");
    assert_eq!(projects["schema"], "pom.project/v1");
    assert_eq!(projects["projects"][0]["name"], "demo");
    assert_eq!(projects["projects"][0]["config"], cfg.as_str());

    let missing = dispatcher
        .dispatch(&request(
            method::WS_STATUS,
            json!({ "cfg": cfg, "branch": "nope" }),
        ))
        .expect_err("no such workspace");
    assert_eq!(missing.code, pom_server::FAILED);
    assert!(missing.message.contains("no workspace nope"), "{missing:?}");

    let logs = dispatcher
        .dispatch(&request(
            method::SVC_LOGS,
            json!({ "cfg": cfg, "branch": "feat", "service": "api/web" }),
        ))
        .expect_err("not running");
    assert_eq!(logs.code, pom_server::FAILED);
}

#[test]
fn bad_parameters_are_refused_before_anything_runs() {
    let project = Project::new();
    let dispatcher = project.dispatcher();
    let cfg = project.config();
    let refused = |name: &str, params: Value| {
        let error = dispatcher
            .dispatch(&request(name, params.clone()))
            .expect_err(&format!("{name} {params}"));
        assert_eq!(error.code, error_code::INVALID_PARAMS, "{name} {params}");
    };
    refused(method::WS_LIST, json!({ "cfg": "/etc/passwd" }));
    refused(method::WS_LIST, json!({ "cfg": "project/pom.yml" }));
    refused(
        method::WS_LIST,
        json!({ "cfg": format!("{cfg}/../pom.yml") }),
    );
    refused(method::WS_LIST, json!({ "cfg": 7 }));
    refused(
        method::WS_STATUS,
        json!({ "cfg": cfg, "branch": "--upload-pack=x" }),
    );
    refused(method::WS_STATUS, json!({ "cfg": cfg, "branch": "a..b" }));
    refused(
        method::SVC_LOGS,
        json!({ "cfg": cfg, "branch": "feat", "service": "../../etc" }),
    );
    refused(
        method::AGENT_TURN,
        json!({ "cfg": cfg, "branch": "feat", "role": "dev; rm -rf /" }),
    );
    refused(
        method::SVC_ACT,
        json!({ "cfg": cfg, "branch": "feat", "action": "explode", "target": "web", "op_id": "a-1" }),
    );
    refused(
        method::SVC_ACT,
        json!({ "cfg": cfg, "branch": "feat", "action": "start", "target": "web", "op_id": "" }),
    );
    refused(method::OP_RESULT, json!({ "op_id": "a b" }));

    let unknown = dispatcher
        .dispatch(&request("fs.write", json!({})))
        .expect_err("unknown");
    assert_eq!(unknown.code, error_code::METHOD_NOT_FOUND);
}

#[test]
fn validators() {
    let project = Project::new();
    let cfg = project.config();
    assert_eq!(
        registered_config(&project.state(), &cfg).expect("registered"),
        PathBuf::from(&cfg)
    );
    let other = project.temp.path().join("other/pom.yml");
    std::fs::create_dir_all(other.parent().expect("parent")).expect("dir");
    std::fs::write(&other, "session: other\n").expect("other");
    assert!(registered_config(&project.state(), &other.to_string_lossy()).is_err());

    for good in ["main", "feat/login-2", "release-1.2"] {
        assert!(check_branch(good).is_ok(), "{good}");
    }
    for bad in ["", "-x", "a..b", "a b", "x~1", &"b".repeat(300)] {
        assert!(check_branch(bad).is_err(), "{bad}");
    }
    for good in ["web", "api/web", "backend.v2", "dev_1"] {
        assert!(check_name("service", good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "/web",
        "-web",
        "a..b",
        "web;id",
        "web web",
        &"w".repeat(200),
    ] {
        assert!(check_name("service", bad).is_err(), "{bad}");
    }
    assert!(check_op_id("1234-abc-7").is_ok());
    assert!(check_op_id("").is_err());
    assert!(check_op_id("a/b").is_err());
}

#[test]
fn an_op_runs_once_and_its_result_is_remembered() {
    let ops = Arc::new(Ops::new(Duration::from_secs(300)));
    let runs = Arc::new(AtomicUsize::new(0));
    let unknown = ops.result("op-1");
    assert_eq!(unknown.state, op_state::UNKNOWN);

    let slow = {
        let (ops, runs) = (ops.clone(), runs.clone());
        std::thread::spawn(move || {
            ops.run("op-1", || {
                runs.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
                Ok("started api/web".to_string())
            })
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while ops.result("op-1").state != op_state::RUNNING {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let again = ops.run("op-1", || {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok("ran twice".to_string())
    });
    assert_eq!(again.as_deref(), Ok("started api/web"));
    assert_eq!(
        slow.join().expect("thread").as_deref(),
        Ok("started api/web")
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        ops.result("op-1"),
        OpResult {
            state: op_state::DONE.into(),
            said: Some("started api/web".into()),
            error: None,
        }
    );

    let failed = ops.run("op-2", || Err("api/web: port taken".to_string()));
    assert!(failed.is_err());
    assert_eq!(
        ops.result("op-2").error.as_deref(),
        Some("api/web: port taken")
    );

    let forgetful = Ops::new(Duration::ZERO);
    assert!(forgetful.run("op-3", || Ok(String::new())).is_ok());
    assert_eq!(forgetful.result("op-3").state, op_state::UNKNOWN);
}
