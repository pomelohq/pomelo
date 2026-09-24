mod cache;
mod pull_request;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_paths::StateDir;
use serde_json::Value;

pub use cache::PrCache;
pub use pull_request::{
    fetch_detail, fetch_heads, severity, Actor, Check, Label, PrTarget, PullRequest, ReviewRequest,
    Reviewer, Severity,
};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const TIMEOUT_SECONDS: &str = "30";
const TOKEN_SECRET: &str = "github";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    Http(u16),
    RateLimited { until: u64 },
    Transport(String),
    Parse(String),
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForgeError::Http(401) => write!(formatter, "github: token rejected (401)"),
            ForgeError::Http(403) => write!(formatter, "github: forbidden (403)"),
            ForgeError::Http(code) => write!(formatter, "github: HTTP {code}"),
            ForgeError::RateLimited { until } => {
                write!(formatter, "github: rate-limited until {until}")
            }
            ForgeError::Transport(message) => write!(formatter, "github: {message}"),
            ForgeError::Parse(message) => {
                write!(formatter, "github: unexpected answer ({message})")
            }
        }
    }
}

impl std::error::Error for ForgeError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<u64>,
    pub remaining: Option<u64>,
    /// Unix seconds when the rate-limit window resets.
    pub reset: Option<u64>,
}

pub trait Transport: Send + Sync {
    fn post(&self, url: &str, token: &str, body: &str) -> Result<Response, ForgeError>;
}

pub struct CurlTransport {
    pub program: PathBuf,
}

impl Default for CurlTransport {
    fn default() -> CurlTransport {
        CurlTransport {
            program: PathBuf::from("/usr/bin/curl"),
        }
    }
}

fn config_string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

