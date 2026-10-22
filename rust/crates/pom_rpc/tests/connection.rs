use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pom_rpc::mock::{self, Faults};
use pom_rpc::{
    error_code, read_frame, write_frame, CloseReason, Config, Connection, ConnectionState, Error,
    Frame, Hello, InputChunk, Kind, PushEvent, RemoteError, Request, RequestContext, Response,
    StreamEvent, Transport, MAX_FRAME, MAX_PENDING_REQUESTS,
};
use serde_json::{json, Value};

const MEBIBYTE: usize = 1024 * 1024;

fn handler(request: Request, context: &RequestContext) -> Result<Value, RemoteError> {
    match request.method.as_str() {
        "echo" => Ok(request.params),
        "sleep" => {
            let millis = request.params.as_u64().unwrap_or(0);
            thread::sleep(Duration::from_millis(millis));
            Ok(request.params)
        }
        "block" => {
            let started = Instant::now();
            while !context.is_cancelled() && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(Value::Null)
        }
        "flood" => {
            let channel = request.params.as_u64().unwrap() as u32;
            let mut sender = context
                .connection()
                .attach_stream(channel, 0)
                .map_err(|error| RemoteError::new("stream", error.to_string()))?;
            thread::spawn(move || {
                let block = vec![b'x'; 64 * 1024];
                while sender.write(&block).is_ok() {}
            });
            Ok(Value::Null)
        }
        "watch" => {
            let channel = request.params.as_u64().unwrap() as u32;
            let publisher = context.connection().publisher(channel).unwrap();
            for index in 0..3 {
                publisher.publish("tick", index).unwrap();
            }
            publisher.finish("done");
            Ok(Value::Null)
        }
        "fail" => Err(RemoteError::new("nope", "asked to fail")),
        other => Err(RemoteError::new(error_code::METHOD_NOT_FOUND, other)),
    }
}

fn connected(config: Config) -> (Connection, Connection) {
    mock::connected(config, handler).unwrap()
}

fn read(stream: &mut UnixStream) -> Frame {
    read_frame(stream).unwrap().unwrap()
}

/// A hand-driven server for a real client, to send what a correct server never would.
fn raw_server(config: Config) -> (Connection, UnixStream) {
    let (client_transport, mut raw) = raw_pair();
    let connecting = thread::spawn(move || {
        Connection::builder(client_transport)
            .config(config)
            .connect(Hello::new("test"))
    });
    let hello = read(&mut raw);
    assert_eq!(hello.kind, Kind::Hello);
    write_frame(
        &mut raw,
        &Frame::json(Kind::Hello, 0, &Hello::new("raw")).unwrap(),
    )
    .unwrap();
    let (client, _) = connecting.join().unwrap().unwrap();
    (client, raw)
}

/// A hand-driven client for a real server.
fn raw_client() -> (Connection, UnixStream) {
    raw_client_with(Config::default())
}

fn raw_client_with(config: Config) -> (Connection, UnixStream) {
    let (server_transport, mut raw) = raw_pair();
    let accepting = thread::spawn(move || {
        Connection::builder(server_transport)
            .config(config)
            .handler(handler)
            .accept(|_| Ok(Hello::new("server")))
    });
    write_frame(
        &mut raw,
        &Frame::json(Kind::Hello, 0, &Hello::new("raw")).unwrap(),
    )
    .unwrap();
    assert_eq!(read(&mut raw).kind, Kind::Hello);
    let (server, _) = accepting.join().unwrap().unwrap();
    (server, raw)
}

fn raw_pair() -> (Transport, UnixStream) {
    let (connection_side, raw) = UnixStream::pair().unwrap();
    (Transport::unix(connection_side).unwrap(), raw)
}

/// Reads until the peer's connection-level END and returns its reason.
fn farewell(raw: &mut UnixStream) -> String {
    loop {
        let frame = read(raw);
        if frame.kind == Kind::End && frame.channel == 0 {
            return pom_rpc::decode::<pom_rpc::End>(&frame.payload)
                .unwrap()
                .reason;
        }
    }
}

