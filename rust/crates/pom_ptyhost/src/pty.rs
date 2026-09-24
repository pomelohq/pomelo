use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// A command running on a pseudo-terminal: the master side to read output from and write input to.
pub struct Pty {
    pub master: File,
    pub child: Child,
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Starts `argv` in `dir` as the leader of a new session whose controlling terminal is a fresh PTY.
pub fn spawn(
    argv: &[String],
    dir: &Path,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
) -> io::Result<Pty> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let mut size = winsize(cols, rows);
    // SAFETY: openpty writes two valid descriptors on success; the name/termios arguments may be null.
    let result = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just returned by openpty and are owned by nobody else.
    let (master, slave) = unsafe {
        (
            OwnedFd::from_raw_fd(master_fd),
            OwnedFd::from_raw_fd(slave_fd),
        )
    };
    set_cloexec(master.as_raw_fd())?;
    set_cloexec(slave.as_raw_fd())?;

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe calls run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok(Pty {
        master: File::from(master),
        child,
    })
}

pub fn resize(master: &File, cols: u16, rows: u16) -> io::Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads a winsize from the pointer, which outlives the call.
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_cloexec(fd: libc::c_int) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn runs_a_command_on_a_terminal() -> io::Result<()> {
        let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let mut pty = spawn(
            &[
                "sh".into(),
                "-c".into(),
                "test -t 1 && printf tty-ok".into(),
            ],
            Path::new("/"),
            &env,
            100,
            30,
        )?;
        let mut output = Vec::new();
        let mut buffer = [0u8; 256];
        loop {
            match pty.master.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => output.extend_from_slice(&buffer[..read]),
                // The master reports EIO once the child side closes.
                Err(_) => break,
            }
        }
        pty.child.wait()?;
        assert!(String::from_utf8_lossy(&output).contains("tty-ok"));
        Ok(())
    }
}