impl Transport for CurlTransport {
    fn post(&self, url: &str, token: &str, body: &str) -> Result<Response, ForgeError> {
        let mut child = Command::new(&self.program)
            .args([
                "-sS",
                "--max-time",
                TIMEOUT_SECONDS,
                "-K",
                "-",
                "-w",
                "\n%{http_code} %header{retry-after} %header{x-ratelimit-remaining} %header{x-ratelimit-reset}",
            ])
            .arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ForgeError::Transport(format!("curl: {error}")))?;
        // The token and body go through curl's config on stdin so neither shows in a process listing.
        let config = format!(
            "header = {}\nheader = \"Accept: application/vnd.github+json\"\nheader = \"User-Agent: pomelo\"\nheader = \"Content-Type: application/json\"\ndata-binary = {}\n",
            config_string(&format!("Authorization: Bearer {token}")),
            config_string(body),
        );
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| ForgeError::Transport(format!("curl: {error}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| ForgeError::Transport(format!("curl: {error}")))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let (body, trailer) = text.rsplit_once('\n').unwrap_or(("", &text));
        let mut fields = trailer.split(' ');
        let status = fields
            .next()
            .and_then(|code| code.trim().parse::<u16>().ok())
            .filter(|code| *code > 0);
        let number = |field: Option<&str>| field.and_then(|value| value.trim().parse::<u64>().ok());
        match status {
            Some(status) => Ok(Response {
                status,
                body: body.to_string(),
                retry_after: number(fields.next()),
                remaining: number(fields.next()),
                reset: number(fields.next()),
            }),
            None => Err(ForgeError::Transport(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            )),
        }
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub struct Client {
    token: String,
    transport: Box<dyn Transport>,
    blocked_until: Mutex<u64>,
}

impl Client {
    pub fn new(token: &str, transport: Box<dyn Transport>) -> Client {
        Client {
            token: token.to_string(),
            transport,
            blocked_until: Mutex::new(0),
        }
    }

    /// After a 403/429 every call fails fast until GitHub's window resets.
    pub fn graphql(&self, query: &str) -> Result<Value, ForgeError> {
        let blocked = self.blocked_until.lock().map_or(0, |until| *until);
        if blocked > now_seconds() {
            return Err(ForgeError::RateLimited { until: blocked });
        }
        let body = serde_json::json!({ "query": query }).to_string();
        let response = self.transport.post(GRAPHQL_URL, &self.token, &body)?;
        if matches!(response.status, 403 | 429) {
            let until = response
                .retry_after
                .map(|seconds| now_seconds() + seconds)
                .or(response.reset.filter(|_| response.remaining == Some(0)))
                .unwrap_or_else(|| now_seconds() + 60);
            if let Ok(mut blocked) = self.blocked_until.lock() {
                *blocked = until;
            }
            return Err(ForgeError::RateLimited { until });
        }
        if response.status >= 400 {
            return Err(ForgeError::Http(response.status));
        }
        let value: Value = serde_json::from_str(&response.body)
            .map_err(|error| ForgeError::Parse(error.to_string()))?;
        match value.get("data") {
            Some(data) if !data.is_null() => Ok(data.clone()),
            _ => Err(ForgeError::Parse(
                value
                    .pointer("/errors/0/message")
                    .and_then(Value::as_str)
                    .unwrap_or("no data")
                    .to_string(),
            )),
        }
    }
}

/// `GH_TOKEN`, then `GITHUB_TOKEN`, then the session's `github` secret.
pub fn resolve_token(
    state: &StateDir,
    session: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let from_env = ["GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .find_map(|name| env(name).filter(|token| !token.trim().is_empty()));
    if from_env.is_some() {
        return from_env;
    }
    pom_secrets::SecretStore::new(state.clone(), session)
        .get(TOKEN_SECRET)
        .unwrap_or_else(|error| {
            eprintln!("github: read the token secret: {error}");
            None
        })
        .filter(|token| !token.trim().is_empty())
}

pub fn resolve(state: &StateDir, session: &str) -> Option<Client> {
    let token = resolve_token(state, session, &|name| std::env::var(name).ok())?;
    Some(Client::new(
        token.trim(),
        Box::new(CurlTransport::default()),
    ))
}

pub(crate) const COOLDOWN: Duration = Duration::from_secs(30);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Answers every POST with the next scripted response and keeps the bodies it was sent.
    #[derive(Clone, Default)]
    pub(crate) struct Scripted {
        pub answers: Arc<Mutex<Vec<Response>>>,
        pub sent: Arc<Mutex<Vec<String>>>,
    }

    impl Transport for Scripted {
        fn post(&self, _url: &str, _token: &str, body: &str) -> Result<Response, ForgeError> {
            if let Ok(mut sent) = self.sent.lock() {
                sent.push(body.to_string());
            }
            let mut answers = self
                .answers
                .lock()
                .map_err(|_| ForgeError::Transport("lock".into()))?;
            if answers.is_empty() {
                return Err(ForgeError::Transport("no scripted answer".into()));
            }
            Ok(answers.remove(0))
        }
    }

    pub(crate) fn ok(body: &str) -> Response {
        Response {
            status: 200,
            body: body.to_string(),
            ..Response::default()
        }
    }

    #[test]
    fn the_environment_wins_over_the_secret() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        pom_secrets::SecretStore::new(state.clone(), "myproject")
            .set(TOKEN_SECRET, "from-secret")
            .map_err(|error| error.to_string())?;
        let none = |_: &str| None;
        assert_eq!(
            resolve_token(&state, "myproject", &none).as_deref(),
            Some("from-secret")
        );
        let github = |name: &str| (name == "GITHUB_TOKEN").then(|| "from-env".to_string());
        assert_eq!(
            resolve_token(&state, "myproject", &github).as_deref(),
            Some("from-env")
        );
        let both = |name: &str| Some(format!("{name}-value"));
        assert_eq!(
            resolve_token(&state, "myproject", &both).as_deref(),
            Some("GH_TOKEN-value")
        );
        Ok(())
    }

    #[test]
    fn a_rate_limit_blocks_until_the_window_resets() {
        let transport = Scripted::default();
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(Response {
                status: 403,
                retry_after: Some(120),
                ..Response::default()
            });
            answers.push(ok(r#"{"data":{}}"#));
        }
        let client = Client::new("t", Box::new(transport.clone()));
        assert!(matches!(
            client.graphql("query {}"),
            Err(ForgeError::RateLimited { .. })
        ));
        assert!(matches!(
            client.graphql("query {}"),
            Err(ForgeError::RateLimited { .. })
        ));
        assert_eq!(transport.sent.lock().map(|sent| sent.len()).unwrap_or(0), 1);
    }

    #[test]
    fn graphql_errors_surface_their_message() {
        let transport = Scripted::default();
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(ok(
                r#"{"data":null,"errors":[{"message":"Bad credentials"}]}"#,
            ));
        }
        let client = Client::new("t", Box::new(transport));
        assert_eq!(
            client.graphql("query {}"),
            Err(ForgeError::Parse("Bad credentials".into()))
        );
    }

    #[test]
    fn curl_config_strings_escape_quotes_and_newlines() {
        assert_eq!(config_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }
}
