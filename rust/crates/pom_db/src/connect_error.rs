//! Why a database could not be opened, sorted into the few causes a person can act on, with the facts used to
//! connect and the driver's full text.

use crate::Engine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectErrorKind {
    DatabaseMissing,
    ServerUnreachable,
    AuthFailed,
    /// The port answers, but a container of another project publishes it.
    WrongServer,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectError {
    pub kind: ConnectErrorKind,
    pub engine: Engine,
    pub database: String,
    pub host: String,
    pub port: u16,
    /// Empty for Redis.
    pub user: String,
    pub raw: String,
    /// The container holding the port, for `WrongServer`.
    pub container: Option<String>,
}

const UNREACHABLE: [&str; 8] = [
    "connection refused",
    "timed out",
    "timeout",
    "no route to host",
    "network is unreachable",
    "host is down",
    "failed to lookup address",
    "nodename nor servname",
];

/// The cause the driver's text names. Auth comes first: a missing role is also "does not exist".
pub fn classify(engine: Engine, raw: &str) -> ConnectErrorKind {
    let text = raw.to_lowercase();
    let unreachable = UNREACHABLE.iter().any(|needle| text.contains(needle));
    match engine {
        Engine::Postgres => {
            if text.contains("password authentication failed")
                || (text.contains("role \"") && text.contains("does not exist"))
                || text.contains("no pg_hba.conf entry")
            {
                ConnectErrorKind::AuthFailed
            } else if text.contains("database \"") && text.contains("does not exist") {
                ConnectErrorKind::DatabaseMissing
            } else if unreachable {
                ConnectErrorKind::ServerUnreachable
            } else {
                ConnectErrorKind::Other
            }
        }
        Engine::Redis if unreachable => ConnectErrorKind::ServerUnreachable,
        Engine::Redis => ConnectErrorKind::Other,
    }
}

impl ConnectError {
    pub fn new(
        engine: Engine,
        database: &str,
        host: &str,
        port: u16,
        user: &str,
        raw: String,
    ) -> ConnectError {
        ConnectError {
            kind: classify(engine, &raw),
            engine,
            database: database.to_string(),
            host: host.to_string(),
            port,
            user: user.to_string(),
            raw,
            container: None,
        }
    }

    /// Something answered on the port, but it is a container outside this project's shared stack.
    pub fn on_foreign_server(mut self, container: String) -> ConnectError {
        self.kind = ConnectErrorKind::WrongServer;
        self.raw = format!(
            "{}\n(the server at {}:{} belongs to container {container})",
            self.raw, self.host, self.port
        );
        self.container = Some(container);
        self
    }

    /// Whether the server answered, so what publishes its port is worth asking.
    pub fn server_answered(&self) -> bool {
        !matches!(self.kind, ConnectErrorKind::ServerUnreachable)
    }

    /// `host:port`.
    pub fn server(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_texts_sort_into_their_causes() {
        let cases = [
            (
                "database \"myproject_api_feat\" does not exist",
                ConnectErrorKind::DatabaseMissing,
            ),
            (
                "error connecting to server: Connection refused (os error 61)",
                ConnectErrorKind::ServerUnreachable,
            ),
            (
                "error connecting to server: timed out",
                ConnectErrorKind::ServerUnreachable,
            ),
            (
                "error connecting to server: No route to host (os error 65)",
                ConnectErrorKind::ServerUnreachable,
            ),
            (
                "password authentication failed for user \"postgres\"",
                ConnectErrorKind::AuthFailed,
            ),
            ("role \"app\" does not exist", ConnectErrorKind::AuthFailed),
            ("permission denied for database x", ConnectErrorKind::Other),
        ];
        for (raw, kind) in cases {
            assert_eq!(classify(Engine::Postgres, raw), kind, "{raw}");
        }
    }

    #[test]
    fn redis_is_unreachable_or_other() {
        assert_eq!(
            classify(
                Engine::Redis,
                "connect to redis://localhost:6379/0: Connection refused (os error 61)"
            ),
            ConnectErrorKind::ServerUnreachable
        );
        assert_eq!(
            classify(Engine::Redis, "NOAUTH Authentication required."),
            ConnectErrorKind::Other
        );
    }

    #[test]
    fn a_foreign_container_on_the_port_is_the_wrong_server() {
        let missing = ConnectError::new(
            Engine::Postgres,
            "myproject_api",
            "localhost",
            5434,
            "postgres",
            "database \"myproject_api\" does not exist".into(),
        );
        assert!(missing.server_answered());
        let wrong = missing.on_foreign_server("other-shared-postgres-1".into());
        assert_eq!(wrong.kind, ConnectErrorKind::WrongServer);
        assert_eq!(wrong.container.as_deref(), Some("other-shared-postgres-1"));
        assert!(wrong.raw.contains("does not exist"));
        assert!(wrong.raw.contains("other-shared-postgres-1"));
        let refused = ConnectError::new(
            Engine::Postgres,
            "x",
            "localhost",
            5434,
            "postgres",
            "error connecting to server: Connection refused".into(),
        );
        assert!(!refused.server_answered());
    }
}
