//! Pushes, shared terminal events and agent leases through a real server: a watch pushes a topic only when its
//! value changed, many terminals share one events channel, and two clients move an agent's lease.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::time::{Duration, Instant};

use pom_remote::server_api::{
    method, topic, AgentClaim, AgentHolderParams, AgentsUpdate, StatusUpdate, TurnParams,
    WatchBranchParams, WatchParams, WorkspacesUpdate,
};
use pom_rpc::pty::{self, SharedEvents};
use pom_rpc::{Connection, PushEvent, Subscription};
use terminals::{Reader, Remote, TIMEOUT};

const AGENT: &str = "ws-demo-feat-claude-raw";

fn subscribe(remote: &Remote, connection: &Connection, branch: &str) -> Subscription {
    let subscription = connection.subscribe().expect("subscribe");
    let params = WatchParams {
        cfg: remote.cfg(),
        branch: branch.into(),
        topics: topic::ALL.iter().map(|name| name.to_string()).collect(),
        channel: subscription.channel(),
        turn: None,
    };
    connection
        .request(method::WATCH_SUBSCRIBE, params)
        .expect("watch.subscribe");
    subscription
}

/// Every push that arrives within `wait`, as (topic, value).
fn pushes(subscription: &Subscription, wait: Duration) -> Vec<(String, serde_json::Value)> {
    let deadline = Instant::now() + wait;
    let mut seen = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match subscription.recv_timeout(left) {
            Ok(PushEvent::Value { topic, value }) => seen.push((topic, value)),
            Ok(PushEvent::End { reason }) => panic!("the watch ended: {reason}"),
            Err(pom_rpc::Error::Timeout) => break,
            Err(error) => panic!("{error}"),
        }
    }
    seen
}

fn until_agents(subscription: &Subscription, done: impl Fn(&AgentsUpdate) -> bool) -> AgentsUpdate {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "no such agents push");
        if let Ok(PushEvent::Value { topic, value }) =
            subscription.recv_timeout(Duration::from_millis(100))
        {
            if topic == topic::AGENTS {
                let update: AgentsUpdate = serde_json::from_value(value).expect("agents");
                if done(&update) {
                    return update;
                }
            }
        }
    }
}

#[test]
fn a_server_watch_pushes_each_topic_once_and_again_only_when_it_changes() {
    let remote = Remote::start(&["/bin/sh"]);
    let subscription = subscribe(&remote, &remote.connection, "feat");
    let first = pushes(&subscription, Duration::from_millis(1500));
    let topics: Vec<&str> = first.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        topics,
        [
            topic::WORKSPACES,
            topic::STATUS,
            topic::SERVICES,
            topic::AGENTS
        ]
    );
    let workspaces: WorkspacesUpdate =
        serde_json::from_value(first[0].1.clone()).expect("workspaces");
    assert_eq!(workspaces.list.map(|list| list.workspaces.len()), Some(2));
    let status: StatusUpdate = serde_json::from_value(first[1].1.clone()).expect("status");
    assert_eq!(status.branch, "feat");
    assert!(status.report.is_some_and(|report| !report.services[0].up));

    assert!(
        pushes(&subscription, Duration::from_millis(2500)).is_empty(),
        "nothing changed, so nothing is pushed"
    );

    remote.spawn_holder(AGENT, &["/bin/cat"]);
    let update = until_agents(&subscription, |update| !update.sessions.is_empty());
    assert_eq!(update.sessions[0].role, "claude");
    assert_eq!(update.presence[0].holder, AGENT);
    assert_eq!(update.presence[0].driven_by, None);

    remote
        .connection
        .request(
            method::WATCH_SET_BRANCH,
            WatchBranchParams {
                channel: subscription.channel(),
                branch: "main".into(),
                turn: None,
            },
        )
        .expect("watch.set_branch");
    let moved = pushes(&subscription, Duration::from_millis(1500));
    let topics: Vec<&str> = moved.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        topics,
        [topic::STATUS, topic::SERVICES, topic::AGENTS],
        "the new workspace in full"
    );
}

#[test]
fn a_watch_refuses_unknown_topics_and_another_connections_channel() {
    let remote = Remote::start(&["/bin/sh"]);
    let subscription = remote.connection.subscribe().expect("subscribe");
    let mut params = WatchParams {
        cfg: remote.cfg(),
        branch: "feat".into(),
        topics: vec!["secrets".into()],
        channel: subscription.channel(),
        turn: None,
    };
    let refused = remote.connection.request(method::WATCH_SUBSCRIBE, &params);
    assert!(
        matches!(refused, Err(pom_rpc::Error::Remote(_))),
        "{refused:?}"
    );
    params.topics = vec![topic::STATUS.into()];
    params.branch = "../etc".into();
    let refused = remote.connection.request(method::WATCH_SUBSCRIBE, &params);
    assert!(
        matches!(refused, Err(pom_rpc::Error::Remote(_))),
        "{refused:?}"
    );
    let other = remote.connect();
    let refused = other.request(
        method::WATCH_SET_BRANCH,
        WatchBranchParams {
            channel: subscription.channel(),
            branch: "main".into(),
            turn: None,
        },
    );
    assert!(
        matches!(refused, Err(pom_rpc::Error::Remote(_))),
        "{refused:?}"
    );
    other.close();
}

