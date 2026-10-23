//! A real server on a socket in a temp dir, serving a scratch project whose terminals are real holders (the
//! `pomelo-pty` binary of this build) in a temp socket dir.
// Each test file uses a part of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_rpc::pty::{self, AttachParams, Kind, OpenParams, Opened};
use pom_rpc::{Connection, StreamEvent};
use pom_server::{client, Dispatcher, ServerHandle, ServerOptions, ServerPaths, ShellCommand};

pub const TIMEOUT: Duration = Duration::from_secs(10);

/// `cargo test` builds every binary of the workspace before it runs a test; alone, build the holder first.
pub fn holder_binary() -> PathBuf {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|test| Some(test.parent()?.parent()?.join("pomelo-pty")))
        .unwrap_or_default();
    assert!(
        binary.is_file(),
        "{} is missing: cargo build -p pom_ptyhost --bin pomelo-pty",
        binary.display()
    );
    binary
}

pub struct Remote {
    pub temp: tempfile::TempDir,
    pub handle: ServerHandle,
    pub paths: ServerPaths,
    thread: Option<JoinHandle<String>>,
    pub connection: Connection,
    shell: Vec<String>,
    retention: Option<Duration>,
}

impl Remote {
    /// A server whose new shells run `shell` (a plain command, never a login shell); empty follows the
    /// `terminal_shell` setting.
    pub fn start(shell: &[&str]) -> Remote {
        Self::start_retaining(shell, None)
    }

    /// Like [`Remote::start`], keeping what vanished clients left for `retention` instead of the default.
    pub fn start_retaining(shell: &[&str], retention: Option<Duration>) -> Remote {
        let temp = tempfile::Builder::new()
            .prefix("pomt")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api/src"] {
            std::fs::create_dir_all(root.join(repo)).expect("worktree");
        }
        for worktree in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(worktree).join(".git")).expect("git");
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web:\n        type: backend\n        cmd: \"exec sleep 60\"\n",
        )
        .expect("pom.yml");
        let state = StateDir::new(temp.path().join("state"));
        let mut sessions = pom_sessions::Sessions::load(&state);
        sessions.touch("demo", &root.to_string_lossy(), 10);
        sessions.save(&state).expect("sessions");

        let shell: Vec<String> = shell.iter().map(|part| part.to_string()).collect();
        let paths = ServerPaths::new(&temp.path().join("run"), &temp.path().join("log"), "9.9.9");
        let (handle, thread) = Self::serve(temp.path(), &paths, &shell, retention);
        let connection = Self::connect_to(&paths);
        Remote {
            temp,
            handle,
            paths,
            thread: Some(thread),
            connection,
            shell,
            retention,
        }
    }

    fn serve(
        root: &Path,
        paths: &ServerPaths,
        shell: &[String],
        retention: Option<Duration>,
    ) -> (ServerHandle, JoinHandle<String>) {
        let dispatcher = Dispatcher::new(
            StateDir::new(root.join("state")),
            SocketDir::new(root.join("s")),
            holder_binary(),
            pom_server::OP_MEMORY,
        );
        if !shell.is_empty() {
            dispatcher.ptys().set_shell(ShellCommand {
                argv: shell.to_vec(),
                env: vec![("PS1".into(), "$ ".into())],
            });
        }
        if let Some(retention) = retention {
            dispatcher.ptys().set_retention(retention);
        }
        dispatcher
            .jobs()
            .set_task_shell(vec!["/bin/sh".into(), "-c".into()]);
        let mut options = ServerOptions::new(paths.clone(), "9.9.9", dispatcher);
        options.idle_exit = None;
        let server = pom_server::bind(options).expect("bind");
        let handle = server.handle();
        (handle, std::thread::spawn(move || server.run()))
    }

    /// Stops the server and starts another on the same socket, state and holders, as a crash and restart would.
    pub fn restart(&mut self) {
        self.handle.stop();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("the server thread panicked");
            }
        }
        let (handle, thread) =
            Self::serve(self.temp.path(), &self.paths, &self.shell, self.retention);
        self.handle = handle;
        self.thread = Some(thread);
        self.connection = Self::connect_to(&self.paths);
    }

    /// A connection that says it is client `client_id`, as an app reconnecting under the same id does, and the
    /// server's HELLO.
    pub fn connect_as(&self, client_id: &str) -> (Connection, pom_rpc::Hello) {
        let stream =
            std::os::unix::net::UnixStream::connect(&self.paths.socket).expect("server socket");
        let transport = pom_rpc::Transport::unix(stream).expect("transport");
        let mut hello = pom_rpc::Hello::new("9.9.9");
        hello.client_id = Some(client_id.to_string());
        Connection::builder(transport)
            .connect(hello)
            .expect("handshake")
    }

    fn connect_to(paths: &ServerPaths) -> Connection {
        client::connect(paths)
            .expect("connect")
            .expect("server up")
            .0
    }

    pub fn connect(&self) -> Connection {
        Self::connect_to(&self.paths)
    }

    pub fn cfg(&self) -> String {
        self.temp
            .path()
            .join("project/pom.yml")
            .to_string_lossy()
            .into_owned()
    }

    pub fn worktree(&self) -> PathBuf {
        self.temp.path().join("project/workspace--feat")
    }

    pub fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    pub fn open_params(&self) -> OpenParams {
        OpenParams {
            cfg: self.cfg(),
            branch: "feat".into(),
            kind: Kind::Shell,
            cols: 80,
            rows: 24,
            ..OpenParams::default()
        }
    }

    pub fn open(&self, connection: &Connection) -> String {
        let opened: Opened = serde_json::from_value(
            connection
                .request(pty::method::OPEN, self.open_params())
                .expect("pty.open"),
        )
        .expect("opened");
        opened.holder
    }

    pub fn attach_params(&self, holder: &str) -> AttachParams {
        AttachParams {
            cfg: self.cfg(),
            branch: "feat".into(),
            holder: holder.to_string(),
            terminal: "1".into(),
            cols: 80,
            rows: 24,
            ..AttachParams::default()
        }
    }

    pub fn attach(&self, connection: &Connection, holder: &str) -> pty::Terminal {
        pty::Terminal::attach(connection, self.attach_params(holder), TIMEOUT).expect("attach")
    }

    /// Spawns a holder named `name` running `argv` directly, as a service or agent would be.
    pub fn spawn_holder(&self, name: &str, argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|part| part.to_string()).collect();
        let binary = holder_binary();
        pom_ptyhost::spawn_holder(
            &self.holders(),
            &pom_ptyhost::SpawnRequest {
                binary: &binary,
                name,
                cwd: Path::new("/"),
                cols: 80,
                rows: 24,
                argv: &argv,
                env: &[],
                group: None,
            },
        )
        .expect("spawn");
        pom_ptyhost::wait_for_holder(&self.holders(), name, TIMEOUT).expect("holder up");
    }

    pub fn streams(&self) -> usize {
        self.handle.info().streams
    }

    pub fn wait_streams(&self, count: usize) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while self.streams() != count {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.connection.close();
        self.handle.stop();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("the server thread panicked");
            }
        }
        let dir = self.holders();
        let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
        dir.kill_holders_now(&names);
    }
}

