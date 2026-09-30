use std::time::{Duration, SystemTime};

use auto_update::{Snapshot, Status};
use ui::IconKind;
use workspace::{UpdateAction, UpdateButton, UpdateInfo, UpdateMenuItem, UpdateTone};

fn percent(progress: f32) -> u32 {
    (progress.clamp(0.0, 1.0) * 100.0).round() as u32
}

pub fn downloading_tooltip(version: &str, progress: Option<f32>) -> String {
    match progress {
        Some(progress) => format!("Update to {version} ({}% downloaded)", percent(progress)),
        None => format!("Update to {version}"),
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

fn button(label: String, icon: IconKind) -> UpdateButton {
    UpdateButton {
        label,
        icon,
        progress: None,
        tooltip: None,
        tone: UpdateTone::Normal,
        dismissable: false,
        click: None,
    }
}

pub fn title_bar(snapshot: &Snapshot) -> Option<UpdateButton> {
    if !snapshot.supported || snapshot.dismissed {
        return None;
    }
    match &snapshot.status {
        Status::Idle => None,
        Status::Checking | Status::UpToDate if !snapshot.manual => None,
        Status::Checking => Some(button(
            "Checking for updates...".into(),
            IconKind::LoadCircle,
        )),
        Status::UpToDate => Some(button("Pomelo is up to date".into(), IconKind::Check)),
        Status::Downloading { version, progress } => Some(UpdateButton {
            progress: *progress,
            tooltip: Some(downloading_tooltip(version, *progress)),
            ..button(format!("Downloading {version}"), IconKind::Download)
        }),
        Status::Verifying { version } => Some(UpdateButton {
            tooltip: Some("Checking the signature before it is installed".into()),
            ..button(format!("Verifying {version}..."), IconKind::LoadCircle)
        }),
        Status::Ready { version } => Some(UpdateButton {
            tooltip: Some(format!(
                "{version} is ready. Services and terminals keep running."
            )),
            tone: UpdateTone::Ready,
            dismissable: true,
            click: Some(UpdateAction::Restart),
            ..button("Restart to Update".into(), IconKind::RotateCw)
        }),
        Status::Failed(why) => Some(UpdateButton {
            tooltip: Some(format!("{} Click for details.", sentence(why))),
            tone: UpdateTone::Warning,
            dismissable: true,
            click: Some(UpdateAction::OpenDetails),
            ..button("Update failed".into(), IconKind::Warning)
        }),
    }
}

pub fn menu_item(snapshot: &Snapshot) -> Option<UpdateMenuItem> {
    if !snapshot.supported {
        return None;
    }
    let item = |label: String, enabled: bool, action: UpdateAction| UpdateMenuItem {
        label,
        enabled,
        action,
    };
    Some(match &snapshot.status {
        Status::Checking if snapshot.manual => {
            item("Checking for Updates...".into(), false, UpdateAction::Check)
        }
        Status::Downloading { version, .. } => item(
            format!("Downloading {version}..."),
            false,
            UpdateAction::Check,
        ),
        Status::Verifying { version } => item(
            format!("Verifying {version}..."),
            false,
            UpdateAction::Check,
        ),
        Status::Ready { version } => item(
            format!("Restart to Update {version}"),
            true,
            UpdateAction::Restart,
        ),
        _ => item("Check for Updates...".into(), true, UpdateAction::Check),
    })
}

pub fn update_info(snapshot: &Snapshot) -> UpdateInfo {
    UpdateInfo {
        button: title_bar(snapshot),
        menu: menu_item(snapshot),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRow {
    pub note: String,
    pub button: &'static str,
    pub enabled: bool,
}

pub fn ago(then: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(then).unwrap_or(Duration::ZERO).as_secs();
    let plural = |count: u64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match seconds {
        0..60 => "just now".into(),
        60..3_600 => plural(seconds / 60, "minute"),
        3_600..86_400 => plural(seconds / 3_600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

pub fn settings_row(snapshot: &Snapshot, now: SystemTime) -> SettingsRow {
    let row = |note: String, button: &'static str, enabled: bool| SettingsRow {
        note,
        button,
        enabled,
    };
    if !snapshot.supported {
        return row(
            "Only the installed Pomelo updates itself; this build does not.".into(),
            "Check Now",
            false,
        );
    }
    match &snapshot.status {
        Status::Idle | Status::UpToDate => row(
            match snapshot.last_checked {
                Some(at) => format!("Checked {}. You have the newest release.", ago(at, now)),
                None => "Looks for a newer release right away.".into(),
            },
            "Check Now",
            true,
        ),
        Status::Checking => row(
            "Checking for a newer release...".into(),
            "Checking...",
            false,
        ),
        Status::Downloading { version, progress } => row(
            match progress {
                Some(progress) => format!("Downloading {version} ({}%)...", percent(*progress)),
                None => format!("Downloading {version}..."),
            },
            "Checking...",
            false,
        ),
        Status::Verifying { version } => {
            row(format!("Verifying {version}..."), "Checking...", false)
        }
        Status::Ready { version } => row(
            format!("{version} is ready. Restart Pomelo to finish."),
            "Restart to Update",
            true,
        ),
        Status::Failed(why) => row(format!("Could not update: {why}"), "Try Again", true),
    }
}

pub fn settings_action(snapshot: &Snapshot) -> UpdateAction {
    match snapshot.status {
        Status::Ready { .. } => UpdateAction::Restart,
        _ => UpdateAction::Check,
    }
}

pub fn updated_notification(version: &str) -> (String, String, &'static str) {
    (
        format!("Updated to Pomelo {version}"),
        String::new(),
        "Release Notes",
    )
}

pub fn release_notes_markdown(version: &str, body: &str, releases_url: &str) -> String {
    let body = body.trim();
    let body = if body.is_empty() {
        "No notes were published for this release."
    } else {
        body
    };
    format!("# Pomelo {version}\n\n{body}\n\n[View all releases on GitHub]({releases_url})\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(status: Status, manual: bool) -> Snapshot {
        Snapshot {
            status,
            manual,
            dismissed: false,
            supported: true,
            last_checked: None,
        }
    }

    fn label(snapshot: &Snapshot) -> Option<String> {
        title_bar(snapshot).map(|button| button.label)
    }

    #[test]
    fn quiet_checks_stay_out_of_the_title_bar() {
        assert_eq!(label(&snapshot(Status::Checking, false)), None);
        assert_eq!(label(&snapshot(Status::UpToDate, false)), None);
        assert_eq!(label(&snapshot(Status::Idle, true)), None);
        assert_eq!(
            label(&snapshot(Status::Checking, true)).as_deref(),
            Some("Checking for updates...")
        );
        assert_eq!(
            label(&snapshot(Status::UpToDate, true)).as_deref(),
            Some("Pomelo is up to date")
        );
    }

    #[test]
    fn a_download_shows_its_progress_whoever_started_it() {
        let downloading = snapshot(
            Status::Downloading {
                version: "0.7.4".into(),
                progress: Some(0.454),
            },
            false,
        );
        let button = title_bar(&downloading);
        assert_eq!(
            button.as_ref().map(|b| b.label.as_str()),
            Some("Downloading 0.7.4")
        );
        assert_eq!(
            button.as_ref().and_then(|b| b.tooltip.as_deref()),
            Some("Update to 0.7.4 (45% downloaded)")
        );
        assert_eq!(button.and_then(|b| b.click), None);
        assert_eq!(
            downloading_tooltip("0.7.4", Some(1.5)),
            "Update to 0.7.4 (100% downloaded)"
        );
        assert_eq!(downloading_tooltip("0.7.4", None), "Update to 0.7.4");
        let menu = menu_item(&downloading);
        assert_eq!(
            menu.map(|item| (item.label, item.enabled)),
            Some(("Downloading 0.7.4...".into(), false))
        );
        assert_eq!(
            settings_row(&downloading, SystemTime::UNIX_EPOCH),
            SettingsRow {
                note: "Downloading 0.7.4 (45%)...".into(),
                button: "Checking...",
                enabled: false
            }
        );
    }

    #[test]
    fn ready_restarts_and_dismissing_moves_it_to_the_menu() {
        let mut ready = snapshot(
            Status::Ready {
                version: "0.7.4".into(),
            },
            false,
        );
        let button = title_bar(&ready);
        assert_eq!(
            button
                .as_ref()
                .map(|b| (b.label.as_str(), b.tone, b.dismissable, b.click)),
            Some((
                "Restart to Update",
                UpdateTone::Ready,
                true,
                Some(UpdateAction::Restart)
            ))
        );
        ready.dismissed = true;
        assert_eq!(title_bar(&ready), None);
        assert_eq!(
            menu_item(&ready).map(|item| (item.label, item.action)),
            Some(("Restart to Update 0.7.4".into(), UpdateAction::Restart))
        );
        let row = settings_row(&ready, SystemTime::UNIX_EPOCH);
        assert_eq!(row.note, "0.7.4 is ready. Restart Pomelo to finish.");
        assert_eq!(row.button, "Restart to Update");
        assert_eq!(settings_action(&ready), UpdateAction::Restart);
    }

    #[test]
    fn a_failure_says_why_and_offers_another_try() {
        let failed = snapshot(
            Status::Failed("the download's signature did not verify".into()),
            true,
        );
        let button = title_bar(&failed);
        assert_eq!(
            button.as_ref().map(|b| (b.label.as_str(), b.tone, b.click)),
            Some((
                "Update failed",
                UpdateTone::Warning,
                Some(UpdateAction::OpenDetails)
            ))
        );
        assert_eq!(
            button.and_then(|b| b.tooltip),
            Some("The download's signature did not verify. Click for details.".into())
        );
        let row = settings_row(&failed, SystemTime::UNIX_EPOCH);
        assert_eq!(
            row.note,
            "Could not update: the download's signature did not verify"
        );
        assert_eq!((row.button, row.enabled), ("Try Again", true));
        assert_eq!(settings_action(&failed), UpdateAction::Check);
        assert_eq!(
            menu_item(&failed).map(|item| item.label),
            Some("Check for Updates...".into())
        );
    }

    #[test]
    fn settings_say_when_the_last_check_was() {
        let mut idle = snapshot(Status::Idle, false);
        idle.last_checked = Some(SystemTime::UNIX_EPOCH);
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(
            settings_row(&idle, at(12 * 60)).note,
            "Checked 12 minutes ago. You have the newest release."
        );
        assert_eq!(
            settings_row(&idle, at(5)).note,
            "Checked just now. You have the newest release."
        );
        assert_eq!(ago(SystemTime::UNIX_EPOCH, at(3_600)), "1 hour ago");
        let checking = settings_row(&snapshot(Status::Checking, true), at(0));
        assert_eq!((checking.button, checking.enabled), ("Checking...", false));
        assert_eq!(
            menu_item(&snapshot(Status::Checking, true)).map(|item| (item.label, item.enabled)),
            Some(("Checking for Updates...".into(), false))
        );
    }

    #[test]
    fn a_build_that_does_not_update_itself_offers_nothing() {
        let mut dev = snapshot(
            Status::Ready {
                version: "0.7.4".into(),
            },
            true,
        );
        dev.supported = false;
        assert_eq!(update_info(&dev), UpdateInfo::default());
        assert!(!settings_row(&dev, SystemTime::UNIX_EPOCH).enabled);
    }
}