#[test]
fn more_than_forty_terminals_share_one_events_channel_on_a_server_connection() {
    const COUNT: usize = 45;
    let remote = Remote::start(&["/bin/cat"]);
    let connection = remote.connect();
    let events = SharedEvents::open(&connection).expect("events");
    let mut terminals = Vec::new();
    for _ in 0..COUNT {
        let holder = remote.open(&connection);
        let terminal = pty::Terminal::attach_shared(
            &connection,
            &events,
            remote.attach_params(&holder),
            TIMEOUT,
        )
        .expect("attach");
        terminals.push(terminal);
    }
    assert_eq!(remote.streams(), COUNT);
    for (index, terminal) in terminals.iter().enumerate() {
        terminal
            .control
            .input(format!("typed-{index}\n").as_bytes())
            .expect("type");
    }
    for (index, terminal) in terminals.iter().enumerate() {
        let mut reader = Reader::new();
        assert!(
            reader.until(terminal, &format!("typed-{index}")),
            "terminal {index}: {:?}",
            String::from_utf8_lossy(&reader.text)
        );
    }

    let closing = terminals.remove(7);
    let holder = closing.control.holder().to_string();
    connection
        .request(
            pty::method::CLOSE,
            pty::CloseParams {
                cfg: remote.cfg(),
                branch: "feat".into(),
                holder: holder.clone(),
            },
        )
        .expect("close");
    let mut reader = Reader::new();
    assert_eq!(reader.until_end(&closing).as_deref(), Some("exit"));
    let exit = closing.output.exit(TIMEOUT).expect("its exit");
    assert_eq!(exit.holder, holder);
    assert_eq!(exit.channel, closing.control.channel());
    assert!(
        terminals[0]
            .output
            .exit(Duration::from_millis(200))
            .is_none(),
        "the others did not exit"
    );

    let survivor = &terminals[COUNT - 2];
    survivor.control.input(b"still-here\n").expect("type");
    let mut reader = Reader::new();
    assert!(
        reader.until(survivor, "still-here"),
        "the shared channel stays open"
    );
    connection.close();
}

fn read_until(terminal: &pty::Terminal, want: &str, wait: Duration) -> bool {
    let mut reader = Reader::new();
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        reader.step(terminal, Duration::from_millis(50));
        if String::from_utf8_lossy(&reader.text).contains(want) {
            return true;
        }
    }
    false
}

#[test]
fn a_server_lease_is_claimed_by_one_client_and_taken_over_by_another() {
    let remote = Remote::start(&["/bin/sh"]);
    remote.spawn_holder(AGENT, &["/bin/cat"]);
    let orchestrator = remote.connect();
    let person = remote.connect();
    let subscription = subscribe(&remote, &person, "feat");
    until_agents(&subscription, |update| !update.sessions.is_empty());

    let claim: AgentClaim = serde_json::from_value(
        orchestrator
            .request(
                method::AGENT_CLAIM,
                TurnParams {
                    cfg: remote.cfg(),
                    branch: "feat".into(),
                    role: "claude".into(),
                },
            )
            .expect("agent.claim"),
    )
    .expect("claim");
    assert_eq!(claim.holder, AGENT);
    assert!(!claim.token.is_empty());
    let driven = until_agents(&subscription, |update| {
        update
            .presence
            .first()
            .is_some_and(|presence| presence.driven_by.is_some())
    });
    assert_eq!(
        driven.presence[0].driven_by.as_deref(),
        Some("an orchestrator")
    );

    let mut claimed = remote.attach_params(AGENT);
    claimed.claim = Some(claim.token.clone());
    claimed.terminal = "orchestrator".into();
    let driving = pty::Terminal::attach(&orchestrator, claimed, TIMEOUT).expect("attach");
    let mut watching_params = remote.attach_params(AGENT);
    watching_params.terminal = "person".into();
    let watching = pty::Terminal::attach(&person, watching_params, TIMEOUT).expect("attach");

    watching.control.input(b"from-person\n").expect("type");
    driving.control.input(b"from-orchestrator\n").expect("type");
    assert!(read_until(&watching, "from-orchestrator", TIMEOUT));
    assert!(
        !read_until(&watching, "from-person", Duration::from_millis(300)),
        "the holder drops the keys of a client without the lease"
    );

    person
        .request(
            method::AGENT_TAKE_OVER,
            AgentHolderParams {
                cfg: remote.cfg(),
                branch: "feat".into(),
                holder: AGENT.into(),
            },
        )
        .expect("agent.take_over");
    until_agents(&subscription, |update| {
        update
            .presence
            .first()
            .is_some_and(|presence| presence.driven_by.is_none())
    });
    assert_eq!(remote.holders().input_lease(AGENT), None);
    watching.control.input(b"person-again\n").expect("type");
    assert!(read_until(&watching, "person-again", TIMEOUT));

    let refused = person.request(
        method::AGENT_TAKE_OVER,
        AgentHolderParams {
            cfg: remote.cfg(),
            branch: "feat".into(),
            holder: "../other".into(),
        },
    );
    assert!(
        matches!(refused, Err(pom_rpc::Error::Remote(_))),
        "{refused:?}"
    );
    orchestrator.close();
    person.close();
}