/// What a terminal printed, acknowledged as it is read.
pub struct Reader {
    pub text: Vec<u8>,
    pub next: Option<u64>,
    pub resets: Vec<u64>,
    pub ended: Option<String>,
}

impl Reader {
    pub fn new() -> Reader {
        Reader {
            text: Vec::new(),
            next: None,
            resets: Vec::new(),
            ended: None,
        }
    }

    /// Reads until the output holds `want`; offsets must follow on from each other.
    pub fn until(&mut self, terminal: &pty::Terminal, want: &str) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while !String::from_utf8_lossy(&self.text).contains(want) {
            if Instant::now() >= deadline || self.ended.is_some() {
                return false;
            }
            self.step(terminal, Duration::from_millis(50));
        }
        true
    }

    pub fn step(&mut self, terminal: &pty::Terminal, wait: Duration) {
        match terminal.output.recv_timeout(wait) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                if let Some(next) = self.next {
                    assert_eq!(offset, next, "data must follow on");
                }
                self.next = Some(offset + bytes.len() as u64);
                self.text.extend_from_slice(&bytes);
                terminal.control.ack(offset + bytes.len() as u64);
            }
            Ok(StreamEvent::Reset { offset }) => {
                self.resets.push(offset);
                self.next = Some(offset);
            }
            Ok(StreamEvent::End { reason }) => self.ended = Some(reason),
            Err(pom_rpc::Error::Timeout) => {}
            Err(error) => self.ended = Some(error.to_string()),
        }
    }

    pub fn until_end(&mut self, terminal: &pty::Terminal) -> Option<String> {
        let deadline = Instant::now() + TIMEOUT;
        while self.ended.is_none() && Instant::now() < deadline {
            self.step(terminal, Duration::from_millis(50));
        }
        self.ended.clone()
    }
}
