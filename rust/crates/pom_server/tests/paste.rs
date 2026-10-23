//! Pasting a file through a real server: it streams whole into the private paste folder, and a file past the
//! limit or with an odd extension is refused before anything is kept.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use pom_remote::server_api::{method, PasteParams, PASTE_LIMIT};
use pom_remote::{Host, HostUrl, ProjectSource, ServerSource};
use pom_rpc::error_code;
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

#[test]
fn a_pasted_image_streams_whole_into_the_private_folder() -> std::io::Result<()> {
    let remote = Remote::start(&["/bin/sh"]);
    let source = source(&remote);
    assert!(source.transport().expect("transport").paste);
    let image: Vec<u8> = (0..3 * 1024 * 1024 + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    let path = source
        .paste_file(&image, "png")
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let folder = remote.temp.path().join("state/paste");
    assert_eq!(path.parent(), Some(folder.as_path()));
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("png"));
    assert_eq!(std::fs::read(&path)?, image);
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&folder)?.permissions().mode() & 0o777,
        0o700
    );

    let small = source
        .paste_file(b"\x89PNG", "png")
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    assert_ne!(small, path);
    assert_eq!(std::fs::read(&small)?, b"\x89PNG");
    Ok(())
}

#[test]
fn a_paste_past_the_limit_or_with_an_odd_extension_is_refused() {
    let remote = Remote::start(&["/bin/sh"]);
    let ask = |params: PasteParams| {
        let stream = remote.connection.open_stream().expect("stream");
        let params = PasteParams {
            channel: stream.channel(),
            ..params
        };
        remote
            .connection
            .start_request(method::PASTE_PUT, &params)
            .and_then(|mut pending| pending.wait_timeout(std::time::Duration::from_secs(10)))
    };
    let too_large = ask(PasteParams {
        extension: "png".into(),
        size: PASTE_LIMIT + 1,
        ..PasteParams::default()
    });
    let Err(pom_rpc::Error::Remote(error)) = too_large else {
        panic!("not refused: {too_large:?}");
    };
    assert_eq!(error.code, pom_remote::server_api::fs_error::TOO_LARGE);

    let odd = ask(PasteParams {
        extension: "../png".into(),
        size: 4,
        ..PasteParams::default()
    });
    let Err(pom_rpc::Error::Remote(error)) = odd else {
        panic!("not refused: {odd:?}");
    };
    assert_eq!(error.code, error_code::INVALID_PARAMS);
    assert!(!remote.temp.path().join("state/paste").exists());
}
