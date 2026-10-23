//! The Store page of a project on another machine, from a stand-in host whose pom cannot run the store's
//! requests: the page says so, names the host and asks to update pom there, instead of reading forever.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use module_store_ui::{RemoteStore, Request, StoreAccess, StoreState, StoreUpdate};
use pom_remote::{
    AgentTurn, ProjectSource, ServiceAction, SourceError, StatusReport, TransportInfo, Via,
    WorkspaceList,
};

struct Host {
    info: TransportInfo,
}

impl ProjectSource for Host {
    fn location(&self) -> String {
        "dev".into()
    }
    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        Ok(WorkspaceList::default())
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
        Some(self.info.clone())
    }
}

fn first_update(info: TransportInfo) -> StoreUpdate {
    let store = RemoteStore::new(Arc::new(Host { info }), "dev");
    let (sender, receiver) = mpsc::channel();
    store.start(
        Request::Refresh,
        None,
        Box::new(move |update| sender.send(update).is_ok()),
    );
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("an update")
}

#[test]
fn a_host_whose_pom_predates_the_store_asks_to_update_it() {
    let update = first_update(TransportInfo {
        via: Via::Server,
        remote_version: Some("0.9.6".into()),
        jobs: true,
        ..TransportInfo::default()
    });
    let StoreUpdate::Failed { message } = update.clone() else {
        panic!("{update:?}");
    };
    assert_eq!(
        message,
        "pom 0.9.6 on dev cannot show its node_modules store here. Update pom there to see it in this window."
    );

    let mut state = StoreState {
        busy: Some(Request::Refresh),
        ..StoreState::default()
    };
    state.apply(update, 0);
    state.ended();
    assert_eq!(state.unavailable.as_deref(), Some(message.as_str()));
    assert!(!state.loading() && !state.animating());
    assert!(state.busy.is_none());
}

#[test]
fn a_host_reached_over_plain_ssh_says_it_needs_pom_server() {
    let update = first_update(TransportInfo {
        via: Via::Ssh,
        ..TransportInfo::default()
    });
    let StoreUpdate::Failed { message } = update else {
        panic!("{update:?}");
    };
    assert!(
        message.contains("on dev needs pom server there"),
        "{message}"
    );
}
