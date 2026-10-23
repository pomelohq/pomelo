//! The Services panel over a project on another machine, from a stand-in host: what its server reports is
//! what the panel shows, and a server too old for a control leaves it out with the reason.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pom_remote::server_api::{service_status, ServiceFact, ServicesFacts};
use pom_remote::{
    AgentTurn, ProjectSource, ServiceAction, ServiceOffers, SourceError, StatusReport,
    TransportInfo, Via, Watch, WorkspaceList, WorkspaceSummary,
};
use services_ui::{RemoteServices, ServiceControl, ServicesAccess, ServicesPanel, Status};

const CONFIG: &str = "session: myproject\nrepos:\n  api:\n    services:\n      web:\n        type: backend\n        port: true\n        mode: dev\n        modes:\n          dev: bin/web\n";

struct Host {
    offers: ServiceOffers,
}

impl ProjectSource for Host {
    fn location(&self) -> String {
        "dev".into()
    }
    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        Ok(WorkspaceList {
            workspaces: vec![WorkspaceSummary {
                branch: "feat".into(),
                ..WorkspaceSummary::default()
            }],
            ..WorkspaceList::default()
        })
    }
    fn status(&self, _: &str) -> Result<StatusReport, SourceError> {
        Ok(StatusReport::default())
    }
    fn logs(&self, _: &str, _: &str) -> Result<String, SourceError> {
        Ok(String::new())
    }
    fn agents(&self, _: &str) -> Result<Vec<pom_agent::SessionView>, SourceError> {
        Ok(Vec::new())
    }
    fn agent_turn(&self, _: &str, _: &str) -> Result<Option<AgentTurn>, SourceError> {
        Ok(None)
    }
    fn act(&self, _: &str, _: ServiceAction, _: &str) -> Result<String, SourceError> {
        Ok(String::new())
    }
    fn transport(&self) -> Option<TransportInfo> {
        Some(TransportInfo {
            via: Via::Server,
            services: self.offers,
            ..TransportInfo::default()
        })
    }
    fn services_facts(&self, branch: &str) -> Result<ServicesFacts, SourceError> {
        Ok(ServicesFacts {
            branch: branch.into(),
            root: "/home/dev/pom/myproject/workspace--feat".into(),
            config: CONFIG.into(),
            services: vec![ServiceFact {
                repo: "api".into(),
                service: "web".into(),
                holder: "svc-myproject-feat-api-web".into(),
                status: service_status::RUNNING.into(),
                port: Some(3104),
                mode: "dev".into(),
                url: Some("http://web.api.feat.localhost:8767".into()),
                ..ServiceFact::default()
            }],
            repos: vec!["api".into()],
            ..ServicesFacts::default()
        })
    }
}

fn watching(offers: ServiceOffers) -> (Watch, RemoteServices) {
    let watch = Watch::start(Arc::new(Host { offers }), "feat", Arc::new(|| {}));
    watch.set_visible(true);
    let services = RemoteServices::new(watch.clone(), "feat", "dev");
    (watch, services)
}

#[test]
fn what_the_host_reports_is_what_the_panel_shows() {
    let offers = ServiceOffers {
        facts: true,
        ..ServiceOffers::default()
    };
    let (watch, services) = watching(offers);
    let deadline = Instant::now() + Duration::from_secs(10);
    while watch.services().0.is_none() {
        assert!(Instant::now() < deadline, "no facts");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(services.unavailable(), None);
    let config = services.config().expect("the host's pom.yml");
    assert_eq!(config.session, "myproject");
    let services = Arc::new(services);
    let panel = ServicesPanel::with_access(
        services.clone(),
        services_ui::WorkspaceInfo {
            branch: "feat".into(),
            ticket: String::new(),
            is_main: false,
        },
        Arc::new(|| {}),
        "/home/dev/pom/myproject/workspace--feat".into(),
    );
    assert_eq!(panel.status("api", "web"), Status::Running);
    let target = pom_services::ServiceTarget {
        branch: "feat".into(),
        is_main: false,
        repo: "api".into(),
        service: "web".into(),
    };
    assert_eq!(services.port(&config, &target), Some(3104));
    let refused = services
        .browser_url(&config, &target)
        .expect_err("a localhost proxy on the host does not open here");
    assert!(refused.contains("answers on dev only"), "{refused}");
    for control in [
        ServiceControl::Relocate,
        ServiceControl::Shared,
        ServiceControl::Mode,
        ServiceControl::EnvProfile,
        ServiceControl::Tasks,
    ] {
        let reason = services.missing(control).expect("hidden");
        assert!(reason.contains("newer pom on dev"), "{reason}");
    }
}

#[test]
fn a_server_without_the_panel_says_to_update_pom() {
    let (_watch, services) = watching(ServiceOffers::default());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(reason) = services.unavailable() {
            if reason.contains("newer pom") {
                assert_eq!(reason, "Services need a newer pom on dev; update it there");
                break;
            }
        }
        assert!(Instant::now() < deadline, "{:?}", services.unavailable());
        std::thread::sleep(Duration::from_millis(10));
    }
}