fn request_frame(id: u32, method: &str) -> Frame {
    Frame::json(
        Kind::Request,
        id,
        &Request {
            method: method.to_string(),
            params: Value::Null,
        },
    )
    .unwrap()
}

#[test]
fn handshake_exchanges_hellos() {
    let (client, server) = connected(Config::default());
    assert_eq!(client.state(), ConnectionState::Open);
    assert_eq!(server.state(), ConnectionState::Open);
    client.close();
    server.wait_closed(Duration::from_secs(1));
}

#[test]
fn request_round_trips_and_errors_come_back_typed() {
    let (client, server) = connected(Config::default());
    assert_eq!(
        client.request("echo", json!({"a": [1, 2]})).unwrap(),
        json!({"a": [1, 2]})
    );
    match client.request("fail", ()) {
        Err(Error::Remote(error)) => assert_eq!(error.code, "nope"),
        other => panic!("expected a remote error, got {other:?}"),
    }
    match client.request("missing", ()) {
        Err(Error::Remote(error)) => assert_eq!(error.code, error_code::METHOD_NOT_FOUND),
        other => panic!("expected method_not_found, got {other:?}"),
    }
    // The server side may call the client too; with no handler it refuses.
    match server.request("echo", 1) {
        Err(Error::Remote(error)) => assert_eq!(error.code, error_code::METHOD_NOT_FOUND),
        other => panic!("expected method_not_found, got {other:?}"),
    }
    client.close();
}

