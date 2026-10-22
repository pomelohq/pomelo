//! How `pom server` stands on a machine the app reaches over ssh: whether it runs, what it keeps for clients
//! that dropped, whether its socket folder is private, and what would stop it behind the user's back.

use std::path::PathBuf;

use crate::{Finding, Severity};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketDirState {
    Missing,
    Private,
    /// Group or others may enter it; `mode` is its permission bits.
    Open {
        mode: u32,
    },
    NotDirectory,
    OtherOwner,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunningServer {
    pub pid: u32,
    pub uptime_seconds: u64,
    pub connections: usize,
    pub streams: usize,
    pub retained: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherServer {
    pub version: String,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSurvey {
    pub version: String,
    pub socket_dir: PathBuf,
    pub socket_dir_state: SocketDirState,
    /// The server of this pom's version, when it answered.
    pub running: Option<RunningServer>,
    /// Servers of other versions still holding their lock.
    pub others: Vec<OtherServer>,
    pub unit_installed: bool,
    pub linux: bool,
    pub linger: bool,
}

impl ServerSurvey {
    fn used(&self) -> bool {
        self.running.is_some()
            || !self.others.is_empty()
            || self.unit_installed
            || self.socket_dir_state != SocketDirState::Missing
    }
}

/// `major.minor.patch` as numbers, so `0.10.0` sorts after `0.9.3`; anything else sorts first.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// Nothing on a machine that never ran a server.
pub fn server_findings(survey: &ServerSurvey) -> Vec<Finding> {
    if !survey.used() {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let dir = survey.socket_dir.display();
    match &survey.socket_dir_state {
        SocketDirState::Missing | SocketDirState::Private => {}
        SocketDirState::Open { mode } => findings.push(
            Finding::new(
                "server.socket_dir",
                Severity::Warn,
                "The pom server's socket folder is open to other users",
            )
            .detail(format!(
                "{dir} has mode {mode:04o}; only you should be able to enter it"
            ))
            .fix(format!(
                "chmod 700 {dir} (the server also does when it starts)"
            )),
        ),
        SocketDirState::NotDirectory => findings.push(
            Finding::new(
                "server.socket_dir",
                Severity::Error,
                "The pom server cannot start",
            )
            .detail(format!("{dir} is not a folder"))
            .fix(format!("remove {dir}")),
        ),
        SocketDirState::OtherOwner => findings.push(
            Finding::new(
                "server.socket_dir",
                Severity::Error,
                "The pom server cannot start",
            )
            .detail(format!("{dir} belongs to another user"))
            .fix(format!(
                "remove {dir} as its owner, or log in so $XDG_RUNTIME_DIR is set"
            )),
        ),
    }
    let this = version_key(&survey.version);
    for other in &survey.others {
        let pid = other
            .pid
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default();
        let stop = other
            .pid
            .map(|pid| format!("`kill {pid}`"))
            .unwrap_or_else(|| "`pkill -f 'pom server run'`".to_string());
        if version_key(&other.version) < this {
            findings.push(
                Finding::new(
                    format!("server.old_version:{}", other.version),
                    Severity::Warn,
                    format!("An older pom server still runs: {}{pid}", other.version),
                )
                .detail(format!(
                    "the app now talks to pom {}; the old server only holds memory until it is idle for server.idle_exit_minutes",
                    survey.version
                ))
                .fix(format!("stop it now with {stop}; terminals and services keep running")),
            );
        } else {
            findings.push(
                Finding::new(
                    format!("server.new_version:{}", other.version),
                    Severity::Warn,
                    format!("A newer pom server runs: {}{pid}", other.version),
                )
                .detail(format!(
                    "this pom is {}, older than the server the app uses",
                    survey.version
                ))
                .fix("update pom on this machine, or put the newer one first on the PATH"),
            );
        }
    }
    if survey.linux && !survey.linger && (survey.running.is_some() || survey.unit_installed) {
        findings.push(
            Finding::new(
                "server.linger",
                Severity::Warn,
                "The pom server stops when you log out",
            )
            .detail("lingering is off: at your last logout systemd removes $XDG_RUNTIME_DIR with the server's socket, so the server exits and forgets the terminals of apps that dropped (terminals and services keep running)")
            .fix("loginctl enable-linger $USER"),
        );
    }
    let unit = if survey.unit_installed {
        "; pom-server.service is installed"
    } else {
        ""
    };
    findings.push(match &survey.running {
        Some(server) => Finding::new(
            "server.running",
            Severity::Ok,
            format!("pom server {} runs", survey.version),
        )
        .detail(format!(
            "pid {}, up {}s, {} connection(s), {} stream(s), {} kept for apps that dropped{unit}",
            server.pid, server.uptime_seconds, server.connections, server.streams, server.retained
        )),
        None => Finding::new(
            "server.stopped",
            Severity::Ok,
            format!("pom server {} is not running", survey.version),
        )
        .detail(format!(
            "it starts on demand when the app connects over ssh{unit}"
        )),
    });
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survey() -> ServerSurvey {
        ServerSurvey {
            version: "0.9.3".into(),
            socket_dir: PathBuf::from("/run/user/1000/pom-server"),
            socket_dir_state: SocketDirState::Private,
            running: Some(RunningServer {
                pid: 42,
                uptime_seconds: 7,
                connections: 1,
                streams: 3,
                retained: 2,
            }),
            others: Vec::new(),
            unit_installed: false,
            linux: true,
            linger: true,
        }
    }

    fn ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|finding| finding.id.as_str()).collect()
    }

    #[test]
    fn a_machine_that_never_ran_a_server_hears_nothing() {
        let quiet = ServerSurvey {
            socket_dir_state: SocketDirState::Missing,
            running: None,
            ..survey()
        };
        assert!(server_findings(&quiet).is_empty());
    }

    #[test]
    fn a_running_server_reports_its_streams_and_what_it_keeps() {
        let findings = server_findings(&survey());
        assert_eq!(ids(&findings), ["server.running"]);
        assert_eq!(findings[0].severity, Severity::Ok);
        assert!(findings[0].detail.contains("3 stream(s)"));
        assert!(findings[0].detail.contains("2 kept for apps that dropped"));
    }

    #[test]
    fn other_versions_an_open_folder_and_no_linger_are_warned_about() {
        let findings = server_findings(&ServerSurvey {
            socket_dir_state: SocketDirState::Open { mode: 0o755 },
            others: vec![
                OtherServer {
                    version: "0.9.2".into(),
                    pid: Some(7),
                },
                OtherServer {
                    version: "0.10.0".into(),
                    pid: None,
                },
            ],
            linger: false,
            running: None,
            unit_installed: true,
            ..survey()
        });
        assert_eq!(
            ids(&findings),
            [
                "server.socket_dir",
                "server.old_version:0.9.2",
                "server.new_version:0.10.0",
                "server.linger",
                "server.stopped"
            ]
        );
        assert!(findings[0].detail.contains("0755"));
        assert!(findings[1].fix.contains("kill 7"));
        assert!(findings[4]
            .detail
            .contains("pom-server.service is installed"));
        assert!(findings.iter().all(|finding| finding.title.is_ascii()
            && finding.detail.is_ascii()
            && finding.fix.is_ascii()));
    }

    #[test]
    fn a_folder_of_another_user_blocks_the_server() {
        let findings = server_findings(&ServerSurvey {
            socket_dir_state: SocketDirState::OtherOwner,
            running: None,
            ..survey()
        });
        assert_eq!(findings[0].severity, Severity::Error);
        let mac = server_findings(&ServerSurvey {
            linux: false,
            linger: false,
            ..survey()
        });
        assert_eq!(ids(&mac), ["server.running"], "linger is Linux only");
    }
}
