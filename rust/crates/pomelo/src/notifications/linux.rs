//! Notifications through `notify-send` (libnotify): the freedesktop notification service shows them, and
//! waiting on the default action tells which one the user clicked.

use std::process::Command;
use std::sync::Mutex;

/// The workspace branch of the notification the user clicked, for the app to switch to.
static CLICKED: Mutex<Option<String>> = Mutex::new(None);

pub fn start() {}

pub fn post(title: &str, body: &str, branch: &str) {
    deliver(title, body, branch, false);
}

pub fn post_test() {
    deliver(
        "Pomelo",
        "Test notification - delivery is working.",
        "",
        true,
    );
}

/// The desktop's sound theme has its own names, so every pick plays its message sound.
pub fn play_sound(_name: &str) {
    let spawned = Command::new("canberra-gtk-play")
        .args(["--id", "message-new-instant"])
        .spawn();
    match spawned {
        Ok(child) => reap(child),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("notification sound: {error}"),
    }
}

fn deliver(title: &str, body: &str, branch: &str, sound: bool) {
    let mut command = Command::new("notify-send");
    command.args(["--app-name=Pomelo", "--icon=dialog-information"]);
    if !branch.is_empty() {
        // `--wait` keeps notify-send alive until the notification closes and prints the action taken.
        command.args(["--action=default=Open", "--wait"]);
    }
    command.arg("--").arg(title).arg(body);
    let child = match command.stdout(std::process::Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("notifications need notify-send (libnotify-bin)");
            return;
        }
        Err(error) => {
            eprintln!("notification not delivered: {error}");
            return;
        }
    };
    if sound {
        play_sound("");
    }
    let branch = branch.to_string();
    let waited = std::thread::Builder::new()
        .name("notification".into())
        .spawn(move || match child.wait_with_output() {
            Ok(output) => {
                if !branch.is_empty() && String::from_utf8_lossy(&output.stdout).trim() == "default"
                {
                    if let Ok(mut clicked) = CLICKED.lock() {
                        *clicked = Some(branch);
                    }
                    ui::wake();
                }
            }
            Err(error) => eprintln!("notification: {error}"),
        });
    if let Err(error) = waited {
        eprintln!("notification: {error}");
    }
}

fn reap(mut child: std::process::Child) {
    let reaped = std::thread::Builder::new()
        .name("notification-sound".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("notification sound: {error}");
            }
        });
    if let Err(error) = reaped {
        eprintln!("notification sound: {error}");
    }
}

pub fn take_clicked() -> Option<String> {
    CLICKED.lock().ok().and_then(|mut clicked| clicked.take())
}
