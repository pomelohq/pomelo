//! Whether a workspace's repo services are up, listening and healthy, and waiting until they all are.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_config::{Config, ServiceHealthCheck};
use serde::Serialize;

use crate::control::{ServiceRunner, ServiceTarget};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const LISTEN_PROBE: Duration = Duration::from_millis(300);
const CRASH_TAIL_LINES: usize = 20;

/// `500ms`, `2s`, `1m`; empty or unreadable means `default`.
pub fn parse_duration(text: &str, default: Duration) -> Duration {
    let text = text.trim();
    let (number, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let Ok(value) = number.parse::<u64>() else {
        return default;
    };
    match unit {
        "ms" => Duration::from_millis(value),
        "" | "s" => Duration::from_secs(value),
        "m" => Duration::from_secs(value * 60),
        _ => default,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ServiceHealth {
    pub repo: String,
    pub service: String,
    /// Its holder runs.
    pub up: bool,
    /// It answers: the healthcheck passes, else its port listens, else it is up.
    pub ready: bool,
    /// The healthcheck's last result; null without one.
    pub healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// Not up and its holder left a crash record.
    pub crashed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    Ready(Vec<ServiceHealth>),
    TimedOut(Vec<ServiceHealth>),
    /// A service stopped while waiting: its label and the last lines it printed.
    Crashed {
        service: String,
        tail: String,
        services: Vec<ServiceHealth>,
    },
}

fn loopbacks(port: u16) -> [SocketAddr; 2] {
    [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
    ]
}

/// A GET on the service's own port answered 2xx or 3xx within `timeout`.
pub fn http_ok(port: u16, path: &str, timeout: Duration) -> bool {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    loopbacks(port).iter().any(|address| {
        let Ok(mut stream) = TcpStream::connect_timeout(address, timeout) else {
            return false;
        };
        if stream.set_read_timeout(Some(timeout)).is_err()
            || stream.set_write_timeout(Some(timeout)).is_err()
        {
            return false;
        }
        let request =
            format!("GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        if stream.write_all(request.as_bytes()).is_err() {
            return false;
        }
        let mut head = [0u8; 32];
        let Ok(read) = stream.read(&mut head) else {
            return false;
        };
        let line = String::from_utf8_lossy(&head[..read]);
        line.split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .is_some_and(|code| (200..400).contains(&code))
    })
}

/// `command` exited 0 within `timeout`, run with `sh` in `cwd` with `env`.
pub fn cmd_ok(command: &str, cwd: &Path, env: &[(String, String)], timeout: Duration) -> bool {
    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("PATH", crate::tool_path())
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("services: healthcheck {command}: {error}");
                }
                // Reaps the killed check; its status no longer matters.
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn last_lines(output: &[u8], count: usize) -> String {
    let text = String::from_utf8_lossy(output);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

impl ServiceRunner {
    fn healthcheck_of(config: &Config, target: &ServiceTarget) -> Option<ServiceHealthCheck> {
        config
            .repos
            .get(&target.repo)?
            .services
            .get(&target.service)?
            .healthcheck
            .clone()
            .filter(|check| !check.http.is_empty() || !check.cmd.is_empty())
    }

    fn healthcheck_passes(
        &self,
        config: &Config,
        target: &ServiceTarget,
        check: &ServiceHealthCheck,
        port: Option<u16>,
    ) -> bool {
        let timeout = parse_duration(&check.timeout, DEFAULT_TIMEOUT);
        if !check.http.is_empty() {
            return port.is_some_and(|port| http_ok(port, &check.http, timeout));
        }
        let Some(dir) = config.repos.get(&target.repo) else {
            return false;
        };
        let service_dir = dir
            .services
            .get(&target.service)
            .map(|service| service.dir.clone())
            .unwrap_or_default();
        let worktree = pom_layout::repo_worktree(
            self.project_root(),
            &target.repo,
            &target.branch,
            target.is_main,
        )
        .join(service_dir);
        let env = self
            .workspace_env(config, &target.branch)
            .service_env(&target.repo, &target.service);
        cmd_ok(&check.cmd, &worktree, &env, timeout)
    }

    /// Up, ready and healthy for one repo service, checked now.
    pub fn service_health(&self, config: &Config, target: &ServiceTarget) -> ServiceHealth {
        let holder = self.holder_name(target);
        let holders = self.holders();
        let up = holders.holder_alive(&holder);
        let port = self.port(config, target);
        let service = config
            .repos
            .get(&target.repo)
            .and_then(|dir| dir.services.get(&target.service));
        let mode = service
            .map(|service| self.mode(&target.repo, &target.service, service))
            .unwrap_or_default();
        let check = Self::healthcheck_of(config, target);
        let healthy = check
            .as_ref()
            .map(|check| up && self.healthcheck_passes(config, target, check, port));
        let listening = port.is_some_and(|port| pom_ports::loopback_up(port, LISTEN_PROBE));
        let ready = up
            && match healthy {
                Some(healthy) => healthy,
                None if port.is_some() => listening,
                None => true,
            };
        ServiceHealth {
            repo: target.repo.clone(),
            service: target.service.clone(),
            up,
            ready,
            healthy,
            port,
            pid: up.then(|| holders.holder_pid(&holder)).flatten(),
            mode,
            crashed: !up && holders.crash_info(&holder).is_some_and(|info| info.crashed),
        }
    }

    /// Polls `targets` until every one is ready, one stops, or `timeout` passes.
    pub fn wait_ready(
        &self,
        config: &Config,
        targets: &[ServiceTarget],
        timeout: Duration,
    ) -> WaitOutcome {
        let deadline = Instant::now() + timeout;
        let interval = targets
            .iter()
            .filter_map(|target| Self::healthcheck_of(config, target))
            .map(|check| parse_duration(&check.interval, DEFAULT_INTERVAL))
            .min()
            .unwrap_or(DEFAULT_INTERVAL)
            .min(DEFAULT_INTERVAL);
        loop {
            let services: Vec<ServiceHealth> = targets
                .iter()
                .map(|target| self.service_health(config, target))
                .collect();
            if let Some(stopped) = services.iter().position(|health| !health.up) {
                let target = &targets[stopped];
                let tail = self
                    .holders()
                    .crash_info(&self.holder_name(target))
                    .map(|info| last_lines(&info.output, CRASH_TAIL_LINES))
                    .unwrap_or_default();
                return WaitOutcome::Crashed {
                    service: format!("{}/{}", target.repo, target.service),
                    tail,
                    services,
                };
            }
            if services.iter().all(|health| health.ready) {
                return WaitOutcome::Ready(services);
            }
            if Instant::now() >= deadline {
                return WaitOutcome::TimedOut(services);
            }
            std::thread::sleep(interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_the_config_writes_them() {
        assert_eq!(
            parse_duration("500ms", DEFAULT_INTERVAL),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_duration("2s", DEFAULT_INTERVAL),
            Duration::from_secs(2)
        );
        assert_eq!(
            parse_duration("1m", DEFAULT_INTERVAL),
            Duration::from_secs(60)
        );
        assert_eq!(parse_duration("", DEFAULT_INTERVAL), DEFAULT_INTERVAL);
        assert_eq!(parse_duration("soon", DEFAULT_INTERVAL), DEFAULT_INTERVAL);
    }

    #[test]
    fn an_http_check_passes_on_2xx_and_3xx_only() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let answers = ["200 OK", "302 Found", "503 Service Unavailable"];
        let server = std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0u8; 512];
                if stream.read(&mut request).is_ok() {
                    let reply = format!("HTTP/1.1 {answer}\r\nContent-Length: 0\r\n\r\n");
                    if let Err(error) = stream.write_all(reply.as_bytes()) {
                        eprintln!("{error}");
                    }
                }
            }
        });
        let timeout = Duration::from_secs(2);
        assert!(http_ok(port, "/health", timeout));
        assert!(http_ok(port, "health", timeout));
        assert!(!http_ok(port, "/health", timeout));
        assert!(server.join().is_ok());
        assert!(
            !http_ok(port, "/health", Duration::from_millis(200)),
            "nothing listens"
        );
    }

    #[test]
    fn a_cmd_check_is_its_exit_status_within_the_timeout() {
        let cwd = std::env::temp_dir();
        let env = vec![("WANTED".to_string(), "yes".to_string())];
        let quick = Duration::from_secs(2);
        assert!(cmd_ok("test \"$WANTED\" = yes", &cwd, &env, quick));
        assert!(!cmd_ok("exit 1", &cwd, &env, quick));
        assert!(!cmd_ok("sleep 5", &cwd, &env, Duration::from_millis(200)));
    }
}
