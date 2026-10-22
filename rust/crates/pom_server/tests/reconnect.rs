//! A client that loses its connection and comes back under the same client id, through a real server and real
//! holders: its terminals carry on from the last applied offset, input sent around the drop is typed once, a
//! restarted server never types unconfirmed input, a followed service output carries on at its offsets, a watch
//! pushes every value again, and what a client left is dropped once the retention passes.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_remote::server_api::{method, topic, FollowLogsParams, FollowedLogs, WatchParams};
use pom_rpc::pty::{
    self, AttachParams, LinkState, Mode, PtyProvider, Request, Resume, ResumeError, ResumingOutput,
    ResumingTerminal, Target, TerminalEvent,
};
use pom_rpc::{Connection, ConnectionState, PushEvent, StreamEvent, StreamReceiver};
use pom_server::ServerPaths;
use terminals::{Remote, TIMEOUT};

const SERVICE: &str = "svc-demo-feat-api-web";

/// One app's session to the server: it can be cut, and reopened under the same client id.
struct Session {
    paths: ServerPaths,
    cfg: String,
    client: String,
    current: Mutex<Option<(Connection, String)>>,
}

impl Session {
    fn open(remote: &Remote, client: &str) -> Arc<Session> {
        let session = Arc::new(Session {
            paths: remote.paths.clone(),
            cfg: remote.cfg(),
            client: client.to_string(),
            current: Mutex::new(None),
        });
        session.reconnect();
        session
    }

    fn live(&self) -> Option<(Connection, String)> {
        self.current
            .lock()
            .ok()?
            .clone()
            .filter(|(connection, _)| connection.state() == ConnectionState::Open)
    }

    fn connection(&self) -> Connection {
        self.live().expect("a live session").0
    }

    /// Drops the session the way a killed proxy does: whatever was still queued never goes out.
    fn cut(&self) {
        if let Some((connection, _)) = self.live() {
            connection.close();
        }
    }

    fn reconnect(&self) {
        let stream = std::os::unix::net::UnixStream::connect(&self.paths.socket).expect("socket");
        let transport = pom_rpc::Transport::unix(stream).expect("transport");
        let mut hello = pom_rpc::Hello::new("9.9.9");
        hello.client_id = Some(self.client.clone());
        let (connection, server) = Connection::builder(transport)
            .connect(hello)
            .expect("handshake");
        if let Ok(mut current) = self.current.lock() {
            *current = Some((connection, server.server_id.unwrap_or_default()));
        }
    }

    fn params(&self, holder: &str, terminal: &str) -> AttachParams {
        AttachParams {
            cfg: self.cfg.clone(),
            branch: "feat".into(),
            holder: holder.to_string(),
            terminal: terminal.to_string(),
            cols: 80,
            rows: 24,
            ..AttachParams::default()
        }
    }
}

impl PtyProvider for Session {
    fn attach(&self, request: &Request) -> Result<pty::Terminal, String> {
        let Target::Holder(holder) = &request.target else {
            return Err("only holders".into());
        };
        let (connection, server) = self.live().ok_or("offline")?;
        let mut terminal = pty::Terminal::attach(&connection, self.params(holder, "1"), TIMEOUT)
            .map_err(|error| error.to_string())?;
        terminal.server = server;
        Ok(terminal)
    }

    fn close(&self, _holder: &str) -> Result<(), String> {
        Ok(())
    }

    fn resume(&self, resume: &Resume) -> Result<pty::Terminal, ResumeError> {
        let (connection, server) = self
            .live()
            .ok_or_else(|| ResumeError::Offline("offline".into()))?;
        let mut params = self.params(&resume.holder, &resume.terminal);
        params.since = resume.since;
        params.input_from = (resume.server == server)
            .then_some(resume.input_from)
            .flatten();
        let mut terminal =
            pty::Terminal::attach(&connection, params, TIMEOUT).map_err(|error| match error {
                pom_rpc::Error::Remote(error) => ResumeError::Gone(error.message),
                other => ResumeError::Offline(other.to_string()),
            })?;
        terminal.server = server;
        Ok(terminal)
    }

    fn link(&self) -> LinkState {
        match self.live() {
            Some(_) => LinkState::Connected,
            None => LinkState::Reconnecting {
                attempt: 1,
                next_in: Duration::ZERO,
            },
        }
    }
}

fn attach(session: &Arc<Session>, holder: &str) -> (ResumingTerminal, ResumingOutput) {
    let provider: Arc<dyn PtyProvider> = session.clone();
    let request = Request {
        target: Target::Holder(holder.to_string()),
        cols: 80,
        rows: 24,
        mode: Mode::ReadWrite,
        primary: true,
        claim: None,
    };
    let (terminal, output, _) = ResumingTerminal::attach(provider, &request).expect("attach");
    (terminal, output)
}

