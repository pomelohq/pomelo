pub mod branch;
mod resolver;
pub mod template;

pub use branch::{
    branch_hash, branch_host, branch_safe, port_ws_key, stable_shared_port, workspace_label, ws_key,
};
pub use resolver::{
    http_to_ws, EnvSources, ResolveContext, SlotAllocation, BIND_IP, DEFAULT_PROXY_PORT,
    DEV_PROXY_PREFIX,
};
