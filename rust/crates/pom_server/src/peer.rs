use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

pub fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// The process at the other end of `stream`, as the kernel saw it connect.
#[cfg(target_os = "linux")]
pub fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into `credentials`; `length` holds its size.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.pid)
}

/// The process at the other end of `stream`, as the kernel saw it connect.
#[cfg(target_os = "macos")]
pub fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one pid_t into `pid`; `length` holds its size.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn peer_pid(_stream: &UnixStream) -> io::Result<i32> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// The user id of the process at the other end of `stream`, as the kernel saw it connect.
#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into `credentials`; `length` holds its size.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

/// The user id of the process at the other end of `stream`, as the kernel saw it connect.
#[cfg(not(target_os = "linux"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid writes the peer's ids into the two locals.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
