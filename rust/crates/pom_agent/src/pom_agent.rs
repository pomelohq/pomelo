mod claude;
mod hooks;
mod launch;
mod naming;
mod onboard;
mod side;
mod watch;

pub use claude::{install_mcp, mcp_config_json, ClaudeHome, InstallError};
pub use hooks::{
    branch_from_cwd, event_state, install_hooks, notification_for, read_states, record_hook, run,
    AgentState, AgentStatus,
};
pub use launch::{
    claude_launch, claude_task_launch, is_agent_holder, onboard_launch, onboard_system_prompt,
    resolve_claude, session_id, system_prompt, AgentLaunch, LaunchContext,
};
pub use naming::{claude_available, naming_prompt, parse_suggestion, suggest_name, NameSuggestion};
pub use onboard::{onboard_launch_with, AgentCli};
pub use side::{
    format_tokens, last_answer, main_session, other_cli, record_side, second_opinion_launch,
    side_launch, side_records, side_resume, side_title, write_packet, MainSession, SideLaunch,
    SideRecord, SideRole, SideStart, SIDE_AGENT_ENV,
};
pub use watch::AgentWatcher;
