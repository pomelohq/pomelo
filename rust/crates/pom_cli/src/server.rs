//! `pom server run|proxy|status|stop`: the long-lived server a remote app talks to, and the byte pump ssh runs
//! to reach it.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
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
}

pub(crate) fn parse(words: &[&str]) -> Result<ServerCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("server needs run, proxy, status or stop")?;
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
        other => Err(format!(
            "unknown server command {other} (run, proxy, status, stop)"
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
    options.idle_exit = pom_server::idle_exit_from_settings();
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
    }
}
