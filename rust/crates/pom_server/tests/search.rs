//! Project search through a real server finds what a local search of the same folder finds, a file too large
//! for one push included, and a cancelled one leaves the server ready for the next.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use files::search::{FileResult, SearchOptions, SearchRequest};
use files::{LocalFs, ProjectFs};
use pom_remote::{Host, HostUrl, ProjectSource, ServerSource};
use terminals::Remote;

fn remote_files(remote: &Remote) -> (ServerSource, Arc<dyn ProjectFs>) {
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
    assert!(source.transport().expect("transport").files_search);
    let fs = source.files("feat").expect("files over the server");
    (source, fs)
}

fn found(fs: &dyn ProjectFs, request: &SearchRequest) -> io::Result<(Vec<FileResult>, bool)> {
    let files = Mutex::new(Vec::new());
    let limited = fs.search(request, &AtomicBool::new(false), &|file| {
        if let Ok(mut files) = files.lock() {
            files.push(file);
        }
    })?;
    let mut files = files.into_inner().unwrap_or_default();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((files, limited))
}

fn request(query: &str, options: SearchOptions, include: &str, exclude: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        options,
        include: include.into(),
        exclude: exclude.into(),
        include_ignored: false,
    }
}

#[test]
fn a_remote_search_finds_what_a_local_one_finds() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = remote.worktree();
    std::fs::create_dir_all(root.join("api/src"))?;
    std::fs::create_dir_all(root.join("api/dist"))?;
    std::fs::create_dir_all(root.join("web/src"))?;
    std::fs::write(root.join("api/.gitignore"), "dist/\n")?;
    std::fs::write(
        root.join("api/src/main.rs"),
        "fn login() {}\nfn Login_page() { login(); }\n",
    )?;
    std::fs::write(root.join("api/dist/bundle.js"), "login()\n")?;
    std::fs::write(
        root.join("web/src/app.ts"),
        "const login = 'caf\u{e9}';\nexport { login };\n",
    )?;
    let long: String = (0..3000)
        .map(|line| format!("{line}: login {}\n", "x".repeat(200)))
        .collect();
    std::fs::write(root.join("web/src/large.txt"), long)?;
    let (_source, remote_fs) = remote_files(&remote);
    let local_fs = LocalFs::new(root.clone());

    let regex = SearchOptions {
        regex: true,
        ..SearchOptions::default()
    };
    let word = SearchOptions {
        whole_word: true,
        case_sensitive: true,
        ..SearchOptions::default()
    };
    let mut with_ignored = request("login", SearchOptions::default(), "", "");
    with_ignored.include_ignored = true;
    let requests = [
        request("login", SearchOptions::default(), "", ""),
        request("login", word, "", ""),
        request(r"fn (\w+)\(", regex, "", ""),
        request("CAF\u{c9}", SearchOptions::default(), "", ""),
        request("login", SearchOptions::default(), "api", ""),
        request("login", SearchOptions::default(), "", "*.txt"),
        with_ignored,
    ];
    for asked in &requests {
        let local = found(&local_fs, asked)?;
        let over_server = found(&*remote_fs, asked)?;
        assert_eq!(over_server, local, "{asked:?}");
    }
    let large = found(&*remote_fs, &requests[0])?
        .0
        .into_iter()
        .find(|file| file.path == "web/src/large.txt")
        .map(|file| (file.match_count, file.excerpts.len()));
    assert_eq!(large, Some((3000, 1)), "a split file comes back whole");

    let bad = request("(", regex, "", "");
    assert!(remote_fs
        .search(&bad, &AtomicBool::new(false), &|_| {})
        .is_err());
    Ok(())
}

#[test]
fn a_remote_search_stops_when_cancelled() -> io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = remote.worktree().join("api/src");
    std::fs::create_dir_all(&root)?;
    for index in 0..4000 {
        std::fs::write(root.join(format!("f{index}.rs")), "let needle = 1;\n")?;
    }
    let (_source, fs) = remote_files(&remote);
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let stop = cancel.clone();
    fs.search(
        &request("needle", SearchOptions::default(), "", ""),
        &cancel,
        &move |_| stop.store(true, Ordering::Relaxed),
    )?;
    assert!(started.elapsed() < Duration::from_secs(10));
    let (again, _) = found(&*fs, &request("needle", SearchOptions::default(), "", ""))?;
    assert_eq!(again.len(), 4000, "the next search runs whole");
    Ok(())
}
