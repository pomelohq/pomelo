//! Whether a shared service answers, asked from the host on its published port. Nothing here runs inside a
//! container: a periodic `docker exec` queues up behind a stuck runtime and can hang the whole machine.

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_config::{Config, SharedServiceDef};
use pom_env::BIND_IP;
use serde::Serialize;

use crate::control::ServiceRunner;
use crate::health::{loopbacks, parse_duration};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a connection that a server accepted gets to be closed again before it counts as answered.
const CLOSE_GRACE: Duration = Duration::from_millis(200);
const COMMAND_NOT_FOUND: i32 = 127;
const PG_PROTOCOL_3: u32 = 196_608;
const PG_STARTING_UP: &str = "57P03";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SharedHealth {
    pub name: String,
    /// Its container or command runs.
    pub up: bool,
    /// It answers on its port (or its `healthcheck.test` passes on the host).
    pub ready: bool,
    pub port: u16,
}

/// How a shared service is asked whether it answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedProbe {
    /// The configured `healthcheck.test`, run on the host: a shell line or an argv.
    Command(CommandProbe),
    Postgres {
        user: String,
    },
    Redis,
    Tcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandProbe {
    Shell(String),
    Argv(Vec<String>),
}

impl SharedProbe {
    pub fn of(name: &str, def: &SharedServiceDef) -> SharedProbe {
        match host_command(def) {
            Some(command) => SharedProbe::Command(command),
            None => Self::built_in(name, def),
        }
    }

    fn built_in(name: &str, def: &SharedServiceDef) -> SharedProbe {
        let kind = if def.kind.is_empty() { name } else { &def.kind };
        let image = def
            .image
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if def.is_command() {
            SharedProbe::Tcp
        } else if kind == "postgres"
            || image.starts_with("postgres")
            || image.starts_with("postgis")
        {
            SharedProbe::Postgres {
                user: if def.db_user.is_empty() {
                    "postgres".into()
                } else {
                    def.db_user.clone()
                },
            }
        } else if kind == "redis" || image.starts_with("redis") || image.starts_with("valkey") {
            SharedProbe::Redis
        } else {
            SharedProbe::Tcp
        }
    }
}

/// `healthcheck.test` as a host command: a string runs in `sh`, `[CMD, ...]` as argv, `[CMD-SHELL, line]` in
/// `sh`; `[NONE]` or nothing leaves the built-in probe.
fn host_command(def: &SharedServiceDef) -> Option<CommandProbe> {
    let test = def.healthcheck.as_ref()?.test.as_ref()?;
    if let Some(line) = test.scalar_value() {
        let line = line.trim();
        return (!line.is_empty()).then(|| CommandProbe::Shell(line.to_string()));
    }
    let words: Vec<String> = test
        .items()?
        .iter()
        .map(|item| item.text().to_string())
        .collect();
    match words.split_first() {
        Some((first, rest)) if first == "CMD-SHELL" => {
            let line = rest.join(" ");
            (!line.trim().is_empty()).then_some(CommandProbe::Shell(line))
        }
        Some((first, rest)) if first == "CMD" => {
            (!rest.is_empty()).then(|| CommandProbe::Argv(rest.to_vec()))
        }
        Some((first, _)) if first == "NONE" => None,
        Some(_) => Some(CommandProbe::Argv(words)),
        None => None,
    }
}

/// The server accepted a connection and did not drop it at once. A port publisher accepts on the host even
/// while nothing listens in the container, then closes, so a bare connect is not enough.
pub fn tcp_answers(port: u16, timeout: Duration) -> bool {
    loopbacks(port).iter().any(|address| {
        let Ok(mut stream) = TcpStream::connect_timeout(address, timeout) else {
            return false;
        };
        if stream.set_read_timeout(Some(CLOSE_GRACE)).is_err() {
            return false;
        }
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(read) => read > 0,
            Err(error) => matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        }
    })
}

