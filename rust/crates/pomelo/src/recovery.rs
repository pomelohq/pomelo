//! Startup crash recovery: a marker the app writes while it runs tells the next launch whether the last one
//! died before it settled. After such a crash the next launch skips what crashed it (the restored tabs, then the
//! riskier subsystems) and looks for a fixed release before anything else, since a build that crashes on launch
//! can't update itself otherwise.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A run that lasted this long settled: a crash after it is not one at startup.
pub const SETTLED_AFTER: Duration = Duration::from_secs(60);
/// From this many startup crashes in a row, language servers and language packages stay off too.
const RISKY_AFTER: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    version: String,
    started: u64,
    pid: u32,
    /// Set once the run lasted `SETTLED_AFTER`.
    settled: bool,
    /// Startup crashes in a row before this run.
    startup_crashes: u32,
}

/// How the last run ended, as its marker tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastRun {
    Clean,
    StartupCrash { version: String, in_a_row: u32 },
}

/// What this launch does differently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Check for a fixed release before restoring anything.
    pub check_update_first: bool,
    /// Leave the saved tabs closed; `hold_saved` keeps their saved state untouched until they are reopened.
    pub restore_tabs: bool,
    pub hold_saved: bool,
    /// Language servers and language packages stay off for this run.
    pub risky_off: bool,
    /// The version that crashed, for the notice.
    pub crashed_version: Option<String>,
}

impl Plan {
    pub fn recovering(&self) -> bool {
        self.crashed_version.is_some()
    }
}

/// This launch's plan from how the last run ended and the `restore_on_startup` setting.
pub fn plan(last: &LastRun, restore_on_startup: &str) -> Plan {
    match last {
        LastRun::StartupCrash { version, in_a_row } => Plan {
            check_update_first: true,
            restore_tabs: false,
            hold_saved: true,
            risky_off: *in_a_row >= RISKY_AFTER,
            crashed_version: Some(version.clone()),
        },
        LastRun::Clean => Plan {
            restore_tabs: restore_on_startup != "none",
            ..Plan::default()
        },
    }
}

/// The marker of this app bundle (the dev and release apps run side by side).
pub fn marker_path(dir: &Path, app: &str) -> PathBuf {
    dir.join(format!("running-{app}.json"))
}

/// Reads how the last run ended and records this one as running.
pub fn begin(
    marker: &Path,
    version: &str,
    now: SystemTime,
    pid: u32,
    alive: impl Fn(u32) -> bool,
) -> LastRun {
    let previous = std::fs::read_to_string(marker)
        .ok()
        .and_then(|text| serde_json::from_str::<Marker>(&text).ok());
    let last = match &previous {
        Some(previous) if !previous.settled && previous.pid != pid && !alive(previous.pid) => {
            LastRun::StartupCrash {
                version: previous.version.clone(),
                in_a_row: previous.startup_crashes + 1,
            }
        }
        _ => LastRun::Clean,
    };
    let startup_crashes = match &last {
        LastRun::StartupCrash { in_a_row, .. } => *in_a_row,
        LastRun::Clean => 0,
    };
    write(
        marker,
        &Marker {
            version: version.to_string(),
            started: now
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
            pid,
            settled: false,
            startup_crashes,
        },
    );
    last
}

/// The run lasted long enough: a crash from now on is not one at startup, and the count starts over.
pub fn settle(marker: &Path) {
    let Some(mut current) = std::fs::read_to_string(marker)
        .ok()
        .and_then(|text| serde_json::from_str::<Marker>(&text).ok())
    else {
        return;
    };
    current.settled = true;
    current.startup_crashes = 0;
    write(marker, &current);
}

/// A clean quit: the next launch is a normal one.
pub fn end(marker: &Path) {
    if let Err(error) = std::fs::remove_file(marker) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("recovery: remove {}: {error}", marker.display());
        }
    }
}

fn write(marker: &Path, value: &Marker) {
    let written = marker
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(marker, serde_json::to_string(value).unwrap_or_default()));
    if let Err(error) = written {
        eprintln!("recovery: write {}: {error}", marker.display());
    }
}

