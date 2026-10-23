//! Saving a workspace's files and changing its tree through a real server: a save checked against the file
//! the client read, a large save streamed, each change run once per op id, nothing of a file in the log.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use files::{FileStat, ProjectFs, WriteError};
use pom_remote::server_api::{method, FsWritten};
use pom_remote::{Host, HostUrl, ProjectSource, ServerSource};
use serde_json::{json, Value};
use terminals::Remote;

fn source(remote: &Remote) -> ServerSource {
    let socket = remote.paths.socket.clone();
    let opener: pom_remote::Opener = Arc::new(move |_reconnect| {
        let stream = std::os::unix::net::UnixStream::connect(&socket)?;
        pom_rpc::Transport::unix(stream)
    });
    let host = Host {
        name: "dev".into(),
        url: HostUrl::parse("ssh://dev@dev.example.net").expect("url"),
        pom_path: None,
        transport: None,
    };
    let source = ServerSource::with_opener(host, remote.cfg(), opener);
    source.workspaces().expect("connect");
    source
}

fn files_of(remote: &Remote, branch: &str) -> (ServerSource, Arc<dyn ProjectFs>) {
    let source = source(remote);
    assert!(source.transport().expect("transport").files_write);
    let fs = source.files(branch).expect("files over the server");
    (source, fs)
}

