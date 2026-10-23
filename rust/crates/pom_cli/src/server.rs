//! `pom server run|proxy|status|stop`: the long-lived server a remote app talks to, and the byte pump ssh runs
//! to reach it.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_server::{ServerOptions, ServerPaths, StartError};

use crate::args::Args;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// `pom server proxy --reconnect` found no server: the client must start over rather than resume.
pub(crate) const EXIT_SERVER_GONE: i32 = 70;
/// `pom server status` when none runs.
const EXIT_NOT_RUNNING: i32 = 3;
const START_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ServerCommand {
    Run { foreground: bool },
    Proxy { reconnect: bool },
    Status { json: bool },
    Stop,
    InstallUnit { json: bool },
    UninstallUnit { json: bool },
}

pub(crate) fn parse(words: &[&str]) -> Result<ServerCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("server needs run, proxy, status, stop, install-unit or uninstall-unit")?;
    let args = Args::parse(rest, &["-o", "--output"])?;
    args.at_most(0, &format!("server {verb}"))?;
    match *verb {
        "run" => {
            args.allow(&["--foreground"])?;
            Ok(ServerCommand::Run {
                foreground: args.has("--foreground"),
            })
        }
        "proxy" => {
            args.allow(&["--reconnect"])?;
            Ok(ServerCommand::Proxy {
                reconnect: args.has("--reconnect"),
            })
        }
        "status" => {
            args.allow(&["--json", "-o", "--output"])?;
            Ok(ServerCommand::Status {
                json: args.has("--json") || args.json()?,
            })
        }
        "stop" => {
            args.allow(&[])?;
            Ok(ServerCommand::Stop)
        }
        "install-unit" | "uninstall-unit" => {
            args.allow(&["--json", "-o", "--output"])?;
            let json = args.has("--json") || args.json()?;
            Ok(if *verb == "install-unit" {
                ServerCommand::InstallUnit { json }
            } else {
                ServerCommand::UninstallUnit { json }
            })
        }
        other => Err(format!(
            "unknown server command {other} (run, proxy, status, stop, install-unit, uninstall-unit)"
        )),
    }
}

pub(crate) fn execute(command: &ServerCommand, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let paths = ServerPaths::from_env(VERSION);
    let result = match command {
        ServerCommand::Run { foreground: true } => serve(paths, err),
        ServerCommand::Run { foreground: false } => start(&paths, out),
        ServerCommand::Proxy { reconnect } => return proxy(&paths, *reconnect, err),
        ServerCommand::Status { json } => return status(&paths, *json, out, err),
        ServerCommand::Stop => stop(&paths, out),
        ServerCommand::InstallUnit { json } => {
            return unit_command(UnitAction::Install, *json, out, err)
        }
        ServerCommand::UninstallUnit { json } => {
            return unit_command(UnitAction::Uninstall, *json, out, err)
        }
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            1
        }
    }
}

fn serve(paths: ServerPaths, err: &mut dyn Write) -> Result<(), String> {
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
    let dispatcher = pom_server::Dispatcher::new(
        StateDir::from_env(),
        SocketDir::from_env(),
        binary,
        pom_server::OP_MEMORY,
    );
    let mut options = ServerOptions::new(paths, VERSION, dispatcher);
    let settings = settings::ServerSettings::load();
    options.idle_exit = (settings.idle_exit_minutes > 0)
        .then(|| Duration::from_secs(settings.idle_exit_minutes.saturating_mul(60)));
    options.dispatcher.ptys().set_retention(Duration::from_secs(
        settings.reconnect_retention_minutes.saturating_mul(60),
    ));
    match pom_server::bind(options) {
        Ok(server) => {
            server.run();
            Ok(())
        }
        Err(already @ StartError::AlreadyRunning(_)) => {
            crate::report(err, &already.to_string());
            Ok(())
        }
        Err(StartError::Io(message)) => Err(message),
    }
}

/// `pom server run --foreground` in its own process group with no terminal, its stderr in the server log.
fn spawn_detached(paths: &ServerPaths) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(dir) = paths.log.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .map_err(|error| format!("server log: {error}"))?;
    use std::os::unix::process::CommandExt;
    std::process::Command::new(&exe)
        .args(["server", "run", "--foreground"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("start pom server: {error}"))
}

