//! Hands files, folders and links to the desktop environment.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Opens a file, folder or URL with its default app.
pub fn open(target: impl AsRef<OsStr>) -> std::io::Result<()> {
    spawn(opener().arg(target))
}

/// Shows `path` in the file manager: selected on macOS, its folder elsewhere.
pub fn reveal(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("-R").arg(path))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        open(folder)
    }
}

/// Opens `target` in the named app: an app bundle name on macOS, an app's launcher command elsewhere.
pub fn open_with(app: &str, target: impl AsRef<OsStr>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("-a").arg(app).arg(target))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let command = app_command(app);
        let program = find_on_path(&command).unwrap_or_else(|| command.into());
        spawn(Command::new(program).arg(target))
    }
}

/// Whether the named app is installed: its bundle under an Applications folder on macOS, its launcher
/// command on PATH elsewhere.
pub fn app_installed(app: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let bundle = format!("{app}.app");
        Path::new("/Applications").join(&bundle).exists()
            || std::env::var_os("HOME")
                .is_some_and(|home| Path::new(&home).join("Applications").join(&bundle).exists())
    }
    #[cfg(not(target_os = "macos"))]
    {
        find_on_path(&app_command(app)).is_some()
    }
}

/// The launcher command an editor's Linux package installs; most are the app's name in lower case.
#[cfg(not(target_os = "macos"))]
fn app_command(app: &str) -> String {
    match app {
        "Visual Studio Code" => "code".into(),
        "Sublime Text" => "subl".into(),
        "IntelliJ IDEA" => "idea".into(),
        other if other.contains('/') => other.into(),
        other => other.to_lowercase(),
    }
}

/// A desktop session's PATH often leaves out the per-user and snap bin folders editors install into.
#[cfg(not(target_os = "macos"))]
fn find_on_path(command: &str) -> Option<std::path::PathBuf> {
    if command.contains('/') {
        return Some(command.into()).filter(|path: &std::path::PathBuf| path.is_file());
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(home.map(|home| home.join(".local/bin")));
    dirs.push("/snap/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.push("/usr/bin".into());
    dirs.into_iter()
        .map(|dir| dir.join(command))
        .find(|path| path.is_file())
}

fn opener() -> Command {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("xdg-open")
    }
}

// The opener exits once it has handed the target over; waiting on it off-thread keeps it from lingering as a zombie.
fn spawn(command: &mut Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::Builder::new()
        .name("desktop-open".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("desktop open: {error}");
            }
        })?;
    Ok(())
}

/// The menu label for `reveal`, named after the platform's file manager.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Reveal in File Manager"
};

/// How copy names the machine Pomelo runs on.
pub const THIS_MACHINE: &str = if cfg!(target_os = "macos") {
    "this Mac"
} else {
    "this computer"
};
