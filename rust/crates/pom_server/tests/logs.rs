//! A service's output followed through a real server and real holders: new lines arrive as printed, a restart
//! carries on in the new holder after a separator, a stopped service streams what it left and waits, and a
//! follower that stops acknowledging is reset past the ring.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::time::{Duration, Instant};

use pom_remote::server_api::{method, FollowLogsParams, FollowedLogs};
use pom_rpc::{error_code, Connection, Error, StreamEvent, StreamReceiver};
use terminals::{Remote, TIMEOUT};

const HOLDER: &str = "svc-demo-feat-api-web";

fn follow(remote: &Remote, connection: &Connection) -> (StreamReceiver, FollowedLogs) {
    let stream = connection.open_stream().expect("stream");
    let params = FollowLogsParams {
        cfg: remote.cfg(),
        branch: "feat".into(),
        service: "api/web".into(),
        since: 0,
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

/// What a follower read, acknowledged as it goes, with offsets that must follow on.
struct Followed {
    text: String,
    next: Option<u64>,
    resets: Vec<u64>,
}

impl Followed {
    fn new() -> Followed {
        Followed {
            text: String::new(),
            next: None,
            resets: Vec::new(),
        }
    }

    fn until(&mut self, stream: &StreamReceiver, want: &str) -> bool {
        self.until_within(stream, want, TIMEOUT)
    }

    fn until_within(&mut self, stream: &StreamReceiver, want: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while !self.text.contains(want) {
            if Instant::now() >= deadline {
                return false;
            }
            match stream.recv_timeout(Duration::from_millis(50)) {
                Ok(StreamEvent::Data { offset, bytes }) => {
                    if let Some(next) = self.next {
                        assert_eq!(offset, next, "data must follow on");
                    }
                    self.next = Some(offset + bytes.len() as u64);
                    stream.ack(offset + bytes.len() as u64);
                    self.text.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(StreamEvent::Reset { offset }) => {
                    self.resets.push(offset);
                    self.next = Some(offset);
                }
                Ok(StreamEvent::End { reason }) => panic!("ended: {reason}"),
                Err(Error::Timeout) => {}
                Err(error) => panic!("{error}"),
            }
        }
        true
    }
}

#[test]
fn new_lines_arrive_live_and_a_restart_carries_on_in_the_new_holder() {
    let remote = Remote::start(&["/bin/sh"]);
    remote.spawn_holder(
        HOLDER,
        &[
            "/bin/sh",
            "-c",
            "echo first-line; sleep 1; echo second-line; sleep 60",
        ],
    );
    let (stream, followed) = follow(&remote, &remote.connection);
    assert!(followed.running);
    assert_eq!(followed.holder, HOLDER);
    assert_eq!(followed.start, 0, "a fresh follow replays the scrollback");
    let mut read = Followed::new();
    assert!(read.until(&stream, "first-line"), "{:?}", read.text);
    assert!(read.until(&stream, "second-line"), "{:?}", read.text);
    assert_eq!(remote.handle.info().log_follows, 1);

    remote
        .holders()
        .kill_holder(HOLDER)
        .expect("stop the service");
    remote.spawn_holder(HOLDER, &["/bin/sh", "-c", "echo after-restart; sleep 60"]);
    assert!(read.until(&stream, "after-restart"), "{:?}", read.text);
    let separator = read
        .text
        .find("--- api/web restarted ---")
        .expect("a separator");
    assert!(read.text.find("second-line") < Some(separator));
    assert!(Some(separator) < read.text.find("after-restart"));
    assert!(read.resets.is_empty());

    drop(stream);
    assert!(remote.wait_streams(0), "a dropped follower ends its stream");
    assert!(
        remote.holders().holder_alive(HOLDER),
        "following never stops it"
    );
}

#[test]
fn a_stopped_service_streams_what_it_left_and_waits_for_it_to_start() {
    let remote = Remote::start(&["/bin/sh"]);
    let (stream, followed) = follow(&remote, &remote.connection);
    assert!(!followed.running);
    remote.spawn_holder(HOLDER, &["/bin/sh", "-c", "echo now-up; sleep 60"]);
    let mut read = Followed::new();
    assert!(read.until(&stream, "now-up"), "{:?}", read.text);
    assert!(
        read.text.contains("--- api/web started ---"),
        "{:?}",
        read.text
    );
}

#[test]
fn a_follower_that_stops_acknowledging_is_reset_past_the_ring() {
    let remote = Remote::start(&["/bin/sh"]);
    let total: u64 = 6 * 1024 * 1024;
    let script =
        format!("stty -opost; sleep 1; yes | head -c {total}; echo; echo flood-done; sleep 60");
    remote.spawn_holder(HOLDER, &["/bin/sh", "-c", &script]);
    let (stream, _) = follow(&remote, &remote.connection);
    let stalled = Instant::now();
    let mut unread = Vec::new();
    while stalled.elapsed() < Duration::from_secs(4) {
        match stream.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => unread.push(event),
            Err(Error::Timeout) => {}
            Err(error) => panic!("{error}"),
        }
    }
    let mut next = 0;
    let mut resets = Vec::new();
    for event in unread {
        match event {
            StreamEvent::Data { offset, bytes } => next = offset + bytes.len() as u64,
            StreamEvent::Reset { offset } => {
                resets.push(offset);
                next = offset;
            }
            StreamEvent::End { reason } => panic!("ended: {reason}"),
        }
    }
    stream.ack(next);
    let mut read = Followed::new();
    read.next = Some(next);
    assert!(
        read.until_within(&stream, "flood-done", Duration::from_secs(180)),
        "the follow catches up"
    );
    resets.extend(read.resets);
    assert!(!resets.is_empty(), "the stall outran the ring");
}

#[test]
fn odd_services_and_branches_are_refused() {
    let remote = Remote::start(&["/bin/sh"]);
    for (branch, service) in [("feat", "../web"), ("../feat", "api/web"), ("feat", "")] {
        let stream = remote.connection.open_stream().expect("stream");
        let params = FollowLogsParams {
            cfg: remote.cfg(),
            branch: branch.into(),
            service: service.into(),
            since: 0,
            channel: stream.channel(),
        };
        match remote.connection.request(method::LOGS_FOLLOW, params) {
            Err(Error::Remote(error)) => assert_eq!(error.code, error_code::INVALID_PARAMS),
            other => panic!("{branch} {service}: {other:?}"),
        }
    }
}
