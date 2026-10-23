use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::Host;

/// Runs on the remote under `sh -c`, whatever the login shell: `$1` is the configured pom path (empty for
/// the default), the rest are pom's arguments.
const LAUNCHER: &str = r#"p=$1; shift; case $p in "~/"*) p=$HOME/${p#"~/"} ;; "") p=$HOME/.local/bin/pom; [ -x "$p" ] || p=pom ;; esac; exec "$p" "$@""#;

/// One word for a POSIX shell (and fish): single quotes, with `'` and `\` spelled outside them so no shell
/// reads an escape inside.
pub fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|char| {
            char.is_ascii_alphanumeric()
                || matches!(
                    char,
                    '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                )
        });
    if plain {
        return word.to_string();
    }
    let mut quoted = String::from("'");
    for char in word.chars() {
        match char {
            '\'' => quoted.push_str(r#"'"'"'"#),
            '\\' => quoted.push_str(r#"'"\\"'"#),
            other => quoted.push(other),
        }
    }
    quoted.push('\'');
    quoted
}

/// The command line ssh hands to the remote login shell: the launcher, then every argument quoted on its own.
pub fn remote_command(pom_path: Option<&str>, args: &[String]) -> String {
    let mut words = vec![
        "sh".to_string(),
        "-c".to_string(),
        quote(LAUNCHER),
        "pom".to_string(),
        quote(pom_path.unwrap_or("")),
    ];
    words.extend(args.iter().map(|arg| quote(arg)));
    words.join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tty {
    /// A terminal on both ends: the remote gets a TTY and ssh may prompt.
    Interactive,
    /// Scripts and the app: no TTY, and ssh fails instead of prompting.
    Batch,
}

impl Tty {
    pub fn detect() -> Tty {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Tty::Interactive
        } else {
            Tty::Batch
        }
    }
}

/// Where the shared connections' sockets live; `None` turns sharing off rather than risk a path past the
/// socket length limit (104 bytes on macOS).
pub fn control_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty());
    let dir = match runtime {
        Some(runtime) => PathBuf::from(runtime).join("pom-ssh"),
        None => PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?)
            .join(".ssh")
            .join("pom"),
    };
    // %C expands to 40 hex characters.
    if dir.as_os_str().len() + 41 > 100 {
        return None;
    }
    Some(dir)
}

fn ensure_private_dir(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.is_dir() {
        if let Err(error) = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
        {
            eprintln!("ssh connection sharing off: {}: {error}", dir.display());
            return false;
        }
    }
    match std::fs::metadata(dir) {
        Ok(metadata) => metadata.permissions().mode() & 0o077 == 0,
        Err(_) => false,
    }
}

/// ssh's arguments for running pom with `args` on `host`; `control` is the socket dir, if any.
pub fn ssh_args(
    host: &Host,
    args: &[String],
    tty: Tty,
    control: Option<&std::path::Path>,
) -> Vec<String> {
    ssh_args_running(
        host,
        remote_command(host.pom_path.as_deref(), args),
        tty,
        control,
    )
}

fn ssh_args_running(
    host: &Host,
    remote: String,
    tty: Tty,
    control: Option<&std::path::Path>,
) -> Vec<String> {
    let mut ssh: Vec<String> = Vec::new();
    let mut option = |value: String| {
        ssh.push("-o".into());
        ssh.push(value);
    };
    if let Some(dir) = control {
        option("ControlMaster=auto".into());
        option(format!("ControlPath={}/%C", dir.display()));
        option("ControlPersist=10m".into());
    }
    option("ConnectTimeout=10".into());
    option("ServerAliveInterval=15".into());
    option("LogLevel=ERROR".into());
    if tty == Tty::Batch {
        option("BatchMode=yes".into());
    }
    ssh.push(if tty == Tty::Interactive { "-t" } else { "-T" }.into());
    if let Some(port) = host.url.port {
        ssh.push("-p".into());
        ssh.push(port.to_string());
    }
    ssh.push("--".into());
    ssh.push(host.url.destination());
    ssh.push(remote);
    ssh
}

/// The system `ssh` (found on PATH) set up to run pom with `args` on `host`.
pub fn ssh_command(host: &Host, args: &[String], tty: Tty) -> Command {
    Ssh::system().command(host, args, tty)
}

/// Which ssh runs and how: the system one in the app and CLI, a stand-in in tests.
#[derive(Debug, Clone)]
pub struct Ssh {
    program: PathBuf,
    control: Option<PathBuf>,
    envs: Vec<(String, String)>,
}

impl Ssh {
    pub fn system() -> Ssh {
        Ssh {
            program: PathBuf::from("ssh"),
            control: control_dir().filter(|dir| ensure_private_dir(dir)),
            envs: Vec::new(),
        }
    }

    pub fn custom(program: PathBuf, control: Option<PathBuf>, envs: Vec<(String, String)>) -> Ssh {
        Ssh {
            program,
            control,
            envs,
        }
    }

