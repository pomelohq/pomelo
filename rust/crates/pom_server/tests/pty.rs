//! Terminals through a real server and real holders: open, attach, type, resize, detach, reattach by offset,
//! close; a vanished client leaves its holder running; input is never typed twice; bad parameters are refused.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::time::{Duration, Instant};

use pom_rpc::pty::{self, Exit, Kind, Mode};
use pom_rpc::{error_code, Error};
use serde_json::json;
use terminals::{Reader, Remote, TIMEOUT};

#[test]
fn a_shell_opens_types_resizes_detaches_reattaches_and_closes() {
    let remote = Remote::start(&["/bin/sh"]);
    let connection = &remote.connection;
    let holder = remote.open(connection);
    assert!(holder.starts_with("term-demo-feat-"), "{holder}");
    assert!(remote.holders().holder_alive(&holder));

    let terminal = remote.attach(connection, &holder);
    assert_eq!(terminal.attached.holder, holder);
    assert_eq!(remote.streams(), 1);
    let mut reader = Reader::new();
    terminal
        .control
        .input(b"echo typed-$((40 + 2)); pwd\n")
        .expect("input");
    assert!(reader.until(&terminal, "typed-42"), "{:?}", reader.text);
    let worktree = std::fs::canonicalize(remote.worktree()).expect("worktree");
    assert!(
        reader.until(&terminal, &worktree.to_string_lossy()),
        "the shell starts in the workspace: {:?}",
        String::from_utf8_lossy(&reader.text)
    );

    terminal.control.resize(100, 40).expect("resize");
    std::thread::sleep(Duration::from_millis(200));
    terminal.control.input(b"stty size\n").expect("input");
    assert!(reader.until(&terminal, "40 100"), "{:?}", reader.text);

    let printed = reader.next.expect("an offset");
    connection
        .request(
            pty::method::DETACH,
            json!({ "channel": terminal.control.channel() }),
        )
        .expect("detach");
    assert_eq!(reader.until_end(&terminal).as_deref(), Some("detached"));
    assert!(remote.wait_streams(0));
    assert!(remote.holders().holder_alive(&holder), "detach keeps it");

    let again = remote.attach(connection, &holder);
    assert_eq!(again.attached.start, 0, "a fresh attach replays everything");
    let mut replayed = Reader::new();
    assert!(replayed.until(&again, "typed-42"));

    let mut since = remote.attach_params(&holder);
    since.since = printed;
    let resumed = pty::Terminal::attach(connection, since, TIMEOUT).expect("resume");
    assert_eq!(resumed.attached.start, printed, "resumes at the offset");
    resumed
        .control
        .input(b"echo after-$((1 + 1))\n")
        .expect("input");
    let mut tail = Reader::new();
    assert!(tail.until(&resumed, "after-2"));
    assert!(
        !String::from_utf8_lossy(&tail.text).contains("typed-42"),
        "nothing before the offset comes again"
    );

    connection
        .request(
            pty::method::CLOSE,
            json!({ "cfg": remote.cfg(), "branch": "feat", "holder": holder }),
        )
        .expect("close");
    assert_eq!(tail.until_end(&resumed).as_deref(), Some("exit"));
    let exit: Exit = resumed.output.exit(TIMEOUT).expect("pty.exit");
    assert_eq!(exit.holder, holder);
    assert!(!remote.holders().holder_alive(&holder));
    assert!(remote.wait_streams(0));
}

#[test]
fn an_exit_is_pushed_and_ends_the_stream() {
    let remote = Remote::start(&["/bin/sh"]);
    let holder = remote.open(&remote.connection);
    let terminal = remote.attach(&remote.connection, &holder);
    terminal.control.input(b"exit 3\n").expect("input");
    let mut reader = Reader::new();
    assert_eq!(reader.until_end(&terminal).as_deref(), Some("exit"));
    let exit = terminal.output.exit(TIMEOUT).expect("pty.exit");
    assert!(exit.crashed, "a non-zero exit is a crash");
}

