use std::path::PathBuf;
use std::process::Command;

use winit::window::{Window, WindowAttributes};

/// The app id and WM class match the `.desktop` entry, so the shell groups the windows under Pomelo's icon.
pub(crate) fn window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    WindowAttributesExtWayland::with_name(attributes, "pomelo", "pomelo")
}

pub(crate) fn hide_pointer_until_mouse_moves() {}

pub(crate) fn titlebar_double_click(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

pub(crate) fn is_native_fullscreen(window: &Window) -> bool {
    window.fullscreen().is_some()
}

/// Asks for folders through the desktop's own chooser (zenity on GNOME, kdialog on KDE); none when neither exists.
pub(crate) fn choose_folders(multiple: bool) -> Vec<PathBuf> {
    let title = if multiple {
        "Choose the git repositories of the project"
    } else {
        "Choose a project folder with a pom.yml"
    };
    let mut zenity = Command::new("zenity");
    zenity
        .args(["--file-selection", "--directory", "--separator=\n"])
        .arg(format!("--title={title}"));
    if multiple {
        zenity.arg("--multiple");
    }
    let mut kdialog = Command::new("kdialog");
    kdialog
        .args(["--title", title, "--getexistingdirectory"])
        .arg(std::env::var_os("HOME").unwrap_or_default());
    for mut chooser in [zenity, kdialog] {
        match chooser.output() {
            Ok(output) => {
                return String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("folder chooser: {error}");
                return Vec::new();
            }
        }
    }
    eprintln!("folder chooser: install zenity or kdialog to pick folders");
    Vec::new()
}

/// The XDG autostart entry that opens the app at login.
fn login_item_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart/pomelo.desktop"))
}

pub(crate) fn start_at_login() -> bool {
    login_item_path().is_some_and(|path| path.exists())
}

pub(crate) fn set_start_at_login(enabled: bool) -> std::io::Result<()> {
    let Some(path) = login_item_path() else {
        return Ok(());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Pomelo\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry)
}
