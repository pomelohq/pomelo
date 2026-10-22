use std::path::PathBuf;
use std::process::Command;

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
    ssh.push(remote_command(host.pom_path.as_deref(), args));
    ssh
}

/// The system `ssh` (found on PATH) set up to run pom with `args` on `host`.
pub fn ssh_command(host: &Host, args: &[String], tty: Tty) -> Command {
    let control = control_dir().filter(|dir| ensure_private_dir(dir));
    let mut command = Command::new("ssh");
    command.args(ssh_args(host, args, tty, control.as_deref()));
    command
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

    #[test]
    fn the_remote_command_runs_pom_with_every_argument_intact() {
        let temp = tempfile::tempdir().expect("temp");
        let fake = temp.path().join("pom");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\0' \"$arg\"; done\n",
        )
        .expect("fake pom");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let args: Vec<String> = NASTY.iter().map(|word| word.to_string()).collect();
        let line = remote_command(Some(&fake.to_string_lossy()), &args);
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
        use std::os::unix::fs::PermissionsExt;
        let write = |path: PathBuf, text: &str| {
            std::fs::write(&path, format!("#!/bin/sh\necho {text} \"$@\"\n")).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        };
        write(path_dir.join("pom"), "from-path");
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
        assert_eq!(run(None), "from-path --version");
        write(bin.join("pom"), "from-home");
        assert_eq!(run(None), "from-home --version");
        std::fs::create_dir_all(home.join("opt")).expect("opt");
        write(home.join("opt/pom"), "from-config");
        assert_eq!(run(Some("~/opt/pom")), "from-config --version");
    }

    #[test]
    fn ssh_args_share_one_connection_and_never_prompt_in_batch_mode() {
        let host = Host {
            name: "dev".into(),
            url: HostUrl::parse("ssh://dev@dev.example.net:2222").expect("url"),
            pom_path: None,
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