#[test]
fn many_requests_in_flight_complete_concurrently() {
    let (client, _server) = connected(Config::default());
    let started = Instant::now();
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let client = client.clone();
            thread::spawn(move || {
                let pending: Vec<_> = (0..32)
                    .map(|index| {
                        let value = worker * 1000 + index;
                        (
                            value,
                            client.start_request("sleep", 100 + value % 7).unwrap(),
                        )
                    })
                    .collect();
                for (value, request) in pending {
                    assert_eq!(request.wait().unwrap(), json!(100 + value % 7));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    // 256 requests of ~100 ms each finish in about one of them only if they really overlap.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    client.close();
}

#[test]
fn pending_request_limit_is_enforced_locally() {
    let (client, _server) = connected(Config::default());
    let pending: Vec<_> = (0..MAX_PENDING_REQUESTS)
        .map(|_| client.start_request("block", ()).unwrap())
        .collect();
    assert!(matches!(
        client.start_request("echo", ()),
        Err(Error::Limit(_))
    ));
    drop(pending);
    assert_eq!(client.request("echo", 5).unwrap(), json!(5));
    client.close();
}

#[test]
fn peer_exceeding_the_request_limit_is_a_protocol_error() {
    let (server, mut raw) = raw_client();
    for index in 0..=MAX_PENDING_REQUESTS as u32 {
        write_frame(&mut raw, &request_frame(index * 2 + 1, "block")).unwrap();
    }
    assert!(farewell(&mut raw).starts_with(error_code::PROTOCOL_ERROR));
    assert!(matches!(
        server.wait_closed(Duration::from_secs(2)),
        Some(CloseReason::Protocol(_))
    ));
}

#[test]
fn malformed_json_closes_the_connection() {
    let (server, mut raw) = raw_client();
    write_frame(
        &mut raw,
        &Frame::new(Kind::Request, 1, b"{\"method\": ".to_vec()),
    )
    .unwrap();
    assert!(farewell(&mut raw).starts_with(error_code::PROTOCOL_ERROR));
    assert!(matches!(
        server.wait_closed(Duration::from_secs(2)),
        Some(CloseReason::Protocol(_))
    ));
    assert!(read_frame(&mut raw).unwrap().is_none());
}

#[test]
fn unknown_kind_closes_the_connection() {
    let (server, mut raw) = raw_client();
    let mut bytes = 5u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[0x7f, 0, 0, 0, 1]);
    std::io::Write::write_all(&mut raw, &bytes).unwrap();
    assert!(farewell(&mut raw).contains("0x7f"));
    assert!(matches!(
        server.wait_closed(Duration::from_secs(2)),
        Some(CloseReason::Protocol(_))
    ));
}

#[test]
fn oversized_frame_closes_the_connection() {
    let (server, mut raw) = raw_client();
    let mut bytes = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&[Kind::Push.code(), 0, 0, 0, 1]);
    std::io::Write::write_all(&mut raw, &bytes).unwrap();
    assert!(farewell(&mut raw).starts_with(error_code::PROTOCOL_ERROR));
    assert!(matches!(
        server.wait_closed(Duration::from_secs(2)),
        Some(CloseReason::Protocol(_))
    ));
}

#[test]
fn protocol_mismatch_is_refused_in_the_handshake() {
    let (server_transport, mut raw) = raw_pair();
    let accepting = thread::spawn(move || {
        Connection::builder(server_transport).accept(|_| Ok(Hello::new("server")))
    });
    let mut hello = Hello::new("raw");
    hello.protocol = 99;
    write_frame(&mut raw, &Frame::json(Kind::Hello, 0, &hello).unwrap()).unwrap();
    assert!(farewell(&mut raw).starts_with(error_code::PROTOCOL_MISMATCH));
    assert!(matches!(accepting.join().unwrap(), Err(Error::Closed(_))));
}

#[test]
fn server_can_refuse_a_client() {
    let (client_transport, server_transport) = mock::pair().unwrap();
    let accepting = thread::spawn(move || {
        Connection::builder(server_transport)
            .accept(|hello| Err(format!("version {} is too old", hello.pom_version)))
    });
    match Connection::builder(client_transport).connect(Hello::new("0.1.0")) {
        Err(Error::Closed(CloseReason::Peer(reason))) => {
            assert_eq!(reason, "version 0.1.0 is too old")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(accepting.join().unwrap().is_err());
}

#[test]
fn newer_hello_with_extra_fields_is_accepted() {
    let (server_transport, mut raw) = raw_pair();
    let accepting = thread::spawn(move || {
        Connection::builder(server_transport).accept(|_| Ok(Hello::new("server")))
    });
    let newer = br#"{"protocol":1,"pom_version":"2.0.0","client_id":"c","features":["x"],"compression":"zstd"}"#;
    write_frame(&mut raw, &Frame::new(Kind::Hello, 0, newer.to_vec())).unwrap();
    let reply: Hello = pom_rpc::decode(&read(&mut raw).payload).unwrap();
    assert_eq!(reply.pom_version, "server");
    let (server, client_hello) = accepting.join().unwrap().unwrap();
    assert_eq!(client_hello.client_id.as_deref(), Some("c"));
    assert_eq!(client_hello.features, ["x"]);
    server.close();
}

#[test]
fn cancel_fails_the_waiter_and_reaches_the_handler() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    let (client, _server) = mock::connected(
        Config::default(),
        move |_request: Request, context: &RequestContext| {
            let started = Instant::now();
            while !context.is_cancelled() && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(2));
            }
            observed.store(context.is_cancelled(), Ordering::Release);
            Ok(Value::Null)
        },
    )
    .unwrap();
    let pending = client.start_request("block", ()).unwrap();
    let canceller = pending.canceller();
    let waiter = thread::spawn(move || pending.wait());
    thread::sleep(Duration::from_millis(50));
    canceller.cancel();
    assert_eq!(waiter.join().unwrap(), Err(Error::Cancelled));
    let started = Instant::now();
    while !cancelled.load(Ordering::Acquire) {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "handler never saw the cancel"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(client.state(), ConnectionState::Open);
    client.close();
}

#[test]
fn late_response_after_cancel_is_dropped() {
    let (client, mut raw) = raw_server(Config::default());
    let first = client.start_request("slow", ()).unwrap();
    let first_request = read(&mut raw);
    assert_eq!(first_request.kind, Kind::Request);
    first.cancel();
    let cancel = read(&mut raw);
    assert_eq!(
        (cancel.kind, cancel.channel),
        (Kind::Cancel, first_request.channel)
    );
    let late = Response::from_result(Ok(json!("late")));
    write_frame(
        &mut raw,
        &Frame::json(Kind::Response, first_request.channel, &late).unwrap(),
    )
    .unwrap();
    assert_eq!(first.wait(), Err(Error::Cancelled));

    let second = client.start_request("fresh", ()).unwrap();
    let second_request = read(&mut raw);
    assert_ne!(second_request.channel, first_request.channel);
    let fresh = Response::from_result(Ok(json!("fresh")));
    write_frame(
        &mut raw,
        &Frame::json(Kind::Response, second_request.channel, &fresh).unwrap(),
    )
    .unwrap();
    assert_eq!(second.wait().unwrap(), json!("fresh"));
    assert_eq!(client.state(), ConnectionState::Open);
}

#[test]
fn dropping_an_unanswered_request_cancels_it() {
    let (client, mut raw) = raw_server(Config::default());
    let pending = client.start_request("slow", ()).unwrap();
    let request = read(&mut raw);
    drop(pending);
    let cancel = read(&mut raw);
    assert_eq!(
        (cancel.kind, cancel.channel),
        (Kind::Cancel, request.channel)
    );
}

#[test]
fn sender_blocks_at_the_window_and_resumes_on_ack() {
    let window = 64 * 1024;
    let config = Config {
        window,
        ..Config::default()
    };
    let (client, server) = connected(config);
    let receiver = client.open_stream().unwrap();
    let mut sender = server.attach_stream(receiver.channel(), 0).unwrap();
    let total = 200_000usize;
    let (done_sender, done) = mpsc::channel();
    let writer = thread::spawn(move || {
        sender.write(&vec![7u8; total]).unwrap();
        done_sender.send(()).unwrap();
        sender.finish("done");
    });

    let mut received = 0u64;
    let take_until_quiet = |received: &mut u64| loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                assert_eq!(offset, *received);
                *received += bytes.len() as u64;
            }
            Ok(StreamEvent::End { .. }) => return true,
            Ok(other) => panic!("unexpected {other:?}"),
            Err(Error::Timeout) => return false,
            Err(error) => panic!("{error}"),
        }
    };
    assert!(!take_until_quiet(&mut received));
    assert_eq!(received, window);
    assert!(done.try_recv().is_err(), "the writer should be blocked");

    receiver.ack(window);
    assert!(!take_until_quiet(&mut received));
    assert_eq!(received, 2 * window);

    let mut ended = false;
    while !ended {
        receiver.ack(received);
        ended = take_until_quiet(&mut received);
    }
    assert_eq!(received, total as u64);
    done.recv_timeout(Duration::from_secs(1)).unwrap();
    writer.join().unwrap();
    client.close();
}