/// A Postgres startup handshake, the way `pg_isready` does it: any answer but "starting up" means it takes
/// connections. Hanging up during authentication keeps the server's log quiet.
pub fn postgres_answers(port: u16, user: &str, timeout: Duration) -> bool {
    let mut body = Vec::new();
    body.extend_from_slice(&PG_PROTOCOL_3.to_be_bytes());
    for (key, value) in [("user", user), ("database", "postgres")] {
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut message = ((body.len() + 4) as u32).to_be_bytes().to_vec();
    message.extend_from_slice(&body);
    loopbacks(port).iter().any(|address| {
        let Ok(mut stream) = TcpStream::connect_timeout(address, timeout) else {
            return false;
        };
        if stream.set_read_timeout(Some(timeout)).is_err()
            || stream.set_write_timeout(Some(timeout)).is_err()
            || stream.write_all(&message).is_err()
        {
            return false;
        }
        let mut head = [0u8; 9];
        let Ok(read) = read_up_to(&mut stream, &mut head) else {
            return false;
        };
        match head.first() {
            Some(b'R') if read == head.len() => {
                if head[5..9] == [0, 0, 0, 0] {
                    // Trusted login: say goodbye so the server does not log an unexpected disconnect.
                    if let Err(error) = stream.write_all(&[b'X', 0, 0, 0, 4]) {
                        eprintln!("services: postgres probe goodbye: {error}");
                    }
                }
                true
            }
            Some(b'E') if read == head.len() => {
                let length = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
                let mut rest = vec![0u8; length.saturating_sub(head.len() - 1).min(4096)];
                let Ok(filled) = read_up_to(&mut stream, &mut rest) else {
                    return false;
                };
                let text = String::from_utf8_lossy(&head[5..]).into_owned()
                    + &String::from_utf8_lossy(&rest[..filled]);
                !text.contains(&format!("C{PG_STARTING_UP}"))
            }
            _ => false,
        }
    })
}

/// A Redis `PING`: any reply but "loading" means it serves (a password-protected one answers NOAUTH).
pub fn redis_answers(port: u16, timeout: Duration) -> bool {
    loopbacks(port).iter().any(|address| {
        let Ok(mut stream) = TcpStream::connect_timeout(address, timeout) else {
            return false;
        };
        if stream.set_read_timeout(Some(timeout)).is_err()
            || stream.set_write_timeout(Some(timeout)).is_err()
            || stream.write_all(b"PING\r\n").is_err()
        {
            return false;
        }
        let mut reply = [0u8; 64];
        let Ok(read) = stream.read(&mut reply) else {
            return false;
        };
        let reply = String::from_utf8_lossy(&reply[..read]);
        (reply.starts_with('+') || reply.starts_with('-')) && !reply.starts_with("-LOADING")
    })
}

fn read_up_to(stream: &mut TcpStream, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

/// The exit code of `command` within `timeout`; `None` when it could not start or ran too long.
pub(crate) fn exit_code_within(command: &mut Command, timeout: Duration) -> Option<i32> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("services: stop a healthcheck past its timeout: {error}");
                }
                // Reaps the killed check; its status no longer matters.
                let _ = child.wait();
                return None;
            }
        }
    }
}

impl SharedProbe {
    /// Asks once. A host command that is not installed (exit 127) falls back to the built-in probe, so a
    /// test written for inside a container does not keep the service from ever counting as ready.
    pub fn answers(
        &self,
        name: &str,
        def: &SharedServiceDef,
        port: u16,
        cwd: &Path,
        timeout: Duration,
    ) -> bool {
        match self {
            SharedProbe::Command(probe) => {
                let mut command = match probe {
                    CommandProbe::Shell(line) => {
                        let mut command = Command::new("/bin/sh");
                        command.args(["-c", line]);
                        command
                    }
                    CommandProbe::Argv(argv) => {
                        let mut command = Command::new(&argv[0]);
                        command.args(&argv[1..]);
                        command
                    }
                };
                command
                    .current_dir(cwd)
                    .env("PATH", crate::tool_path())
                    .env("PORT", port.to_string())
                    .env("BIND_IP", BIND_IP);
                match exit_code_within(&mut command, timeout) {
                    Some(0) => true,
                    Some(COMMAND_NOT_FOUND) => {
                        SharedProbe::built_in(name, def).answers(name, def, port, cwd, timeout)
                    }
                    _ => false,
                }
            }
            SharedProbe::Postgres { user } => postgres_answers(port, user, timeout),
            SharedProbe::Redis => redis_answers(port, timeout),
            SharedProbe::Tcp => tcp_answers(port, timeout),
        }
    }
}

