use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::frame;
use crate::process::SocketDir;

/// A connection to a holder after its snapshot: write input and sizes through it, read raw live output from
/// `output`.
pub struct Attached {
    pub connection: HolderConnection,
    /// Scrollback after the resume point, with terminal queries removed.
    pub snapshot: Vec<u8>,
    /// Absolute output offset at the end of the snapshot; pass it back to resume without a replay.
    pub end: u64,
    pub output: BufReader<UnixStream>,
}

pub struct HolderConnection {
    stream: UnixStream,
}

impl HolderConnection {
    pub fn input(&mut self, bytes: &[u8]) -> io::Result<()> {
        frame::write_input(&mut self.stream, bytes)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        frame::write_resize(&mut self.stream, cols, rows)
    }

    /// Marks this client as the one whose size the holder follows.
    pub fn primary(&mut self) -> io::Result<()> {
        frame::write_primary(&mut self.stream)
    }

    /// Claims the session's input lease, so this client may type while a lease is set.
    pub fn claim(&mut self, token: &str) -> io::Result<()> {
        frame::write_claim(&mut self.stream, token)
    }

    pub fn try_clone(&self) -> io::Result<HolderConnection> {
        Ok(HolderConnection {
            stream: self.stream.try_clone()?,
        })
    }

    pub fn shutdown(&self) -> io::Result<()> {
        self.stream.shutdown(std::net::Shutdown::Both)
    }
}

/// Connects to holder `name` and reads its snapshot from offset `since` (0 for everything).
pub fn attach(dir: &SocketDir, name: &str, since: u64) -> io::Result<Attached> {
    let mut stream = UnixStream::connect(dir.socket(name))?;
    frame::write_resume(&mut stream, since)?;
    let mut output = BufReader::new(stream.try_clone()?);
    let (snapshot, end) = read_snapshot(&mut output)?;
    Ok(Attached {
        connection: HolderConnection { stream },
        snapshot,
        end,
        output,
    })
}

/// A connection that only writes: no scrollback is replayed to it.
pub fn connect_writer(dir: &SocketDir, name: &str) -> io::Result<HolderConnection> {
    let mut stream = UnixStream::connect(dir.socket(name))?;
    frame::write_resume(&mut stream, frame::NO_SNAPSHOT)?;
    let mut output = BufReader::new(stream.try_clone()?);
    read_snapshot(&mut output)?;
    // Keep draining live output so the holder never blocks on this client.
    std::thread::Builder::new()
        .name("ptyhost-writer-drain".into())
        .spawn(move || {
            let mut sink = std::io::sink();
            if let Err(error) = std::io::copy(&mut output, &mut sink) {
                if error.kind() != io::ErrorKind::ConnectionReset {
                    eprintln!("ptyhost: writer drain: {error}");
                }
            }
        })?;
    Ok(HolderConnection { stream })
}

/// The holder's current scrollback, without staying attached (a log peek).
pub fn snapshot(dir: &SocketDir, name: &str, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(dir.socket(name))?;
    stream.set_read_timeout(Some(timeout))?;
    frame::write_resume(&mut stream, 0)?;
    let mut reader = BufReader::new(stream);
    read_snapshot(&mut reader).map(|(snapshot, _)| snapshot)
}

fn read_snapshot(reader: &mut BufReader<UnixStream>) -> io::Result<(Vec<u8>, u64)> {
    let mut snapshot = Vec::new();
    let mut end = 0;
    loop {
        let frame = frame::read_frame(reader)?;
        match frame.kind {
            frame::META => end = frame::parse_u64(&frame.payload).unwrap_or(0),
            frame::SNAPSHOT => snapshot.extend_from_slice(&frame.payload),
            frame::SNAPSHOT_END => return Ok((snapshot, end)),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected holder frame {other:#04x}"),
                ));
            }
        }
    }
}

/// Waits for holder `name`'s socket to accept connections (a freshly spawned holder takes a moment).
pub fn wait_for_holder(dir: &SocketDir, name: &str, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(dir.socket(name)) {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

pub struct SpawnRequest<'a> {
    /// The binary that runs `pty run` (the app re-executes itself).
    pub binary: &'a Path,
    pub name: &'a str,
    pub cwd: &'a Path,
    pub cols: u16,
    pub rows: u16,
    pub argv: &'a [String],
    /// Added to the inherited environment.
    pub env: &'a [(String, String)],
}

/// Markers a coding agent sets for the processes it runs. A holder outlives whoever spawned it and is never part
/// of that agent's session: inherited, they turn off the transcript of an agent started inside the holder
/// (`CLAUDE_CODE_CHILD_SESSION`) and hand it the parent's messaging token. User settings such as
/// `CLAUDE_CONFIG_DIR` or `CLAUDE_CODE_USE_BEDROCK` are kept.
pub const INHERITED_SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_PID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_EFFORT",
];

fn holder_command(dir: &SocketDir, request: &SpawnRequest<'_>) -> Command {
    let mut command = Command::new(request.binary);
    command
        .args(["pty", "run", request.name, "--cwd"])
        .arg(request.cwd);
    if request.cols > 0 && request.rows > 0 {
        command
            .args(["--cols", &request.cols.to_string()])
            .args(["--rows", &request.rows.to_string()]);
    }
    for marker in INHERITED_SESSION_MARKERS {
        command.env_remove(marker);
    }
    command
        .arg("--")
        .args(request.argv)
        .envs(request.env.iter().map(|(key, value)| (key, value)))
        .env("POM_PTY_SOCK_DIR", dir.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Starts holder `name` detached from this process (own session, no terminal), unless it already runs.
pub fn spawn_holder(dir: &SocketDir, request: &SpawnRequest<'_>) -> io::Result<()> {
    if dir.holder_alive(request.name) {
        return Ok(());
    }
    let mut command = holder_command(dir, request);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // The holder outlives this process when it quits; while we run, reap it so it never lingers as a zombie.
    std::thread::Builder::new()
        .name("ptyhost-reap".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("ptyhost: reap holder: {error}");
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_holder_drops_the_spawning_agent_session_markers_but_keeps_what_it_is_given() {
        let dir = SocketDir::new("/tmp/pom-test-markers");
        let env = [("POM_AGENT_ROLE".to_string(), "reviewer".to_string())];
        let command = holder_command(
            &dir,
            &SpawnRequest {
                binary: Path::new("/bin/true"),
                name: "ws-myproject-feat-login-claude-raw",
                cwd: Path::new("/"),
                cols: 80,
                rows: 24,
                argv: &[],
                env: &env,
            },
        );
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for marker in [
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_MESSAGING_TOKEN",
        ] {
            assert!(
                envs.contains(&(marker.to_string(), None)),
                "{marker} is removed"
            );
        }
        assert!(envs.contains(&("POM_AGENT_ROLE".into(), Some("reviewer".into()))));
        assert!(!envs.iter().any(|(key, _)| key == "CLAUDE_CONFIG_DIR"));
    }
}
