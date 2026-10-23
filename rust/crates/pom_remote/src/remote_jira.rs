//! Jira for a project on another machine. The machine's own settings come first, read and used there so its
//! token stays there; without them, this Mac's settings for the remote project (`<project>@<host>`), then
//! those of the local project of the same name, asked from here.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_jira::{
    Board, Issue, IssueDetail, Jira, JiraError, JiraSetup, SetupChange, SetupStore, SprintIssue,
    TicketList,
};
use pom_paths::StateDir;
use serde::de::DeserializeOwned;

use crate::server_api::{method, JiraAccount, JiraParams};
use crate::ProjectSource;

/// Settings can change on either machine; look again this often.
const RECHECK: Duration = Duration::from_secs(60);

/// The key this Mac keeps a remote project's Jira cache and its own settings for it under.
pub fn remote_jira_session(project: &str, host: &str) -> String {
    format!("{project}@{host}")
}

/// Jira and its settings through the project's machine.
#[derive(Clone)]
pub struct HostJira {
    source: Arc<dyn ProjectSource>,
}

impl HostJira {
    pub fn new(source: Arc<dyn ProjectSource>) -> HostJira {
        HostJira { source }
    }

    fn ask<T: DeserializeOwned>(&self, name: &str, params: JiraParams) -> Result<T, String> {
        let value = self
            .source
            .jira_request(name, params)
            .map_err(|error| error.to_string())?;
        serde_json::from_value(value).map_err(|error| format!("{name}: {error}"))
    }

    fn read<T: DeserializeOwned>(&self, name: &str, params: JiraParams) -> Result<T, JiraError> {
        self.ask(name, params).map_err(JiraError::Remote)
    }
}

impl Jira for HostJira {
    fn myself(&self) -> Result<(String, String), JiraError> {
        let account: JiraAccount = self.read(method::JIRA_TEST, JiraParams::default())?;
        Ok((account.name, account.email))
    }

    fn boards(&self) -> Result<Vec<Board>, JiraError> {
        self.read(method::JIRA_BOARDS, JiraParams::default())
    }

    fn issues(&self, list: TicketList) -> Result<Vec<SprintIssue>, JiraError> {
        let params = JiraParams {
            list: list.encode(),
            ..JiraParams::default()
        };
        self.read(method::JIRA_LIST, params)
    }

    fn search_by_keys(&self, keys: &[String]) -> Result<Vec<Issue>, JiraError> {
        let params = JiraParams {
            keys: keys.to_vec(),
            ..JiraParams::default()
        };
        self.read(method::JIRA_SEARCH, params)
    }

    fn issue_detail(&self, key: &str) -> Result<IssueDetail, JiraError> {
        let params = JiraParams {
            key: key.to_string(),
            ..JiraParams::default()
        };
        self.read(method::JIRA_ISSUE, params)
    }
}

impl SetupStore for HostJira {
    fn read(&self) -> Result<JiraSetup, String> {
        self.ask(method::JIRA_SETTINGS, JiraParams::default())
    }

    fn change(&self, change: &SetupChange) -> Result<JiraSetup, String> {
        let params = JiraParams {
            change: change.clone(),
            ..JiraParams::default()
        };
        self.ask(method::JIRA_SAVE, params)
    }

    fn test(&self) -> Result<String, String> {
        Jira::myself(self)
            .map(|(name, email)| format!("{name} ({email})"))
            .map_err(|error| error.to_string())
    }
}

/// Whose settings a remote project's Jira is read with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JiraPlace {
    Host,
    /// This Mac's settings for that session.
    Here(String),
}

pub struct RemoteJira {
    host: HostJira,
    state: StateDir,
    /// This Mac's sessions to fall back on, in order.
    sessions: Vec<String>,
    chosen: Mutex<Option<(Instant, Option<JiraPlace>)>>,
}

impl RemoteJira {
    pub fn new(
        source: Arc<dyn ProjectSource>,
        state: StateDir,
        project: &str,
        host: &str,
    ) -> RemoteJira {
        RemoteJira {
            host: HostJira::new(source),
            state,
            sessions: vec![remote_jira_session(project, host), project.to_string()],
            chosen: Mutex::new(None),
        }
    }

    /// Where Jira is read from now, looking again when the last look is a minute old. May block on the host.
    pub fn place(&self) -> Option<JiraPlace> {
        let previous = self
            .chosen
            .lock()
            .map(|chosen| chosen.clone())
            .unwrap_or_default();
        if let Some((at, place)) = &previous {
            if at.elapsed() < RECHECK {
                return place.clone();
            }
        }
        let place = match SetupStore::read(&self.host) {
            Ok(setup) if setup.ready() => Some(JiraPlace::Host),
            Err(_)
                if previous
                    .as_ref()
                    .is_some_and(|(_, place)| *place == Some(JiraPlace::Host)) =>
            {
                Some(JiraPlace::Host)
            }
            _ => self
                .sessions
                .iter()
                .find(|session| {
                    JiraSetup::read(&self.state, session, &|name| std::env::var(name).ok()).ready()
                })
                .map(|session| JiraPlace::Here(session.clone())),
        };
        if let Ok(mut chosen) = self.chosen.lock() {
            *chosen = Some((Instant::now(), place.clone()));
        }
        place
    }

    /// What the last look found, without looking: `None` before the first.
    pub fn known(&self) -> Option<Option<JiraPlace>> {
        self.chosen
            .lock()
            .ok()
            .and_then(|chosen| chosen.as_ref().map(|(_, place)| place.clone()))
    }

