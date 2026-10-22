//! The user's login shell, and how to run a POSIX script through it. Commands Pomelo builds (exports, `cd`,
//! `exec`) and the ones in pom.yml are POSIX shell syntax; the login shell contributes the user's profile
//! (tool versions, exports) and, when it speaks POSIX, runs the script itself.

use std::ffi::CStr;
use std::path::Path;

const FALLBACK: &str = "/bin/sh";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    program: String,
}

impl Shell {
    /// `$SHELL`, else the account's shell from the user database, else `/bin/sh`.
    pub fn user() -> Shell {
        Shell::choose(
            std::env::var("SHELL").ok().as_deref(),
            account_shell().as_deref(),
            |path| Path::new(path).is_file(),
        )
    }

    pub fn new(program: impl Into<String>) -> Shell {
        Shell {
            program: program.into(),
        }
    }

    fn choose(env: Option<&str>, account: Option<&str>, exists: impl Fn(&str) -> bool) -> Shell {
        [env, account]
            .into_iter()
            .flatten()
            .find(|path| path.starts_with('/') && exists(path))
            .map_or_else(|| Shell::new(FALLBACK), Shell::new)
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    fn name(&self) -> &str {
        Path::new(&self.program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    /// Shells that read POSIX `sh` syntax.
    pub fn is_posix(&self) -> bool {
        matches!(
            self.name(),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "yash"
        )
    }

    /// Runs `script` after the user's login profile. A shell that doesn't speak POSIX (fish, nushell...)
    /// loads its profile, then hands the script to `/bin/sh` with the environment it set up.
    pub fn login_argv(&self, script: &str) -> Vec<String> {
        if self.is_posix() {
            return vec![self.program.clone(), "-lc".into(), script.into()];
        }
        let hand_off = match self.name() {
            "fish" => "exec /bin/sh -c $argv[1]",
            _ => "exec /bin/sh -c \"$1\"",
        };
        vec![
            self.program.clone(),
            "-l".into(),
            "-c".into(),
            hand_off.into(),
            script.into(),
        ]
    }

    /// Runs `script` without the login profile, in the user's shell when it speaks POSIX, else `/bin/sh`.
    pub fn script_argv(&self, script: &str) -> Vec<String> {
        let program = if self.is_posix() {
            self.program.clone()
        } else {
            FALLBACK.into()
        };
        vec![program, "-c".into(), script.into()]
    }

    /// An interactive login shell, for a terminal.
    pub fn interactive_argv(&self) -> Vec<String> {
        vec![self.program.clone(), "-l".into()]
    }

    /// `login_argv` as a ready command.
    pub fn login_command(&self, script: &str) -> std::process::Command {
        command(self.login_argv(script))
    }

    /// `script_argv` as a ready command.
    pub fn script_command(&self, script: &str) -> std::process::Command {
        command(self.script_argv(script))
    }
}

fn command(argv: Vec<String>) -> std::process::Command {
    let mut argv = argv.into_iter();
    let mut command = std::process::Command::new(argv.next().unwrap_or_else(|| FALLBACK.into()));
    command.args(argv);
    command
}

fn account_shell() -> Option<String> {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 4096];
    // SAFETY: getpwuid_r writes only into `entry` and `buffer`, whose sizes are passed; `found` points at
    // `entry` on success, and its strings live in `buffer`, read before either goes out of scope.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_shell.is_null() {
        return None;
    }
    // SAFETY: a successful lookup leaves `pw_shell` pointing at a NUL-terminated string inside `buffer`.
    let shell = unsafe { CStr::from_ptr(entry.pw_shell) };
    Some(shell.to_string_lossy().into_owned()).filter(|shell| !shell.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_wins_then_the_account_then_sh() {
        let all = |_: &str| true;
        assert_eq!(
            Shell::choose(Some("/bin/zsh"), Some("/bin/bash"), all).program(),
            "/bin/zsh"
        );
        assert_eq!(
            Shell::choose(None, Some("/bin/bash"), all).program(),
            "/bin/bash"
        );
        assert_eq!(
            Shell::choose(Some(""), Some("bash"), all).program(),
            "/bin/sh"
        );
        assert_eq!(
            Shell::choose(Some("/opt/gone/zsh"), Some("/bin/bash"), |path| path
                == "/bin/bash")
            .program(),
            "/bin/bash",
            "a shell that is not installed is skipped"
        );
        assert_eq!(Shell::choose(None, None, all).program(), "/bin/sh");
    }

    #[test]
    fn posix_shells_run_the_script_themselves() {
        let zsh = Shell::new("/bin/zsh");
        assert_eq!(zsh.login_argv("echo hi"), ["/bin/zsh", "-lc", "echo hi"]);
        assert_eq!(zsh.script_argv("echo hi"), ["/bin/zsh", "-c", "echo hi"]);
        assert_eq!(zsh.interactive_argv(), ["/bin/zsh", "-l"]);
        assert_eq!(
            Shell::new("/usr/bin/bash").login_argv("x"),
            ["/usr/bin/bash", "-lc", "x"]
        );
    }

    #[test]
    fn other_shells_load_their_profile_then_hand_the_script_to_sh() {
        let fish = Shell::new("/usr/bin/fish");
        assert!(!fish.is_posix());
        assert_eq!(
            fish.login_argv("echo $PORT && exit 3"),
            [
                "/usr/bin/fish",
                "-l",
                "-c",
                "exec /bin/sh -c $argv[1]",
                "echo $PORT && exit 3"
            ]
        );
        assert_eq!(fish.script_argv("x"), ["/bin/sh", "-c", "x"]);
        assert_eq!(fish.interactive_argv(), ["/usr/bin/fish", "-l"]);
    }

    #[test]
    fn a_login_script_runs_and_keeps_its_exit_status() -> std::io::Result<()> {
        for program in ["/bin/sh", "/bin/bash", "/bin/zsh", "/usr/bin/fish"] {
            if !Path::new(program).is_file() {
                continue;
            }
            let output = Shell::new(program)
                .login_command("VALUE=ok; printf '%s' \"$VALUE\" && exit 7")
                .env("HOME", std::env::temp_dir())
                .output()?;
            assert_eq!(String::from_utf8_lossy(&output.stdout), "ok", "{program}");
            assert_eq!(output.status.code(), Some(7), "{program}");
        }
        Ok(())
    }

    #[test]
    fn the_account_shell_is_an_absolute_path() {
        if let Some(shell) = account_shell() {
            assert!(shell.starts_with('/'), "{shell}");
        }
    }
}
