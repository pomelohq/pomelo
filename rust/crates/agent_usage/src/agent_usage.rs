//! What the coding agents used. The Claude account signed in on this Mac and its plan limits (the 5-hour
//! and weekly windows) come from the same files and usage endpoint Claude Code itself uses; tokens per
//! turn come from its transcripts, so nothing is spent to know them.

mod account;
mod cost;
mod time;
mod transcripts;

pub use account::{fetch_limits, parse_limits, read_account, Account, Limits, LimitsError, Window};
pub use cost::cost;
pub use time::{local_day, parse_timestamp, unix_now};
pub use transcripts::{Transcripts, Turn};