    pub fn command(&self, host: &Host, args: &[String], tty: Tty) -> Command {
        let mut command = Command::new(&self.program);
        command.args(ssh_args(host, args, tty, self.control.as_deref()));
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    /// Runs `script` with `sh` on `host`, without a terminal; for work that is not a pom command.
    pub fn script(&self, host: &Host, script: &str) -> Command {
        let remote = format!("sh -c {}", quote(script));
        let mut command = Command::new(&self.program);
        command.args(ssh_args_running(
            host,
            remote,
            Tty::Batch,
            self.control.as_deref(),
        ));
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    pub fn command_with(&self, host: &Host, args: &[String], options: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        for option in options {
            command.arg("-o").arg(option);
        }
        command.args(ssh_args(host, args, Tty::Batch, self.control.as_deref()));
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    /// Runs pom with `args` on `host` without a terminal, killing ssh once `timeout` passes.
    pub fn output(
        &self,
        host: &Host,
        args: &[String],
        timeout: Duration,
    ) -> std::io::Result<Output> {
        run_with_timeout(self.command(host, args, Tty::Batch), &host.name, timeout)
    }

    /// `ssh -O <verb>` on the shared connection; the host's port and destination follow.
    fn control_command(&self, control: &std::path::Path, verb: &str) -> Command {
        let mut command = Command::new(&self.program);
        command
            .arg("-O")
            .arg(verb)
            .arg("-o")
            .arg(format!("ControlPath={}/%C", control.display()))
            .args(["-o", "BatchMode=yes", "-o", "LogLevel=ERROR"]);
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    fn destination(command: &mut Command, host: &Host) {
        if let Some(port) = host.url.port {
            command.arg("-p").arg(port.to_string());
        }
        command.arg("--").arg(host.url.destination());
    }

    /// A shared connection to `host` is up, so forwards can ride on it without logging in again.
    pub fn master_running(&self, host: &Host) -> bool {
        let Some(control) = self.control.as_deref() else {
            return false;
        };
        let mut command = self.control_command(control, "check");
        Ssh::destination(&mut command, host);
        run_with_timeout(command, &host.name, MASTER_TIMEOUT)
            .is_ok_and(|output| output.status.success())
    }

    /// Adds (`forward`) or removes (`cancel`) the loopback forward `spec` on the shared connection to `host`.
    pub fn master_forward(&self, host: &Host, verb: &str, spec: &str) -> Result<(), String> {
        let control = self
            .control
            .as_deref()
            .ok_or_else(|| "ssh connection sharing is off".to_string())?;
        let mut command = self.control_command(control, verb);
        command.arg("-L").arg(spec);
        Ssh::destination(&mut command, host);
        let output = run_with_timeout(command, &host.name, MASTER_TIMEOUT)
            .map_err(|error| format!("ssh -O {verb}: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let reason = said
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("no message from ssh");
        Err(format!("ssh -O {verb} {spec}: {}", reason.trim()))
    }

    /// A connection of its own that only holds the loopback forward `spec`, for a host without a shared one.
    pub fn tunnel(&self, host: &Host, spec: &str) -> Command {
        let mut command = Command::new(&self.program);
        command.args([
            "-N",
            "-o",
            "ControlPath=none",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "LogLevel=ERROR",
            "-L",
            spec,
        ]);
        Ssh::destination(&mut command, host);
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }
}

const MASTER_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `command` without a terminal, killing it once `timeout` passes.
fn run_with_timeout(
    mut command: Command,
    host_name: &str,
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                if let Err(error) = pipe.read_to_end(&mut bytes) {
                    bytes.extend_from_slice(error.to_string().as_bytes());
                }
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            if let Err(error) = child.kill() {
                eprintln!("ssh to {host_name}: kill after timeout: {error}");
            }
            child.wait()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer from {host_name} after {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Whether the command asks for machine-readable output (`--json`, `-o json`, `--output=json`).
pub fn json_requested(args: &[String]) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        matches!(arg.as_str(), "--json" | "-o=json" | "--output=json")
            || (matches!(arg.as_str(), "-o" | "--output")
                && args.get(index + 1).map(String::as_str) == Some("json"))
    })
}

/// The version out of `pom --version` (`pom 0.9.3`).
pub fn parse_version(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pom "))
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
}

/// Two pom versions disagree on major or minor (patch releases keep the same JSON and commands).
pub fn version_mismatch(local: &str, remote: &str) -> bool {
    let major_minor = |version: &str| -> Vec<String> {
        version
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(2)
            .map(str::to_string)
            .collect()
    };
    major_minor(local) != major_minor(remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostUrl;
    use std::path::Path;

    /// What a POSIX shell makes of `line`, one word per output line (NUL-separated).
    fn words_after_shell(line: &str) -> Vec<String> {
        words_after(Path::new("/bin/sh"), line)
    }

    fn words_after(shell: &Path, line: &str) -> Vec<String> {
        let output = Command::new(shell)
            .arg("-c")
            .arg(format!(
                "for word in {line}; do printf '%s\\0' \"$word\"; done"
            ))
            .output()
            .expect("sh");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf-8")
            .split_terminator('\0')
            .map(str::to_string)
            .collect()
    }

    const NASTY: &[&str] = &[
        "plain",
        "",
        "two words",
        "it's",
        "\"double\"",
        "$HOME",
        "${PATH}",
        "`id`",
        "$(id)",
        "line\nbreak",
        "tab\there",
        "back\\slash\\\\",
        "*.rs",
        "semi;colon && | > <",
        "~/not-expanded",
        "-H",
        "--json",
        "unicode: Tiếng Việt, 日本語",
        "'\\''",
        "!history",
        "#hash",
    ];

    #[test]
    fn quoted_words_reach_the_shell_unchanged() {
        for word in NASTY {
            assert_eq!(
                words_after_shell(&quote(word)),
                vec![word.to_string()],
                "{word:?}"
            );
        }
        let line: Vec<String> = NASTY.iter().map(|word| quote(word)).collect();
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
            let shell = Path::new(shell);
            if shell.exists() {
                assert_eq!(
                    words_after(shell, &line.join(" ")),
                    NASTY.to_vec(),
                    "{shell:?}"
                );
            }
        }
    }

    #[test]
    fn safe_words_stay_bare_and_others_are_single_quoted() {
        assert_eq!(quote("ws"), "ws");
        assert_eq!(quote("feat/login-2"), "feat/login-2");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
        assert_eq!(quote("a\\b"), r#"'a'"\\"'b'"#);
    }

    // Checked in rather than written by the tests: a file written while another test thread spawns can fail to
    // run (ETXTBSY).
    const FAKE_POM_ARGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-pom-args.sh");
    const FAKE_POM_WHERE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-pom-where.sh");

    #[test]
    fn the_remote_command_runs_pom_with_every_argument_intact() {
        let args: Vec<String> = NASTY.iter().map(|word| word.to_string()).collect();
        let line = remote_command(Some(FAKE_POM_ARGS), &args);
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(&line)
            .output()
            .expect("sh");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let received: Vec<String> = String::from_utf8(output.stdout)
            .expect("utf-8")
            .split_terminator('\0')
            .map(str::to_string)
            .collect();
        assert_eq!(received, args);
    }

    #[test]
    fn the_launcher_finds_pom_under_home_then_on_path() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path().join("home");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let path_dir = temp.path().join("path");
        std::fs::create_dir_all(&path_dir).expect("path");
        let link = |path: PathBuf| std::os::unix::fs::symlink(FAKE_POM_WHERE, path).expect("link");
        link(path_dir.join("pom"));
        let run = |pom_path: Option<&str>| {
            let line = remote_command(pom_path, &["--version".to_string()]);
            let output = Command::new("/bin/sh")
                .arg("-c")
                .arg(&line)
                .env("HOME", &home)
                .env("PATH", format!("{}:/usr/bin:/bin", path_dir.display()))
                .output()
                .expect("sh");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        assert_eq!(run(None), "path --version");
        link(bin.join("pom"));
        assert_eq!(run(None), "bin --version");
        std::fs::create_dir_all(home.join("opt")).expect("opt");
        link(home.join("opt/pom"));
        assert_eq!(run(Some("~/opt/pom")), "opt --version");
    }

    #[test]
    fn ssh_args_share_one_connection_and_never_prompt_in_batch_mode() {
        let host = Host {
            name: "dev".into(),
            url: HostUrl::parse("ssh://dev@dev.example.net:2222").expect("url"),
            pom_path: None,
            transport: None,
        };
        let control = PathBuf::from("/run/user/1000/pom-ssh");
        let args = ssh_args(&host, &["status".into()], Tty::Batch, Some(&control));
        let joined = args.join(" ");
        assert!(joined.contains("-o ControlMaster=auto"));
        assert!(joined.contains("-o ControlPath=/run/user/1000/pom-ssh/%C"));
        assert!(joined.contains("-o ControlPersist=10m"));
        assert!(joined.contains("-o BatchMode=yes"));
        assert!(joined.contains("-o ConnectTimeout=10"));
        assert!(args.contains(&"-T".to_string()));
        let tail: Vec<&str> = args
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(String::as_str)
            .collect();
        assert_eq!(tail[0], "2222");
        assert_eq!(tail[1], "--");
        assert_eq!(tail[2], "dev@dev.example.net");
        assert!(tail[3].ends_with(" '' status"));

        let interactive = ssh_args(&host, &[], Tty::Interactive, None);
        assert!(interactive.contains(&"-t".to_string()));
        assert!(!interactive.join(" ").contains("BatchMode"));
        assert!(!interactive.join(" ").contains("ControlMaster"));
    }

    #[test]
    fn json_flags_and_versions() {
        let words =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_string).collect() };
        assert!(json_requested(&words("ws ls --json")));
        assert!(json_requested(&words("status -o json")));
        assert!(json_requested(&words("status --output=json")));
        assert!(!json_requested(&words("status -o table")));
        assert!(!json_requested(&words("logs api/web")));

        assert_eq!(parse_version("pom 0.9.3\n").as_deref(), Some("0.9.3"));
        assert_eq!(parse_version("bash: pom: not found"), None);
        assert!(!version_mismatch("0.9.3", "0.9.7"));
        assert!(version_mismatch("0.9.3", "0.10.0"));
        assert!(version_mismatch("1.0.0", "0.9.3"));
    }
}
