//! The Store page's source for a project on another machine: each request runs there as a job on the
//! project's pom server, whose progress carries the same updates a local run sends.

use std::sync::Arc;
use std::time::Duration;

use module_store::{Overview, Request, StoreUpdate};
use pom_remote::server_api::{progress_kind, JobRequest, StoreArgs};
use pom_remote::{JobEvent, ProjectSource, SourceError, Via};

use crate::access::{StoreAccess, Updates};

const FOLLOW_POLL: Duration = Duration::from_millis(250);
const RECONNECT_TRIES: u32 = 5;
const RECONNECT_WAIT: Duration = Duration::from_secs(1);

pub struct RemoteStore {
    source: Arc<dyn ProjectSource>,
    /// The machine's name, for what is said about it.
    host: String,
}

impl RemoteStore {
    pub fn new(source: Arc<dyn ProjectSource>, host: &str) -> RemoteStore {
        RemoteStore {
            source,
            host: host.to_string(),
        }
    }
}

/// Why the store on `host` cannot be shown here, when it cannot.
pub fn refusal(source: &dyn ProjectSource, host: &str) -> Option<String> {
    let info = match source.transport() {
        Some(info) => info,
        None => match source.workspaces() {
            Ok(_) => source.transport()?,
            Err(error) => return Some(format!("Could not reach {host}: {error}")),
        },
    };
    if info.via != Via::Server {
        return Some(format!(
            "The node_modules store on {host} needs pom server there; this window reaches {host} over plain ssh."
        ));
    }
    if info.module_store {
        return None;
    }
    Some(match info.remote_version {
        Some(version) => format!(
            "pom {version} on {host} cannot show its node_modules store here. Update pom there to see it in this window."
        ),
        None => format!(
            "pom on {host} cannot show its node_modules store here. Update pom there to see it in this window."
        ),
    })
}

fn op_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("app-{}-store-{nanos:x}", std::process::id())
}

/// Passes on the job's updates until it ends; a lost connection follows again from the last event seen.
fn follow(source: &dyn ProjectSource, host: &str, job: &str, send: &Updates) -> Result<(), String> {
    let mut next = 0;
    let mut lost = 0;
    let mut last_error: String;
    loop {
        match source.follow_job(job, next) {
            Ok(feed) => loop {
                match feed.next(FOLLOW_POLL) {
                    JobEvent::Progress(progress) => {
                        lost = 0;
                        next = progress.seq + 1;
                        if progress.kind == progress_kind::DATA {
                            let decoded = progress.data.map(serde_json::from_value::<StoreUpdate>);
                            let update = match decoded {
                                Some(Ok(StoreUpdate::Place { root, method })) => {
                                    Some(StoreUpdate::Place {
                                        root: format!("{root} on {host}"),
                                        method,
                                    })
                                }
                                Some(Ok(update)) => Some(update),
                                Some(Err(error)) => {
                                    eprintln!("module store: an update from {host}: {error}");
                                    None
                                }
                                None => None,
                            };
                            if update.is_some_and(|update| !send(update)) {
                                return Ok(());
                            }
                        } else if progress.kind == progress_kind::FAILED {
                            return Err(progress.line);
                        }
                        if progress.done {
                            return Ok(());
                        }
                    }
                    JobEvent::Idle => {}
                    JobEvent::Ended(reason) => {
                        last_error = reason;
                        break;
                    }
                }
            },
            Err(SourceError::Failed(message)) => return Err(message),
            Err(SourceError::Unreachable(reason)) => last_error = reason,
        }
        // The session coming back on its own is no lost try: the server keeps the job's events meanwhile.
        let reconnecting = source.link().is_some_and(|report| {
            matches!(report.state, pom_remote::LinkState::Reconnecting { .. })
        });
        if !reconnecting {
            lost += 1;
        }
        if lost > RECONNECT_TRIES {
            return Err(format!("lost {host}: {last_error}"));
        }
        std::thread::sleep(RECONNECT_WAIT);
    }
}

impl StoreAccess for RemoteStore {
    // The host reads its own overview, so paths and copies are checked there rather than taken from here.
    fn start(&self, request: Request, _previous: Option<Overview>, send: Updates) {
        let source = self.source.clone();
        let host = self.host.clone();
        std::thread::spawn(move || {
            if let Some(message) = refusal(source.as_ref(), &host) {
                send(StoreUpdate::Failed { message });
                return;
            }
            let job = JobRequest::Store(StoreArgs { request });
            let outcome = source
                .start_job(&job, &op_id())
                .map_err(|error| error.to_string())
                .and_then(|id| follow(source.as_ref(), &host, &id, &send));
            if let Err(error) = outcome {
                send(StoreUpdate::Failed {
                    message: format!("The store on {host} could not be read or changed: {error}"),
                });
            }
        });
    }
}