#[test]
fn try_write_takes_only_the_available_credit() {
    let config = Config {
        window: 1000,
        ..Config::default()
    };
    let (client, server) = connected(config);
    let receiver = client.open_stream().unwrap();
    let mut sender = server.attach_stream(receiver.channel(), 500).unwrap();
    assert_eq!(sender.try_write(&[1; 600]).unwrap(), 600);
    assert_eq!(sender.try_write(&[1; 600]).unwrap(), 400);
    assert_eq!(sender.credit(), 0);
    assert_eq!(sender.offset(), 1500);
    let mut received = 500;
    while received < 1500 {
        match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            StreamEvent::Data { offset, bytes } => {
                assert_eq!(offset, received);
                received += bytes.len() as u64;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    receiver.ack(1500);
    let started = Instant::now();
    while sender.credit() < 1000 {
        assert!(started.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(1));
    }
    client.close();
}

#[test]
fn a_flooding_stream_does_not_delay_control_frames() {
    let (client, _server) = connected(Config::default());
    let receiver = client.open_stream().unwrap();
    let target = 50 * MEBIBYTE as u64;
    client.request("flood", receiver.channel()).unwrap();
    let received = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let consumer = {
        let received = received.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut total = 0u64;
            while !stop.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
                    StreamEvent::Data { offset, bytes } => {
                        assert_eq!(offset, total);
                        total += bytes.len() as u64;
                        receiver.ack(total);
                        received.store(total, Ordering::Release);
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        })
    };
    let mut slowest_request = Duration::ZERO;
    let mut slowest_ping = Duration::ZERO;
    let mut samples = 0;
    while samples < 20 || received.load(Ordering::Acquire) < target {
        let started = Instant::now();
        assert_eq!(client.request("echo", samples).unwrap(), json!(samples));
        slowest_request = slowest_request.max(started.elapsed());
        slowest_ping = slowest_ping.max(client.ping(Duration::from_secs(1)).unwrap());
        samples += 1;
        thread::sleep(Duration::from_millis(2));
    }
    stop.store(true, Ordering::Release);
    consumer.join().unwrap();
    assert!(received.load(Ordering::Acquire) >= target);
    assert!(
        slowest_request < Duration::from_millis(50),
        "request took {slowest_request:?} over {samples} samples"
    );
    assert!(
        slowest_ping < Duration::from_millis(50),
        "ping took {slowest_ping:?} over {samples} samples"
    );
    client.close();
}

#[test]
fn round_robin_shares_the_pipe_between_streams() {
    let size = 4 * MEBIBYTE;
    let (server, mut raw) = raw_client_with(Config {
        window: size as u64,
        ..Config::default()
    });
    let mut first = server.attach_stream(1, 0).unwrap();
    let mut second = server.attach_stream(3, 0).unwrap();
    first.write(&vec![1; size]).unwrap();
    second.write(&vec![2; size]).unwrap();
    let mut order = Vec::new();
    let mut remaining = [size, size];
    while remaining.iter().any(|left| *left > 0) {
        let frame = read(&mut raw);
        if frame.kind != Kind::Data {
            continue;
        }
        let (_, bytes) = frame.offset_and_bytes().unwrap();
        let index = if frame.channel == 1 { 0 } else { 1 };
        remaining[index] -= bytes.len();
        order.push(frame.channel);
    }
    let first_of_second = order.iter().position(|channel| *channel == 3).unwrap();
    let last_of_first = order.iter().rposition(|channel| *channel == 1).unwrap();
    assert!(
        first_of_second < last_of_first,
        "streams were not interleaved: {order:?}"
    );
    let tail = &order[first_of_second..last_of_first];
    assert!(
        tail.windows(2).all(|pair| pair[0] != pair[1]),
        "streams did not alternate once both were queued"
    );
    server.close();
}

#[test]
fn reset_skips_ahead_and_input_flows_back() {
    let (client, server) = connected(Config::default());
    let receiver = client.open_stream().unwrap();
    let mut sender = server.attach_stream(receiver.channel(), 0).unwrap();
    sender.write(b"0123456789").unwrap();
    sender.reset(1000).unwrap();
    assert!(sender.reset(10).is_err());
    sender.write(b"abcde").unwrap();
    let mut events = Vec::new();
    while events.len() < 3 {
        events.push(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    }
    assert_eq!(
        events,
        [
            StreamEvent::Data {
                offset: 0,
                bytes: b"0123456789".to_vec()
            },
            StreamEvent::Reset { offset: 1000 },
            StreamEvent::Data {
                offset: 1000,
                bytes: b"abcde".to_vec()
            },
        ]
    );

    receiver.send_input(b"ls\n").unwrap();
    receiver.set_input_offset(100);
    receiver.send_input(b"q").unwrap();
    let timeout = Duration::from_secs(1);
    assert_eq!(
        sender.recv_input_timeout(timeout).unwrap(),
        InputChunk {
            offset: 0,
            bytes: b"ls\n".to_vec()
        }
    );
    assert_eq!(
        sender.recv_input_timeout(timeout).unwrap(),
        InputChunk {
            offset: 100,
            bytes: b"q".to_vec()
        }
    );

    drop(receiver);
    let started = Instant::now();
    while !sender.is_ended() {
        assert!(started.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(sender.write(b"x"), Err(Error::ChannelEnded(_))));
    client.close();
}

#[test]
fn pushes_arrive_on_their_subscription_and_unsubscribe_reaches_the_publisher() {
    let (client, server) = connected(Config::default());
    let subscription = client.subscribe().unwrap();
    client.request("watch", subscription.channel()).unwrap();
    let timeout = Duration::from_secs(1);
    for index in 0..3 {
        assert_eq!(
            subscription.recv_timeout(timeout).unwrap(),
            PushEvent::Value {
                topic: "tick".to_string(),
                value: json!(index)
            }
        );
    }
    assert_eq!(
        subscription.recv_timeout(timeout).unwrap(),
        PushEvent::End {
            reason: "done".to_string()
        }
    );

    let second = client.subscribe().unwrap();
    let publisher = server.publisher(second.channel()).unwrap();
    publisher.publish("status", json!({"ok": true})).unwrap();
    assert!(matches!(
        second.recv_timeout(timeout).unwrap(),
        PushEvent::Value { .. }
    ));
    drop(second);
    let started = Instant::now();
    while !publisher.is_ended() {
        assert!(started.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        publisher.publish("status", 1),
        Err(Error::ChannelEnded(_))
    ));
    client.close();
}

#[test]
fn channel_limit_is_enforced() {
    let (client, _server) = connected(Config::default());
    let streams: Vec<_> = (0..pom_rpc::MAX_CHANNELS)
        .map(|_| client.open_stream().unwrap())
        .collect();
    assert!(matches!(client.subscribe(), Err(Error::Limit(_))));
    drop(streams);
    assert!(client.subscribe().is_ok());
    client.close();
}

#[test]
fn silent_peer_is_declared_dead_and_pending_requests_fail() {
    let config = Config {
        heartbeat_interval: Duration::from_millis(50),
        dead_after: Duration::from_millis(300),
        ..Config::default()
    };
    let (client, _raw) = raw_server(config);
    let pending = client.start_request("echo", ()).unwrap();
    let started = Instant::now();
    assert_eq!(
        pending.wait(),
        Err(Error::Closed(CloseReason::HeartbeatTimeout))
    );
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(
        client.state(),
        ConnectionState::Closed(CloseReason::HeartbeatTimeout)
    );
}

#[test]
fn heartbeats_keep_an_idle_connection_alive() {
    let config = Config {
        heartbeat_interval: Duration::from_millis(30),
        dead_after: Duration::from_millis(400),
        ..Config::default()
    };
    let (client, server) = connected(config);
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(client.state(), ConnectionState::Open);
    assert_eq!(server.state(), ConnectionState::Open);
    client.close();
}

#[test]
fn stalled_link_is_detected_by_the_heartbeat() {
    let config = Config {
        heartbeat_interval: Duration::from_millis(30),
        dead_after: Duration::from_millis(200),
        ..Config::default()
    };
    let stall = Faults {
        stall_after: Some(300),
        ..Faults::default()
    };
    let (client_transport, server_transport) = mock::faulty_pair(stall.clone(), stall).unwrap();
    let (client, server) =
        mock::connected_over(client_transport, server_transport, config, handler).unwrap();
    // 300 bytes of hellos, pings and pongs pass, then both directions go silent. Whichever side
    // notices first tears its socket down, so the other may see the pipe close instead.
    let reasons = [
        client.wait_closed(Duration::from_secs(5)),
        server.wait_closed(Duration::from_secs(5)),
    ];
    assert!(
        reasons.contains(&Some(CloseReason::HeartbeatTimeout)),
        "{reasons:?}"
    );
    for reason in reasons {
        assert!(
            matches!(
                reason,
                Some(CloseReason::HeartbeatTimeout | CloseReason::PeerGone)
            ),
            "{reason:?}"
        );
    }
}

#[test]
fn cut_link_fails_pending_requests_on_both_sides() {
    let cut = Faults {
        cut_after: Some(400),
        ..Faults::default()
    };
    let (client_transport, server_transport) = mock::faulty_pair(cut, Faults::default()).unwrap();
    let (client, server) = mock::connected_over(
        client_transport,
        server_transport,
        Config::default(),
        handler,
    )
    .unwrap();
    assert!(matches!(
        client.request("echo", "x".repeat(1000)),
        Err(Error::Closed(CloseReason::Io(_)))
    ));
    assert!(matches!(
        client.state(),
        ConnectionState::Closed(CloseReason::Io(_))
    ));
    assert!(matches!(
        client.request("echo", 1),
        Err(Error::Closed(CloseReason::Io(_)))
    ));
    assert!(matches!(
        server.wait_closed(Duration::from_secs(2)),
        Some(CloseReason::PeerGone | CloseReason::Io(_))
    ));
}

#[test]
fn slow_fragmented_reads_and_latency_still_deliver() {
    let slow = Faults {
        write_delay: Duration::from_millis(5),
        read_chunk: Some(3),
        ..Faults::default()
    };
    let (client_transport, server_transport) = mock::faulty_pair(slow.clone(), slow).unwrap();
    let (client, _server) = mock::connected_over(
        client_transport,
        server_transport,
        Config::default(),
        handler,
    )
    .unwrap();
    let started = Instant::now();
    let payload = json!({"text": "x".repeat(300), "list": [1, 2, 3]});
    assert_eq!(client.request("echo", &payload).unwrap(), payload);
    assert!(started.elapsed() >= Duration::from_millis(10));
    client.close();
}

#[test]
fn clean_close_reaches_the_peer_and_fails_its_pending_requests() {
    let (client, server) = connected(Config::default());
    let pending = client.start_request("block", ()).unwrap();
    thread::sleep(Duration::from_millis(20));
    server.close();
    assert_eq!(
        pending.wait(),
        Err(Error::Closed(CloseReason::Peer("closed".to_string())))
    );
    assert_eq!(server.state(), ConnectionState::Closed(CloseReason::Local));
    assert_eq!(
        client.wait_closed(Duration::from_secs(1)),
        Some(CloseReason::Peer("closed".to_string()))
    );
    assert!(matches!(client.request("echo", 1), Err(Error::Closed(_))));
    assert!(matches!(client.open_stream(), Err(Error::Closed(_))));
    assert_eq!(server.close_error(), None);
}

#[test]
fn closing_wakes_a_writer_blocked_on_credit() {
    let config = Config {
        window: 1024,
        ..Config::default()
    };
    let (client, server) = connected(config);
    let receiver = client.open_stream().unwrap();
    let mut sender = server.attach_stream(receiver.channel(), 0).unwrap();
    let shared = Arc::new(Mutex::new(None));
    let result = shared.clone();
    let writer = thread::spawn(move || {
        *result.lock().unwrap() = Some(sender.write(&[0; 4096]));
    });
    thread::sleep(Duration::from_millis(50));
    assert!(shared.lock().unwrap().is_none());
    server.close();
    writer.join().unwrap();
    assert!(matches!(
        shared.lock().unwrap().take(),
        Some(Err(Error::Closed(CloseReason::Local)))
    ));
    drop(receiver);
}