/// Another mtime than the file had: some file systems keep whole seconds.
fn touch_later(path: &std::path::Path, text: &str) -> io::Result<()> {
    let before = std::fs::metadata(path)?.modified()?;
    loop {
        std::fs::write(path, text)?;
        if std::fs::metadata(path)?.modified()? != before {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_save_over_the_file_it_read_writes_and_over_a_changed_one_is_a_conflict() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let file = remote.worktree().join("api/src/main.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    let (_source, fs) = files_of(&remote, "feat");
    let read = fs.read("api/src/main.rs")?;
    let opened = FileStat {
        dir: false,
        size: read.size,
        mtime_ns: read.mtime_ns,
    };
    let saved = fs
        .write("api/src/main.rs", "fn main() { run(); }\n", Some(&opened))
        .map_err(io::Error::other)?;
    assert_eq!(std::fs::read_to_string(&file)?, "fn main() { run(); }\n");
    assert_eq!(saved, fs.stat("api/src/main.rs")?);

    touch_later(&file, "// edited on the host\n")?;
    let conflict = fs.write("api/src/main.rs", "fn main() { mine(); }\n", Some(&saved));
    let Err(WriteError::Conflict(now)) = conflict else {
        panic!("not a conflict: {conflict:?}");
    };
    assert_eq!(std::fs::read_to_string(&file)?, "// edited on the host\n");
    assert_eq!(now, fs.stat("api/src/main.rs")?);

    let again = fs
        .write("api/src/main.rs", "// edited on the host\n", Some(&saved))
        .map_err(io::Error::other)?;
    assert_eq!(
        again, now,
        "the host already has this text: nothing to keep apart"
    );
    let kept = fs
        .write("api/src/main.rs", "fn main() { mine(); }\n", Some(&now))
        .map_err(io::Error::other)?;
    assert_eq!(std::fs::read_to_string(&file)?, "fn main() { mine(); }\n");
    assert_eq!(kept.size, 22);

    let created = fs
        .write("api/src/new/deep.rs", "new\n", None)
        .map_err(io::Error::other)?;
    assert_eq!(created.size, 4);
    assert_eq!(
        std::fs::read_to_string(remote.worktree().join("api/src/new/deep.rs"))?,
        "new\n"
    );
    Ok(())
}

#[test]
fn a_large_save_streams_whole() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let file = remote.worktree().join("api/big.log");
    std::fs::write(&file, "old\n")?;
    let (_source, fs) = files_of(&remote, "feat");
    let opened = fs.stat("api/big.log")?;
    let mut big = String::new();
    let mut line = 0u64;
    while big.len() < 3 * 1024 * 1024 + 17 {
        big.push_str(&format!(
            "line {line:08} {}\n",
            "y".repeat(line as usize % 61)
        ));
        line += 1;
    }
    let saved = fs
        .write("api/big.log", &big, Some(&opened))
        .map_err(io::Error::other)?;
    assert_eq!(saved.size, big.len() as u64);
    assert!(
        std::fs::read_to_string(&file)? == big,
        "the text arrived whole and in order"
    );
    Ok(())
}

#[test]
fn guarded_saves_and_changes_are_refused() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    let outside = remote.temp.path().join("outside");
    std::fs::create_dir_all(&outside)?;
    std::os::unix::fs::symlink(&outside, worktree.join("api/away"))?;
    let (_source, fs) = files_of(&remote, "feat");
    let kind = |result: io::Result<()>| result.err().map(|error| error.kind());
    let write_kind = |path: &str| match fs.write(path, "x", None) {
        Err(WriteError::Failed(error)) => Some(error.kind()),
        _ => None,
    };
    assert_eq!(
        write_kind("api/.git/config"),
        Some(io::ErrorKind::PermissionDenied)
    );
    assert_eq!(
        write_kind("api/away/new.rs"),
        Some(io::ErrorKind::PermissionDenied)
    );
    assert_eq!(write_kind("../pom.yml"), Some(io::ErrorKind::InvalidInput));
    assert!(
        std::fs::read_dir(&outside)?.next().is_none(),
        "nothing written out there"
    );
    assert_eq!(
        kind(fs.remove("api/.git")),
        Some(io::ErrorKind::PermissionDenied)
    );
    assert_eq!(
        kind(fs.rename("api/src", "api/away/src")),
        Some(io::ErrorKind::PermissionDenied)
    );
    assert!(worktree.join("api/src").is_dir());

    let (_main_source, main) = files_of(&remote, "main");
    let refused = main.write("api/x.rs", "x", None);
    assert!(
        matches!(&refused, Err(WriteError::Failed(error)) if error.kind() == io::ErrorKind::PermissionDenied),
        "{refused:?}"
    );
    assert_eq!(
        kind(main.create_file("api/x.rs")),
        Some(io::ErrorKind::PermissionDenied)
    );
    Ok(())
}

#[test]
fn the_tree_changes_through_the_server() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    std::fs::write(worktree.join("api/src/main.rs"), "fn main() {}\n")?;
    let (_source, fs) = files_of(&remote, "feat");
    fs.create_dir("api/docs/guides")?;
    assert!(worktree.join("api/docs/guides").is_dir());
    fs.create_file("api/docs/guides/start.md")?;
    assert!(fs.create_file("api/docs/guides/start.md").is_err(), "taken");
    fs.copy("api/docs", "api/docs copy")?;
    assert!(worktree.join("api/docs copy/guides/start.md").is_file());
    fs.rename("api/docs copy", "api/manual")?;
    assert!(worktree.join("api/manual/guides/start.md").is_file());
    assert!(
        fs.rename("api/manual", "api/manual/inner").is_err(),
        "not into itself"
    );
    fs.remove("api/manual")?;
    assert!(!worktree.join("api/manual").exists());
    fs.add_to_gitignore("api/docs", true)?;
    assert_eq!(
        std::fs::read_to_string(worktree.join("api/.gitignore"))?,
        "docs/\n"
    );
    Ok(())
}

fn call(
    connection: &pom_rpc::Connection,
    name: &str,
    params: Value,
) -> Result<Value, pom_rpc::Error> {
    connection.request(name, params)
}

