pub mod branch;
mod dev_ports;
mod resolver;
pub mod template;
pub mod template_docs;

pub use branch::{
    branch_hash, branch_host, branch_safe, port_ws_key, resolve_branch_tokens, stable_shared_port,
    workspace_label, ws_key,
};
pub use dev_ports::{
    dev_ports, from_settings as dev_ports_from_settings, DevPorts, DEFAULT_PROXY_PORT,
    DEFAULT_WEBHOOK_PORT, PROXY_PORT_KEY, WEBHOOK_PORT_KEY,
};
pub use resolver::{
    http_to_ws, service_host, EnvSources, ResolveContext, SlotAllocation, BIND_IP, DEV_PROXY_PREFIX,
};
