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

/// Starts holder `name` detached from this process (own session, no terminal), unless it already runs.
pub fn spawn_holder(dir: &SocketDir, request: &SpawnRequest<'_>) -> io::Result<()> {
    if dir.holder_alive(request.name) {
        return Ok(());
    }
    let mut command = Command::new(request.binary);
    command
        .args(["pty", "run", request.name, "--cwd"])
        .arg(request.cwd);
    if request.cols > 0 && request.rows > 0 {
        command
            .args(["--cols", &request.cols.to_string()])
            .args(["--rows", &request.rows.to_string()]);
    }
    command
        .arg("--")
        .args(request.argv)
        .envs(request.env.iter().map(|(key, value)| (key, value)))
        .env("POM_PTY_SOCK_DIR", dir.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