impl ServiceRunner {
    /// The host ports a shared service answers on: one per running instance, or its command's port.
    fn shared_probe_ports(&self, name: &str, def: &SharedServiceDef) -> Vec<u16> {
        let base = self.shared_host_port(name);
        let untested_command =
            def.is_command() && !matches!(SharedProbe::of(name, def), SharedProbe::Command(_));
        // A command need not listen anywhere, so without a test it counts as ready once up.
        if base == 0 || untested_command || (!def.is_command() && def.ports.is_empty()) {
            return Vec::new();
        }
        let instances = if def.capacity.is_some() && !def.is_command() {
            self.slots.instance_count(name).max(1)
        } else {
            1
        };
        (0..instances).map(|instance| base + instance).collect()
    }

    /// Whether shared service `name` answers now, asked from the host. One without a port answers once up.
    pub fn shared_ready(&self, config: &Config, name: &str) -> bool {
        let Some(def) = config.shared_services.get(name) else {
            return false;
        };
        let timeout = def.healthcheck.as_ref().map_or(DEFAULT_TIMEOUT, |check| {
            parse_duration(&check.timeout, DEFAULT_TIMEOUT)
        });
        let cwd = if def.is_command() {
            self.shared_command_dir(config, def)
        } else {
            self.project_root.clone()
        };
        let probe = SharedProbe::of(name, def);
        self.shared_probe_ports(name, def)
            .into_iter()
            .all(|port| probe.answers(name, def, port, &cwd, timeout))
    }

    /// Up and ready for every shared service, checked now.
    pub fn shared_health(&self, config: &Config) -> Vec<SharedHealth> {
        let running = self.shared_running();
        self.shared_health_of(config, &running)
    }

    pub(crate) fn shared_health_of(
        &self,
        config: &Config,
        running: &HashSet<String>,
    ) -> Vec<SharedHealth> {
        config
            .shared_services
            .keys()
            .map(|name| {
                let up = running.contains(name);
                SharedHealth {
                    name: name.clone(),
                    up,
                    ready: up && self.shared_ready(config, name),
                    port: self.shared_host_port(name),
                }
            })
            .collect()
    }

