//! Keeping the golden source fresh and branches safe: refresh-main pulls every repo of main on a schedule
//! (migrating what changed), auto-push pushes workspace branches with a snapshot of uncommitted work. Both
//! run in the one process per session that holds the primary lock, which the previous core takes too.

mod autopush;
mod git;
mod refresh;

use std::path::PathBuf;
use std::time::Duration;

use pom_config::Config;
use pom_paths::StateDir;

pub use autopush::{auto_push_once, prune_wip_refs, push_worktree, write_wip_snapshot};
pub use refresh::{refresh_main, RefreshContext, RepoState};

pub const DEFAULT_REFRESH_SECONDS: u64 = 1800;
const DEFAULT_PUSH_SECONDS: i64 = 180;
const MIN_PUSH_SECONDS: i64 = 30;

/// Whether refresh-main runs and how often.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefreshSchedule {
    pub enabled: bool,
    pub interval_seconds: u64,
}

fn integrations_path(state: &StateDir, session: &str) -> PathBuf {
    let name = if session.is_empty() { "_" } else { session };
    state.path("integrations").join(format!("{name}.json"))
}

fn read_integrations(state: &StateDir, session: &str) -> serde_json::Value {
    std::fs::read_to_string(integrations_path(state, session))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

/// What the app set for the session wins; until it sets anything, `pom.yml`'s `sync:` decides.
pub fn refresh_schedule(
    state: &StateDir,
    session: &str,
    config: Option<&Config>,
) -> RefreshSchedule {
    let saved = &read_integrations(state, session)["sync"];
    let (enabled, seconds) = if saved["configured"].as_bool() == Some(true) {
        (
            saved["refresh_main"].as_bool().unwrap_or(false),
            saved["refresh_interval_sec"].as_i64().unwrap_or(0),
        )
    } else {
        match config.and_then(|config| config.sync.as_ref()) {
            Some(sync) => (sync.refresh_main, sync.refresh_interval_sec),
            None => (false, 0),
        }
    };
    RefreshSchedule {
        enabled,
        interval_seconds: u64::try_from(seconds)
            .ok()
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_REFRESH_SECONDS),
    }
}

/// Saves the app's choice, keeping the rest of the session's integrations file.
pub fn save_refresh_schedule(
    state: &StateDir,
    session: &str,
    schedule: RefreshSchedule,
) -> std::io::Result<()> {
    let path = integrations_path(state, session);
    let mut whole = read_integrations(state, session);
    whole["sync"] = serde_json::json!({
        "configured": true,
        "refresh_main": schedule.enabled,
        "refresh_interval_sec": schedule.interval_seconds,
    });
    let mut text = serde_json::to_string_pretty(&whole).map_err(std::io::Error::other)?;
    text.push('\n');
    pom_paths::write_atomic(&path, text.as_bytes(), 0o644)
}

/// How often auto-push runs, when `pom.yml` turns it on.
pub fn auto_push_interval(config: &Config) -> Option<Duration> {
    let sync = config.sync.as_ref().filter(|sync| sync.auto_push)?;
    let seconds = if sync.interval_sec > 0 {
        sync.interval_sec
    } else {
        DEFAULT_PUSH_SECONDS
    };
    Some(Duration::from_secs(seconds.max(MIN_PUSH_SECONDS) as u64))
}

/// The next run on the clock: every `interval` minutes from the top of the (local) hour, so a 30-minute
/// schedule runs at :00 and :30. An hour or more runs at the next top of the hour.
pub fn next_aligned_run(now: i64, utc_offset: i64, interval_seconds: u64) -> i64 {
    let local = now + utc_offset;
    let hour_start = local - local.rem_euclid(3600);
    let every = (interval_seconds / 60).max(1) as i64;
    let next = if every >= 60 {
        hour_start + 3600
    } else {
        let step = (local - hour_start) / 60 / every + 1;
        if step * every >= 60 {
            hour_start + 3600
        } else {
            hour_start + step * every * 60
        }
    };
    next - utc_offset
}

/// This machine's current offset from UTC, in seconds.
pub fn local_utc_offset() -> i64 {
    let now: libc::time_t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as libc::time_t);
    // SAFETY: localtime_r only writes the tm we own; a zeroed tm is a valid initial value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&now, &mut tm) };
    if converted.is_null() {
        0
    } else {
        tm.tm_gmtoff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 1_700_002_800;

    #[test]
    fn runs_line_up_with_the_clock() {
        assert_eq!(HOUR % 3600, 0);
        assert_eq!(next_aligned_run(HOUR + 10 * 60, 0, 1800), HOUR + 30 * 60);
        assert_eq!(next_aligned_run(HOUR + 30 * 60, 0, 1800), HOUR + 3600);
        assert_eq!(next_aligned_run(HOUR + 59 * 60, 0, 600), HOUR + 3600);
        assert_eq!(next_aligned_run(HOUR + 5, 0, 7200), HOUR + 3600);
        assert_eq!(
            next_aligned_run(HOUR + 5, 0, 10),
            HOUR + 60,
            "at least a minute"
        );
        let india = 5 * 3600 + 1800;
        assert_eq!(
            next_aligned_run(HOUR + 10 * 60, india, 3600),
            HOUR + 30 * 60,
            "the top of the local hour"
        );
    }

    #[test]
    fn the_app_setting_wins_over_the_config() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let state = StateDir::new(temp.path());
        let mut config = Config::default();
        assert_eq!(
            refresh_schedule(&state, "demo", Some(&config)),
            RefreshSchedule {
                enabled: false,
                interval_seconds: DEFAULT_REFRESH_SECONDS
            }
        );
        config.sync = Some(pom_config::SyncConfig {
            refresh_main: true,
            refresh_interval_sec: 600,
            auto_push: true,
            interval_sec: 5,
        });
        assert_eq!(
            refresh_schedule(&state, "demo", Some(&config)),
            RefreshSchedule {
                enabled: true,
                interval_seconds: 600
            }
        );
        assert_eq!(auto_push_interval(&config), Some(Duration::from_secs(30)));

        let path = temp.path().join("integrations/demo.json");
        std::fs::create_dir_all(path.parent().expect("parent"))?;
        std::fs::write(&path, r#"{"jira":{"site":"x"}}"#)?;
        let off = RefreshSchedule {
            enabled: false,
            interval_seconds: 900,
        };
        save_refresh_schedule(&state, "demo", off)?;
        assert_eq!(refresh_schedule(&state, "demo", Some(&config)), off);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(saved["jira"]["site"], "x");
        assert_eq!(saved["sync"]["configured"], true);
        Ok(())
    }

    #[test]
    fn the_local_offset_is_whole_minutes() {
        assert_eq!(local_utc_offset() % 60, 0);
    }
}
