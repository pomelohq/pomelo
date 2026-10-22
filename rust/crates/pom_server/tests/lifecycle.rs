//! A real server on a socket in a temp dir: permissions, the lock, the peer check, idle and socket-removed
//! exits, and a log that never holds what a request carried.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_remote::server_api::method;
use pom_server::{client, ServerHandle, ServerOptions, ServerPaths, StartError};
use serde_json::json;

const SECRET: &str = "FAKE-SECRET-7f3a9c";

fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("pomv")
        .tempdir_in("/tmp")
        .expect("temp")
}

fn options(root: &Path) -> ServerOptions {
    let paths = ServerPaths::new(&root.join("run/pom-server"), &root.join("log"), "9.9.9");
    let dispatcher = pom_server::Dispatcher::new(
        StateDir::new(root.join("state")),
        SocketDir::new(root.join("s")),
        PathBuf::from("/bin/false"),
        pom_server::OP_MEMORY,
    );
    let mut options = ServerOptions::new(paths, "9.9.9", dispatcher);
    options.idle_exit = None;
    options
}

fn start(options: ServerOptions) -> (ServerHandle, ServerPaths, JoinHandle<String>) {
    let paths = options.paths.clone();
    let server = pom_server::bind(options).expect("bind");
    let handle = server.handle();
    (handle, paths, std::thread::spawn(move || server.run()))
}

fn finished_within(thread: &JoinHandle<String>, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn the_socket_is_private_and_a_second_server_is_refused() {
    let temp = scratch();
    let (handle, paths, thread) = start(options(temp.path()));
    assert_eq!(mode(&paths.dir), 0o700);
    assert_eq!(mode(&paths.socket), 0o600);
    let pid = std::fs::read_to_string(&paths.pid).expect("pid");
    assert_eq!(pid.trim(), std::process::id().to_string());

    match pom_server::bind(options(temp.path())) {
        Err(StartError::AlreadyRunning(Some(running))) => {
            assert_eq!(running, std::process::id())
        }
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("a second server bound the same version"),
    }

    let (connection, hello) = client::connect(&paths).expect("connect").expect("running");
    assert_eq!(hello.pom_version, "9.9.9");
    assert!(hello.server_id.is_some());
    assert!(hello.features.iter().any(|feature| feature == "ws.list"));
    let info = client::info(&connection).expect("info");
    assert_eq!(info.schema, "pom.server/v1");
    assert_eq!(info.pid, std::process::id());
    assert_eq!(info.connections, 1);
    assert_eq!(info.socket, paths.socket);
    client::stop(&connection).expect("stop");
    assert!(finished_within(&thread, Duration::from_secs(5)));
    assert_eq!(thread.join().expect("run"), "stopped");
    assert!(!paths.socket.exists());
    assert!(!paths.pid.exists());
    drop(handle);

    let (handle, _, thread) = start(options(temp.path()));
    handle.stop();
    assert!(finished_within(&thread, Duration::from_secs(5)));
}

#[test]
fn a_shared_dir_owned_by_someone_else_or_open_to_others_is_not_trusted() {
    let temp = scratch();
    let dir = temp.path().join("pom-server-test");
    std::fs::create_dir(&dir).expect("dir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("chmod");
    pom_server::ensure_private_dir(&dir, pom_server::current_uid()).expect("own dir");
    assert_eq!(mode(&dir), 0o700);
    let refused = pom_server::ensure_private_dir(&dir, pom_server::current_uid() + 1)
        .expect_err("another owner");
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);

    let link = temp.path().join("linked");
    std::os::unix::fs::symlink(&dir, &link).expect("symlink");
    assert!(pom_server::ensure_private_dir(&link, pom_server::current_uid()).is_err());
}

#[test]
fn the_same_user_is_served_and_another_is_refused() {
    let temp = scratch();
    let (handle, paths, thread) = start(options(temp.path()));
    assert_eq!(
        pom_server::peer_uid(&UnixStream::connect(&paths.socket).expect("connect"))
            .expect("peer uid"),
        pom_server::current_uid()
    );
    let (connection, _) = client::connect(&paths).expect("connect").expect("up");
    assert!(client::info(&connection).is_ok());
    connection.close();
    handle.stop();
    assert!(finished_within(&thread, Duration::from_secs(5)));

    let other = scratch();
    let mut options = options(other.path());
    options.peer_uid = Arc::new(|_: &UnixStream| Ok(pom_server::current_uid() + 1));
    let (handle, paths, thread) = start(options);
    assert!(client::connect(&paths).is_err());
    let log = std::fs::read_to_string(&paths.log).expect("log");
    assert!(log.contains("refused: peer uid"), "{log}");
    handle.stop();
    assert!(finished_within(&thread, Duration::from_secs(5)));
}

#[test]
fn an_idle_server_exits_but_not_while_a_client_is_connected() {
    let temp = scratch();
    let mut options = options(temp.path());
    options.idle_exit = Some(Duration::from_millis(300));
    let (_, paths, thread) = start(options);
    let (connection, _) = client::connect(&paths).expect("connect").expect("up");
    std::thread::sleep(Duration::from_millis(900));
    assert!(!thread.is_finished(), "exited with a client connected");
    connection.close();
    assert!(finished_within(&thread, Duration::from_secs(5)));
    assert!(thread.join().expect("run").starts_with("idle"));
    assert!(!paths.socket.exists());
}

#[test]
fn a_deleted_socket_makes_the_server_exit() {
    let temp = scratch();
    let mut options = options(temp.path());
    options.socket_check_every = Duration::from_millis(100);
    let (_, paths, thread) = start(options);
    std::thread::sleep(Duration::from_millis(250));
    assert!(!thread.is_finished());
    std::fs::remove_file(&paths.socket).expect("remove socket");
    assert!(finished_within(&thread, Duration::from_secs(5)));
    assert_eq!(thread.join().expect("run"), "socket removed");
}

#[test]
fn the_log_keeps_methods_and_sizes_never_what_a_request_carried() {
    let temp = scratch();
    let mut options = options(temp.path());
    options.log_rotate_at = 1024;
    let (handle, paths, thread) = start(options);
    let (connection, _) = client::connect(&paths).expect("connect").expect("up");
    let secret_path = format!("/home/dev/{SECRET}/pom.yml");
    for _ in 0..20 {
        let refused = connection.request(method::WS_LIST, json!({ "cfg": secret_path }));
        assert!(refused.is_err());
    }
    assert!(connection
        .request(
            method::AGENT_TURN,
            json!({ "cfg": "/x", "branch": "main", "role": SECRET })
        )
        .is_err());
    assert!(connection
        .request(&format!("x.{SECRET}"), json!({}))
        .is_err());
    connection.close();
    handle.stop();
    assert!(finished_within(&thread, Duration::from_secs(5)));

    let mut rotated = paths.log.clone().into_os_string();
    rotated.push(".1");
    let rotated = PathBuf::from(rotated);
    assert!(rotated.exists(), "the log was not rotated");
    let mut text = std::fs::read_to_string(&rotated).expect("rotated log");
    text.push_str(&std::fs::read_to_string(&paths.log).expect("log"));
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains("method=ws.list"), "{text}");
    assert!(text.contains("result=invalid_params"), "{text}");
    assert!(text.contains("method=unknown"), "{text}");
    assert!(std::fs::metadata(&paths.log).expect("log").len() < 2048);
}