    /// Polls the named shared services from the host until each answers or `timeout` passes. `Err` names the
    /// ones still silent.
    pub fn wait_shared_ready(
        &self,
        config: &Config,
        names: &[String],
        timeout: Duration,
    ) -> Result<(), Vec<String>> {
        let deadline = Instant::now() + timeout;
        let interval = names
            .iter()
            .filter_map(|name| config.shared_services.get(name)?.healthcheck.as_ref())
            .map(|check| parse_duration(&check.interval, DEFAULT_INTERVAL))
            .min()
            .unwrap_or(DEFAULT_INTERVAL)
            .min(DEFAULT_INTERVAL);
        let mut waiting: Vec<String> = names.to_vec();
        loop {
            waiting.retain(|name| !self.shared_ready(config, name));
            if waiting.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(waiting);
            }
            std::thread::sleep(interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pom_config::yaml_node::{Node, NodeKind};
    use pom_config::HealthCheck;
    use std::net::TcpListener;

    fn def(image: &str) -> SharedServiceDef {
        SharedServiceDef {
            image: image.into(),
            ports: vec!["1".into()],
            ..SharedServiceDef::default()
        }
    }

    #[test]
    fn well_known_images_get_a_protocol_probe_and_others_a_connect() {
        assert_eq!(
            SharedProbe::of("db", &def("postgres:16")),
            SharedProbe::Postgres {
                user: "postgres".into()
            }
        );
        assert_eq!(
            SharedProbe::of("cache", &def("docker.io/library/redis:7")),
            SharedProbe::Redis
        );
        assert_eq!(
            SharedProbe::of("search", &def("minio/minio")),
            SharedProbe::Tcp
        );
    }

    #[test]
    fn a_configured_test_runs_on_the_host() {
        let mut shell = def("postgres:16");
        shell.healthcheck = Some(HealthCheck {
            test: Some(Node::scalar("pg_isready -h 127.0.0.1 -p $PORT")),
            ..HealthCheck::default()
        });
        assert_eq!(
            SharedProbe::of("db", &shell),
            SharedProbe::Command(CommandProbe::Shell(
                "pg_isready -h 127.0.0.1 -p $PORT".into()
            ))
        );
        let list = |words: &[&str]| Node {
            kind: NodeKind::Sequence(words.iter().map(|w| Node::scalar(*w)).collect()),
            line: 0,
        };
        let mut argv = def("my/image");
        argv.healthcheck = Some(HealthCheck {
            test: Some(list(&["CMD", "curl", "-sf", "http://127.0.0.1"])),
            ..HealthCheck::default()
        });
        assert_eq!(
            SharedProbe::of("x", &argv),
            SharedProbe::Command(CommandProbe::Argv(vec![
                "curl".into(),
                "-sf".into(),
                "http://127.0.0.1".into()
            ]))
        );
        argv.healthcheck = Some(HealthCheck {
            test: Some(list(&["CMD-SHELL", "true"])),
            ..HealthCheck::default()
        });
        assert_eq!(
            SharedProbe::of("x", &argv),
            SharedProbe::Command(CommandProbe::Shell("true".into()))
        );
        argv.healthcheck = Some(HealthCheck {
            test: Some(list(&["NONE"])),
            ..HealthCheck::default()
        });
        assert_eq!(SharedProbe::of("x", &argv), SharedProbe::Tcp);
    }

    #[test]
    fn a_listener_that_stays_open_answers_and_one_that_hangs_up_does_not() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let held = listener.accept().map(|(stream, _)| stream);
            let dropped = listener.accept().map(|(stream, _)| stream);
            drop(dropped);
            std::thread::sleep(Duration::from_millis(400));
            drop(held);
        });
        let timeout = Duration::from_secs(2);
        assert!(tcp_answers(port, timeout), "an open listener answers");
        assert!(
            !tcp_answers(port, timeout),
            "a publisher that hangs up does not"
        );
        assert!(server.join().is_ok());
        let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
        let free = closed.local_addr().expect("address").port();
        drop(closed);
        assert!(
            !tcp_answers(free, Duration::from_millis(200)),
            "nothing listens"
        );
    }

    fn reply_once(reply: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return Vec::new();
            };
            let mut request = vec![0u8; 512];
            let read = stream.read(&mut request).unwrap_or(0);
            request.truncate(read);
            if let Err(error) = stream.write_all(reply) {
                eprintln!("{error}");
            }
            request
        });
        (port, server)
    }

    #[test]
    fn postgres_answers_once_past_starting_up() {
        let timeout = Duration::from_secs(2);
        let (port, server) = reply_once(&[b'R', 0, 0, 0, 12, 0, 0, 0, 10]);
        assert!(postgres_answers(port, "app", timeout));
        let request = server.join().expect("server");
        assert_eq!(&request[4..8], &PG_PROTOCOL_3.to_be_bytes());
        assert!(String::from_utf8_lossy(&request).contains("user\0app\0"));
        let (port, server) =
            reply_once(b"E\0\0\0\x30SFATAL\0C57P03\0Mthe database system is starting up\0\0");
        assert!(!postgres_answers(port, "app", timeout));
        assert!(server.join().is_ok());
        let (port, server) = reply_once(b"E\0\0\0\x20SFATAL\0C28000\0Mno such role\0\0");
        assert!(
            postgres_answers(port, "app", timeout),
            "a refusal still means it serves"
        );
        assert!(server.join().is_ok());
    }

    #[test]
    fn redis_answers_a_ping_unless_loading() {
        let timeout = Duration::from_secs(2);
        let (port, server) = reply_once(b"+PONG\r\n");
        assert!(redis_answers(port, timeout));
        assert_eq!(server.join().expect("server"), b"PING\r\n");
        let (port, server) = reply_once(b"-LOADING Redis is loading the dataset in memory\r\n");
        assert!(!redis_answers(port, timeout));
        assert!(server.join().is_ok());
    }

    #[test]
    fn a_missing_host_tool_falls_back_to_the_built_in_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let mut service = def("my/image");
        service.healthcheck = Some(HealthCheck {
            test: Some(Node::scalar("pom-no-such-tool-anywhere")),
            ..HealthCheck::default()
        });
        let probe = SharedProbe::of("x", &service);
        let cwd = std::env::temp_dir();
        let timeout = Duration::from_secs(2);
        assert!(probe.answers("x", &service, port, &cwd, timeout));
        service.healthcheck = Some(HealthCheck {
            test: Some(Node::scalar("exit 1")),
            ..HealthCheck::default()
        });
        let failing = SharedProbe::of("x", &service);
        assert!(!failing.answers("x", &service, port, &cwd, timeout));
        drop(listener);
    }
}
