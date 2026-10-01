//! Shared services that are a command rather than a Docker image: one process per project, in its own holder,
//! reached by every workspace on the port it was given.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pom_config::{Config, SharedServiceDef};
use pom_env::BIND_IP;

use crate::control::{shell_quote, ServiceError, ServiceRunner};

/// How long a starting command gets to pass its healthcheck before the services that use it start anyway.
const HEALTH_WAIT: Duration = Duration::from_secs(30);
const HEALTH_POLL: Duration = Duration::from_millis(500);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

impl ServiceRunner {
    /// The holder a command shared service runs in, the same whichever workspace started it.
    pub fn shared_holder(&self, name: &str) -> String {
        format!("shr-{}-{name}", pom_env::branch_safe(&self.session))
    }

    /// Where `def`'s command runs: its repo's checkout in the main workspace, else the project folder.
    pub fn shared_command_dir(&self, config: &Config, def: &SharedServiceDef) -> PathBuf {
        if def.repo.is_empty() {
            return self.project_root.clone();
        }
        pom_layout::repo_worktree(
            &self.project_root,
            &def.repo,
            config.global_default_branch(),
            true,
        )
    }

    /// Starts the command shared service `name` unless it already runs; returns its holder name.
    pub fn start_shared_command(
        &self,
        config: &Config,
        name: &str,
    ) -> Result<String, ServiceError> {
        let def = config
            .shared_services
            .get(name)
            .filter(|def| def.is_command())
            .ok_or_else(|| ServiceError::UnknownService(name.to_string()))?;
        let holder = self.shared_holder(name);
        if self.holders().holder_alive(&holder) {
            return Ok(holder);
        }
        let key = pom_ports::shared_key(name, 0);
        let port = match def.port {
            Some(wanted) => {
                let leased = self.ports.acquire_preferred(&key, wanted, 0);
                if leased != Some(wanted) || !self.ports.bindable(wanted) {
                    if leased.is_some_and(|leased| leased != wanted) {
                        self.ports.release(&key);
                    }
                    return Err(ServiceError::PortBusy {
                        port: wanted,
                        owner: crate::control::port_owner(wanted),
                    });
                }
                wanted
            }
            None => self.preflight_port(&key)?,
        };
        let cwd = self.shared_command_dir(config, def);
        let command = format!(
            "cd {} && export PORT={port} BIND_IP={BIND_IP} && {}",
            shell_quote(&cwd.to_string_lossy()),
            def.cmd
        );
        let env: Vec<(String, String)> = def
            .environment
            .iter()
            .map(|(key, value)| (key.clone(), with_port(value, port)))
            .collect();
        self.ports.set_holder(&key, &holder);
        self.spawn(&holder, &cwd, &command, env)?;
        self.wait_healthy(name, def, port);
        Ok(holder)
    }

    pub fn stop_shared_command(&self, name: &str) -> std::io::Result<()> {
        match self.holders().kill_holder(&self.shared_holder(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// Waits for the healthcheck's command to pass, so services that call this one find it up. One that never
    /// passes only delays them: the service may still work, and its console says what went wrong.
    fn wait_healthy(&self, name: &str, def: &SharedServiceDef, port: u16) {
        let Some(test) = def
            .healthcheck
            .as_ref()
            .and_then(|check| check.test.as_ref())
            .and_then(|test| test.scalar_value())
            .filter(|test| !test.trim().is_empty())
        else {
            return;
        };
        let deadline = Instant::now() + HEALTH_WAIT;
        while Instant::now() < deadline {
            let mut check = std::process::Command::new("/bin/sh");
            check
                .args(["-c", test])
                .env("PATH", crate::tool_path())
                .env("PORT", port.to_string())
                .env("BIND_IP", BIND_IP);
            if crate::run_within(&mut check, HEALTH_TIMEOUT).is_ok() {
                return;
            }
            std::thread::sleep(HEALTH_POLL);
        }
        eprintln!("services: shared {name} did not pass its healthcheck in time");
    }
}

/// `$PORT` and `${PORT}` in an environment value, as the command's own shell would expand them.
fn with_port(value: &str, port: u16) -> String {
    value
        .replace("${PORT}", &port.to_string())
        .replace("$PORT", &port.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_values_get_the_port() {
        assert_eq!(
            with_port("http://127.0.0.1:$PORT/a ${PORT}", 4010),
            "http://127.0.0.1:4010/a 4010"
        );
        assert_eq!(with_port("plain", 1), "plain");
    }
}