    /// Look again on the next ask (the settings just changed).
    pub fn forget(&self) {
        if let Ok(mut chosen) = self.chosen.lock() {
            *chosen = None;
        }
    }

    pub fn connect(self: &Arc<Self>) -> pom_jira::Connect {
        let remote = self.clone();
        Arc::new(move || match remote.place()? {
            JiraPlace::Host => Some(Arc::new(remote.host.clone()) as Arc<dyn Jira>),
            JiraPlace::Here(session) => pom_jira::resolve(&remote.state, &session)
                .map(|client| Arc::new(client) as Arc<dyn Jira>),
        })
    }

    pub fn host_store(&self) -> Arc<dyn SetupStore> {
        Arc::new(self.host.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentTurn, ServiceAction, SourceError, StatusReport, WorkspaceList};

    /// A host whose pom keeps `setup`, or none at all (`None`: a pom from before `jira.*`).
    struct Host {
        setup: Mutex<Option<JiraSetup>>,
        asked: Mutex<Vec<String>>,
    }

    impl ProjectSource for Host {
        fn location(&self) -> String {
            "dev".into()
        }
        fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
            Ok(WorkspaceList::default())
        }
        fn status(&self, _branch: &str) -> Result<StatusReport, SourceError> {
            Ok(StatusReport::default())
        }
        fn logs(&self, _branch: &str, _service: &str) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn agents(&self, _branch: &str) -> Result<Vec<pom_agent::SessionView>, SourceError> {
            Ok(Vec::new())
        }
        fn agent_turn(&self, _branch: &str, _role: &str) -> Result<Option<AgentTurn>, SourceError> {
            Ok(None)
        }
        fn act(
            &self,
            _branch: &str,
            _action: ServiceAction,
            _target: &str,
        ) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn jira_request(
            &self,
            name: &str,
            params: JiraParams,
        ) -> Result<serde_json::Value, SourceError> {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push(name.to_string());
            }
            let setup = self
                .setup
                .lock()
                .map_err(|_| SourceError::Failed("poisoned".into()))?
                .clone()
                .ok_or_else(|| SourceError::Failed("needs a newer pom there".into()))?;
            match name {
                method::JIRA_SETTINGS => Ok(serde_json::to_value(setup).unwrap_or_default()),
                method::JIRA_SEARCH => {
                    Ok(serde_json::json!([{ "key": params.keys[0], "status": "Done" }]))
                }
                other => Err(SourceError::Failed(format!("unexpected {other}"))),
            }
        }
    }

    fn ready() -> JiraSetup {
        JiraSetup {
            site: "example.atlassian.net".into(),
            email: "you@example.com".into(),
            token: Some(pom_jira::TokenOrigin::Secret),
        }
    }

    fn set_up_here(state: &StateDir, session: &str) -> Result<(), String> {
        pom_jira::SetupChange {
            site: Some("example.atlassian.net".into()),
            email: Some("you@example.com".into()),
            token: Some("t0k".into()),
            clear_token: false,
        }
        .apply(state, session)
    }

    fn remote(setup: Option<JiraSetup>, state: &StateDir) -> (Arc<RemoteJira>, Arc<Host>) {
        let host = Arc::new(Host {
            setup: Mutex::new(setup),
            asked: Mutex::new(Vec::new()),
        });
        let jira = Arc::new(RemoteJira::new(
            host.clone(),
            state.clone(),
            "myproject",
            "dev",
        ));
        (jira, host)
    }

    #[test]
    fn the_host_settings_win_and_its_answers_come_through_it() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        set_up_here(&state, "myproject")?;
        let (jira, _) = remote(Some(ready()), &state);
        assert_eq!(jira.known(), None);
        assert_eq!(jira.place(), Some(JiraPlace::Host));
        assert_eq!(jira.known(), Some(Some(JiraPlace::Host)));
        let client = jira.connect()().ok_or("connected")?;
        let found = client
            .search_by_keys(&["PROJ-101".into()])
            .map_err(|error| error.to_string())?;
        assert_eq!(found[0].status, "Done");
        Ok(())
    }

    #[test]
    fn without_host_settings_this_mac_answers_remote_key_first() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        let (jira, host) = remote(Some(JiraSetup::default()), &state);
        assert_eq!(jira.place(), None, "set up nowhere");
        set_up_here(&state, "myproject")?;
        jira.forget();
        assert_eq!(jira.place(), Some(JiraPlace::Here("myproject".into())));
        set_up_here(&state, "myproject@dev")?;
        jira.forget();
        assert_eq!(jira.place(), Some(JiraPlace::Here("myproject@dev".into())));
        let looks = host
            .asked
            .lock()
            .map(|asked| asked.len())
            .unwrap_or_default();
        assert_eq!(jira.place(), Some(JiraPlace::Here("myproject@dev".into())));
        assert_eq!(
            host.asked
                .lock()
                .map(|asked| asked.len())
                .unwrap_or_default(),
            looks,
            "a minute between looks"
        );
        Ok(())
    }

    #[test]
    fn an_older_host_falls_back_to_this_mac() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        set_up_here(&state, "myproject")?;
        let (jira, _) = remote(None, &state);
        assert_eq!(jira.place(), Some(JiraPlace::Here("myproject".into())));
        Ok(())
    }
}
