//! `pty run|attach|kill`, handled before anything else by any binary that hosts holders (the app re-executes
//! itself as `<app> pty run ...`).

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Duration;

use crate::client;
use crate::process::SocketDir;
use crate::server;
use crate::session::StartOptions;

const DETACH_BYTE: u8 = 0x1c;
const USAGE: &str = "usage: pty run <name> [--cwd DIR] [--cols N] [--rows N] -- <cmd> [args...]\n       pty attach <name>\n       pty kill <name>";

/// Runs the `pty` subcommand in `args` (the full argv) and returns the exit code, or `None` when `args` is
/// not a `pty` invocation.
pub fn run(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("pty") {
        return None;
    }
    let rest = args.get(2..).unwrap_or_default();
    let dir = SocketDir::from_env();
    let result = match rest.first().map(String::as_str) {
        Some("run") => run_holder(&dir, &rest[1..]),
        Some("attach") => match rest.get(1) {
            Some(name) => attach(&dir, name),
            None => Err(usage()),
        },
        Some("kill") => match rest.get(1) {
            Some(name) => dir.kill_holder(name),
            None => Err(usage()),
        },
        _ => Err(usage()),
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("pty: {error}");
            1
        }
    })
}

fn usage() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, USAGE)
}

#[derive(Debug, PartialEq, Eq)]
struct RunArgs {
    name: String,
    cwd: Option<PathBuf>,
    cols: u16,
    rows: u16,
    argv: Vec<String>,
}

fn parse_run(args: &[String]) -> io::Result<RunArgs> {
    let separator = args.iter().position(|arg| arg == "--").ok_or_else(usage)?;
    let (flags, command) = (&args[..separator], &args[separator + 1..]);
    let mut flags = flags.iter();
    let name = flags.next().ok_or_else(usage)?.clone();
    let mut parsed = RunArgs {
        name,
        cwd: None,
        cols: 0,
        rows: 0,
        argv: command.to_vec(),
    };
    while let Some(flag) = flags.next() {
        let value = flags.next().ok_or_else(usage)?;
        match flag.as_str() {
            "--cwd" => parsed.cwd = Some(PathBuf::from(value)),
            "--cols" => parsed.cols = value.parse().map_err(|_| usage())?,
            "--rows" => parsed.rows = value.parse().map_err(|_| usage())?,
            _ => return Err(usage()),
        }
    }
    if parsed.argv.is_empty() {
        return Err(usage());
    }
    Ok(parsed)
}

fn run_holder(dir: &SocketDir, args: &[String]) -> io::Result<()> {
    let args = parse_run(args)?;
    let cwd = match args.cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    let session = server::listen_and_serve(
        dir,
        &args.name,
        StartOptions {
            argv: args.argv,
            dir: cwd,
            env: std::env::vars().collect(),
            cols: args.cols,
            rows: args.rows,
            on_exit: None,
        },
    )?;
    session.wait();
    // Give the cleanup thread a moment to remove the socket and pidfile before the process exits.
    std::thread::sleep(Duration::from_millis(50));
    Ok(())
}

/// Attaches this terminal to a holder until Ctrl-\ (the holder keeps running).
fn attach(dir: &SocketDir, name: &str) -> io::Result<()> {
    let attached = client::attach(dir, name, 0).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("no session {name:?} (is it running?): {error}"),
        )
    })?;
    let raw = RawMode::enable()?;
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H")?;
    stdout.write_all(&attached.snapshot)?;
    stdout.flush()?;
    let mut connection = attached.connection;
    let mut output = attached.output;
    std::thread::spawn(move || {
        let mut stdout = io::stdout();
        let mut buffer = [0u8; 8192];
        while let Ok(read) = output.read(&mut buffer) {
            if read == 0 || stdout.write_all(&buffer[..read]).is_err() {
                break;
            }
            let _flushed = stdout.flush();
        }
    });
    let mut sizer = connection.try_clone()?;
    std::thread::spawn(move || {
        let mut last = (0, 0);
        loop {
            if let Some(size) = terminal_size() {
                if size != last && sizer.resize(size.0, size.1).is_err() {
                    return;
                }
                last = size;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    let mut stdin = io::stdin();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stdin.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let typed = &buffer[..read];
        if let Some(at) = typed.iter().position(|&byte| byte == DETACH_BYTE) {
            connection.input(&typed[..at])?;
            break;
        }
        connection.input(typed)?;
    }
    drop(raw);
    stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H")?;
    stdout.flush()
}

fn terminal_size() -> Option<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: TIOCGWINSZ writes a winsize into the pointer.
    let result = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
    (result == 0 && size.ws_col > 0).then_some((size.ws_col, size.ws_row))
}

struct RawMode {
    original: libc::termios,
}

impl RawMode {
    fn enable() -> io::Result<RawMode> {
        let fd = io::stdin().as_raw_fd();
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: tcgetattr/tcsetattr on stdin with a valid termios.
        unsafe {
            if libc::tcgetattr(fd, &mut original) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut raw = original;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(RawMode { original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restores the attributes read in `enable`.
        unsafe {
            libc::tcsetattr(io::stdin().as_raw_fd(), libc::TCSANOW, &self.original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn parses_run_arguments() -> io::Result<()> {
        let parsed = parse_run(&strings(&[
            "svc-a",
            "--cwd",
            "/tmp",
            "--cols",
            "120",
            "--rows",
            "40",
            "--",
            "zsh",
            "-lc",
            "npm run dev",
        ]))?;
        assert_eq!(
            parsed,
            RunArgs {
                name: "svc-a".into(),
                cwd: Some(PathBuf::from("/tmp")),
                cols: 120,
                rows: 40,
                argv: strings(&["zsh", "-lc", "npm run dev"]),
            }
        );
        assert!(parse_run(&strings(&["svc-a", "--", ""])).is_ok());
        assert!(parse_run(&strings(&["svc-a"])).is_err());
        assert!(parse_run(&strings(&["svc-a", "--"])).is_err());
        assert!(parse_run(&strings(&["svc-a", "--bogus", "1", "--", "x"])).is_err());
        Ok(())
    }

    #[test]
    fn ignores_other_invocations() {
        assert_eq!(run(&strings(&["pomelo"])), None);
        assert_eq!(run(&strings(&["pomelo", "--help"])), None);
    }
}
