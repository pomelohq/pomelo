//! `pom server`: a long-lived process per user and version that answers a remote app over one ssh session,
//! through a Unix socket only this user can reach.

pub mod client;
mod dispatch;
mod fs;
mod jobs;
mod log;
mod paths;
mod peer;
mod pty;
mod search;
mod server;
mod services;
mod watch;

pub use dispatch::{
    check_branch, check_name, check_op_id, registered_config, Dispatcher, Ops, FAILED, OP_MEMORY,
};
pub use fs::{FsService, CHUNK_BYTES, CHUNK_ENTRIES, MAX_FS_STREAMS, WATCH_LINGER};
pub use jobs::{Jobs, JOB_MEMORY};
pub use log::{RequestRecord, ServerLog, ROTATE_AT};
pub use paths::{ensure_private_dir, running_versions, ServerPaths};
pub use peer::{current_uid, peer_uid};
pub use pty::{check_cwd, check_holder, check_size, Caller, Ptys, ShellCommand, RETENTION};
pub use search::{SearchService, LINE_BYTES, PUSH_BYTES};
pub use server::{
    bind, PeerCheck, Server, ServerHandle, ServerOptions, StartError, DEAD_AFTER,
    DEFAULT_IDLE_EXIT_MINUTES, SOCKET_CHECK_EVERY,
};
pub use watch::{Watches, READ_EVERY};
