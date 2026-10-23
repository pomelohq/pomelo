//! Driving an agent session end to end through the real `pom` binary: a scripted fake agent runs in a real
//! holder and reports through the same hook command a real agent's hooks call. No real agent runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use pom_ptyhost::{SocketDir, SpawnRequest};
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-agent.sh");
const HOLDER: &str = "ws-demo-feat-claude-reviewer";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    /// A project with a `feat` workspace whose `reviewer` session is the fake agent.
    fn new() -> Fixture {
        Fixture::with_env(&[])
    }

    fn with_env(extra: &[(&str, &str)]) -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomdrv")
            .tempdir_in("/tmp")
            .expect("temp");
        let fixture = Fixture { temp };
        let root = fixture.root();
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        let env: Vec<(String, String)> = [
            (
                "XDG_STATE_HOME",
                fixture
                    .temp
                    .path()
                    .join("state")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("POM", POM.to_string()),
            (
                "FAKE_RELEASE",
                fixture
                    .temp
                    .path()
                    .join("release")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("FAKE_SESSION", "s1".to_string()),
            (
                "FAKE_TRANSCRIPT",
                fixture
                    .temp
                    .path()
                    .join("s1.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("POM_AGENT_HOLDER", HOLDER.to_string()),
            ("POM_AGENT_ROLE", "reviewer".to_string()),
            ("POM_PROJECT_SESSION", "demo".to_string()),
            ("POM_WORKSPACE", "feat".to_string()),
            ("POM_AGENT_DRIVER", "claude".to_string()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .chain(
            extra
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        )
        .collect();
        let argv = vec!["/bin/sh".to_string(), FAKE_AGENT.to_string()];
        pom_ptyhost::spawn_holder(
            &fixture.holders(),
            &SpawnRequest {
                group: None,
                binary: Path::new(POM),
                name: HOLDER,
                cwd: &root.join("workspace--feat"),
                cols: 120,
                rows: 40,
                argv: &argv,
                env: &env,
            },
        )
        .expect("holder");
        pom_ptyhost::wait_for_holder(&fixture.holders(), HOLDER, Duration::from_secs(5))
            .expect("socket");
        fixture.until("the fake agent to report", |fixture| {
            fixture.json(&["agent", "ls", "feat", "--json"])["sessions"][0]["session_id"] == "s1"
        });
        fixture
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    fn pom(&self, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env_remove("POM_AGENT_HOLDER")
            .output()
            .expect("pom")
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.pom(args);
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    fn until(&self, what: &str, done: impl Fn(&Fixture) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let holders = self.holders();
        let names: Vec<String> = holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        holders.kill_holders_now(&names);
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "not JSON: {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or(Value::Null)
}

#[test]
fn ask_sends_one_turn_waits_for_its_stop_and_returns_what_the_agent_did() {
    let fixture = Fixture::new();
    let refused = fixture.pom(&["agent", "send", "feat/reviewer", "hello", "--json"]);
    assert_eq!(
        refused.status.code(),
        Some(6),
        "a person's session is not an orchestrator's to type into"
    );
    assert_eq!(stderr_json(&refused)["error"], "lease");

    let output = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "hello",
        "--take",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer = stdout_json(&output);
    assert_eq!(answer["turn"], 1);
    assert_eq!(answer["wait"]["reached"], "turn-end");
    assert_eq!(answer["wait"]["stop_reason"], "end_turn");
    assert_eq!(answer["result"]["prompt"], "hello");
    assert_eq!(answer["result"]["origin"], "orchestrator");
    assert_eq!(answer["result"]["items"][0]["text"], "echo: hello");
    assert_eq!(answer["result"]["usage"]["output_tokens"], 2);

    let second = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "again",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "the orchestrator keeps its lease"
    );
    assert_eq!(stdout_json(&second)["turn"], 2);
}

#[test]
fn wait_exits_3_when_the_agent_asks_for_permission() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "send",
        "feat/reviewer",
        "needs-permission",
        "--take",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(3));
    assert_eq!(stdout_json(&waited)["reached"], "awaiting_input");
}

#[test]
fn wait_exits_4_with_the_cause_when_the_agent_dies_mid_turn() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "send",
        "feat/reviewer",
        "crash",
        "--take",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(4));
    let outcome = stdout_json(&waited);
    assert_eq!(outcome["reached"], "died");
    assert_eq!(outcome["cause"], "crashed");
    let listed = fixture.json(&["agent", "ls", "feat", "--json"]);
    assert_eq!(listed["sessions"][0]["state"], "died");
}

#[test]
fn interrupt_cancels_the_running_turn() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&["agent", "send", "feat/reviewer", "long", "--take", "--json"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let interrupted = fixture.pom(&["agent", "interrupt", "feat/reviewer", "--json"]);
    assert_eq!(
        interrupted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&interrupted.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "10s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(0));
    assert_eq!(stdout_json(&waited)["stop_reason"], "cancelled");
}

#[test]
fn takeover_and_release_move_the_lease_and_say_so_on_watch() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "hello",
        "--take",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(
        fixture.holders().input_lease(HOLDER).is_some(),
        "the holder only takes the orchestrator's keys"
    );

    let taken = fixture.pom(&["agent", "takeover", "feat", "--session-id", "s1", "--json"]);
    assert_eq!(
        taken.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    assert_eq!(
        fixture.holders().input_lease(HOLDER),
        None,
        "a person types freely"
    );
    let refused = fixture.pom(&["agent", "send", "feat/reviewer", "more", "--json"]);
    assert_eq!(
        stderr_json(&refused)["error"],
        "lease",
        "the orchestrator waits while a person has it"
    );

    let released = fixture.pom(&["agent", "release", "feat/reviewer", "--json"]);
    assert_eq!(
        released.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    assert_eq!(stdout_json(&released)["lease"], "orchestrator");
    assert!(fixture.holders().input_lease(HOLDER).is_some());

    let watched = fixture.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
    let changes: Vec<(String, String)> = String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "lease")
        .map(|line| {
            (
                line["from_class"].as_str().unwrap_or_default().to_string(),
                line["to_class"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        [
            ("human".to_string(), "orchestrator".to_string()),
            ("orchestrator".to_string(), "human".to_string()),
            ("human".to_string(), "orchestrator".to_string()),
        ]
    );
    let messages = String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter(|line| line.contains("\"event\":\"message\""))
        .count();
    assert_eq!(messages, 1, "every turn sent is logged once");
}

impl Fixture {
    fn state_file(&self) -> PathBuf {
        self.temp
            .path()
            .join("state/pom/agents/sessions/s1/state.json")
    }

    fn reviewer(&self) -> Value {
        self.json(&["agent", "ls", "feat", "--json"])["sessions"][0].clone()
    }

    /// Rewrites state.json the way a version that classified the idle reminder as a permission prompt left
    /// it, an event two hours old and no `reason`.
    fn age_into_legacy_idle_reminder(&self) {
        let path = self.state_file();
        let mut record: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("state")).expect("json");
        let record = record.as_object_mut().expect("object");
        record.remove("reason");
        record.insert("state".into(), "awaiting_input".into());
        record.insert("last_event".into(), "Notification".into());
        record.insert("turn_state".into(), "ended".into());
        let two_hours_ago = pom_agent::now_ms() - 2 * 60 * 60 * 1000;
        record.insert("event_ms".into(), two_hours_ago.into());
        std::fs::write(&path, Value::Object(record.clone()).to_string()).expect("write");
    }

    fn send(&self, text: &str, extra: &[&str]) -> Output {
        let mut args = vec!["agent", "send", "feat/reviewer", text, "--json"];
        args.extend_from_slice(extra);
        self.pom(&args)
    }
}

#[test]
fn the_idle_reminder_leaves_a_session_idle_and_sendable() {
    let fixture = Fixture::new();
    let sent = fixture.send("idle-reminder", &["--take"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    fixture.until("the idle reminder", |fixture| {
        fixture.reviewer()["last_event"] == "Notification"
    });
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "idle");
    assert_eq!(reviewer["reason"], "idle_notification");
    let next = fixture.send("hello", &[]);
    assert_eq!(
        next.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
}

#[test]
fn a_session_idle_for_hours_under_an_old_state_file_takes_send_and_queue_at_once() {
    let fixture = Fixture::new();
    let sent = fixture.send("hello", &["--take"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    fixture.until("the turn to end", |fixture| {
        fixture.reviewer()["state"] == "idle"
    });

    fixture.age_into_legacy_idle_reminder();
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "idle");
    assert_eq!(reviewer["reason"], "idle_notification");
    assert!(reviewer["last_event_age_s"].as_u64().unwrap_or(0) > 3600);
    let plain = fixture.send("again", &[]);
    assert_eq!(
        plain.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    fixture.until("the turn to end", |fixture| {
        fixture.reviewer()["last_event"] == "Stop"
    });

    fixture.age_into_legacy_idle_reminder();
    let started = Instant::now();
    let queued = fixture.send("queued", &["--queue"]);
    assert_eq!(
        queued.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&queued.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the queue does not wait on an idle session"
    );
}

#[test]
fn a_pending_approval_still_refuses_a_send_and_a_queue_says_what_it_waited_on() {
    let fixture = Fixture::new();
    let sent = fixture.send("needs-permission", &["--take"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    fixture.until("the permission prompt", |fixture| {
        fixture.reviewer()["state"] == "awaiting_input"
    });
    assert_eq!(fixture.reviewer()["reason"], "pending_approval");

    let refused = fixture.send("hello", &[]);
    assert_eq!(refused.status.code(), Some(6));
    let error = stderr_json(&refused);
    assert_eq!(error["error"], "busy");
    assert_eq!(error["reason"], "pending_approval");

    let queued = fixture.send("hello", &["--queue", "--timeout", "1s"]);
    assert_eq!(queued.status.code(), Some(2));
    let error = stderr_json(&queued);
    assert_eq!(error["error"], "timeout");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("pending_approval")),
        "{error}"
    );
}

impl Fixture {
    /// Runs one hook the way the agent's own hooks do, with a payload shaped like the agent's.
    fn hook(&self, event: &str, extra: Value) {
        let mut payload = serde_json::json!({
            "session_id": "s1",
            "transcript_path": self.temp.path().join("s1.jsonl"),
            "cwd": self.root().join("workspace--feat"),
            "permission_mode": "default",
            "hook_event_name": event,
        });
        if let (Some(payload), Value::Object(extra)) = (payload.as_object_mut(), extra) {
            payload.extend(extra);
        }
        let mut child = Command::new(POM)
            .args(["claude-hook", "--session"])
            .current_dir(self.root().join("workspace--feat"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("POM_AGENT_HOLDER", HOLDER)
            .env("POM_AGENT_ROLE", "reviewer")
            .env("POM_PROJECT_SESSION", "demo")
            .env("POM_WORKSPACE", "feat")
            .env("POM_AGENT_DRIVER", "claude")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("hook");
        std::io::Write::write_all(
            child.stdin.as_mut().expect("stdin"),
            payload.to_string().as_bytes(),
        )
        .expect("payload");
        drop(child.stdin.take());
        assert!(child.wait_with_output().expect("hook").status.success());
    }

    /// Types a turn into the agent's terminal the way a person at the app does: no lease claimed.
    fn type_as_person(&self, text: &str) {
        let mut connection = pom_ptyhost::connect_writer(&self.holders(), HOLDER).expect("connect");
        connection
            .input(format!("\x1b[200~{text}\x1b[201~").as_bytes())
            .expect("paste");
        std::thread::sleep(Duration::from_millis(60));
        connection.input(b"\r").expect("enter");
    }

    fn wait(&self, extra: &[&str]) -> (Option<i32>, Value) {
        let mut args = vec!["agent", "wait", "feat/reviewer", "--json"];
        args.extend_from_slice(extra);
        let output = self.pom(&args);
        (output.status.code(), stdout_json(&output))
    }
}

#[test]
fn ls_says_whether_a_turn_runs_and_its_phase() {
    let fixture = Fixture::new();
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "idle");
    assert_eq!(reviewer["running"], false);
    assert_eq!(reviewer["phase"], "idle");
    let sent = fixture.send("long", &["--take"]);
    assert_eq!(sent.status.code(), Some(0));
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "thinking");
    assert_eq!(reviewer["running"], true);
    assert_eq!(reviewer["phase"], "working");
}

#[test]
fn ls_right_after_send_shows_the_turn_running_through_a_late_idle_reminder() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::Builder::new()
        .prefix("pomdrvbin")
        .tempdir_in("/tmp")
        .expect("bin");
    let real_agent = bin.path().join("claude");
    std::fs::write(&real_agent, "#!/bin/sh\ntouch \"$0.ran\"\nexit 97\n").expect("stub");
    std::fs::set_permissions(&real_agent, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let fixture = Fixture::with_env(&[("PATH", &path)]);

    let sent = fixture.send("late-reminder", &["--take"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["running"], true, "{reviewer}");
    assert_eq!(reviewer["phase"], "working", "{reviewer}");

    let log = fixture
        .temp
        .path()
        .join("state/pom/agents/sessions/s1/events.ndjson");
    fixture.until("the stale idle reminder", |_| {
        std::fs::read_to_string(&log).is_ok_and(|text| text.contains("\"Notification\""))
    });
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "thinking", "{reviewer}");
    assert_eq!(reviewer["running"], true, "{reviewer}");

    fixture.hook("Stop", serde_json::json!({}));
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "idle", "{reviewer}");
    assert_eq!(reviewer["reason"], "stop", "{reviewer}");
    assert_eq!(reviewer["running"], false, "{reviewer}");

    let logged = std::fs::read_to_string(&log).expect("events");
    assert!(
        logged.contains("\"SessionStart\"") && logged.contains("\"UserPromptSubmit\""),
        "the stand-in agent reported: {logged}"
    );
    assert!(
        !bin.path().join("claude.ran").exists(),
        "the real agent CLI never runs"
    );
}

#[test]
fn turn_end_on_an_idle_session_waits_for_the_next_turn_not_the_last_one() {
    let fixture = Fixture::new();
    fixture.type_as_person("hello");
    fixture.until("the person's turn to end", |fixture| {
        fixture.reviewer()["last_event"] == "Stop"
    });
    let (code, outcome) = fixture.wait(&["--timeout", "2s"]);
    assert_eq!(
        code,
        Some(2),
        "the person's finished turn is not what a later wait waits for: {outcome}"
    );
    assert_eq!(outcome["reached"], "timeout");
    assert_eq!(outcome["for_turn"], 2);

    let (code, outcome) = fixture.wait(&["--turn", "1", "--timeout", "2s"]);
    assert_eq!(code, Some(0), "{outcome}");
    assert_eq!(
        (outcome["reached"].as_str(), outcome["for_turn"].as_u64()),
        (Some("turn-end"), Some(1))
    );

    let sent = fixture.send("again", &["--as-person"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let (code, outcome) = fixture.wait(&["--timeout", "10s"]);
    assert_eq!(code, Some(0), "{outcome}");
    assert_eq!(
        outcome["for_turn"], 2,
        "the turn the last send started counts once it ended"
    );
    let (code, _) = fixture.wait(&["--turn", "3", "--timeout", "1s"]);
    assert_eq!(code, Some(2), "turn 3 never started");
}

#[test]
fn turn_end_after_a_reopen_waits_for_a_new_turn() {
    let fixture = Fixture::new();
    let sent = fixture.send("hello", &["--take"]);
    assert_eq!(sent.status.code(), Some(0));
    let (code, outcome) = fixture.wait(&["--timeout", "10s"]);
    assert_eq!(code, Some(0), "{outcome}");
    assert_eq!(outcome["for_turn"], 1);

    fixture.hook("SessionStart", serde_json::json!({"source": "compact"}));
    let (code, outcome) = fixture.wait(&["--timeout", "2s"]);
    assert_eq!(
        (code, outcome["for_turn"].as_u64()),
        (Some(0), Some(1)),
        "compacting is not a reopen"
    );

    fixture.hook("SessionStart", serde_json::json!({"source": "resume"}));
    let (code, outcome) = fixture.wait(&["--timeout", "2s"]);
    assert_eq!(
        code,
        Some(2),
        "a resumed session's old turn is over: {outcome}"
    );
    assert_eq!(outcome["for_turn"], 2);
    let (code, outcome) = fixture.wait(&["--turn", "1", "--timeout", "2s"]);
    assert_eq!((code, outcome["for_turn"].as_u64()), (Some(0), Some(1)));
}

#[test]
fn the_person_who_drives_may_send_from_another_client_without_moving_the_lease() {
    let fixture = Fixture::new();
    let plain = fixture.send("hello", &[]);
    assert_eq!(stderr_json(&plain)["error"], "lease");
    let both = fixture.send("hello", &["--take", "--as-person"]);
    assert_eq!(both.status.code(), Some(2), "a usage error");

    let sent = fixture.send("hello", &["--as-person"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert_eq!(
        fixture.holders().input_lease(HOLDER),
        None,
        "still the person's"
    );
    fixture.until("the turn to end", |fixture| {
        fixture.reviewer()["last_event"] == "Stop"
    });
    let read = fixture.json(&["agent", "read", "feat/reviewer", "--json"]);
    assert_eq!(read["turns"][0]["origin"], "human");
    assert_eq!(read["turns"][0]["items"][0]["text"], "echo: hello");

    let taken = fixture.send("mine", &["--take"]);
    assert_eq!(taken.status.code(), Some(0));
    fixture.until("the turn to end", |fixture| {
        fixture.reviewer()["last_event"] == "Stop"
    });
    let refused = fixture.send("hello", &["--as-person"]);
    assert_eq!(refused.status.code(), Some(6));
    let error = stderr_json(&refused);
    assert_eq!(error["error"], "lease");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("an orchestrator")),
        "{error}"
    );
}

impl Fixture {
    fn pending(&self) -> Value {
        self.reviewer()["pending_permission"].clone()
    }

    fn fallback_log(&self) -> Vec<Value> {
        let watched = self.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
        String::from_utf8_lossy(&watched.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["event"] == "screen_fallback")
            .collect()
    }

    fn answer(&self, verb: &str, extra: &[&str]) -> Output {
        let mut args = vec!["agent", verb, "feat/reviewer", "--json"];
        args.extend_from_slice(extra);
        self.pom(&args)
    }

    fn last_text(&self) -> Value {
        let read = self.json(&["agent", "read", "feat/reviewer", "--json"]);
        read["turns"][0]["items"][0]["text"].clone()
    }
}

#[test]
fn an_orchestrator_answers_the_agents_own_prompt_while_pom_holds_it() {
    let fixture = Fixture::new();
    let sent = fixture.send("ask-permission", &["--take"]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the held prompt", |fixture| {
        fixture.pending()["answer_by"] == "hook"
    });
    let pending = fixture.pending();
    assert_eq!(pending["kind"], "agent");
    assert_eq!(pending["tool_name"], "Bash");
    assert_eq!(pending["tool_input"]["command"], "make test");
    assert_eq!(pending["summary"], "make test");
    let id = pending["id"].as_str().expect("id").to_string();
    let reviewer = fixture.reviewer();
    assert_eq!(
        (reviewer["state"].as_str(), reviewer["reason"].as_str()),
        (Some("awaiting_input"), Some("pending_approval"))
    );
    assert_eq!(reviewer["running"], true);
    let watched = fixture.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
    assert!(String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(
            |line| line["event"] == "PermissionRequest" && line["detail"]["request"] == id.as_str()
        ));

    let stale = fixture.answer("allow", &["--id", "0123456789ab"]);
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(stderr_json(&stale)["error"], "stale");

    let allowed = fixture.answer("allow", &["--id", &id]);
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    assert_eq!(stdout_json(&allowed)["via"], "hook");
    let (code, _) = fixture.wait(&["--timeout", "10s"]);
    assert_eq!(code, Some(0));
    assert_eq!(fixture.last_text(), "ran make test");
    assert_eq!(fixture.pending(), Value::Null);
    let again = fixture.answer("allow", &["--id", &id]);
    assert_eq!(
        stderr_json(&again)["error"],
        "stale",
        "an answered prompt takes no second answer"
    );

    let sent = fixture.send("ask-permission again", &[]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the next held prompt", |fixture| {
        fixture.pending()["answer_by"] == "hook"
    });
    let denied = fixture.answer("deny", &["--reason", "not now"]);
    assert_eq!(
        denied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let (code, _) = fixture.wait(&["--timeout", "10s"]);
    assert_eq!(code, Some(0));
    assert_eq!(fixture.last_text(), "not allowed: not now");
}

#[test]
fn taking_over_lets_go_of_a_held_prompt_and_the_person_answers_its_dialog() {
    let fixture = Fixture::new();
    let sent = fixture.send("ask-permission", &["--take"]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the held prompt", |fixture| {
        fixture.pending()["answer_by"] == "hook"
    });
    let taken = fixture.pom(&["agent", "takeover", "feat", "--session-id", "s1", "--json"]);
    assert_eq!(taken.status.code(), Some(0));
    fixture.until("the dialog", |fixture| {
        fixture.pending()["answer_by"] == "terminal"
    });
    let mut connection = pom_ptyhost::connect_writer(&fixture.holders(), HOLDER).expect("connect");
    connection.input(b"\r").expect("enter");
    fixture.until(
        "the person at the terminal to answer as before",
        |fixture| fixture.reviewer()["last_event"] == "Stop",
    );
    assert_eq!(fixture.last_text(), "ran make test");
}

#[test]
fn pom_only_denies_the_agents_dialog_and_never_while_a_person_drives() {
    fallback_answers_the_dialog(Fixture::new());
}

#[test]
fn pom_only_denies_the_older_dialog_with_its_long_deny_choice() {
    fallback_answers_the_dialog(Fixture::with_env(&[("FAKE_DIALOG", "old")]));
}

fn fallback_answers_the_dialog(fixture: Fixture) {
    let sent = fixture.send("ask-permission", &["--as-person"]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the dialog", |fixture| {
        fixture.pending()["answer_by"] == "terminal"
    });
    for flags in [&[][..], &["--as-person"], &["--take"]] {
        let refused = fixture.answer("allow", flags);
        assert_eq!(refused.status.code(), Some(1));
        assert_eq!(
            stderr_json(&refused)["error"],
            "allow_needs_hook",
            "an allow is never typed, {flags:?}"
        );
    }
    let refused = fixture.answer("deny", &["--as-person"]);
    assert_eq!(
        stderr_json(&refused)["error"],
        "person_drives",
        "nothing is typed for the person who drives it"
    );
    assert_eq!(fixture.pending()["answer_by"], "terminal", "still waits");

    let mut connection = pom_ptyhost::connect_writer(&fixture.holders(), HOLDER).expect("connect");
    connection.input(b"\r").expect("enter");
    fixture.until("the person at the terminal to allow it", |fixture| {
        fixture.reviewer()["last_event"] == "Stop"
    });
    assert_eq!(fixture.last_text(), "ran make test");

    let sent = fixture.send("ask-permission again", &["--as-person"]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the next dialog", |fixture| {
        fixture.pending()["answer_by"] == "terminal"
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let denied = loop {
        let output = fixture.answer("deny", &["--take"]);
        if output.status.code() == Some(0) || Instant::now() > deadline {
            break output;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(
        denied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    assert_eq!(stdout_json(&denied)["via"], "terminal");
    let reviewer = fixture.reviewer();
    assert_eq!(
        (reviewer["state"].as_str(), reviewer["reason"].as_str()),
        (Some("idle"), Some("interrupted")),
        "the dialog's own cancel key stops the turn"
    );
    assert_eq!(fixture.pending(), Value::Null);

    let logged = fixture.fallback_log();
    let actions: Vec<(&str, &str, bool)> = logged
        .iter()
        .map(|line| {
            (
                line["action"].as_str().unwrap_or_default(),
                line["sent"].as_str().unwrap_or("-"),
                line["skipped"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        actions
            .iter()
            .filter(|action| action.0 == "permission_allow")
            .count(),
        3
    );
    assert!(actions.contains(&("permission_deny", "-", true)));
    assert_eq!(
        actions.last(),
        Some(&("permission_deny", "esc", false)),
        "{actions:?}"
    );
    let typed = logged.last().expect("typed");
    assert!(
        typed["seen"]
            .as_array()
            .is_some_and(|seen| seen.iter().any(|line| line == "Do you want to proceed?")),
        "the log keeps the dialog it acted on: {typed}"
    );
}

#[test]
fn read_live_and_watch_live_show_the_reply_while_it_is_written() {
    let fixture = Fixture::new();
    let sent = fixture.send("stream", &["--take"]);
    assert_eq!(sent.status.code(), Some(0));
    fixture.until("the first lines of the reply", |fixture| {
        fixture.json(&["agent", "read", "feat/reviewer", "--live", "--json"])["live"]["text"]
            == "Working on it:"
    });
    let read = fixture.json(&["agent", "read", "feat/reviewer", "--live", "--json"]);
    assert_eq!(read["running"], true);
    assert_eq!(read["phase"], "working");
    assert_eq!(read["live"]["turn"], 1);
    assert_eq!(read["live"]["messages"][0]["final"], false);
    assert_eq!(
        read["turns"][0]["items"],
        serde_json::json!([]),
        "the transcript has none of it yet"
    );

    std::fs::write(fixture.temp.path().join("release"), "").expect("release");
    let watched = fixture.pom(&[
        "agent",
        "watch",
        "feat",
        "--role",
        "reviewer",
        "--live",
        "--until",
        "turn-end",
        "--from-start",
        "--timeout",
        "20s",
    ]);
    let lines: Vec<Value> = String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let deltas: Vec<(&str, bool)> = lines
        .iter()
        .filter(|line| line["event"] == "live_text")
        .map(|line| {
            (
                line["delta"].as_str().unwrap_or_default(),
                line["final"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(deltas, [("Working on it:\n", false), ("done", true)]);
    let end = lines.last().expect("lines");
    assert_eq!(
        (
            end["event"].as_str(),
            end["reached"].as_str(),
            end["for_turn"].as_u64()
        ),
        (Some("watch_end"), Some("turn-end"), Some(1))
    );
    assert!(lines.iter().any(|line| line["event"] == "Stop"
        && line["running"] == false
        && line["phase"] == "idle"));
}

#[test]
fn send_never_presses_its_second_enter_into_a_dialog_drawn_while_the_prompt_hook_is_slow() {
    let fixture = Fixture::new();
    let sent = fixture.send("slow-dialog", &["--as-person"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert_eq!(stdout_json(&sent)["turn"], 1);
    fixture.until("the dialog", |fixture| {
        fixture.pending()["answer_by"] == "terminal"
    });
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        fixture.pending()["answer_by"],
        "terminal",
        "the dialog still waits"
    );
    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["reason"], "pending_approval");
    assert_ne!(reviewer["last_event"], "PostToolUse");
    let transcript =
        std::fs::read_to_string(fixture.temp.path().join("s1.jsonl")).expect("transcript");
    assert!(
        transcript.contains("slow-dialog") && !transcript.contains("ran make test"),
        "the tool did not run: {transcript}"
    );
}

#[test]
fn a_prompt_left_in_the_input_box_is_submitted_by_exactly_one_more_enter() {
    let fixture = Fixture::new();
    let sent = fixture.send("lost-enter", &["--take"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert_eq!(stdout_json(&sent)["turn"], 1);
    let (code, _) = fixture.wait(&["--timeout", "10s"]);
    assert_eq!(code, Some(0));
    assert_eq!(fixture.last_text(), "echo: lost-enter");
    let enters = std::fs::read_to_string(fixture.temp.path().join("s1.jsonl.enters"))
        .expect("the stand-in counted the Enters it got after the first");
    assert_eq!(enters, "1");
    assert_eq!(fixture.reviewer()["turn"], 1);
    let logged = fixture.fallback_log();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["action"], "submit_retry");
    assert_eq!(logged[0]["sent"], "enter");
}
