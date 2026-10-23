//! Remote machines driven over SSH: the hosts the user named (`~/.config/pomelo/hosts.json`) and the `ssh`
//! command that runs `pom` on one of them, and the [`ProjectSource`] a view reads a project through, here or there.
//! Pomelo relies on the user's own SSH config and keys.

mod editor;
mod forward;
mod local;
mod remote_fs;
mod remote_git;
mod remote_jira;
mod remote_ports;
pub mod server_api;
mod server_source;
pub mod server_transport;
mod source;
mod ssh;
pub mod update;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use editor::{editor_argv, find_program};
pub use forward::{
    choose_port, forward_spec, port_free, ports_dir, recorded_ports, ForwardKind, ForwardRequest,
    ForwardState, Forwarder, LocalPort, PortForward, PortsRecord, PORTS_SCHEMA,
};
pub use local::{
    host_setting, local_projects, ordered_services, panel_targets, service_output, status_report,
    workspace_list, LocalSource, OpenedProject, SharedModes, WorkspaceTerminals, SHARED_LOG_LINES,
    WORKSPACE_REPO,
};
pub use pom_rpc::pty::LinkState;
pub use remote_fs::RemoteFs;
pub use remote_git::{repo_sources, RemoteGit};
pub use remote_jira::{remote_jira_session, HostJira, JiraPlace, RemoteJira};
pub use remote_ports::{
    reachable_notice, GatewayRoute, PortsSetup, RemotePorts, SharedPort, GATEWAY_LABEL,
};
pub use server_source::{project_source, Opener, ServerSource, VERSION};
pub use source::{
    remote_projects, run_pom, AgentTurn, FailedRun, Feed, FeedEvent, JobEvent, JobFeed, LastSync,
    LinkReport, LogEvent, LogFeed, ProjectEntry, ProjectSource, RemoteSource, ServiceAction,
    ServiceOffers, ServiceVerb, SourceError, StatusReport, TransportInfo, Update, Via,
    WorkspaceList, WorkspaceSummary, PROJECT_SCHEMA, WORKSPACE_SCHEMA,
};
pub use ssh::{
    control_dir, json_requested, parse_version, quote, remote_command, ssh_args, ssh_command,
    version_mismatch, Ssh, Tty,
};
pub use watch::{Connection, Snapshot, Timing, Watch, POLL_EVERY, SAFETY_REFRESH};

pub const SCHEMA: &str = "pom.hosts/v1";
const HOSTS_FILE: &str = "hosts.json";

/// `pom -H` exits with this when ssh itself fails (ssh's own 255), so it never passes for a remote exit code.
pub const EXIT_UNREACHABLE: i32 = 69;

/// Where an `ssh://user@host[:port]` URL points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUrl {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl HostUrl {
    pub fn parse(text: &str) -> Result<HostUrl, String> {
        let rest = text
            .strip_prefix("ssh://")
            .ok_or_else(|| format!("{text:?} is not an ssh:// URL (ssh://user@host[:port])"))?;
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains('/') {
            return Err(format!("{text:?} has a path; use ssh://user@host[:port]"));
        }
        let (user, authority) = match rest.rsplit_once('@') {
            Some((user, authority)) => (Some(user), authority),
            None => (None, rest),
        };
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, after) = bracketed
                .split_once(']')
                .ok_or_else(|| format!("{text:?}: unclosed [ in the host"))?;
            let port =
                match after {
                    "" => None,
                    _ => Some(after.strip_prefix(':').ok_or_else(|| {
                        format!("{text:?}: expected :port after the bracketed host")
                    })?),
                };
            (host, port)
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        let port = port
            .map(|port| {
                port.parse::<u16>()
                    .ok()
                    .filter(|port| *port > 0)
                    .ok_or_else(|| format!("{text:?}: {port:?} is not a port"))
            })
            .transpose()?;
        let user = user.map(str::to_string);
        if let Some(user) = &user {
            if !valid_word(user, false) {
                return Err(format!("{text:?}: {user:?} is not a valid user name"));
            }
        }
        if !valid_word(host, true) {
            return Err(format!("{text:?}: {host:?} is not a valid host name"));
        }
        Ok(HostUrl {
            user,
            host: host.to_string(),
            port,
        })
    }

    /// What ssh connects to: `user@host`, or `host` alone so the user's ssh config picks the user.
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// `user@host[:port]`, the form remote-capable editors take.
    pub fn alias(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let mut alias = match &self.user {
            Some(user) => format!("{user}@{host}"),
            None => host,
        };
        if let Some(port) = self.port {
            alias.push_str(&format!(":{port}"));
        }
        alias
    }

    pub fn url(&self) -> String {
        format!("ssh://{}", self.alias())
    }
}

