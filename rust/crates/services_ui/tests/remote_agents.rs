//! Agent tabs of a project on another machine come back attached to their agents, and an agent that no longer
//! runs gets no tab; the lease of a tab is read from the pushed presence.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pom_agent::SessionView;
use pom_remote::server_api::{AgentPresence, AgentsUpdate};
use pom_remote::{
    AgentTurn, Feed, FeedEvent, ProjectSource, ServiceAction, SourceError, StatusReport, Update,
    WorkspaceList, WorkspaceSummary,
};
use pom_rpc::pty::{AttachParams, Attached, PtyProvider, Request, Target, Terminal};
use pom_rpc::{Connection, RemoteError, RequestContext, StreamSender};
use services_ui::{restore_agent_tabs, RemoteAgentTabs, RemoteLease};
use terminal_ui::LeaseSource;

const DEV: &str = "ws-myproject-feat-login-claude-raw";

fn serve(
    request: pom_rpc::Request,
    context: &RequestContext,
) -> Result<serde_json::Value, RemoteError> {
    let params: AttachParams = serde_json::from_value(request.params)
        .map_err(|error| RemoteError::new("invalid_params", error.to_string()))?;
    let sender: StreamSender = context
        .connection()
        .attach_stream(params.channel, 0)
        .map_err(|error| RemoteError::new("failed", error.to_string()))?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(10));
        drop(sender);
    });
    serde_json::to_value(Attached {
        holder: params.holder,
        ..Attached::default()
    })
    .map_err(|error| RemoteError::new("internal", error.to_string()))
}

/// Attaches only to the running agent `dev`, as the server would.
struct Agents {
    client: Connection,
    _server: Connection,
}

impl PtyProvider for Agents {
    fn attach(&self, request: &Request) -> Result<Terminal, String> {
        let holder = match &request.target {
            Target::Agent(role) if role == "dev" => DEV.to_string(),
            other => return Err(format!("{other:?} is not running")),
        };
        let params = AttachParams {
            holder,
            cols: request.cols,
            rows: request.rows,
            ..AttachParams::default()
        };
        Terminal::attach(&self.client, params, Duration::from_secs(5))
            .map_err(|error| error.to_string())
    }

    fn close(&self, _holder: &str) -> Result<(), String> {
        Ok(())
    }
}

struct Pushed {
    sent: AtomicBool,
}

impl Feed for Pushed {
    fn next(&self, timeout: Duration) -> FeedEvent {
        if self.sent.swap(true, Ordering::SeqCst) {
            std::thread::sleep(timeout);
            return FeedEvent::Idle;
        }
        FeedEvent::Update(Update::Agents(AgentsUpdate {
            branch: "feat-login".into(),
            sessions: sessions(),
            presence: vec![AgentPresence {
                holder: DEV.into(),
                driven_by: Some("an orchestrator".into()),
                pending: None,
            }],
            error: None,
        }))
    }

    fn follow(&self, _branch: &str, _turn: Option<&str>) -> Result<(), SourceError> {
        Ok(())
    }
}

fn sessions() -> Vec<SessionView> {
    vec![SessionView {
        role: "dev".into(),
        holder: DEV.into(),
        alive: true,
        state: "idle".into(),
        ..SessionView::default()
    }]
}

struct Host {
    agents: Arc<Agents>,
}

impl ProjectSource for Host {
    fn location(&self) -> String {
        "dev (dev@dev.example.net)".into()
    }
    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        Ok(WorkspaceList {
            workspaces: vec![WorkspaceSummary {
                branch: "feat-login".into(),
                ..WorkspaceSummary::default()
            }],
            ..WorkspaceList::default()
        })
    }
    fn status(&self, _branch: &str) -> Result<StatusReport, SourceError> {
        Ok(StatusReport::default())
    }
    fn logs(&self, _branch: &str, _service: &str) -> Result<String, SourceError> {
        Ok(String::new())
    }
    fn agents(&self, _branch: &str) -> Result<Vec<SessionView>, SourceError> {
        Ok(sessions())
    }
    fn agent_turn(&self, _: &str, _: &str) -> Result<Option<AgentTurn>, SourceError> {
        Ok(None)
    }
    fn act(&self, _: &str, _: ServiceAction, _: &str) -> Result<String, SourceError> {
        Ok(String::new())
    }
    fn pty(&self, _branch: &str) -> Option<Arc<dyn PtyProvider>> {
        Some(self.agents.clone() as Arc<dyn PtyProvider>)
    }
    fn events(
        &self,
        _branch: &str,
        _turn: Option<&str>,
    ) -> Result<Option<Arc<dyn Feed>>, SourceError> {
        Ok(Some(Arc::new(Pushed {
            sent: AtomicBool::new(false),
        })))
    }
}

#[test]
fn saved_remote_agent_tabs_reattach_to_running_agents_only() {
    let (client, server) =
        pom_rpc::mock::connected(pom_rpc::Config::default(), serve).expect("connected");
    let host = Arc::new(Host {
        agents: Arc::new(Agents {
            client,
            _server: server,
        }),
    });
    let watch = pom_remote::Watch::start(host, "feat-login", Arc::new(|| {}));
    watch.set_visible(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while watch.snapshot().presence.is_empty() {
        assert!(Instant::now() < deadline, "no presence pushed");
        std::thread::sleep(Duration::from_millis(10));
    }

    let temp = tempfile::tempdir().expect("temp");
    let tabs = RemoteAgentTabs::new(temp.path().join("remote-agents.json"));
    tabs.save(
        "dev",
        "myproject",
        "feat-login",
        &["dev".to_string(), "gone".to_string()],
    )
    .expect("save");
    let mut number = 10;
    let restored = restore_agent_tabs(&watch, &tabs, "dev", "myproject", &mut number);
    assert_eq!(number, 12);
    assert_eq!(
        restored.len(),
        1,
        "an agent that no longer runs gets no tab"
    );
    let (id, item) = &restored[0];
    assert_eq!(id, "agent:feat-login/dev");
    let holder = item
        .as_any()
        .and_then(|any| any.downcast_ref::<terminal_ui::TerminalItem>())
        .and_then(|terminal| terminal.terminal().holder_name());
    assert_eq!(
        holder.as_deref(),
        Some(DEV),
        "the tab is attached to the agent"
    );
    assert!(
        restore_agent_tabs(&watch, &tabs, "lab", "myproject", &mut number).is_empty(),
        "another host's tabs are its own"
    );

    let lease = RemoteLease::new(watch.clone(), "dev");
    assert_eq!(lease.state().driver.as_deref(), Some("an orchestrator"));
}
