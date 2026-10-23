//! A workspace's files through a real server and the app's source: the tree as a snapshot then patches, files
//! read inline and streamed, paths that leave the workspace refused, nothing of a file in the log.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use files::{FileEntry, ProjectFs, TreeEvent, TreeFeed};
use pom_remote::{Host, HostUrl, ProjectSource, ServerSource};
use terminals::{Remote, TIMEOUT};

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

fn files_of(remote: &Remote) -> (ServerSource, Arc<dyn ProjectFs>) {
    let source = source(remote);
    assert!(source.transport().expect("transport").files);
    let fs = source.files("feat").expect("files over the server");
    (source, fs)
}

/// Polls `feed` until `done` holds for an event, returning every event seen.
fn until(feed: &mut dyn TreeFeed, done: impl Fn(&TreeEvent) -> bool) -> Vec<TreeEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let events = feed.poll();
        let finished = events.iter().any(&done);
        seen.extend(events);
        if finished {
            return seen;
        }
        assert!(Instant::now() < deadline, "not seen; got {seen:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn complete(event: &TreeEvent) -> bool {
    matches!(event, TreeEvent::Snapshot { complete: true, .. })
}

fn snapshot(events: &[TreeEvent]) -> Vec<String> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            TreeEvent::Snapshot { entries, .. } => {
                Some(entries.iter().map(|entry| entry.path.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn patched(event: &TreeEvent, wanted: impl Fn(&files::TreePatch) -> bool) -> bool {
    matches!(event, TreeEvent::Patch(patch) if wanted(patch))
}

#[test]
fn the_tree_comes_as_a_snapshot_then_as_patches() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    std::fs::write(worktree.join("api/src/main.rs"), "fn main() {}\n")?;
    std::fs::write(worktree.join("api/.git/HEAD"), "ref: x\n")?;
    let (_source, fs) = files_of(&remote);
    let mut feed = fs.watch(Arc::new(|| {}));
    let events = until(&mut *feed, complete);
    assert_eq!(snapshot(&events), ["api", "api/src", "api/src/main.rs"]);

    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(worktree.join("api/src/new.rs"), "new\n")?;
    until(&mut *feed, |event| {
        patched(event, |patch| {
            patch
                .upserted
                .iter()
                .any(|entry| entry.path == "api/src/new.rs")
                || patch.upserted.iter().any(|entry| entry.path == "api/src")
        })
    });
    std::fs::write(worktree.join("api/src/main.rs"), "fn main() { run(); }\n")?;
    until(&mut *feed, |event| {
        patched(event, |patch| {
            patch
                .changed
                .iter()
                .any(|file| file.path == "api/src/main.rs" && file.size == 21)
        })
    });
    std::fs::remove_file(worktree.join("api/src/new.rs"))?;
    until(&mut *feed, |event| {
        patched(event, |patch| {
            patch.removed.iter().any(|path| path == "api/src/new.rs")
                && !patch
                    .upserted
                    .iter()
                    .any(|entry| entry.path == "api/src/new.rs")
        })
    });

    let again = fs.watch(Arc::new(|| {}));
    let mut again = again;
    let events = until(&mut *again, complete);
    assert_eq!(
        snapshot(&events),
        ["api", "api/src", "api/src/main.rs"],
        "a second client gets the folder's tree from the shared watch"
    );
    assert_eq!(remote.handle.info().file_watches, 1);
    Ok(())
}

#[test]
fn a_large_file_streams_whole_and_a_small_one_comes_inline() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    let mut big = String::new();
    let mut line = 0u64;
    while big.len() < 2 * 1024 * 1024 {
        big.push_str(&format!(
            "line {line:08} {}\n",
            "x".repeat(line as usize % 97)
        ));
        line += 1;
    }
    std::fs::write(worktree.join("api/big.log"), &big)?;
    std::fs::write(worktree.join("api/small.rs"), "fn small() {}\n")?;
    std::fs::write(worktree.join("api/blob.bin"), [0u8, 1, 2, 3])?;
    let (_source, fs) = files_of(&remote);

    let read = fs.read("api/big.log")?;
    assert_eq!(read.size, big.len() as u64);
    assert!(
        read.text.as_deref() == Some(big.as_str()),
        "the text arrived whole and in order"
    );
    let stat = fs.stat("api/big.log")?;
    assert_eq!((stat.size, stat.mtime_ns), (read.size, read.mtime_ns));

    let small = fs.read("api/small.rs")?;
    assert_eq!(small.text.as_deref(), Some("fn small() {}\n"));
    let blob = fs.read("api/blob.bin")?;
    assert!(blob.binary && blob.text.is_none());
    assert_eq!(fs.read_bytes("api/blob.bin", 1024)?, [0u8, 1, 2, 3]);
    let refused = fs.read_bytes("api/big.log", 1024 * 1024);
    assert!(
        matches!(&refused, Err(error) if error.kind() == io::ErrorKind::InvalidData),
        "{refused:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn paths_that_leave_the_workspace_are_refused() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    let outside = remote.temp.path().join("outside.txt");
    std::fs::write(&outside, "not yours")?;
    std::os::unix::fs::symlink(&outside, worktree.join("api/leak"))?;
    let fifo = worktree.join("api/pipe");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()?
        .success());
    let (_source, fs) = files_of(&remote);
    let kind = |result: io::Result<files::FileResult>| result.err().map(|error| error.kind());
    assert_eq!(
        kind(fs.read("api/leak")),
        Some(io::ErrorKind::PermissionDenied)
    );
    assert_eq!(
        kind(fs.read("../pom.yml")),
        Some(io::ErrorKind::InvalidInput)
    );
    assert_eq!(
        kind(fs.read("/etc/hosts")),
        Some(io::ErrorKind::InvalidInput)
    );
    assert_eq!(kind(fs.read("api/pipe")), Some(io::ErrorKind::InvalidInput));
    assert_eq!(
        kind(fs.read("api/missing.rs")),
        Some(io::ErrorKind::NotFound)
    );
    assert_eq!(kind(fs.read("api")), Some(io::ErrorKind::InvalidInput));
    Ok(())
}

#[test]
fn the_finder_lists_files_over_a_stream() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    std::fs::write(worktree.join("api/src/main.rs"), "fn main() {}\n")?;
    std::fs::write(worktree.join("api/.gitignore"), "dist/\n")?;
    std::fs::create_dir_all(worktree.join("api/dist"))?;
    std::fs::write(worktree.join("api/dist/app.js"), "x")?;
    let (_source, fs) = files_of(&remote);
    assert_eq!(
        fs.project_files(false)?,
        ["api/.gitignore", "api/src/main.rs"]
    );
    assert_eq!(
        fs.project_files(true)?,
        ["api/.gitignore", "api/dist/app.js", "api/src/main.rs"]
    );
    Ok(())
}