/// Leading `-` is refused so a host or user can never be read by ssh as an option.
fn valid_word(word: &str, host: bool) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && word.chars().all(|char| {
            char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-') || (host && char == ':')
        })
}

pub fn valid_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name.len() <= 64
        && name
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostTransport {
    Server,
    Ssh,
}

impl HostTransport {
    pub fn parse(text: &str) -> Result<HostTransport, String> {
        match text {
            "server" => Ok(HostTransport::Server),
            "ssh" => Ok(HostTransport::Ssh),
            other => Err(format!("unknown transport {other:?} (server, ssh)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HostTransport::Server => "server",
            HostTransport::Ssh => "ssh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pom_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<HostTransport>,
}

/// A named host, its URL already checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub url: HostUrl,
    pub pom_path: Option<String>,
    pub transport: Option<HostTransport>,
}

impl Host {
    pub fn entry(&self) -> HostEntry {
        HostEntry {
            url: self.url.url(),
            pom_path: self.pom_path.clone(),
            transport: self.transport,
        }
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "url": self.url.url(),
            "destination": self.url.destination(),
            "port": self.url.port,
            "pom_path": self.pom_path,
            "transport": self.transport.map(HostTransport::name),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HostsDocument {
    schema: String,
    #[serde(default)]
    hosts: BTreeMap<String, HostEntry>,
}

/// The `hosts.json` registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    pub hosts: BTreeMap<String, HostEntry>,
}

impl Hosts {
    pub fn default_path() -> Option<PathBuf> {
        pom_paths::config_dir().map(|dir| dir.join(HOSTS_FILE))
    }

    /// A missing file is no hosts; an unreadable one is an error, so a save never overwrites what the user wrote.
    pub fn load(path: &Path) -> Result<Hosts, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Hosts::default())
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let document: HostsDocument =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        if document.schema != SCHEMA {
            return Err(format!(
                "{}: schema {:?} is not {SCHEMA}",
                path.display(),
                document.schema
            ));
        }
        Ok(Hosts {
            hosts: document.hosts,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let document = HostsDocument {
            schema: SCHEMA.to_string(),
            hosts: self.hosts.clone(),
        };
        let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
        pom_paths::write_atomic(path, format!("{text}\n").as_bytes(), 0o644)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn get(&self, name: &str) -> Result<Host, String> {
        let entry = self.hosts.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.hosts.keys().map(String::as_str).collect();
            if known.is_empty() {
                format!("no host {name}; add one with: pom host add {name} ssh://user@host")
            } else {
                format!("no host {name} (have: {})", known.join(", "))
            }
        })?;
        Ok(Host {
            name: name.to_string(),
            url: HostUrl::parse(&entry.url).map_err(|error| format!("host {name}: {error}"))?,
            pom_path: entry.pom_path.clone().filter(|path| !path.is_empty()),
            transport: entry.transport,
        })
    }

    /// Every host whose entry still parses, by name.
    pub fn all(&self) -> Vec<Host> {
        self.hosts
            .keys()
            .filter_map(|name| self.get(name).ok())
            .collect()
    }

    /// Adds or replaces `name`; true when it replaced one.
    pub fn insert(&mut self, host: &Host) -> Result<bool, String> {
        if !valid_name(&host.name) {
            return Err(format!(
                "{:?} is not a host name (letters, digits, '.', '_', '-')",
                host.name
            ));
        }
        Ok(self.hosts.insert(host.name.clone(), host.entry()).is_some())
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.hosts.remove(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_parse_with_and_without_user_and_port() {
        let url = HostUrl::parse("ssh://dev@dev.example.net:2222").expect("url");
        assert_eq!(url.user.as_deref(), Some("dev"));
        assert_eq!(url.host, "dev.example.net");
        assert_eq!(url.port, Some(2222));
        assert_eq!(url.destination(), "dev@dev.example.net");
        assert_eq!(url.alias(), "dev@dev.example.net:2222");

        let url = HostUrl::parse("ssh://100.64.0.10").expect("bare host");
        assert_eq!(url.destination(), "100.64.0.10");
        assert_eq!(url.url(), "ssh://100.64.0.10");

        let url = HostUrl::parse("ssh://me@[2001:db8::10]:22/").expect("ipv6");
        assert_eq!(url.host, "2001:db8::10");
        assert_eq!(url.alias(), "me@[2001:db8::10]:22");
    }

    #[test]
    fn urls_that_could_smuggle_an_ssh_option_are_refused() {
        for bad in [
            "dev.example.net",
            "http://dev.example.net",
            "ssh://-oProxyCommand=x",
            "ssh://-l@dev",
            "ssh://dev@host name",
            "ssh://dev@host:0",
            "ssh://dev@host:99999",
            "ssh://dev@host/path",
            "ssh://dev@host;rm",
            "ssh://",
        ] {
            assert!(HostUrl::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn hosts_round_trip_through_the_file() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = temp.path().join("pomelo/hosts.json");
        assert_eq!(Hosts::load(&path)?, Hosts::default());

        let mut hosts = Hosts::default();
        let dev = Host {
            name: "dev".into(),
            url: HostUrl::parse("ssh://dev@192.0.2.10:2222")?,
            pom_path: Some("/opt/pom/bin/pom".into()),
            transport: Some(HostTransport::Ssh),
        };
        assert!(!hosts.insert(&dev)?);
        assert!(hosts.insert(&dev)?);
        hosts.insert(&Host {
            name: "lab".into(),
            url: HostUrl::parse("ssh://lab.example.net")?,
            pom_path: None,
            transport: None,
        })?;
        assert!(hosts
            .insert(&Host {
                name: "bad name".into(),
                url: HostUrl::parse("ssh://x")?,
                pom_path: None,
                transport: None,
            })
            .is_err());
        hosts.save(&path)?;

        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["hosts"]["dev"]["url"], "ssh://dev@192.0.2.10:2222");
        assert_eq!(value["hosts"]["dev"]["pom_path"], "/opt/pom/bin/pom");
        assert!(value["hosts"]["lab"].get("pom_path").is_none());
        assert_eq!(value["hosts"]["dev"]["transport"], "ssh");
        assert!(value["hosts"]["lab"].get("transport").is_none());

        let loaded = Hosts::load(&path)?;
        assert_eq!(loaded, hosts);
        assert_eq!(loaded.get("dev")?, dev);
        assert!(loaded.get("nope").is_err());

        let mut loaded = loaded;
        assert!(loaded.remove("lab"));
        assert!(!loaded.remove("lab"));
        assert_eq!(loaded.all().len(), 1);
        Ok(())
    }

    #[test]
    fn a_corrupt_or_foreign_file_is_an_error_not_an_empty_list() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = temp.path().join("hosts.json");
        std::fs::write(&path, "{nope").map_err(|error| error.to_string())?;
        assert!(Hosts::load(&path).is_err());
        std::fs::write(&path, r#"{"schema":"pom.hosts/v9","hosts":{}}"#)
            .map_err(|error| error.to_string())?;
        assert!(Hosts::load(&path).is_err());
        std::fs::write(&path, r#"{"schema":"pom.hosts/v1"}"#).map_err(|error| error.to_string())?;
        assert_eq!(Hosts::load(&path)?, Hosts::default());
        Ok(())
    }
}
