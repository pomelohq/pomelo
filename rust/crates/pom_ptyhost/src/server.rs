use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use crate::frame::{self, Frame};
use crate::process::{remove_quietly, SocketDir};
use crate::queries::strip_device_queries;
use crate::session::{Session, StartOptions};

/// How long a new connection may take to send its resume offset before it gets the whole scrollback (a
/// one-shot reader sends nothing).
const RESUME_WAIT: Duration = Duration::from_millis(40);
const SNAPSHOT_CHUNK: usize = 32 * 1024;

/// Runs `options` as holder `name`: socket, pidfile and crash log in `dir`. Returns the running session;
/// the socket and pidfile go away when the command exits.
pub fn listen_and_serve(
    dir: &SocketDir,
    name: &str,
    mut options: StartOptions,
) -> io::Result<Session> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir.root())?;
    let socket = dir.socket(name);
    let pidfile = dir.pidfile(name);
    remove_quietly(&socket);
    std::fs::write(&pidfile, format!("{}\n{name}\n", std::process::id()))?;
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            remove_quietly(&pidfile);
            return Err(error);
        }
    };
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    remove_quietly(&dir.crash_log(name));
    let crash_log = dir.crash_log(name);
    let holder_name = name.to_string();
    options.on_exit = Some(Box::new(move |scrollback, status| {
        let mut record = crash_header(&holder_name, status).into_bytes();
        record.extend_from_slice(scrollback);
        if let Err(error) = std::fs::write(&crash_log, record) {
            eprintln!("ptyhost: crash log: {error}");
        }
    }));
    let session = match Session::start(options) {
        Ok(session) => session,
        Err(error) => {
            remove_quietly(&socket);
            remove_quietly(&pidfile);
            return Err(error);
        }
    };
    serve(listener, session.clone())?;
    let watched = session.clone();
    std::thread::Builder::new()
        .name("ptyhost-cleanup".into())
        .spawn(move || {
            watched.wait();
            remove_quietly(&socket);
            remove_quietly(&pidfile);
        })?;
    Ok(session)
}

/// Accepts clients on `listener` for as long as `session` runs.
pub fn serve(listener: UnixListener, session: Session) -> io::Result<()> {
    std::thread::Builder::new()
        .name("ptyhost-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if session.exit_status().is_some() {
                    return;
                }
                let Ok(stream) = stream else {
                    continue;
                };
                let session = session.clone();
                let spawned = std::thread::Builder::new()
                    .name("ptyhost-client".into())
                    .spawn(move || {
                        if let Err(error) = serve_client(&session, stream) {
                            if error.kind() != io::ErrorKind::BrokenPipe {
                                eprintln!("ptyhost: client: {error}");
                            }
                        }
                    });
                if let Err(error) = spawned {
                    eprintln!("ptyhost: client thread: {error}");
                }
            }
        })?;
    Ok(())
}

/// `STOP` for a clean exit or a TERM (a deliberate stop), `CRASH` otherwise.
fn crash_header(name: &str, status: &ExitStatus) -> String {
    let deliberate = status.success() || status.signal() == Some(libc::SIGTERM);
    let kind = if deliberate { "STOP" } else { "CRASH" };
    let detail = if status.success() {
        "exited cleanly".to_string()
    } else {
        format!("exited: {status}")
    };
    format!("{kind}\t{name} - {detail}\n")
}

fn serve_client(session: &Session, mut stream: UnixStream) -> io::Result<()> {
    let (since, pending) = read_leading_resume(&mut stream)?;
    let subscription = session.subscribe_since(since);
    let client = session.add_client();
    let result = (|| {
        frame::write_frame(&mut stream, frame::META, &subscription.end.to_be_bytes())?;
        let snapshot = strip_device_queries(&subscription.snapshot);
        for chunk in snapshot.chunks(SNAPSHOT_CHUNK) {
            frame::write_frame(&mut stream, frame::SNAPSHOT, chunk)?;
        }
        frame::write_frame(&mut stream, frame::SNAPSHOT_END, &[])?;
        let mut input = stream.try_clone()?;
        let reader_session = session.clone();
        let subscription_id = subscription.id();
        std::thread::Builder::new()
            .name("ptyhost-input".into())
            .spawn(move || {
                if let Some(frame) = pending {
                    apply(&reader_session, client, &frame);
                }
                while let Ok(frame) = frame::read_frame(&mut input) {
                    apply(&reader_session, client, &frame);
                }
                // The client is gone: drop its size and wake the output loop so it ends too.
                reader_session.remove_client(client);
                reader_session.unsubscribe_id(subscription_id);
            })?;
        for chunk in subscription.output.iter() {
            stream.write_all(&chunk)?;
        }
        Ok(())
    })();
    if let Err(error) = stream.shutdown(std::net::Shutdown::Both) {
        if error.kind() != io::ErrorKind::NotConnected {
            eprintln!("ptyhost: close client: {error}");
        }
    }
    session.remove_client(client);
    session.unsubscribe(&subscription);
    result
}

/// The client's optional resume offset, sent first. Anything else arriving first is handed back to apply.
fn read_leading_resume(stream: &mut UnixStream) -> io::Result<(u64, Option<Frame>)> {
    stream.set_read_timeout(Some(RESUME_WAIT))?;
    let first = frame::read_frame(stream);
    stream.set_read_timeout(None)?;
    Ok(match first {
        Ok(frame) if frame.kind == frame::RESUME => {
            (frame::parse_u64(&frame.payload).unwrap_or(0), None)
        }
        Ok(frame) => (0, Some(frame)),
        Err(_) => (0, None),
    })
}

fn apply(session: &Session, client: u64, frame: &Frame) {
    match frame.kind {
        frame::INPUT => {
            if let Err(error) = session.write(&frame.payload) {
                eprintln!("ptyhost: input: {error}");
            }
        }
        frame::RESIZE => {
            if let Some((cols, rows)) = frame::parse_resize(&frame.payload) {
                session.client_size(client, cols, rows);
            }
        }
        frame::PRIMARY => session.set_primary(client),
        _ => {}
    }
}