#[test]
fn a_ring_that_moved_on_replays_what_it_still_holds() {
    let remote = Remote::start(&["/bin/sh"]);
    let name = "term-demo-feat-flood-1";
    let script = format!(
        "stty -opost; head -c {} /dev/zero | tr '\\0' a; printf END; sleep 30",
        pom_ptyhost::RING_CAPACITY + 300_000
    );
    remote.spawn_holder(name, &["/bin/sh", "-c", &script]);
    let deadline = Instant::now() + TIMEOUT;
    while pom_ptyhost::output_end(&remote.holders(), name, TIMEOUT).expect("end")
        < pom_ptyhost::RING_CAPACITY as u64 + 300_003
    {
        assert!(Instant::now() < deadline, "the holder did not print");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut params = remote.attach_params(name);
    params.since = 10;
    let terminal = pty::Terminal::attach(&remote.connection, params, TIMEOUT).expect("attach");
    assert_eq!(
        terminal.attached.start,
        terminal.attached.end - pom_ptyhost::RING_CAPACITY as u64,
        "starts at the oldest byte the ring holds"
    );
    let mut reader = Reader::new();
    assert!(reader.until(&terminal, "END"));
    assert_eq!(reader.text.len(), pom_ptyhost::RING_CAPACITY);
}

#[test]
fn a_vanished_client_leaves_the_holder_running() {
    let remote = Remote::start(&["/bin/sh"]);
    let client = remote.connect();
    let holder = remote.open(&client);
    let terminal = remote.attach(&client, &holder);
    terminal
        .control
        .input(b"echo still-$((6 * 7))\n")
        .expect("input");
    let mut reader = Reader::new();
    assert!(reader.until(&terminal, "still-42"));
    assert_eq!(remote.streams(), 1);
    client.close();
    assert!(remote.wait_streams(0), "its streams end with it");
    std::thread::sleep(Duration::from_millis(200));
    assert!(remote.holders().holder_alive(&holder));

    let back = remote.attach(&remote.connection, &holder);
    let mut replayed = Reader::new();
    assert!(replayed.until(&back, "still-42"));
}

#[test]
fn resent_input_is_typed_once() {
    let remote = Remote::start(&["/bin/sh", "-c", "exec cat > typed.txt"]);
    let holder = remote.open(&remote.connection);
    let typed = remote.worktree().join("typed.txt");

    let mut params = remote.attach_params(&holder);
    params.terminal = "7".into();
    let first = pty::Terminal::attach(&remote.connection, params.clone(), TIMEOUT).expect("first");
    assert_eq!(first.attached.input_from, 0);
    first.control.input(b"one\n").expect("input");
    let mut reader = Reader::new();
    assert!(reader.until(&first, "one"));
    drop(first);
    assert!(remote.wait_streams(0));

    params.input_from = Some(0);
    let resent = pty::Terminal::attach(&remote.connection, params.clone(), TIMEOUT).expect("again");
    resent.control.input(b"one\ntwo\n").expect("resend");
    let mut reader = Reader::new();
    assert!(reader.until(&resent, "two"));

    params.input_from = None;
    params.terminal = "8".into();
    let other = pty::Terminal::attach(&remote.connection, params, TIMEOUT).expect("other");
    assert_eq!(
        other.attached.input_from, 0,
        "another terminal numbers its own"
    );
    other.control.input(b"three\n").expect("input");
    let mut reader = Reader::new();
    assert!(reader.until(&other, "three"));

    let deadline = Instant::now() + TIMEOUT;
    loop {
        let text = std::fs::read_to_string(&typed).unwrap_or_default();
        if text == "one\ntwo\nthree\n" {
            break;
        }
        assert!(Instant::now() < deadline, "typed {text:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_read_only_terminal_watches_without_typing() {
    let remote = Remote::start(&["/bin/sh", "-c", "exec cat > typed.txt"]);
    let holder = remote.open(&remote.connection);
    let mut params = remote.attach_params(&holder);
    params.mode = Mode::ReadOnly;
    let watcher = pty::Terminal::attach(&remote.connection, params, TIMEOUT).expect("ro");
    watcher.control.input(b"ignored\n").expect("input");
    let writer = remote.attach(&remote.connection, &holder);
    writer.control.input(b"kept\n").expect("input");
    let mut reader = Reader::new();
    assert!(reader.until(&watcher, "kept"), "the watcher sees output");
    std::thread::sleep(Duration::from_millis(200));
    let text = std::fs::read_to_string(remote.worktree().join("typed.txt")).unwrap_or_default();
    assert_eq!(text, "kept\n");
}

#[test]
fn service_and_agent_terminals_attach_to_running_holders_only() {
    let remote = Remote::start(&["/bin/sh"]);
    let mut params = remote.open_params();
    params.kind = Kind::Service;
    params.service = Some("api/web".into());
    let stopped = remote
        .connection
        .request(pty::method::OPEN, &params)
        .expect_err("not running");
    assert!(stopped.to_string().contains("not running"), "{stopped}");

    remote.spawn_holder(
        "svc-demo-feat-api-web",
        &["/bin/sh", "-c", "echo web-up; sleep 30"],
    );
    let opened: pty::Opened = serde_json::from_value(
        remote
            .connection
            .request(pty::method::OPEN, &params)
            .expect("service"),
    )
    .expect("opened");
    assert_eq!(opened.holder, "svc-demo-feat-api-web");
    let terminal = remote.attach(&remote.connection, &opened.holder);
    let mut reader = Reader::new();
    assert!(reader.until(&terminal, "web-up"));

    params.kind = Kind::Agent;
    params.service = None;
    params.role = Some("dev".into());
    let missing = remote
        .connection
        .request(pty::method::OPEN, &params)
        .expect_err("no agent");
    assert!(missing.to_string().contains("no agent"), "{missing}");
}

fn refused(remote: &Remote, method: &str, params: serde_json::Value) {
    match remote.connection.request(method, &params) {
        Err(Error::Remote(error)) => {
            assert_eq!(error.code, error_code::INVALID_PARAMS, "{method} {params}")
        }
        other => panic!("{method} {params}: {other:?}"),
    }
}

#[test]
fn foreign_holders_escaping_folders_and_odd_sizes_are_refused() {
    let remote = Remote::start(&["/bin/sh"]);
    let cfg = remote.cfg();
    let outside = remote.temp.path().to_string_lossy().into_owned();
    let open = |extra: serde_json::Value| {
        let mut params =
            json!({ "cfg": cfg, "branch": "feat", "kind": "shell", "cols": 80, "rows": 24 });
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        params
    };
    refused(&remote, pty::method::OPEN, open(json!({ "cwd": "../.." })));
    refused(&remote, pty::method::OPEN, open(json!({ "cwd": outside })));
    refused(&remote, pty::method::OPEN, open(json!({ "cwd": "/etc" })));
    refused(&remote, pty::method::OPEN, open(json!({ "cols": 0 })));
    refused(&remote, pty::method::OPEN, open(json!({ "rows": 1001 })));
    refused(&remote, pty::method::OPEN, open(json!({ "kind": "root" })));
    refused(
        &remote,
        pty::method::OPEN,
        open(json!({ "cfg": "/etc/passwd" })),
    );
    refused(
        &remote,
        pty::method::OPEN,
        open(json!({ "branch": "a..b" })),
    );
    std::fs::create_dir_all(remote.worktree().join("api/src")).expect("dir");
    std::os::unix::fs::symlink("/", remote.worktree().join("api/out")).expect("link");
    refused(
        &remote,
        pty::method::OPEN,
        open(json!({ "cwd": "api/out" })),
    );
    let inside: pty::Opened = serde_json::from_value(
        remote
            .connection
            .request(pty::method::OPEN, open(json!({ "cwd": "api/src" })))
            .expect("a folder inside"),
    )
    .expect("opened");
    assert!(inside.holder.starts_with("term-demo-feat-"));

    remote.spawn_holder("term-demo-main-1", &["/bin/sh", "-c", "sleep 30"]);
    let stream = remote.connection.open_stream().expect("stream");
    let attach = |holder: &str| json!({ "cfg": cfg, "branch": "feat", "holder": holder, "channel": stream.channel(), "cols": 80, "rows": 24 });
    for holder in [
        "term-demo-main-1",
        "term-other-feat-1",
        "x-demo-feat-1",
        "term-demo-feat-",
        "term-demo-feat-../../x",
        "svc-demo-feat-a b",
    ] {
        refused(&remote, pty::method::ATTACH, attach(holder));
    }
    refused(
        &remote,
        pty::method::CLOSE,
        json!({ "cfg": cfg, "branch": "feat", "holder": "term-demo-main-1" }),
    );
    assert!(remote.holders().holder_alive("term-demo-main-1"));
    refused(
        &remote,
        pty::method::RESIZE,
        json!({ "channel": 9999, "cols": 80, "rows": 24 }),
    );
}

#[test]
fn the_log_keeps_no_terminal_bytes() {
    let remote = Remote::start(&["/bin/sh"]);
    let holder = remote.open(&remote.connection);
    let terminal = remote.attach(&remote.connection, &holder);
    terminal
        .control
        .input(b"echo FAKE-SECRET-$((1+1))-x\n")
        .expect("input");
    let mut reader = Reader::new();
    assert!(reader.until(&terminal, "FAKE-SECRET-2-x"));
    drop(terminal);
    remote.handle.stop();
    let log = std::fs::read_to_string(&remote.paths.log).expect("log");
    assert!(!log.contains("FAKE-SECRET"), "{log}");
    assert!(log.contains("method=pty.attach"), "{log}");
}