fn connect_within(paths: &ServerPaths, wait: Duration) -> Option<UnixStream> {
    let deadline = Instant::now() + wait;
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            return Some(stream);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start(paths: &ServerPaths, out: &mut dyn Write) -> Result<(), String> {
    if pom_server::client::connect(paths)?.is_none() {
        spawn_detached(paths)?;
        if connect_within(paths, START_WAIT).is_none() {
            return Err(format!(
                "pom server did not start within {}s; see {}",
                START_WAIT.as_secs(),
                paths.log.display()
            ));
        }
    }
    let (connection, _) = pom_server::client::connect(paths)?
        .ok_or_else(|| "pom server stopped right after starting".to_string())?;
    let info = pom_server::client::info(&connection);
    connection.close();
    let info = info?;
    writeln!(
        out,
        "pom server {} running (pid {}, socket {})",
        info.version,
        info.pid,
        info.socket.display()
    )
    .map_err(|error| error.to_string())
}

fn fail_json(err: &mut dyn Write, code: &str) {
    crate::report(err, &serde_json::json!({ "error": code }).to_string());
}

/// Pumps raw bytes between stdio and the server socket, starting the server first unless resuming.
fn proxy(paths: &ServerPaths, reconnect: bool, err: &mut dyn Write) -> i32 {
    let stream = match UnixStream::connect(&paths.socket) {
        Ok(stream) => stream,
        Err(_) if reconnect => {
            fail_json(err, "server_gone");
            return EXIT_SERVER_GONE;
        }
        Err(_) => {
            if let Err(message) = spawn_detached(paths) {
                fail_json(err, &message);
                return 1;
            }
            match connect_within(paths, START_WAIT) {
                Some(stream) => stream,
                None => {
                    fail_json(err, "server_not_started");
                    return 1;
                }
            }
        }
    };
    match pump(stream) {
        Ok(()) => 0,
        Err(message) => {
            fail_json(err, &message);
            1
        }
    }
}

fn pump(stream: UnixStream) -> Result<(), String> {
    let mut to_server = stream.try_clone().map_err(|error| error.to_string())?;
    std::thread::Builder::new()
        .name("pom-server-proxy-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let Some(bytes) = buffer.get(..count) else {
                            break;
                        };
                        if to_server.write_all(bytes).is_err() {
                            break;
                        }
                    }
                }
            }
            if let Err(error) = to_server.shutdown(std::net::Shutdown::Write) {
                eprintln!("pom server proxy: {error}");
            }
        })
        .map_err(|error| error.to_string())?;
    let mut from_server = stream;
    let mut stdout = std::io::stdout().lock();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = match from_server.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        };
        let bytes = buffer.get(..count).unwrap_or_default();
        if stdout
            .write_all(bytes)
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return Ok(());
        }
    }
}

fn status(paths: &ServerPaths, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let connected = match pom_server::client::connect(paths) {
        Ok(connected) => connected,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            return 1;
        }
    };
    let Some((connection, _)) = connected else {
        crate::report(err, &format!("no pom server {VERSION} runs"));
        return EXIT_NOT_RUNNING;
    };
    let info = pom_server::client::info(&connection);
    connection.close();
    let text = match info {
        Ok(info) if json => serde_json::to_string(&info).map_err(|error| error.to_string()),
        Ok(info) => Ok(format!(
            "pom server {} (pid {}), up {}s, {} connection(s), {} stream(s)\n  socket {}\n  log    {}",
            info.version,
            info.pid,
            info.uptime_seconds,
            info.connections,
            info.streams,
            info.socket.display(),
            info.log.display()
        )),
        Err(message) => Err(message),
    };
    match text.and_then(|text| writeln!(out, "{text}").map_err(|error| error.to_string())) {
        Ok(()) => 0,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            1
        }
    }
}

