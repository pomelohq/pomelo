//! `jira.*`: Jira read with the settings and token kept on this machine for the project, so the token never
//! leaves it.

use std::sync::Arc;

use pom_jira::{JiraSetup, TicketList, Transport};
use pom_paths::StateDir;
use pom_remote::server_api::{method, JiraAccount, JiraParams};
use pom_rpc::{error_code, RemoteError};
use serde_json::Value;

use crate::dispatch::FAILED;

const MAX_KEYS: usize = 100;

pub type MakeTransport = Arc<dyn Fn() -> Box<dyn Transport> + Send + Sync>;
pub type ReadEnv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct JiraService {
    transport: MakeTransport,
    env: ReadEnv,
}

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn to_value(value: impl serde::Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

fn asked(error: pom_jira::JiraError) -> RemoteError {
    RemoteError::new(FAILED, error.to_string())
}

fn check_key(key: &str) -> Result<(), RemoteError> {
    if pom_jira::valid_key(key) {
        Ok(())
    } else {
        Err(invalid(format!("{key:?} is not a Jira ticket key")))
    }
}

impl JiraService {
    pub fn new(transport: MakeTransport, env: ReadEnv) -> JiraService {
        JiraService { transport, env }
    }

    pub fn from_env() -> JiraService {
        JiraService::new(
            Arc::new(|| Box::new(pom_jira::CurlTransport::default())),
            Arc::new(|name| std::env::var(name).ok()),
        )
    }

    fn client(&self, state: &StateDir, session: &str) -> Result<pom_jira::Client, RemoteError> {
        pom_jira::resolve_with(state, session, &*self.env, (self.transport)()).ok_or_else(|| {
            RemoteError::new(
                FAILED,
                format!("Jira is not set up for {session} on this machine"),
            )
        })
    }

    /// Answers `name` for the project whose session is `session`.
    pub fn answer(
        &self,
        state: &StateDir,
        session: &str,
        name: &str,
        params: JiraParams,
    ) -> Result<Value, RemoteError> {
        let setup = || JiraSetup::read(state, session, &*self.env);
        match name {
            method::JIRA_SETTINGS => to_value(setup()),
            method::JIRA_SAVE => {
                params
                    .change
                    .apply(state, session)
                    .map_err(|message| RemoteError::new(FAILED, message))?;
                to_value(setup())
            }
            method::JIRA_TEST => {
                let (name, email) = self.client(state, session)?.myself().map_err(asked)?;
                to_value(JiraAccount { name, email })
            }
            method::JIRA_SEARCH => {
                if params.keys.len() > MAX_KEYS {
                    return Err(invalid(format!("at most {MAX_KEYS} keys at once")));
                }
                for key in &params.keys {
                    check_key(key)?;
                }
                let client = self.client(state, session)?;
                to_value(client.search_by_keys(&params.keys).map_err(asked)?)
            }
            method::JIRA_ISSUE => {
                check_key(&params.key)?;
                let client = self.client(state, session)?;
                to_value(client.issue_detail(&params.key).map_err(asked)?)
            }
            method::JIRA_BOARDS => to_value(self.client(state, session)?.boards().map_err(asked)?),
            method::JIRA_LIST => {
                let list = TicketList::decode(&params.list).ok_or_else(|| {
                    invalid("list must be sprint:<board>, backlog:<board> or assigned")
                })?;
                let client = self.client(state, session)?;
                to_value(pom_jira::Jira::issues(&client, list).map_err(asked)?)
            }
            other => Err(RemoteError::new(
                error_code::METHOD_NOT_FOUND,
                format!("no method {other}"),
            )),
        }
    }
}
