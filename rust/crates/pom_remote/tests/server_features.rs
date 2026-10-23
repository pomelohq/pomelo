//! The app against in-process servers that list different features in their HELLO: one from before pushes and
//! shared terminal events is polled and gets an events channel per terminal; a newer one shares the channel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_remote::server_api::{
    method, AgentListing, BranchArgs, JobRequest, UsageParams, UsageReport, FEATURES, FILES_SEARCH,
    JOBS, LIVE_LOGS, METHODS, MODULE_STORE, REQUIRED,
};
use pom_remote::{ProjectSource, ServerSource, StatusReport, Timing, Watch, WorkspaceList};
use pom_rpc::pty::{self, AttachParams, Attached, Mode, Request, Target};
use pom_rpc::{Connection, Hello, Publisher, RemoteError, RequestContext, StreamSender};

#[derive(Default)]
struct Served {
    listings: AtomicUsize,
    usage_asks: AtomicUsize,
    logs: AtomicUsize,
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
        method::SVC_LOGS => {
            let asked = served.logs.fetch_add(1, Ordering::SeqCst);
            json(serde_json::to_value(format!("web says {asked}\n")))
        }
        method::AGENT_LIST => json(serde_json::to_value(AgentListing::default())),
        method::JIRA_SETTINGS => {
            let params: pom_remote::server_api::JiraParams = serde_json::from_value(request.params)
                .map_err(|error| RemoteError::new("invalid_params", error.to_string()))?;
            json(serde_json::to_value(pom_jira::JiraSetup {
                site: params.cfg,
                ..pom_jira::JiraSetup::default()
            }))
        }
        method::USAGE_GET => {
            served.usage_asks.fetch_add(1, Ordering::SeqCst);
            let params: UsageParams = serde_json::from_value(request.params)
                .map_err(|error| RemoteError::new("invalid_params", error.to_string()))?;
            json(serde_json::to_value(UsageReport {
                account: Some(agent_usage::Account {
                    name: "vm-dev".into(),
                    ..agent_usage::Account::default()
                }),
                limits_at: Some(params.since),
                ..UsageReport::default()
            }))
        }
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
    let opener: pom_remote::Opener = Arc::new(move |_reconnect| {
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
fn files_open_only_through_a_server_that_reads_them() {
    let (older, _) = source(older_features());
    older.workspaces().expect("connect");
    assert!(!older.transport().expect("transport").files);
    assert!(
        older.files("main").is_none(),
        "an older server keeps the page"
    );
    assert!(!older.transport().expect("transport").files_write);
    let (current, _) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").files);
    assert!(current.transport().expect("transport").files_write);
    assert!(current.files("main").is_some());
}

#[test]
fn a_server_that_reads_files_but_predates_search_says_to_update_it() {
    let before_search: Vec<String> = current_features()
        .into_iter()
        .filter(|name| {
            ![FILES_SEARCH, method::SEARCH_START, method::SEARCH_CANCEL].contains(&name.as_str())
        })
        .collect();
    let (older, _) = source(before_search);
    older.workspaces().expect("connect");
    let transport = older.transport().expect("transport");
    assert!(transport.files && !transport.files_search);
    let refusal = older
        .files("main")
        .expect("files")
        .search_refusal()
        .unwrap_or_default();
    assert!(
        refusal.contains("cannot search its files here") && refusal.contains("Update it"),
        "{refusal}"
    );
    let (current, _) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").files_search);
    assert_eq!(current.files("main").expect("files").search_refusal(), None);
}

#[test]
fn usage_comes_only_from_a_server_that_reports_it() {
    let (older, served) = source(older_features());
    older.workspaces().expect("connect");
    assert!(!older.transport().expect("transport").usage);
    assert_eq!(older.usage(100), Ok(None), "an older server hides the chip");
    assert_eq!(served.usage_asks.load(Ordering::SeqCst), 0);
    let (current, served) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").usage);
    let report = current.usage(100).expect("asked").expect("reported");
    assert_eq!(
        report.account.map(|account| account.name),
        Some("vm-dev".into())
    );
    assert_eq!(report.limits_at, Some(100), "the day the app asked for");
    assert_eq!(served.usage_asks.load(Ordering::SeqCst), 1);
}

#[test]
fn jira_goes_through_a_server_that_keeps_its_settings() {
    let (older, _) = source(older_features());
    older.workspaces().expect("connect");
    assert!(!older.transport().expect("transport").jira);
    let refused = older.jira_request(method::JIRA_SETTINGS, Default::default());
    assert!(matches!(refused, Err(pom_remote::SourceError::Failed(_))));
    let (current, _) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").jira);
    let setup = pom_remote::HostJira::new(Arc::new(current));
    let read = pom_jira::SetupStore::read(&setup).expect("settings");
    assert_eq!(
        read.site, "/srv/myproject/pom.yml",
        "asked about this project"
    );
}

#[test]
fn an_older_server_is_asked_to_update_before_an_image_is_pasted() {
    let (older, _) = source(older_features());
    older.workspaces().expect("connect");
    assert!(!older.transport().expect("transport").paste);
    let refused = older.paste_file(b"\x89PNG", "png");
    let Err(pom_remote::SourceError::Failed(message)) = refused else {
        panic!("not refused: {refused:?}");
    };
    assert!(message.contains("needs a newer pom on"), "{message}");
    let (current, _) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").paste);
}

#[test]
fn only_a_server_that_lists_the_store_runs_its_requests() {
    let before_store: Vec<String> = current_features()
        .into_iter()
        .filter(|name| name != MODULE_STORE)
        .collect();
    let (older, _) = source(before_store);
    older.workspaces().expect("connect");
    assert!(!older.transport().expect("transport").module_store);
    let (current, _) = source(current_features());
    current.workspaces().expect("connect");
    assert!(current.transport().expect("transport").module_store);
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

#[test]
fn a_server_without_live_logs_or_jobs_is_asked_for_the_output_every_poll() {
    let features: Vec<String> = current_features()
        .into_iter()
        .filter(|name| !LIVE_LOGS.contains(&name.as_str()) && !JOBS.contains(&name.as_str()))
        .collect();
    let (source, served) = source(features);
    let source = Arc::new(source);
    let timing = Timing {
        poll: Duration::from_millis(50),
        safety: Duration::from_secs(60),
    };
    let watch = Watch::start_with(source.clone(), "main", Arc::new(|| {}), timing);
    watch.set_visible(true);
    watch.show_logs(Some("api/web".into()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while served.logs.load(Ordering::SeqCst) < 4 {
        assert!(
            Instant::now() < deadline,
            "the output was not asked for again"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let snapshot = watch.snapshot();
    assert!(!snapshot.logs_live);
    assert!(matches!(&snapshot.logs, Some((_, Ok(text))) if text.starts_with("web says")));
    let transport = source.transport().expect("transport");
    assert!(!transport.live_logs && !transport.jobs && transport.pushes);
    assert!(matches!(
        source.start_job(
            &JobRequest::Wake(BranchArgs {
                branch: "main".into(),
                force: false
            }),
            "op-1"
        ),
        Err(pom_remote::SourceError::Failed(_))
    ));
}

/// A host whose server can be cut off, made unreachable, or replaced by a new one.
#[derive(Default)]
struct Flaky {
    unreachable: std::sync::atomic::AtomicBool,
    /// The running server's id; bumped to stand for a restart.
    generation: AtomicUsize,
    /// The generation `--reconnect` still finds; another one means the old server is gone.
    reachable_generation: AtomicUsize,
    dials: Mutex<Vec<bool>>,
}

fn flaky_source() -> (ServerSource, Arc<Served>, Arc<Flaky>) {
    let served = Arc::new(Served::default());
    let flaky = Arc::new(Flaky::default());
    let (opened, host_state) = (served.clone(), flaky.clone());
    let opener: pom_remote::Opener = Arc::new(move |reconnect| {
        if let Ok(mut dials) = host_state.dials.lock() {
            dials.push(reconnect);
        }
        if host_state.unreachable.load(Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "ssh: connect to host: Connection refused",
            ));
        }
        let generation = host_state.generation.load(Ordering::SeqCst);
        if reconnect && host_state.reachable_generation.load(Ordering::SeqCst) != generation {
            host_state
                .reachable_generation
                .store(generation, Ordering::SeqCst);
            return Err(std::io::Error::other(
                pom_remote::server_transport::SERVER_GONE,
            ));
        }
        let (client, server) = pom_rpc::mock::pair()?;
        let served = opened.clone();
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
                    hello.server_id = Some(format!("server-{generation}"));
                    hello.features = current_features();
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
    let source = ServerSource::with_opener(host, "/srv/myproject/pom.yml".into(), opener);
    source.set_backoff(vec![Duration::from_millis(40); 3]);
    (source, served, flaky)
}

fn cut(served: &Served) {
    let connections = served
        .connections
        .lock()
        .map(|connections| connections.clone())
        .unwrap_or_default();
    for connection in connections {
        connection.close();
    }
}

fn wait_link(source: &ServerSource, what: &str, done: impl Fn(&pom_remote::LinkReport) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = source.link().expect("a session");
        if done(&report) {
            return;
        }
        assert!(Instant::now() < deadline, "no {what}: {report:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_dropped_session_comes_back_with_backoff_and_a_new_server_counts_as_a_restart() {
    use pom_remote::LinkState;
    let (source, served, flaky) = flaky_source();
    assert!(source.link().is_none(), "no session before the first look");
    source.workspaces().expect("connect");
    assert_eq!(source.server_id(), "server-0");

    cut(&served);
    wait_link(&source, "reconnecting", |report| {
        matches!(report.state, LinkState::Reconnecting { attempt: 1, .. })
    });
    wait_link(&source, "the same server again", |report| {
        report.state == LinkState::Connected && report.restarts == 0
    });
    assert_eq!(source.server_id(), "server-0");
    assert_eq!(
        flaky
            .dials
            .lock()
            .map(|dials| dials.clone())
            .unwrap_or_default(),
        vec![false, true],
        "the reconnect resumes rather than starting a server"
    );
    assert!(source.workspaces().is_ok());

    flaky.unreachable.store(true, Ordering::SeqCst);
    cut(&served);
    wait_link(&source, "attempt 3", |report| {
        matches!(report.state, LinkState::Reconnecting { attempt: 3, .. })
    });
    wait_link(&source, "disconnected", |report| {
        matches!(report.state, LinkState::Disconnected { .. })
    });
    assert!(
        matches!(
            source.workspaces(),
            Err(pom_remote::SourceError::Unreachable(_))
        ),
        "a disconnected source answers at once"
    );
    let dials = flaky
        .dials
        .lock()
        .map(|dials| dials.len())
        .unwrap_or_default();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        flaky
            .dials
            .lock()
            .map(|dials| dials.len())
            .unwrap_or_default(),
        dials,
        "no attempts until asked"
    );

    flaky.unreachable.store(false, Ordering::SeqCst);
    flaky.generation.store(1, Ordering::SeqCst);
    source.reconnect_now();
    wait_link(&source, "a restarted server", |report| {
        report.state == LinkState::Connected && report.restarts == 1
    });
    assert_eq!(source.server_id(), "server-1");
    assert!(source.workspaces().is_ok());
}