fn stop(paths: &ServerPaths, out: &mut dyn Write) -> Result<(), String> {
    let Some((connection, _)) = pom_server::client::connect(paths)? else {
        return writeln!(out, "no pom server {VERSION} runs").map_err(|error| error.to_string());
    };
    let pid = pom_server::client::info(&connection)
        .map(|info| info.pid)
        .ok();
    pom_server::client::stop(&connection)?;
    connection.close();
    let deadline = Instant::now() + START_WAIT;
    while paths.socket.exists() {
        if Instant::now() >= deadline {
            return Err(format!(
                "pom server is still running ({})",
                paths.socket.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid = pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    writeln!(out, "stopped pom server {VERSION}{pid}").map_err(|error| error.to_string())
}

fn socket_dir_state(dir: &Path) -> pom_doctor::SocketDirState {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match std::fs::symlink_metadata(dir) {
        Err(_) => pom_doctor::SocketDirState::Missing,
        Ok(metadata) if !metadata.file_type().is_dir() => pom_doctor::SocketDirState::NotDirectory,
        Ok(metadata) if metadata.uid() != pom_server::current_uid() => {
            pom_doctor::SocketDirState::OtherOwner
        }
        Ok(metadata) => match metadata.permissions().mode() & 0o777 {
            mode if mode & 0o077 != 0 => pom_doctor::SocketDirState::Open { mode },
            _ => pom_doctor::SocketDirState::Private,
        },
    }
}

/// What `pom doctor` reports about this machine's server; asks the running one of this version for its numbers.
pub(crate) fn survey(linger: bool) -> pom_doctor::ServerSurvey {
    let paths = ServerPaths::from_env(VERSION);
    let running = match pom_server::client::connect(&paths) {
        Ok(Some((connection, _))) => {
            let info = pom_server::client::info(&connection);
            connection.close();
            info.ok().map(|info| pom_doctor::RunningServer {
                pid: info.pid,
                uptime_seconds: info.uptime_seconds,
                connections: info.connections.saturating_sub(1),
                streams: info.streams,
                retained: info.retained,
            })
        }
        Ok(None) | Err(_) => None,
    };
    let others = pom_server::running_versions(&paths.dir)
        .into_iter()
        .filter(|(version, _)| version != VERSION)
        .map(|(version, pid)| pom_doctor::OtherServer { version, pid })
        .collect();
    pom_doctor::ServerSurvey {
        version: VERSION.to_string(),
        socket_dir_state: socket_dir_state(&paths.dir),
        socket_dir: paths.dir,
        running,
        others,
        unit_installed: unit_path().is_some_and(|unit| unit.is_file()),
        linux: cfg!(target_os = "linux"),
        linger,
    }
}

const UNIT_NAME: &str = "pom-server.service";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnitAction {
    Install,
    Uninstall,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct UnitReport {
    schema: &'static str,
    unit: PathBuf,
    installed: bool,
    enabled: bool,
}

/// `$XDG_CONFIG_HOME/systemd/user/pom-server.service`, else under `~/.config`.
pub(crate) fn unit_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(config.join("systemd/user").join(UNIT_NAME))
}

/// One word of a unit line: systemd expands `%` specifiers and splits on spaces unless quoted.
fn unit_word(word: &str) -> String {
    let escaped = word.replace('%', "%%");
    if escaped
        .chars()
        .any(|char| char.is_whitespace() || matches!(char, '"' | '\\' | '\''))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

fn unit_text(exe: &Path, path_env: &str) -> String {
    format!(
        "[Unit]\n\
         Description=pom server for remote Pomelo sessions\n\
         \n\
         [Service]\n\
         ExecStart={} server run --foreground\n\
         Environment={}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        unit_word(&exe.to_string_lossy()),
        unit_word(&format!("PATH={path_env}")),
    )
}

fn systemctl(program: &str, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new(program)
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("systemctl: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "systemctl --user {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn install_unit(
    unit: &Path,
    exe: &Path,
    path_env: &str,
    program: &str,
) -> Result<UnitReport, String> {
    if let Some(dir) = unit.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    pom_paths::write_atomic(unit, unit_text(exe, path_env).as_bytes(), 0o644)
        .map_err(|error| format!("{}: {error}", unit.display()))?;
    systemctl(program, &["daemon-reload"])?;
    systemctl(program, &["enable", UNIT_NAME])?;
    Ok(UnitReport {
        schema: pom_remote::server_api::SERVER_SCHEMA,
        unit: unit.to_path_buf(),
        installed: true,
        enabled: true,
    })
}

fn uninstall_unit(unit: &Path, program: &str) -> Result<UnitReport, String> {
    if unit.exists() {
        systemctl(program, &["disable", UNIT_NAME])?;
        std::fs::remove_file(unit).map_err(|error| format!("{}: {error}", unit.display()))?;
        systemctl(program, &["daemon-reload"])?;
    }
    Ok(UnitReport {
        schema: pom_remote::server_api::SERVER_SCHEMA,
        unit: unit.to_path_buf(),
        installed: false,
        enabled: false,
    })
}

fn unit_command(action: UnitAction, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    if !cfg!(target_os = "linux") {
        crate::report(
            err,
            "error: pom server install-unit and uninstall-unit are Linux only (systemd user units); \
             on this machine the server starts on demand when the app connects over ssh",
        );
        return 1;
    }
    let result = unit_path()
        .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is set".to_string())
        .and_then(|unit| match action {
            UnitAction::Install => {
                let exe = std::env::current_exe().map_err(|error| error.to_string())?;
                let path_env = std::env::var("PATH").unwrap_or_default();
                install_unit(&unit, &exe, &path_env, "systemctl")
            }
            UnitAction::Uninstall => uninstall_unit(&unit, "systemctl"),
        });
    let text = result.and_then(|report| {
        if json {
            return serde_json::to_string(&report).map_err(|error| error.to_string());
        }
        Ok(match action {
            UnitAction::Install => format!(
                "wrote {}\nenabled {UNIT_NAME}: the server starts with your user manager \
                 (start it now with `systemctl --user start pom-server`)",
                report.unit.display()
            ),
            UnitAction::Uninstall => format!(
                "disabled and removed {UNIT_NAME} ({}); a server it started runs until it is idle or stopped",
                report.unit.display()
            ),
        })
    });
    match text.and_then(|text| writeln!(out, "{text}").map_err(|error| error.to_string())) {
        Ok(()) => 0,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_commands_parse() {
        assert_eq!(
            parse(&["run"]),
            Ok(ServerCommand::Run { foreground: false })
        );
        assert_eq!(
            parse(&["run", "--foreground"]),
            Ok(ServerCommand::Run { foreground: true })
        );
        assert_eq!(
            parse(&["proxy", "--reconnect"]),
            Ok(ServerCommand::Proxy { reconnect: true })
        );
        assert_eq!(
            parse(&["status", "--json"]),
            Ok(ServerCommand::Status { json: true })
        );
        assert_eq!(parse(&["stop"]), Ok(ServerCommand::Stop));
        assert!(parse(&["stop", "--now"]).is_err());
        assert!(parse(&["run", "extra"]).is_err());
        assert!(parse(&["install"]).is_err());
        assert_eq!(
            parse(&["install-unit", "--json"]),
            Ok(ServerCommand::InstallUnit { json: true })
        );
        assert_eq!(
            parse(&["uninstall-unit"]),
            Ok(ServerCommand::UninstallUnit { json: false })
        );
        assert!(parse(&["install-unit", "--now"]).is_err());
    }

    #[test]
    fn the_server_settings_default_to_what_server_and_app_fall_back_to() {
        let defaults = settings::ServerSettings::default();
        assert_eq!(
            defaults.idle_exit_minutes,
            pom_server::DEFAULT_IDLE_EXIT_MINUTES
        );
        assert_eq!(
            Duration::from_secs(defaults.reconnect_retention_minutes * 60),
            pom_server::RETENTION
        );
        assert_eq!(
            defaults.unconfirmed_input_kib * 1024,
            pom_rpc::pty::UNCONFIRMED_INPUT_LIMIT as u64
        );
    }

    #[test]
    fn the_unit_runs_the_server_in_the_foreground_and_restarts_it_on_failure() {
        let text = unit_text(Path::new("/opt/my tools/pom"), "/usr/bin:/home/me/100%");
        assert!(
            text.contains("ExecStart=\"/opt/my tools/pom\" server run --foreground\n"),
            "{text}"
        );
        assert!(
            text.contains("Environment=PATH=/usr/bin:/home/me/100%%\n"),
            "{text}"
        );
        assert!(text.contains("Restart=on-failure\n"), "{text}");
        assert!(text.contains("WantedBy=default.target\n"), "{text}");
        assert!(text.is_ascii());
    }

    #[test]
    fn install_and_uninstall_write_the_unit_and_ask_systemctl() -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let calls = temp.path().join("calls");
        let fake = temp.path().join("systemctl");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\n", calls.display()),
        )
        .map_err(|error| error.to_string())?;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        let unit = temp.path().join("config/systemd/user").join(UNIT_NAME);
        let program = fake.to_string_lossy();
        let installed = install_unit(&unit, Path::new("/usr/bin/pom"), "/usr/bin", &program)?;
        assert!(installed.installed && installed.enabled);
        let text = std::fs::read_to_string(&unit).map_err(|error| error.to_string())?;
        assert!(text.contains("ExecStart=/usr/bin/pom server run --foreground"));
        let removed = uninstall_unit(&unit, &program)?;
        assert!(!removed.installed);
        assert!(!unit.exists());
        assert_eq!(
            std::fs::read_to_string(&calls).map_err(|error| error.to_string())?,
            "--user daemon-reload\n--user enable pom-server.service\n--user disable pom-server.service\n--user daemon-reload\n"
        );
        uninstall_unit(&unit, "/nonexistent/systemctl")?;
        Ok(())
    }

    #[test]
    fn a_failing_systemctl_is_reported_with_what_it_said() -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let fake = temp.path().join("systemctl");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'Failed to connect to bus' >&2\nexit 1\n",
        )
        .map_err(|error| error.to_string())?;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        let unit = temp.path().join(UNIT_NAME);
        let failed = install_unit(
            &unit,
            Path::new("/usr/bin/pom"),
            "",
            &fake.to_string_lossy(),
        );
        assert_eq!(
            failed,
            Err("systemctl --user daemon-reload failed: Failed to connect to bus".to_string())
        );
        Ok(())
    }
}
