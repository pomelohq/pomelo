//! An app opened from the Dock inherits a bare environment; servers installed through a version manager or
//! Homebrew are only on the PATH a login shell sets up. So the environment is read from the user's login
//! shell, started in the project directory so per-directory hooks apply too.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// The environment of `$SHELL -l -i` after `cd`-ing into `directory`. The shell writes `env -0` to its stdin
/// descriptor, a pipe here, because an interactive shell's startup files may print to stdout.
pub fn capture_login_env(directory: &Path) -> std::io::Result<HashMap<String, String>> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let (mut reader, writer) = std::io::pipe()?;
    let directory = directory.to_string_lossy().replace('\'', r"'\''");
    let mut command = Command::new(&shell);
    command
        .args(["-l", "-i", "-c"])
        .arg(format!("cd '{directory}'; /usr/bin/env -0 >&0"))
        .stdin(writer)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    // The command keeps its copy of the pipe's write end open, which would keep the read below from ending.
    drop(command);
    let mut output = Vec::new();
    let read = reader.read_to_end(&mut output);
    let status = child.wait()?;
    read?;
    let env = parse_env(&output);
    if env.is_empty() {
        return Err(std::io::Error::other(format!(
            "login shell {shell} gave no environment ({status})"
        )));
    }
    Ok(env)
}

fn parse_env(output: &[u8]) -> HashMap<String, String> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = std::str::from_utf8(entry).ok()?;
            let (name, value) = entry.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_separated_pairs() {
        let env = parse_env(b"PATH=/a:/b\0EMPTY=\0MULTI=x=y\nz\0junk\0");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/a:/b"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(env.get("MULTI").map(String::as_str), Some("x=y\nz"));
        assert_eq!(env.len(), 3);
    }
}