#[test]
fn each_change_runs_once_per_op_id() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    std::fs::write(worktree.join("api/src/a.rs"), "a\n")?;
    let connection = remote.connect();
    let cfg = remote.cfg();
    let base =
        |path: &str, op: &str| json!({"cfg": cfg, "branch": "feat", "path": path, "op_id": op});

    let first = call(
        &connection,
        method::FS_CREATE_FILE,
        base("api/new.rs", "op-create"),
    )
    .map_err(io::Error::other)?;
    std::fs::remove_file(worktree.join("api/new.rs"))?;
    let repeat = call(
        &connection,
        method::FS_CREATE_FILE,
        base("api/new.rs", "op-create"),
    )
    .map_err(io::Error::other)?;
    assert_eq!(first, repeat);
    assert!(
        !worktree.join("api/new.rs").exists(),
        "not made a second time"
    );

    call(
        &connection,
        method::FS_CREATE_DIR,
        base("api/dir", "op-dir"),
    )
    .map_err(io::Error::other)?;
    std::fs::remove_dir(worktree.join("api/dir"))?;
    call(
        &connection,
        method::FS_CREATE_DIR,
        base("api/dir", "op-dir"),
    )
    .map_err(io::Error::other)?;
    assert!(!worktree.join("api/dir").exists());

    let mut copy = base("api/b.rs", "op-copy");
    copy["from"] = json!("api/src/a.rs");
    call(&connection, method::FS_COPY, copy.clone()).map_err(io::Error::other)?;
    std::fs::remove_file(worktree.join("api/b.rs"))?;
    call(&connection, method::FS_COPY, copy).map_err(io::Error::other)?;
    assert!(!worktree.join("api/b.rs").exists());

    let mut rename = base("api/c.rs", "op-rename");
    rename["from"] = json!("api/src/a.rs");
    call(&connection, method::FS_RENAME, rename.clone()).map_err(io::Error::other)?;
    std::fs::rename(worktree.join("api/c.rs"), worktree.join("api/src/a.rs"))?;
    call(&connection, method::FS_RENAME, rename).map_err(io::Error::other)?;
    assert!(
        worktree.join("api/src/a.rs").exists(),
        "not moved a second time"
    );

    std::fs::write(worktree.join("api/gone.rs"), "x")?;
    call(
        &connection,
        method::FS_REMOVE,
        base("api/gone.rs", "op-remove"),
    )
    .map_err(io::Error::other)?;
    std::fs::write(worktree.join("api/gone.rs"), "back")?;
    call(
        &connection,
        method::FS_REMOVE,
        base("api/gone.rs", "op-remove"),
    )
    .map_err(io::Error::other)?;
    assert!(
        worktree.join("api/gone.rs").exists(),
        "not removed a second time"
    );

    let mut ignore = base("api/src", "op-ignore");
    ignore["is_dir"] = json!(true);
    call(&connection, method::FS_APPEND_GITIGNORE, ignore.clone()).map_err(io::Error::other)?;
    call(&connection, method::FS_APPEND_GITIGNORE, ignore).map_err(io::Error::other)?;
    assert_eq!(
        std::fs::read_to_string(worktree.join("api/.gitignore"))?,
        "src/\n"
    );

    let mut save = base("api/src/a.rs", "op-save");
    save["text"] = json!("saved\n");
    let saved: FsWritten = serde_json::from_value(
        call(&connection, method::FS_WRITE, save.clone()).map_err(io::Error::other)?,
    )
    .map_err(io::Error::other)?;
    assert!(saved.written);
    std::fs::write(worktree.join("api/src/a.rs"), "edited on the host\n")?;
    let repeat: FsWritten = serde_json::from_value(
        call(&connection, method::FS_WRITE, save).map_err(io::Error::other)?,
    )
    .map_err(io::Error::other)?;
    assert_eq!(repeat, saved, "the first save's answer");
    assert_eq!(
        std::fs::read_to_string(worktree.join("api/src/a.rs"))?,
        "edited on the host\n"
    );

    let refused = call(
        &connection,
        method::FS_TRASH,
        base("api/src/a.rs", "bad op id!"),
    );
    assert!(
        matches!(refused, Err(pom_rpc::Error::Remote(error)) if error.code == "invalid_params")
    );
    Ok(())
}

#[test]
fn the_log_never_holds_a_saved_file() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let (_source, fs) = files_of(&remote, "feat");
    fs.write(
        "api/canary-name-5d1e.env",
        "TOKEN=canary-content-a83b\n",
        None,
    )
    .map_err(io::Error::other)?;
    fs.create_file("api/canary-name-5d1e.other")?;
    let log = std::fs::read_to_string(&remote.paths.log)?;
    assert!(log.contains("method=fs.write"), "{log}");
    assert!(log.contains("method=fs.create_file"), "{log}");
    for secret in ["canary-name-5d1e", "canary-content-a83b"] {
        assert!(!log.contains(secret), "{secret} reached the log:\n{log}");
    }
    Ok(())
}
