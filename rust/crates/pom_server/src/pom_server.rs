//! `pom server`: a long-lived process per user and version that answers a remote app over one ssh session,
//! through a Unix socket only this user can reach.

pub mod client;
mod dispatch;
mod log;
mod paths;
mod peer;
mod pty;
mod server;

pub use dispatch::{
    check_branch, check_name, check_op_id, registered_config, Dispatcher, Ops, FAILED, OP_MEMORY,
};
pub use log::{RequestRecord, ServerLog, ROTATE_AT};
pub use paths::{ensure_private_dir, ServerPaths};
pub use peer::{current_uid, peer_uid};
pub use pty::{check_cwd, check_holder, check_size, Caller, Ptys, ShellCommand};
pub use server::{
    bind, idle_exit_from_settings, PeerCheck, Server, ServerHandle, ServerOptions, StartError,
    DEAD_AFTER, DEFAULT_IDLE_EXIT_MINUTES, SOCKET_CHECK_EVERY,
};
