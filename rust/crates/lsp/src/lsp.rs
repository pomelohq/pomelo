//! A language server client. Each server is a child process spoken to in JSON-RPC over its stdin/stdout:
//! a writer thread sends framed messages, a reader thread parses the replies and wakes the UI, and the UI
//! thread drains them each frame with `poll`, answering the server's own requests there.

mod adapters;
mod completion;
mod position;
mod shell_env;
mod store;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

pub use adapters::{adapter_for, Adapter};
pub use completion::{CompletionsResponse, LspCompletion, ResolvedCompletion};
pub use lsp_types;
pub use position::{char_to_position, diagnostic_char_range, position_to_char};
pub use shell_env::capture_login_env;
pub use store::{
    DefinitionKind, DefinitionTarget, DefinitionsResponse, DiagnosticsUpdate, HoverResponse,
    LspStore, StoreEvent, SyncedText,
};

const CONTENT_LENGTH: &str = "Content-Length: ";
/// How long a server gets to answer `shutdown` before it is killed.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const METHOD_NOT_FOUND: i64 = -32601;

/// Called from the reader thread whenever something arrived, so the UI draws a frame and polls.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
enum Incoming {
    Response {
        id: i64,
        result: Result<Value, ResponseError>,
    },
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// What a poll hands to the owner: replies to its requests and the server's notifications.
#[derive(Debug)]
pub enum ServerEvent {
    Response {
        id: i64,
        method: String,
        result: Result<Value, ResponseError>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// The server's output closed: it exited or crashed.
    Exited,
}

pub struct LanguageServer {
    name: String,
    child: Option<Child>,
    outgoing: Option<Sender<String>>,
    incoming: Receiver<Incoming>,
    next_id: i64,
    pending: HashMap<i64, String>,
    root_uri: Option<lsp_types::Url>,
    exited: bool,
}

impl LanguageServer {
    /// Start `binary` in `root` with `env` as its whole environment.
    pub fn spawn(
        name: &str,
        binary: &Path,
        args: &[String],
        root: &Path,
        env: &HashMap<String, String>,
        waker: Waker,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(binary)
            .args(args)
            .current_dir(root)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(std::io::Error::other("server pipes missing"));
        };
        let (outgoing, to_write) = channel::<String>();
        std::thread::spawn(move || write_messages(stdin, to_write));
        let (sender, incoming) = channel();
        std::thread::spawn(move || read_messages(stdout, sender, waker));
        let server_name = name.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("{server_name}: {line}");
            }
        });
        Ok(Self {
            name: name.to_string(),
            child: Some(child),
            outgoing: Some(outgoing),
            incoming,
            next_id: 0,
            pending: HashMap::new(),
            root_uri: lsp_types::Url::from_directory_path(root).ok(),
            exited: false,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    fn send(&mut self, message: Value) {
        let Some(outgoing) = self.outgoing.as_ref() else {
            return;
        };
        if outgoing.send(message.to_string()).is_err() {
            self.outgoing = None;
        }
    }

    /// Send a request; its reply comes back from `poll` under the returned id.
    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, method.to_string());
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn respond(&mut self, id: Value, result: Result<Value, ResponseError>) {
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": error.code, "message": error.message},
            }),
        };
        self.send(message);
    }

    /// Everything that arrived since the last poll. The server's requests are answered here: settings as
    /// null, registrations and progress tokens accepted, anything else refused as unknown.
    pub fn poll(&mut self) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(Incoming::Response { id, result }) => {
                    let method = self.pending.remove(&id).unwrap_or_default();
                    events.push(ServerEvent::Response { id, method, result });
                }
                Ok(Incoming::Notification { method, params }) => {
                    events.push(ServerEvent::Notification { method, params });
                }
                Ok(Incoming::Request { id, method, params }) => {
                    let reply = self.answer(&method, &params);
                    self.respond(id, reply);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.exited {
                        self.exited = true;
                        events.push(ServerEvent::Exited);
                    }
                    break;
                }
            }
        }
        events
    }

    fn answer(&self, method: &str, params: &Value) -> Result<Value, ResponseError> {
        match method {
            "workspace/configuration" => {
                let count = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Ok(Value::Array(vec![Value::Null; count]))
            }
            "workspace/workspaceFolders" => Ok(self.root_uri.as_ref().map_or(
                Value::Null,
                |uri| json!([{"uri": uri, "name": folder_name(uri)}]),
            )),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "workspace/diagnostic/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/semanticTokens/refresh"
            | "workspace/codeLens/refresh" => Ok(Value::Null),
            _ => Err(ResponseError {
                code: METHOD_NOT_FOUND,
                message: format!("unhandled method {method}"),
            }),
        }
    }

    /// Ask the server to shut down and exit, killing it if it hasn't within the timeout. Runs off-thread.
    pub fn shutdown(mut self) {
        let id = self.request("shutdown", Value::Null);
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + SHUTDOWN_TIMEOUT;
            while std::time::Instant::now() < deadline {
                let answered = self.poll().iter().any(
                    |event| matches!(event, ServerEvent::Response { id: got, .. } if *got == id),
                ) || self.exited;
                if answered {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            self.notify("exit", Value::Null);
            self.outgoing = None;
            std::thread::sleep(Duration::from_millis(100));
            self.kill();
        });
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(error) = child.kill() {
                eprintln!("{}: kill failed: {error}", self.name);
            }
            if let Err(error) = child.wait() {
                eprintln!("{}: wait failed: {error}", self.name);
            }
        }
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.kill();
    }
}

