//! This machine's side of the socket: `pom server status` and `stop`.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use pom_remote::server_api::{method, ServerInfo};
use pom_rpc::{Config, Connection, Hello, Transport};

use crate::paths::ServerPaths;

const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// Connects to the running server of `paths`; `None` when none listens there.
pub fn connect(paths: &ServerPaths) -> Result<Option<(Connection, Hello)>, String> {
    let stream = match UnixStream::connect(&paths.socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(format!("{}: {error}", paths.socket.display())),
    };
    let transport = Transport::unix(stream).map_err(|error| error.to_string())?;
    let config = Config {
        handshake_timeout: ANSWER_WITHIN,
        ..Config::default()
    };
    Connection::builder(transport)
        .config(config)
        .connect(Hello::new(env!("CARGO_PKG_VERSION")))
        .map(Some)
        .map_err(|error| format!("pom server: {error}"))
}

pub fn info(connection: &Connection) -> Result<ServerInfo, String> {
    let value = connection
        .start_request(method::SERVER_INFO, serde_json::Value::Null)
        .and_then(|mut pending| pending.wait_timeout(ANSWER_WITHIN))
        .map_err(|error| format!("pom server: {error}"))?;
    serde_json::from_value(value).map_err(|error| format!("pom server answered: {error}"))
}

/// Asks the server to exit; the connection closing before the answer counts as done.
pub fn stop(connection: &Connection) -> Result<(), String> {
    let answer = connection
        .start_request(method::SERVER_STOP, serde_json::Value::Null)
        .and_then(|mut pending| pending.wait_timeout(ANSWER_WITHIN));
    match answer {
        Ok(_) | Err(pom_rpc::Error::Closed(_)) => Ok(()),
        Err(error) => Err(format!("pom server: {error}")),
    }
}
