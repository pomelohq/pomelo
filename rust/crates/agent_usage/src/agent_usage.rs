//! What the coding agents used. The Claude account signed in on this Mac and its plan limits (the 5-hour
//! and weekly windows) come from the same files and usage endpoint Claude Code itself uses; tokens per
//! turn come from its transcripts, so nothing is spent to know them.

mod account;
mod cost;
mod keeper;
mod summary;
mod time;
mod transcripts;

pub use account::{
    fetch_limits, parse_limits, read_account, reported_limits, Account, Limits, LimitsError, Window,
};
pub use cost::cost;
pub use keeper::{LimitsKeeper, ASK_EVERY, BACK_OFF, REPORTED_FRESH};
pub use summary::{day_summary, DaySummary};
pub use time::{local_day, local_day_start, parse_timestamp, unix_now};
pub use transcripts::{Transcripts, Turn};