/// Reads output until `done` says so, acknowledging it; data offsets must follow on.
fn read_until(
    output: &mut ResumingOutput,
    events: &mut Vec<TerminalEvent>,
    done: impl Fn(&[TerminalEvent]) -> bool,
) {
    let deadline = Instant::now() + TIMEOUT;
    while !done(events) {
        assert!(Instant::now() < deadline, "{events:?}");
        let event = output.next(Duration::from_millis(50));
        if let TerminalEvent::Data { offset, bytes } = &event {
            output.ack(offset + bytes.len() as u64);
        }
        if event != TerminalEvent::Idle {
            events.push(event);
        }
    }
}

fn resumed(events: &[TerminalEvent]) -> Option<bool> {
    events.iter().rev().find_map(|event| match event {
        TerminalEvent::Resumed { restarted } => Some(*restarted),
        _ => None,
    })
}

fn wait_file(path: &std::path::Path, want: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text == want {
            return;
        }
        assert!(Instant::now() < deadline, "typed {text:?}, want {want:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn input_sent_as_the_connection_drops_is_typed_once_after_the_client_comes_back() {
    let remote = Remote::start(&["/bin/sh", "-c", "exec cat > typed.txt"]);
    let holder = remote.open(&remote.connection);
    let typed = remote.worktree().join("typed.txt");
    let session = Session::open(&remote, "app-1");
    let (terminal, mut output) = attach(&session, &holder);
    let mut events = Vec::new();

    terminal.input(b"one\n").expect("type");
    wait_file(&typed, "one\n");
    session.cut();
    assert!(
        terminal.input(b"lost\n").is_err(),
        "typing is refused while the connection is down"
    );
    read_until(&mut output, &mut events, |events| {
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Reconnecting(_)))
    });
    session.reconnect();
    read_until(&mut output, &mut events, |events| resumed(events).is_some());
    assert_eq!(resumed(&events), Some(false), "the same server");

    // Cut right after typing: the bytes may or may not have reached the holder.
    terminal.input(b"two\n").expect("type");
    session.cut();
    session.reconnect();
    events.clear();
    read_until(&mut output, &mut events, |events| resumed(events).is_some());
    terminal.input(b"three\n").expect("type");
    wait_file(&typed, "one\ntwo\nthree\n");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(&typed).unwrap_or_default(),
        "one\ntwo\nthree\n"
    );
}

#[test]
fn a_restarted_server_never_types_the_unconfirmed_input_again() {
    let mut remote = Remote::start(&["/bin/sh", "-c", "exec cat > typed.txt"]);
    let holder = remote.open(&remote.connection);
    let typed = remote.worktree().join("typed.txt");
    let session = Session::open(&remote, "app-2");
    let (terminal, mut output) = attach(&session, &holder);
    terminal.input(b"before\n").expect("type");
    wait_file(&typed, "before\n");

    remote.restart();
    let mut events = Vec::new();
    read_until(&mut output, &mut events, |events| {
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Reconnecting(_)))
    });
    session.reconnect();
    read_until(&mut output, &mut events, |events| resumed(events).is_some());
    assert_eq!(resumed(&events), Some(true), "another server");
    terminal.input(b"after\n").expect("type");
    wait_file(&typed, "before\nafter\n");
}

#[test]
fn a_holder_killed_while_attached_or_while_away_ends_the_terminal_with_an_exit() {
    let remote = Remote::start(&["/bin/sh", "-c", "exec cat"]);
    let session = Session::open(&remote, "app-3");

    let first = remote.open(&remote.connection);
    let (_terminal, mut output) = attach(&session, &first);
    remote.holders().kill_holder(&first).expect("kill");
    let mut events = Vec::new();
    read_until(&mut output, &mut events, |events| {
        matches!(events.last(), Some(TerminalEvent::Exit { .. }))
    });

    let second = remote.open(&remote.connection);
    let (_terminal, mut output) = attach(&session, &second);
    session.cut();
    let mut events = Vec::new();
    read_until(&mut output, &mut events, |events| {
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Reconnecting(_)))
    });
    remote.holders().kill_holder(&second).expect("kill");
    session.reconnect();
    read_until(&mut output, &mut events, |events| {
        matches!(events.last(), Some(TerminalEvent::Exit { .. }))
    });
}

