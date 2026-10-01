mod caller;
mod claude;
mod driver;
mod gate;
mod hooks;
mod identity;
mod launch;
mod naming;
mod onboard;
mod policy;
mod sessions;
mod side;
mod statusline;
mod transcript;
mod turns;
mod watch;

pub use caller::Caller;
pub use claude::{install_mcp, mcp_config_json, ClaudeHome, InstallError};
pub use driver::{driver, AgentDriver, ClaudeDriver, Keystrokes};
pub use gate::{ApproveScope, Gate, Limits, Refusal, Workspace, REFUSED_EXIT};
pub use hooks::{
    branch_from_cwd, event_state, install_hooks, notification_for, read_states, record_hook, run,
    session_event, AgentState, AgentStatus,
};
pub use identity::{
    fresh_holder, holder_role, looks_like_agent_holder, role_of_kind, workspace_prefix, HolderRole,
    Identity, CLAUDE_DRIVER, NO_DRIVER,
};
pub use launch::{
    claude_launch, claude_task_launch, is_agent_holder, main_session_id, onboard_launch,
    onboard_system_prompt, resolve_claude, session_id, system_prompt, AgentLaunch, LaunchContext,
};
pub use naming::{claude_available, naming_prompt, parse_suggestion, suggest_name, NameSuggestion};
pub use onboard::{onboard_launch_with, AgentCli};
pub use policy::{
    answer_path, ask_policy, decide, hook_output, wait_for_answer, workspace_policy, write_answer,
    Answer, Decision, Policy, Verdict,
};
pub use sessions::{
    events, events_path, now_ms, project_sessions, read_state, record_event, session_dir,
    with_lock, workspace_sessions, EventLine, IndexEntry, SessionEvent, SessionState, TurnState,
    SCHEMA,
};
pub use side::{
    format_tokens, last_answer, main_session, other_cli, record_side, second_opinion_launch,
    side_launch, side_records, side_resume, side_title, write_packet, MainSession, SideLaunch,
    SideRecord, SideRole, SideStart, SIDE_AGENT_ENV,
};
pub use statusline::{record_rate_limits, run_statusline, statusline_settings, RATE_LIMITS_FILE};
pub use transcript::{Item, ToolCall, TurnContent, Usage, RESULT_PREVIEW};
pub use turns::{
    read_turns, turn_content, turn_meta, turn_spans, write_turn_meta, Origin, RecordedTurn,
    TurnMeta, TurnSpan,
};
pub use watch::AgentWatcher;
