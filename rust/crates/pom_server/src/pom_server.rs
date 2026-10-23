//! `pom server`: a long-lived process per user and version that answers a remote app over one ssh session,
//! through a Unix socket only this user can reach.

pub mod client;
mod dispatch;
mod fs;
mod git;
mod jira;
mod jobs;
mod log;
mod paste;
mod paths;
mod peer;
mod pty;
mod search;
mod server;
mod services;
mod settings;
mod usage;
mod watch;

pub use dispatch::{
    check_branch, check_name, check_op_id, registered_config, Dispatcher, Ops, FAILED, OP_MEMORY,
};
pub use fs::{FsService, CHUNK_BYTES, CHUNK_ENTRIES, MAX_FS_STREAMS, WATCH_LINGER};
pub use git::{GitService, WATCH_EVERY};
pub use jira::{JiraService, MakeTransport, ReadEnv};
pub use jobs::{Jobs, JOB_MEMORY};
pub use log::{RequestRecord, ServerLog, ROTATE_AT};
pub use paste::{PasteService, PASTE_LIFETIME};
pub use paths::{running_versions, ServerPaths};
pub use peer::{current_uid, peer_pid, peer_uid};
pub use pty::{check_cwd, check_holder, check_size, Caller, Ptys, ShellCommand, RETENTION};
pub use search::{SearchService, LINE_BYTES, PUSH_BYTES};
pub use server::{
    bind, PeerCheck, Server, ServerHandle, ServerOptions, StartError, DEAD_AFTER,
    DEFAULT_IDLE_EXIT_MINUTES, SOCKET_CHECK_EVERY,
};
pub use settings::{AgentCheck, SettingsService};
pub use usage::{FetchLimits, UsageService};
pub use watch::{Watches, READ_EVERY};