fn follow(session: &Session, since: u64) -> (StreamReceiver, FollowedLogs) {
    let connection = session.connection();
    let stream = connection.open_stream().expect("stream");
    let params = FollowLogsParams {
        cfg: session.cfg.clone(),
        branch: "feat".into(),
        service: "api/web".into(),
        since,
        channel: stream.channel(),
    };
    let followed = serde_json::from_value(
        connection
            .request(method::LOGS_FOLLOW, params)
            .expect("logs.follow"),
    )
    .expect("followed");
    (stream, followed)
}

/// Reads a followed stream until its text holds `want`; returns the offset after the last byte.
fn read_logs(stream: &StreamReceiver, from: u64, text: &mut String, want: &str) -> u64 {
    let deadline = Instant::now() + TIMEOUT;
    let mut next = from;
    while !text.contains(want) {
        assert!(Instant::now() < deadline, "{text:?}");
        match stream.recv_timeout(Duration::from_millis(50)) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                assert_eq!(offset, next, "data must follow on");
                next = offset + bytes.len() as u64;
                stream.ack(next);
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
            Ok(other) => panic!("{other:?}"),
            Err(pom_rpc::Error::Timeout) => {}
            Err(error) => panic!("{error}"),
        }
    }
    next
}

#[test]
fn a_followed_output_carries_on_at_its_offsets_across_a_reconnect_and_a_restart_meanwhile() {
    let remote = Remote::start(&["/bin/sh"]);
    remote.spawn_holder(SERVICE, &["/bin/sh", "-c", "echo first-run; sleep 60"]);
    let session = Session::open(&remote, "app-4");
    let (stream, followed) = follow(&session, 0);
    let mut text = String::new();
    let next = read_logs(&stream, followed.start, &mut text, "first-run");

    session.cut();
    assert!(remote.wait_streams(0));
    remote.holders().kill_holder(SERVICE).expect("stop");
    remote.spawn_holder(SERVICE, &["/bin/sh", "-c", "echo second-run; sleep 60"]);
    session.reconnect();
    let (stream, followed) = follow(&session, next);
    assert_eq!(followed.start, next, "it carries on where it stopped");
    let mut after = String::new();
    read_logs(&stream, next, &mut after, "second-run");
    assert!(
        after.contains("--- api/web restarted ---"),
        "{after:?} says the service restarted meanwhile"
    );
    assert!(
        !after.contains("first-run"),
        "nothing comes twice: {after:?}"
    );
}

#[test]
fn a_watch_subscribed_again_after_a_reconnect_pushes_every_value_again() {
    let remote = Remote::start(&["/bin/sh"]);
    let session = Session::open(&remote, "app-5");
    let topics = |connection: &Connection| {
        let subscription = connection.subscribe().expect("subscribe");
        let params = WatchParams {
            cfg: remote.cfg(),
            branch: "feat".into(),
            topics: topic::ALL.iter().map(|name| name.to_string()).collect(),
            channel: subscription.channel(),
            turn: None,
        };
        connection
            .request(method::WATCH_SUBSCRIBE, params)
            .expect("watch.subscribe");
        let mut seen = Vec::new();
        let deadline = Instant::now() + TIMEOUT;
        while !(seen.contains(&topic::WORKSPACES.to_string())
            && seen.contains(&topic::STATUS.to_string()))
        {
            assert!(Instant::now() < deadline, "pushed {seen:?}");
            if let Ok(PushEvent::Value { topic, .. }) =
                subscription.recv_timeout(Duration::from_millis(100))
            {
                seen.push(topic);
            }
        }
        seen
    };
    topics(&session.connection());
    session.cut();
    session.reconnect();
    let again = topics(&session.connection());
    assert!(again.contains(&topic::WORKSPACES.to_string()), "{again:?}");
}

#[test]
fn what_a_vanished_client_left_is_kept_for_the_retention_then_dropped() {
    let remote = Remote::start_retaining(
        &["/bin/sh", "-c", "exec cat"],
        Some(Duration::from_millis(600)),
    );
    remote.spawn_holder(SERVICE, &["/bin/sh", "-c", "echo up; sleep 60"]);
    let holder = remote.open(&remote.connection);
    let session = Session::open(&remote, "app-6");
    let (_terminal, _output) = attach(&session, &holder);
    let (stream, followed) = follow(&session, 0);
    let mut text = String::new();
    read_logs(&stream, followed.start, &mut text, "up");
    assert_eq!(
        remote.handle.info().retained,
        0,
        "nothing is left while it is here"
    );

    session.cut();
    assert!(remote.wait_streams(0));
    assert_eq!(
        remote.handle.info().retained,
        2,
        "its terminal's input numbering and its followed stream"
    );
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(
        remote.handle.info().retained,
        0,
        "dropped after the retention"
    );
}
