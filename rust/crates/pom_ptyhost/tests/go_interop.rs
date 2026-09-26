//! Opt-in (`POM_GO_ORACLE=1`): holders of the previous core and of this crate reach each other both ways,
//! so switching binaries never strands a running service.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_ptyhost::{SocketDir, SpawnRequest};

const BINARY: &str = env!("CARGO_BIN_EXE_pomelo-pty");
const TIMEOUT: Duration = Duration::from_secs(10);

fn go_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn enabled() -> bool {
    std::env::var_os("POM_GO_ORACLE").is_some()
}

#[test]
fn a_go_holder_accepts_a_rust_client() {
    if !enabled() {
        return;
    }
    let temp = tempfile::Builder::new()
        .prefix("pty")
        .tempdir_in("/tmp")
        .expect("temp");
    let dir = SocketDir::new(temp.path());
    let go_binary = temp.path().join("pom");
    let built = Command::new("go")
        .args(["build", "-o"])
        .arg(&go_binary)
        .arg("./cmd/pom")
        .current_dir(go_root())
        .status()
        .expect("go build");
    assert!(built.success());
    let mut holder = Command::new(&go_binary)
        .args(["pty", "run", "go-holder", "--cwd", "/", "--", "cat"])
        .env("POM_PTY_SOCK_DIR", temp.path())
        .stdout(Stdio::null())
        .spawn()
        .expect("go holder");
    pom_ptyhost::wait_for_holder(&dir, "go-holder", TIMEOUT).expect("go holder socket");
    assert_eq!(
        dir.holders().first().map(|(name, _)| name.as_str()),
        Some("go-holder")
    );

    let mut attached = pom_ptyhost::attach(&dir, "go-holder", 0).expect("attach");
    attached.connection.input(b"rust-says-hi\n").expect("input");
    attached
        .output
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("timeout");
    let mut seen = Vec::new();
    let mut buffer = [0u8; 4096];
    let deadline = Instant::now() + TIMEOUT;
    while !String::from_utf8_lossy(&seen).contains("rust-says-hi") && Instant::now() < deadline {
        if let Ok(read) = attached.output.read(&mut buffer) {
            seen.extend_from_slice(&buffer[..read]);
        }
    }
    assert!(String::from_utf8_lossy(&seen).contains("rust-says-hi"));
    attached
        .connection
        .resize(132, 43)
        .expect("resize frame accepted");

    dir.kill_holder("go-holder").expect("kill go holder");
    holder.wait().expect("reap go holder");
}

#[test]
fn a_rust_holder_accepts_a_go_client() {
    if !enabled() {
        return;
    }
    let temp = tempfile::Builder::new()
        .prefix("pty")
        .tempdir_in("/tmp")
        .expect("temp");
    let dir = SocketDir::new(temp.path());
    pom_ptyhost::spawn_holder(
        &dir,
        &SpawnRequest {
            binary: Path::new(BINARY),
            name: "rust-holder",
            cwd: Path::new("/"),
            cols: 100,
            rows: 30,
            argv: &[
                "sh".into(),
                "-c".into(),
                "printf before-go; exec cat".into(),
            ],
            env: &[],
        },
    )
    .expect("spawn");
    pom_ptyhost::wait_for_holder(&dir, "rust-holder", TIMEOUT).expect("socket");
    std::thread::sleep(Duration::from_millis(200));

    let output = temp.path().join("go-client.txt");
    let status = Command::new("go")
        .args([
            "test",
            "./internal/ptyhost",
            "-run",
            "^TestOracleClient$",
            "-count=1",
        ])
        .current_dir(go_root())
        .env("POM_ORACLE_SOCK_DIR", temp.path())
        .env("POM_ORACLE_PTY_NAME", "rust-holder")
        .env("POM_ORACLE_OUT", &output)
        .status()
        .expect("go test");
    assert!(status.success());
    let seen = std::fs::read_to_string(&output).expect("go client output");
    assert!(
        seen.contains("before-go"),
        "snapshot reached the Go client: {seen:?}"
    );
    assert!(
        seen.contains("go-says-hi"),
        "Go input reached the holder: {seen:?}"
    );
    dir.kill_holder("rust-holder").expect("kill");
}
