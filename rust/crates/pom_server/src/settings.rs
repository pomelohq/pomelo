//! `settings.get` / `settings.set`: the host-scoped keys of this user's settings.json, changed only by a person.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pom_ptyhost::SocketDir;
use pom_remote::server_api::{HostSettings, SettingsSetParams};
use pom_rpc::{error_code, RemoteError};
use settings::HostSettingsError;

use crate::dispatch::FAILED;

/// Whether the process `pid` runs inside one of this machine's agents.
pub type AgentCheck = Arc<dyn Fn(i32) -> bool + Send + Sync>;

pub struct SettingsService {
    path: Option<PathBuf>,
    is_agent: AgentCheck,
    writing: Mutex<()>,
}

fn answered(
    path: &std::path::Path,
    result: Result<serde_json::Value, HostSettingsError>,
) -> Result<HostSettings, RemoteError> {
    result
        .map(|values| HostSettings::new(values).kept_in(path))
        .map_err(|error| match error {
            HostSettingsError::File(message) => RemoteError::new(FAILED, message),
            other => RemoteError::new(error_code::INVALID_PARAMS, other.to_string()),
        })
}

impl SettingsService {
    pub fn new(path: Option<PathBuf>, is_agent: AgentCheck) -> SettingsService {
        SettingsService {
            path,
            is_agent,
            writing: Mutex::new(()),
        }
    }

    /// This user's settings.json, an agent told apart by the agent holders in `holders`.
    pub fn from_env(holders: SocketDir) -> SettingsService {
        SettingsService::new(
            settings::Settings::path(),
            Arc::new(move |pid| pom_agent::Caller::of_process(pid, &holders.holders()).is_agent()),
        )
    }

    fn path(&self) -> Result<&PathBuf, RemoteError> {
        self.path
            .as_ref()
            .ok_or_else(|| RemoteError::new(FAILED, "no home folder to keep settings.json in"))
    }

    pub fn get(&self) -> Result<HostSettings, RemoteError> {
        let path = self.path()?;
        answered(path, settings::read_host_settings(path))
    }

    /// `peer` is the process at the other end of the caller's socket: the `pom server proxy` it runs.
    pub fn set(
        &self,
        peer: Option<i32>,
        params: SettingsSetParams,
    ) -> Result<HostSettings, RemoteError> {
        let person = peer.is_some_and(|pid| !(self.is_agent)(pid));
        if !person {
            return Err(RemoteError::new(
                FAILED,
                "only a person can change this machine's settings, not an agent running on it",
            ));
        }
        let path = self.path()?;
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        answered(path, settings::write_host_settings(path, &params.values))
    }
}