#[test]
fn the_log_keeps_methods_and_sizes_but_never_a_file() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    std::fs::write(
        worktree.join("api/canary-name-7f3a.env"),
        "TOKEN=canary-content-91c2\n",
    )?;
    let (_source, fs) = files_of(&remote);
    let mut feed = fs.watch(Arc::new(|| {}));
    until(&mut *feed, complete);
    assert!(fs.read("api/canary-name-7f3a.env")?.text.is_some());
    assert!(fs.read("api/canary-name-7f3a.missing").is_err());
    assert!(fs.stat("api/canary-name-7f3a.env").is_ok());
    assert!(!fs.project_files(true)?.is_empty());
    let log = std::fs::read_to_string(&remote.paths.log)?;
    assert!(log.contains("method=fs.read"), "{log}");
    assert!(log.contains("method=fs.watch"), "{log}");
    for secret in ["canary-name-7f3a", "canary-content-91c2"] {
        assert!(!log.contains(secret), "{secret} reached the log:\n{log}");
    }
    Ok(())
}

/// 50,000 files in 500 folders: the first chunk shows quickly, the whole tree soon after.
#[test]
fn a_large_tree_arrives_quickly() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let worktree = remote.worktree();
    for folder in 0..500 {
        let dir = worktree.join(format!("api/src/m{folder:03}"));
        std::fs::create_dir_all(&dir)?;
        for file in 0..100 {
            std::fs::write(dir.join(format!("f{file:03}.rs")), "")?;
        }
    }
    let (_source, fs) = files_of(&remote);
    let started = Instant::now();
    let mut feed = fs.watch(Arc::new(|| {}));
    let mut first = None;
    let mut entries: Vec<FileEntry> = Vec::new();
    let deadline = started + Duration::from_secs(30);
    loop {
        for event in feed.poll() {
            if let TreeEvent::Snapshot {
                entries: found,
                complete,
            } = event
            {
                if first.is_none() && !found.is_empty() {
                    first = Some(started.elapsed());
                }
                if complete {
                    entries = found;
                }
            }
        }
        if !entries.is_empty() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let whole = started.elapsed();
    let first = first.unwrap_or(whole);
    eprintln!(
        "50k tree: first chunk {} ms, complete {} ms, {} entries",
        first.as_millis(),
        whole.as_millis(),
        entries.len()
    );
    assert_eq!(entries.len(), 50_000 + 500 + 2);
    if cfg!(target_os = "linux") {
        assert!(first < Duration::from_millis(300), "first chunk {first:?}");
        assert!(whole < Duration::from_secs(2), "complete {whole:?}");
    }
    Ok(())
}