/// Whether process `pid` still runs.
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 only checks the process exists; EPERM still means it does.
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_clean_quit_leaves_no_marker_and_a_normal_launch() {
        let temp = tempfile::tempdir().expect("temp");
        let marker = marker_path(temp.path(), "Pomelo");
        assert_eq!(
            begin(&marker, "0.8.1", at(10), 100, |_| false),
            LastRun::Clean
        );
        assert!(marker.is_file());
        end(&marker);
        assert!(!marker.exists());
        assert_eq!(
            begin(&marker, "0.8.1", at(20), 101, |_| false),
            LastRun::Clean
        );
    }

    #[test]
    fn a_run_that_died_before_settling_was_a_startup_crash_and_they_count() {
        let temp = tempfile::tempdir().expect("temp");
        let marker = marker_path(temp.path(), "Pomelo");
        begin(&marker, "0.8.0", at(10), 100, |_| false);
        assert_eq!(
            begin(&marker, "0.8.0", at(20), 101, |_| false),
            LastRun::StartupCrash {
                version: "0.8.0".into(),
                in_a_row: 1
            }
        );
        assert_eq!(
            begin(&marker, "0.8.0", at(30), 102, |_| false),
            LastRun::StartupCrash {
                version: "0.8.0".into(),
                in_a_row: 2
            }
        );
    }

    #[test]
    fn a_settled_run_that_was_killed_later_is_not_a_startup_crash() {
        let temp = tempfile::tempdir().expect("temp");
        let marker = marker_path(temp.path(), "Pomelo");
        begin(&marker, "0.8.0", at(10), 100, |_| false);
        begin(&marker, "0.8.0", at(20), 101, |_| false);
        settle(&marker);
        assert_eq!(
            begin(&marker, "0.8.1", at(500), 102, |_| false),
            LastRun::Clean
        );
        assert_eq!(
            begin(&marker, "0.8.1", at(510), 103, |_| false),
            LastRun::StartupCrash {
                version: "0.8.1".into(),
                in_a_row: 1
            },
            "settling started the count over"
        );
    }

    #[test]
    fn a_marker_whose_process_still_runs_is_not_a_crash() {
        let temp = tempfile::tempdir().expect("temp");
        let marker = marker_path(temp.path(), "Pomelo");
        begin(&marker, "0.8.1", at(10), 100, |_| true);
        assert_eq!(
            begin(&marker, "0.8.1", at(11), 101, |pid| pid == 100),
            LastRun::Clean
        );
    }

    #[test]
    fn the_dev_and_release_apps_keep_their_own_marker() {
        let temp = tempfile::tempdir().expect("temp");
        let release = marker_path(temp.path(), "Pomelo");
        let dev = marker_path(temp.path(), "PomeloDev");
        begin(&release, "0.8.1", at(10), 100, |_| false);
        assert_eq!(begin(&dev, "0.8.1", at(11), 200, |_| false), LastRun::Clean);
    }

    #[test]
    fn a_startup_crash_checks_for_updates_first_and_keeps_the_tabs_closed() {
        let once = plan(
            &LastRun::StartupCrash {
                version: "0.8.0".into(),
                in_a_row: 1,
            },
            "last_session",
        );
        assert!(once.check_update_first && !once.restore_tabs && once.hold_saved);
        assert!(!once.risky_off);
        let twice = plan(
            &LastRun::StartupCrash {
                version: "0.8.0".into(),
                in_a_row: 2,
            },
            "last_session",
        );
        assert!(twice.risky_off);
        assert_eq!(
            plan(&LastRun::Clean, "last_session"),
            Plan {
                restore_tabs: true,
                ..Plan::default()
            }
        );
        let none = plan(&LastRun::Clean, "none");
        assert!(!none.restore_tabs && !none.hold_saved && !none.check_update_first);
    }

    #[test]
    fn this_process_is_alive_and_a_made_up_one_is_not() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(u32::MAX));
    }
}
