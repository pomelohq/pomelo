//! The app against in-process servers that list different features in their HELLO: one from before pushes and
//! shared terminal events is polled and gets an events channel per terminal; a newer one shares the channel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_remote::server_api::{method, AgentListing, FEATURES, METHODS, REQUIRED};
use pom_remote::{ProjectSource, ServerSource, StatusReport, Timing, Watch, WorkspaceList};
use pom_rpc::pty::{self, AttachParams, Attached, Mode, Request, Target};
use pom_rpc::{Connection, Hello, Publisher, RemoteError, RequestContext, StreamSender};

#[derive(Default)]
struct Served {
    listings: AtomicUsize,
    attaches: Mutex<Vec<AttachParams>>,
    streams: Mutex<Vec<StreamSender>>,
    publishers: Mutex<Vec<Publisher>>,
    connections: Mutex<Vec<Connection>>,
}

fn answer(
    served: &Served,
    request: pom_rpc::Request,
    context: &RequestContext,
) -> Result<serde_json::Value, RemoteError> {
    let json = |value: serde_json::Result<serde_json::Value>| {
        value.map_err(|error| RemoteError::new("internal", error.to_string()))
    };
    match request.method.as_str() {
        method::WS_LIST => {
            served.listings.fetch_add(1, Ordering::SeqCst);
            let mut list = WorkspaceList {
                project: "myproject".into(),
                ..WorkspaceList::default()
            };
            list.workspaces.push(pom_remote::WorkspaceSummary {
                branch: "main".into(),
                is_main: true,
                ..Default::default()
            });
            json(serde_json::to_value(list))
        }
        method::WS_STATUS => json(serde_json::to_value(StatusReport::default())),
        method::AGENT_LIST => json(serde_json::to_value(AgentListing::default())),
        method::PTY_ATTACH => {
            let params: AttachParams = serde_json::from_value(request.params)
                .map_err(|error| RemoteError::new("invalid_params", error.to_string()))?;
            let sender = context
                .connection()
                .attach_stream(params.channel, 0)
                .map_err(|error| RemoteError::new("failed", error.to_string()))?;
            if let Some(events) = params.events {
                if served
                    .attaches
                    .lock()
                    .map(|known| known.iter().all(|seen| seen.events != Some(events)))
                    .unwrap_or(true)
                {
                    let publisher = context
                        .connection()
                        .publisher(events)
                        .map_err(|error| RemoteError::new("failed", error.to_string()))?;
                    if let Ok(mut publishers) = served.publishers.lock() {
                        publishers.push(publisher);
                    }
                }
            }
            if let Ok(mut streams) = served.streams.lock() {
                streams.push(sender);
            }
            let holder = params.holder.clone();
            if let Ok(mut attaches) = served.attaches.lock() {
                attaches.push(params);
            }
            json(serde_json::to_value(Attached {
                holder,
                ..Attached::default()
            }))
        }
        other => Err(RemoteError::new("method_not_found", other.to_string())),
    }
}

/// A source whose every connection reaches a fresh in-process server listing `features`.
fn source(features: Vec<String>) -> (ServerSource, Arc<Served>) {
    let served = Arc::new(Served::default());
    let opened = served.clone();
    let opener: pom_remote::Opener = Arc::new(move || {
        let (client, server) = pom_rpc::mock::pair()?;
        let (served, features) = (opened.clone(), features.clone());
        std::thread::spawn(move || {
            let handler = {
                let served = served.clone();
                move |request: pom_rpc::Request, context: &RequestContext| {
                    answer(&served, request, context)
                }
            };
            let accepted = Connection::builder(server)
                .handler(handler)
                .accept(move |_| {
                    let mut hello = Hello::new(pom_remote::VERSION);
                    hello.server_id = Some("test".into());
                    hello.features = features;
                    Ok(hello)
                });
            if let (Ok((connection, _)), Ok(mut connections)) =
                (accepted, served.connections.lock())
            {
                connections.push(connection);
            }
        });
        Ok(client)
    });
    let host = pom_remote::Host {
        name: "dev".into(),
        url: pom_remote::HostUrl::parse("ssh://dev.example.net").expect("url"),
        pom_path: None,
        transport: None,
    };
    (
        ServerSource::with_opener(host, "/srv/myproject/pom.yml".into(), opener),
        served,
    )
}

/// What a server of PR (c) listed: the reads, actions and terminals, no watch and no shared events.
fn older_features() -> Vec<String> {
    let mut features: Vec<String> = REQUIRED.iter().map(|name| name.to_string()).collect();
    features.extend(pty::METHODS.iter().map(|name| name.to_string()));
    features
}

fn current_features() -> Vec<String> {
    METHODS
        .iter()
        .chain(FEATURES)
        .map(|name| name.to_string())
        .collect()
}

fn attach_two(source: &ServerSource) -> Vec<pty::Terminal> {
    source.workspaces().expect("connect");
    let terminals = source.pty("main").expect("terminals");
    (1..=2)
        .map(|index| {
            terminals
                .attach(&Request {
                    target: Target::Holder(format!("term-myproject-main-{index}")),
                    cols: 80,
                    rows: 24,
                    mode: Mode::ReadWrite,
                    primary: true,
                    claim: None,
                })
                .expect("attach")
        })
        .collect()
}

fn attached(served: &Served) -> Vec<AttachParams> {
    served
        .attaches
        .lock()
        .map(|attaches| attaches.clone())
        .unwrap_or_default()
}

#[test]
fn an_older_server_without_pushes_is_polled() {
    let (source, served) = source(older_features());
    let source = Arc::new(source);
    let timing = Timing {
        poll: Duration::from_millis(50),
        safety: Duration::from_secs(60),
    };
    let watch = Watch::start_with(source.clone(), "", Arc::new(|| {}), timing);
    watch.set_visible(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while served.listings.load(Ordering::SeqCst) < 5 {
        assert!(Instant::now() < deadline, "the older server was not polled");
        std::thread::sleep(Duration::from_millis(20));
    }
    let transport = source.transport().expect("transport");
    assert!(!transport.pushes && !transport.agents && transport.terminals);
    assert!(!watch.snapshot().pushing);
    assert!(matches!(
        source.start_agent("main", "claude"),
        Err(pom_remote::SourceError::Failed(_))
    ));
}

#[test]
fn an_older_server_gets_an_events_channel_per_terminal() {
    let (source, served) = source(older_features());
    let _terminals = attach_two(&source);
    let attaches = attached(&served);
    assert_eq!(attaches.len(), 2);
    assert!(attaches.iter().all(|params| !params.shared_events));
    assert_ne!(attaches[0].events, attaches[1].events);
}

#[test]
fn a_current_server_shares_one_events_channel() {
    let (source, served) = source(current_features());
    let _terminals = attach_two(&source);
    let attaches = attached(&served);
    assert_eq!(attaches.len(), 2);
    assert!(attaches.iter().all(|params| params.shared_events));
    assert!(attaches[0].events.is_some());
    assert_eq!(attaches[0].events, attaches[1].events);
    let transport = source.transport().expect("transport");
    assert!(transport.pushes && transport.agents);
}