fn folder_name(uri: &lsp_types::Url) -> String {
    uri.path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_string()
}

fn write_messages(stdin: ChildStdin, messages: Receiver<String>) {
    let mut stdin = std::io::BufWriter::new(stdin);
    for message in messages {
        let framed = write!(stdin, "{CONTENT_LENGTH}{}\r\n\r\n{message}", message.len())
            .and_then(|()| stdin.flush());
        if framed.is_err() {
            return;
        }
    }
}

/// One framed message's body, or `None` once the stream ends or breaks.
fn read_message(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix(CONTENT_LENGTH) {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

fn parse_incoming(body: &[u8]) -> Option<Incoming> {
    let mut message: Value = serde_json::from_slice(body).ok()?;
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let params = message
        .get_mut("params")
        .map(Value::take)
        .unwrap_or(Value::Null);
    match (method, message.get("id").cloned()) {
        (Some(method), Some(id)) => Some(Incoming::Request { id, method, params }),
        (Some(method), None) => Some(Incoming::Notification { method, params }),
        (None, Some(id)) => {
            let id = id.as_i64()?;
            let result = match message.get("error") {
                Some(error) => Err(ResponseError {
                    code: error
                        .get("code")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }),
                None => Ok(message
                    .get_mut("result")
                    .map(Value::take)
                    .unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, result })
        }
        (None, None) => None,
    }
}

fn read_messages(stdout: impl Read, sender: Sender<Incoming>, waker: Waker) {
    let mut reader = BufReader::new(stdout);
    while let Some(body) = read_message(&mut reader) {
        let Some(message) = parse_incoming(&body) else {
            eprintln!("lsp: unreadable message {}", String::from_utf8_lossy(&body));
            continue;
        };
        if sender.send(message).is_err() {
            return;
        }
        waker();
    }
    // Dropping the sender tells the owner the server is gone; wake it to notice.
    drop(sender);
    waker();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages_and_classifies_them() {
        let first = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        let second = r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a"}}"#;
        let third = r#"{"jsonrpc":"2.0","id":"x","method":"workspace/configuration","params":{"items":[{},{}]}}"#;
        let stream = format!(
            "Content-Length: {}\r\n\r\n{first}Content-Type: utf-8\r\nContent-Length: {}\r\n\r\n{second}Content-Length: {}\r\n\r\n{third}",
            first.len(),
            second.len(),
            third.len()
        );
        let mut reader = BufReader::new(stream.as_bytes());
        let messages: Vec<Incoming> = std::iter::from_fn(|| read_message(&mut reader))
            .filter_map(|body| parse_incoming(&body))
            .collect();
        assert_eq!(messages.len(), 3);
        assert!(
            matches!(&messages[0], Incoming::Response { id: 3, result: Ok(v) } if v["ok"] == true)
        );
        assert!(
            matches!(&messages[1], Incoming::Notification { method, .. } if method == "textDocument/publishDiagnostics")
        );
        assert!(
            matches!(&messages[2], Incoming::Request { method, .. } if method == "workspace/configuration")
        );
    }

    #[test]
    fn error_responses_carry_code_and_message() {
        let body = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
        let Some(Incoming::Response {
            result: Err(error), ..
        }) = parse_incoming(body)
        else {
            panic!("expected an error response");
        };
        assert_eq!(error.code, METHOD_NOT_FOUND);
        assert_eq!(error.message, "nope");
    }

    fn poll_until(
        server: &mut LanguageServer,
        done: impl Fn(&ServerEvent) -> bool,
    ) -> Vec<ServerEvent> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            let events = server.poll();
            let finished = events.iter().any(&done);
            seen.extend(events);
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    }

    /// A shell script stands in for a server: it answers the first request, then exits.
    #[test]
    fn round_trips_a_request_and_notices_the_exit() {
        let reply = r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#;
        let script = format!(
            "printf 'Content-Length: {}\\r\\n\\r\\n%s' '{reply}'",
            reply.len()
        );
        let env: HashMap<String, String> = std::env::vars().collect();
        let mut server = LanguageServer::spawn(
            "fake",
            Path::new("/bin/sh"),
            &["-c".to_string(), script],
            Path::new("/"),
            &env,
            Arc::new(|| {}),
        )
        .unwrap();
        let id = server.request("initialize", json!({}));
        let events = poll_until(&mut server, |event| matches!(event, ServerEvent::Exited));
        assert!(events.iter().any(|event| matches!(
            event,
            ServerEvent::Response { id: got, method, result: Ok(_) } if *got == id && method == "initialize"
        )));
        assert!(server.has_exited());
    }

    #[test]
    fn answers_settings_and_registrations_and_refuses_the_rest() {
        let env: HashMap<String, String> = HashMap::new();
        let server = LanguageServer::spawn(
            "fake",
            Path::new("/bin/sh"),
            &["-c".to_string(), "true".to_string()],
            Path::new("/tmp"),
            &env,
            Arc::new(|| {}),
        )
        .unwrap();
        assert_eq!(
            server.answer("workspace/configuration", &json!({"items": [{}, {}]})),
            Ok(json!([null, null]))
        );
        assert_eq!(
            server.answer("client/registerCapability", &json!({})),
            Ok(Value::Null)
        );
        let folders = server
            .answer("workspace/workspaceFolders", &Value::Null)
            .unwrap();
        assert_eq!(folders[0]["name"], "tmp");
        assert_eq!(
            server
                .answer("window/showDocument", &json!({}))
                .unwrap_err()
                .code,
            METHOD_NOT_FOUND
        );
    }
}
