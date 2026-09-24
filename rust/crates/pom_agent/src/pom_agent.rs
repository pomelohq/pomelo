mod claude;
mod hooks;
mod watch;

pub use claude::{install_mcp, mcp_config_json, ClaudeHome, InstallError};
pub use hooks::{
    branch_from_cwd, event_state, install_hooks, notification_for, read_states, record_hook, run,
    AgentState, AgentStatus,
};
pub use watch::AgentWatcher;
