//! The language servers of one project folder: each started on first need (after the login-shell environment
//! is known), one per adapter and project root, with every open document mirrored to each of its language's
//! servers, their diagnostics handed back per file and server, and requests answered by all of them together.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use editor::{EditorBuffer, Lang};
use lsp_types::{
    Diagnostic, Hover, HoverContents, HoverProviderCapability, MarkedString, MarkupKind,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncSaveOptions, Url,
};
use ropey::Rope;
use serde_json::{json, Value};

use crate::adapters::{adapters_for, Adapter};
use crate::install::{BinaryStatus, Found, Located};
use crate::position::{char_to_position, position_to_char};
use crate::{LanguageServer, ServerEvent, Waker};

/// Versions of a document kept after sending them, so diagnostics a server computed for an older version can
/// still be placed.
const RETAINED_VERSIONS: usize = 10;

/// One running (or stopped, or failed) server: an adapter in a project folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerId(pub u32);

/// A document's text as sent to its server under `lsp_version`, and the buffer version it matched.
#[derive(Clone, Debug)]
pub struct SyncedText {
    pub lsp_version: i32,
    pub buffer_version: u64,
    pub rope: Rope,
}

/// One server's latest diagnostics for a file, with the text their positions refer to when the file is open.
#[derive(Clone, Debug)]
pub struct DiagnosticsUpdate {
    pub path: PathBuf,
    pub server: ServerId,
    pub diagnostics: Vec<Diagnostic>,
    pub synced: Option<SyncedText>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageLevel {
    Error,
    Warning,
    Info,
}

/// Another server to run for a language instead of one that said it cannot work here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerSwitch {
    pub language: &'static str,
    pub from: &'static str,
    pub to: &'static str,
}

/// A server's `window/showMessage`, or a `window/showMessageRequest` waiting for one of `actions`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerMessage {
    pub server: ServerId,
    pub name: &'static str,
    pub level: MessageLevel,
    pub message: String,
    pub actions: Vec<String>,
    pub request: Option<Value>,
    pub switch: Option<ServerSwitch>,
    /// The server could not be found or downloaded.
    pub missing: bool,
}

#[derive(Clone, Debug)]
pub enum StoreEvent {
    Message(ServerMessage),
    /// None of the file's servers can do what was asked: the line to tell the user.
    Unsupported(String),
    Diagnostics(DiagnosticsUpdate),
    Hover(HoverResponse),
    Completions(crate::CompletionsResponse),
    CompletionResolved(crate::ResolvedCompletion),
    Definitions(DefinitionsResponse),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
}

impl DefinitionKind {
    fn label(self) -> &'static str {
        match self {
            DefinitionKind::Definition => "go to definition",
            DefinitionKind::Declaration => "go to declaration",
            DefinitionKind::TypeDefinition => "go to type definition",
            DefinitionKind::Implementation => "go to implementation",
        }
    }

    fn method(self) -> &'static str {
        match self {
            DefinitionKind::Definition => "textDocument/definition",
            DefinitionKind::Declaration => "textDocument/declaration",
            DefinitionKind::TypeDefinition => "textDocument/typeDefinition",
            DefinitionKind::Implementation => "textDocument/implementation",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub range: lsp_types::Range,
}

#[derive(Clone, Debug)]
pub struct DefinitionsResponse {
    pub request: u64,
    pub origin: Option<std::ops::Range<usize>>,
    pub targets: Vec<DefinitionTarget>,
    pub synced: SyncedText,
}

#[derive(Clone, Debug)]
pub struct HoverResponse {
    pub request: u64,
    pub markdown: Option<String>,
    pub range: Option<std::ops::Range<usize>>,
    pub synced: SyncedText,
}

const MAX_HOVER_BYTES: usize = 100_000;
/// How long a newer download may keep a server waiting when there is a copy already downloaded.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// A server's progress reports closer together than this are skipped.
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

/// Where a server is in its life, as the language-server menu shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServerStatus {
    CheckingForUpdate,
    Downloading,
    Starting,
    Running,
    Stopped,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ServerSummary {
    /// What the server is restarted and stopped by.
    pub key: ServerId,
    pub name: &'static str,
    /// The folder it runs in.
    pub root: PathBuf,
    pub status: ServerStatus,
    pub message: Option<String>,
    pub version: Option<String>,
    pub binary: Option<Located>,
    pub process_id: Option<u32>,
}

/// Work a server reports progress on (`$/progress`).
#[derive(Clone, Debug)]
pub struct ServerWork {
    pub server: ServerId,
    pub token: String,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
    pub cancellable: bool,
    pub updated: Instant,
}

struct Locating {
    found: Receiver<Found>,
    fallback: Option<Located>,
    status: Option<BinaryStatus>,
    started: Instant,
}

enum ServerState {
    Locating(Locating),
    Starting {
        initialize_id: i64,
    },
    Running {
        capabilities: Box<ServerCapabilities>,
    },
}

struct Server {
    server: Option<LanguageServer>,
    state: ServerState,
    /// The servers the settings allowed when it started, to restart it when they change.
    candidates: Vec<&'static str>,
    /// The candidate it is, once known; its first until then.
    name: &'static str,
    binary: Option<Located>,
    version: Option<String>,
    progress_tokens: HashSet<String>,
    work: HashMap<String, ServerWork>,
}

impl Server {
    fn new(state: ServerState, candidates: Vec<&'static str>, name: &'static str) -> Self {
        Server {
            server: None,
            state,
            candidates,
            name,
            binary: None,
            version: None,
            progress_tokens: HashSet::new(),
            work: HashMap::new(),
        }
    }

    fn running(&mut self) -> Option<(&mut LanguageServer, &ServerCapabilities)> {
        match self {
            Server {
                server: Some(server),
                state: ServerState::Running { capabilities },
                ..
            } => Some((server, capabilities)),
            _ => None,
        }
    }
}

/// A server that could not be found or run: not retried until restarted or the settings allow others.
struct Unavailable {
    candidates: Vec<&'static str>,
    name: &'static str,
    message: String,
}

enum Environment {
    Unrequested,
    Loading(Receiver<HashMap<String, String>>),
    Ready(HashMap<String, String>),
}

/// What one server has of a document.
struct DocumentServer {
    id: ServerId,
    /// Oldest first; the last is what the server has now.
    versions: VecDeque<SyncedText>,
}

struct Document {
    uri: Url,
    servers: Vec<DocumentServer>,
}

impl Document {
    fn server(&self, id: ServerId) -> Option<&DocumentServer> {
        self.servers.iter().find(|server| server.id == id)
    }
}

/// What a request asked of several servers has gathered so far.
enum Gathered {
    Hover {
        markdown: Vec<String>,
        range: Option<std::ops::Range<usize>>,
    },
    Completions {
        offset: usize,
        items: Vec<crate::LspCompletion>,
        is_incomplete: bool,
        timeout: Option<Duration>,
    },
    Definitions {
        origin: Option<std::ops::Range<usize>>,
        targets: Vec<DefinitionTarget>,
    },
}

struct Pending {
    gathered: Gathered,
    waiting: usize,
    synced: SyncedText,
    started: Instant,
}

pub struct LspStore {
    root: PathBuf,
    waker: Waker,
    environment: Environment,
    /// Each server's adapter and folder, by id; kept for good so an id always names the same server.
    slots: Vec<(&'static str, PathBuf)>,
    servers: HashMap<ServerId, Server>,
    unavailable: HashMap<ServerId, Unavailable>,
    /// Servers stopped from the menu, with the name they had; started again only on restart.
    stopped: HashMap<ServerId, &'static str>,
    documents: HashMap<PathBuf, Document>,
    /// Diagnostics for files not open on that server yet, delivered when they are.
    unopened_diagnostics: HashMap<(PathBuf, ServerId), Vec<Diagnostic>>,
    /// Every file's latest diagnostics as each server published them, open or not.
    published: HashMap<PathBuf, HashMap<ServerId, Vec<Diagnostic>>>,
    /// Bumped whenever `published` changes, so a view of all diagnostics knows to refresh.
    published_generation: u64,
    updates: Vec<StoreEvent>,
    /// Requests sent to a server, by the request they are part of.
    calls: HashMap<(ServerId, i64), u64>,
    pending: HashMap<u64, Pending>,
    summaries: HashMap<(PathBuf, ServerId), (usize, usize)>,
    resolves: HashMap<(ServerId, i64), (u64, SyncedText)>,
    next_request: u64,
    /// Each server's output, kept across its restarts.
    logs: HashMap<ServerId, crate::SharedLog>,
}

impl LspStore {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self {
            root,
            waker,
            environment: Environment::Unrequested,
            slots: Vec::new(),
            servers: HashMap::new(),
            unavailable: HashMap::new(),
            stopped: HashMap::new(),
            documents: HashMap::new(),
            unopened_diagnostics: HashMap::new(),
            published: HashMap::new(),
            published_generation: 0,
            updates: Vec::new(),
            calls: HashMap::new(),
            pending: HashMap::new(),
            summaries: HashMap::new(),
            resolves: HashMap::new(),
            next_request: 0,
            logs: HashMap::new(),
        }
    }

    /// Use `env` instead of reading the login shell's, e.g. in tests.
    pub fn with_environment(mut self, env: HashMap<String, String>) -> Self {
        self.environment = Environment::Ready(env);
        self
    }

    pub fn open_documents(&self) -> impl Iterator<Item = &Path> {
        self.documents.keys().map(PathBuf::as_path)
    }

    /// The login-shell environment, once read; asking the first time starts reading it.
    fn environment(&mut self) -> Option<&HashMap<String, String>> {
        let next = match &self.environment {
            Environment::Unrequested => {
                let (sender, receiver) = channel();
                let root = self.root.clone();
                let waker = self.waker.clone();
                std::thread::spawn(move || {
                    let env = crate::capture_login_env(&root).unwrap_or_else(|error| {
                        eprintln!("lsp: using the app's own environment: {error}");
                        std::env::vars().collect()
                    });
                    if sender.send(env).is_ok() {
                        waker();
                    }
                });
                Some(Environment::Loading(receiver))
            }
            Environment::Loading(receiver) => match receiver.try_recv() {
                Ok(env) => Some(Environment::Ready(env)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Environment::Ready(std::env::vars().collect()))
                }
            },
            Environment::Ready(_) => None,
        };
        if let Some(next) = next {
            self.environment = next;
        }
        match &self.environment {
            Environment::Ready(env) => Some(env),
            _ => None,
        }
    }

    /// The id of `adapter`'s server for `root`, new if there wasn't one.
    fn server_id(&mut self, adapter: &'static str, root: &Path) -> ServerId {
        let found = self
            .slots
            .iter()
            .position(|(name, folder)| *name == adapter && folder == root);
        let index = found.unwrap_or_else(|| {
            self.slots.push((adapter, root.to_path_buf()));
            self.slots.len() - 1
        });
        ServerId(index as u32)
    }

    fn slot(&self, id: ServerId) -> Option<&(&'static str, PathBuf)> {
        self.slots.get(id.0 as usize)
    }

    /// Start the server if it isn't running (finding its binary off-thread first); `true` once it is
    /// initialized.
    fn ensure_server(&mut self, id: ServerId, adapter: &Adapter) -> bool {
        let candidates: Vec<&'static str> = adapter
            .candidates
            .iter()
            .map(|candidate| candidate.name)
            .collect();
        if self.stopped.contains_key(&id)
            || self
                .unavailable
                .get(&id)
                .is_some_and(|unavailable| unavailable.candidates == candidates)
        {
            return false;
        }
        self.unavailable.remove(&id);
        if self
            .servers
            .get(&id)
            .is_some_and(|server| server.candidates != candidates)
        {
            self.drop_server(id);
        }
        let located = match self.servers.get_mut(&id) {
            Some(Server {
                state: ServerState::Running { .. },
                ..
            }) => return true,
            Some(Server {
                state: ServerState::Starting { .. },
                ..
            }) => return false,
            Some(Server {
                state: ServerState::Locating(locating),
                name,
                ..
            }) => match Self::take_located(locating, name) {
                Some(located) => located,
                None => return false,
            },
            None => {
                let Some(env) = self.environment().cloned() else {
                    return false;
                };
                let (sender, found) = channel();
                let waker = self.waker.clone();
                let finder = adapter.clone();
                std::thread::spawn(move || {
                    let downloads = crate::install::languages_dir();
                    crate::install::locate(
                        &finder.candidates,
                        &env,
                        downloads.as_deref(),
                        &|event| {
                            if sender.send(event).is_ok() {
                                waker();
                            }
                        },
                    );
                });
                let waker = self.waker.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(DOWNLOAD_TIMEOUT);
                    waker();
                });
                let first = candidates.first().copied().unwrap_or(adapter.name);
                let locating = Locating {
                    found,
                    fallback: None,
                    status: None,
                    started: Instant::now(),
                };
                self.servers.insert(
                    id,
                    Server::new(ServerState::Locating(locating), candidates, first),
                );
                return false;
            }
        };
        let located = match located {
            Ok(located) => located,
            Err(message) => {
                eprintln!("lsp: {}: {message}", adapter.name);
                let name = self
                    .servers
                    .get(&id)
                    .map_or(adapter.name, |server| server.name);
                self.updates.push(StoreEvent::Message(ServerMessage {
                    server: id,
                    name,
                    level: MessageLevel::Warning,
                    message: message.clone(),
                    actions: Vec::new(),
                    request: None,
                    switch: None,
                    missing: true,
                }));
                self.fail(id, message);
                return false;
            }
        };
        let (Some(env), Some(root)) = (
            self.environment().cloned(),
            self.slot(id).map(|(_, root)| root.clone()),
        ) else {
            return false;
        };
        let log = self.logs.entry(id).or_default().clone();
        if let Ok(mut log) = log.lock() {
            log.push(format!(
                "Starting {} {} in {}",
                located.binary.display(),
                located.args.join(" "),
                root.display()
            ));
        }
        let mut server = match LanguageServer::spawn(
            located.name,
            &located.binary,
            &located.args,
            &root,
            &env,
            self.waker.clone(),
            log,
        ) {
            Ok(server) => server,
            Err(error) => {
                let message = format!("could not start {}: {error}", located.binary.display());
                eprintln!("lsp: {message}");
                self.fail(id, message);
                return false;
            }
        };
        server.set_configuration(crate::adapters::workspace_configuration(
            adapter.name,
            &root,
        ));
        let initialize_id = server.request(
            "initialize",
            initialize_params(&root, crate::adapters::initialization_options(adapter.name)),
        );
        if let Some(entry) = self.servers.get_mut(&id) {
            entry.name = located.name;
            entry.binary = Some(located);
            entry.server = Some(server);
            entry.state = ServerState::Starting { initialize_id };
        }
        false
    }

    /// What the search for a binary came to, once it has: its answer, or the copy downloaded before when
    /// the answer failed or is taking too long.
    fn take_located(
        locating: &mut Locating,
        name: &mut &'static str,
    ) -> Option<Result<Located, String>> {
        loop {
            match locating.found.try_recv() {
                Ok(Found::Status(candidate, status)) => {
                    *name = candidate;
                    locating.status = Some(status);
                }
                Ok(Found::Fallback(located)) => {
                    *name = located.name;
                    locating.fallback = Some(located);
                }
                Ok(Found::Done(Ok(located))) => return Some(Ok(located)),
                Ok(Found::Done(Err(message))) => {
                    return Some(locating.fallback.take().ok_or(message))
                }
                Err(TryRecvError::Disconnected) => {
                    return Some(
                        locating
                            .fallback
                            .take()
                            .ok_or_else(|| "the search for a binary stopped".to_string()),
                    )
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if locating.started.elapsed() >= DOWNLOAD_TIMEOUT {
            return locating.fallback.take().map(Ok);
        }
        None
    }

    fn fail(&mut self, id: ServerId, message: String) {
        let fallback = self.slot(id).map_or("server", |(name, _)| *name);
        let (candidates, name) = self
            .servers
            .get(&id)
            .map(|server| (server.candidates.clone(), server.name))
            .unwrap_or((Vec::new(), fallback));
        self.drop_server(id);
        self.unavailable.insert(
            id,
            Unavailable {
                candidates,
                name,
                message,
            },
        );
    }

    /// Keep `path`'s document open on each of its language's servers and in step with `buffer`: opened once
    /// a server is up, then sent each change (just the edited ranges when the server accepts that).
    pub fn sync_document(&mut self, path: &Path, lang: Lang, buffer: &EditorBuffer) {
        let wanted: Vec<(ServerId, Adapter, &'static str)> = adapters_for(lang)
            .into_iter()
            .map(|(adapter, language_id)| {
                let root = adapter.manifest.root_for(path, &self.root);
                (self.server_id(adapter.name, &root), adapter, language_id)
            })
            .collect();
        let dropped: Vec<ServerId> = self
            .documents
            .get(path)
            .map(|document| {
                document
                    .servers
                    .iter()
                    .map(|server| server.id)
                    .filter(|id| !wanted.iter().any(|(wanted, _, _)| wanted == id))
                    .collect()
            })
            .unwrap_or_default();
        for id in dropped {
            self.close_on(path, id);
            self.clear_diagnostics(path.to_path_buf(), id);
        }
        if wanted.is_empty() {
            self.documents.remove(path);
            return;
        }
        for (id, adapter, language_id) in wanted {
            if self.ensure_server(id, &adapter) {
                self.sync_with(path, id, language_id, buffer);
            }
        }
    }

    fn sync_with(&mut self, path: &Path, id: ServerId, language_id: &str, buffer: &EditorBuffer) {
        let Ok(uri) = Url::from_file_path(path) else {
            return;
        };
        let Some((server, capabilities)) = self.servers.get_mut(&id).and_then(Server::running)
        else {
            return;
        };
        let document = self
            .documents
            .entry(path.to_path_buf())
            .or_insert_with(|| Document {
                uri: uri.clone(),
                servers: Vec::new(),
            });
        let Some(synced) = document.servers.iter_mut().find(|server| server.id == id) else {
            server.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 0,
                    "text": buffer.rope.to_string(),
                }}),
            );
            let text = SyncedText {
                lsp_version: 0,
                buffer_version: buffer.version(),
                rope: buffer.rope.clone(),
            };
            document.servers.push(DocumentServer {
                id,
                versions: VecDeque::from([text.clone()]),
            });
            if let Some(diagnostics) = self.unopened_diagnostics.remove(&(path.to_path_buf(), id)) {
                self.updates
                    .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                        path: path.to_path_buf(),
                        server: id,
                        diagnostics,
                        synced: Some(text),
                    }));
            }
            return;
        };
        let Some(last) = synced.versions.back() else {
            return;
        };
        if last.buffer_version == buffer.version() {
            return;
        }
        let changes = match sync_kind(capabilities) {
            Some(TextDocumentSyncKind::INCREMENTAL) => {
                incremental_changes(last, buffer).unwrap_or_else(|| full_change(buffer))
            }
            Some(TextDocumentSyncKind::FULL) => full_change(buffer),
            _ => Vec::new(),
        };
        let version = last.lsp_version + 1;
        if !changes.is_empty() {
            server.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": document.uri, "version": version},
                    "contentChanges": changes,
                }),
            );
        }
        synced.versions.push_back(SyncedText {
            lsp_version: version,
            buffer_version: buffer.version(),
            rope: buffer.rope.clone(),
        });
        while synced.versions.len() > RETAINED_VERSIONS {
            synced.versions.pop_front();
        }
    }

    /// Tell each server `path` was saved, with its text if it asked for that.
    pub fn did_save(&mut self, path: &Path, buffer: &EditorBuffer) {
        let Some(document) = self.documents.get(path) else {
            return;
        };
        for id in document.servers.iter().map(|server| server.id) {
            let Some((server, capabilities)) = self.servers.get_mut(&id).and_then(Server::running)
            else {
                continue;
            };
            let Some(include_text) = save_includes_text(capabilities) else {
                continue;
            };
            let mut params = json!({"textDocument": {"uri": document.uri}});
            if include_text {
                params["text"] = Value::String(buffer.rope.to_string());
            }
            server.notify("textDocument/didSave", params);
        }
    }

    pub fn close_document(&mut self, path: &Path) {
        let ids: Vec<ServerId> = self
            .documents
            .get(path)
            .map(|document| document.servers.iter().map(|server| server.id).collect())
            .unwrap_or_default();
        for id in ids {
            self.close_on(path, id);
        }
        self.documents.remove(path);
    }

    /// Close `path` on one server.
    fn close_on(&mut self, path: &Path, id: ServerId) {
        let Some(document) = self.documents.get_mut(path) else {
            return;
        };
        document.servers.retain(|server| server.id != id);
        let uri = document.uri.clone();
        if let Some(server) = self
            .servers
            .get_mut(&id)
            .and_then(|server| server.server.as_mut())
        {
            server.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            );
        }
    }

    /// The servers `path` is open on, running and in step with `buffer`, with what they can do.
    fn servers_in_step(&self, path: &Path, buffer: &EditorBuffer) -> Vec<(ServerId, SyncedText)> {
        let Some(document) = self.documents.get(path) else {
            return Vec::new();
        };
        document
            .servers
            .iter()
            .filter_map(|server| {
                let synced = server.versions.back()?;
                (synced.buffer_version == buffer.version()).then(|| (server.id, synced.clone()))
            })
            .filter(|(id, _)| {
                matches!(
                    self.servers.get(id).map(|server| &server.state),
                    Some(ServerState::Running { .. })
                )
            })
            .collect()
    }

    fn capabilities(&self, id: ServerId) -> Option<&ServerCapabilities> {
        match self.servers.get(&id).map(|server| &server.state) {
            Some(ServerState::Running { capabilities }) => Some(capabilities),
            _ => None,
        }
    }

    /// Send `method` to each of `targets`, gathering their answers under one request.
    fn ask(
        &mut self,
        targets: Vec<ServerId>,
        method: &str,
        params: Value,
        gathered: Gathered,
        synced: SyncedText,
    ) -> Option<u64> {
        if targets.is_empty() {
            return None;
        }
        let request = self.next_request;
        self.next_request += 1;
        let mut waiting = 0;
        for id in targets {
            if let Some((server, _)) = self.servers.get_mut(&id).and_then(Server::running) {
                let call = server.request(method, params.clone());
                self.calls.insert((id, call), request);
                waiting += 1;
            }
        }
        if waiting == 0 {
            return None;
        }
        self.pending.insert(
            request,
            Pending {
                gathered,
                waiting,
                synced,
                started: Instant::now(),
            },
        );
        Some(request)
    }

    pub fn hover(
        &mut self,
        path: &Path,
        lang: Lang,
        buffer: &EditorBuffer,
        offset: usize,
    ) -> Option<u64> {
        self.sync_document(path, lang, buffer);
        let in_step = self.servers_in_step(path, buffer);
        let synced = in_step.first()?.1.clone();
        let targets: Vec<ServerId> = in_step
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| {
                self.capabilities(*id).is_some_and(|capabilities| {
                    matches!(
                        capabilities.hover_provider,
                        Some(
                            HoverProviderCapability::Simple(true)
                                | HoverProviderCapability::Options(_)
                        )
                    )
                })
            })
            .collect();
        let uri = self.documents.get(path)?.uri.clone();
        let params = json!({
            "textDocument": {"uri": uri},
            "position": char_to_position(&synced.rope, offset),
        });
        let gathered = Gathered::Hover {
            markdown: Vec::new(),
            range: None,
        };
        self.ask(targets, "textDocument/hover", params, gathered, synced)
    }

    /// `None` when no running server completes `path`; else the characters that ask one to without a word.
    pub fn completion_triggers(&mut self, path: &Path) -> Option<Vec<String>> {
        let document = self.documents.get(path)?;
        let mut triggers: Option<Vec<String>> = None;
        for server in &document.servers {
            let Some(provider) = self
                .capabilities(server.id)
                .and_then(|capabilities| capabilities.completion_provider.as_ref())
            else {
                continue;
            };
            let all = triggers.get_or_insert_with(Vec::new);
            for trigger in provider.trigger_characters.iter().flatten() {
                if !all.contains(trigger) {
                    all.push(trigger.clone());
                }
            }
        }
        triggers
    }

    pub fn completion(
        &mut self,
        path: &Path,
        lang: Lang,
        buffer: &EditorBuffer,
        offset: usize,
        trigger: Option<&str>,
    ) -> Option<u64> {
        self.sync_document(path, lang, buffer);
        let choice = crate::adapters::choice_for(lang);
        if !choice.completions {
            return None;
        }
        let in_step = self.servers_in_step(path, buffer);
        let synced = in_step.first()?.1.clone();
        let uri = self.documents.get(path)?.uri.clone();
        let request = self.next_request;
        self.next_request += 1;
        let mut waiting = 0;
        for (id, _) in in_step {
            let Some((server, capabilities)) = self.servers.get_mut(&id).and_then(Server::running)
            else {
                continue;
            };
            let Some(provider) = capabilities.completion_provider.as_ref() else {
                continue;
            };
            let knows = |t: &&str| {
                provider
                    .trigger_characters
                    .iter()
                    .flatten()
                    .any(|known| known == t)
            };
            // A character only one server triggers on is that server's to answer.
            let context = match trigger {
                Some(character) if knows(&character) => {
                    json!({"triggerKind": 2, "triggerCharacter": character})
                }
                Some(_) => continue,
                None => json!({"triggerKind": 1}),
            };
            let call = server.request(
                "textDocument/completion",
                json!({
                    "textDocument": {"uri": uri},
                    "position": char_to_position(&synced.rope, offset),
                    "context": context,
                }),
            );
            self.calls.insert((id, call), request);
            waiting += 1;
        }
        if waiting == 0 {
            return None;
        }
        let timeout = (choice.completion_timeout_ms > 0)
            .then(|| Duration::from_millis(choice.completion_timeout_ms));
        self.pending.insert(
            request,
            Pending {
                gathered: Gathered::Completions {
                    offset,
                    items: Vec::new(),
                    is_incomplete: false,
                    timeout,
                },
                waiting,
                synced,
                started: Instant::now(),
            },
        );
        Some(request)
    }

    /// Ask the server a completion came from to fill in the rest of it.
    pub fn resolve_completion(
        &mut self,
        path: &Path,
        server: ServerId,
        raw: &Value,
    ) -> Option<u64> {
        let synced = self
            .documents
            .get(path)?
            .server(server)?
            .versions
            .back()?
            .clone();
        let (language_server, capabilities) =
            self.servers.get_mut(&server).and_then(Server::running)?;
        let resolves = capabilities
            .completion_provider
            .as_ref()?
            .resolve_provider
            .unwrap_or(false);
        if !resolves {
            return None;
        }
        let call = language_server.request("completionItem/resolve", raw.clone());
        let request = self.next_request;
        self.next_request += 1;
        self.resolves.insert((server, call), (request, synced));
        Some(request)
    }

    pub fn definitions(
        &mut self,
        path: &Path,
        lang: Lang,
        buffer: &EditorBuffer,
        offset: usize,
        kind: DefinitionKind,
    ) -> Option<u64> {
        use lsp_types::{
            DeclarationCapability, ImplementationProviderCapability, OneOf,
            TypeDefinitionProviderCapability,
        };
        self.sync_document(path, lang, buffer);
        let in_step = self.servers_in_step(path, buffer);
        let synced = in_step.first()?.1.clone();
        let supported = |capabilities: &ServerCapabilities| match kind {
            DefinitionKind::Definition => !matches!(
                capabilities.definition_provider,
                None | Some(OneOf::Left(false))
            ),
            DefinitionKind::Declaration => !matches!(
                capabilities.declaration_provider,
                None | Some(DeclarationCapability::Simple(false))
            ),
            DefinitionKind::TypeDefinition => !matches!(
                capabilities.type_definition_provider,
                None | Some(TypeDefinitionProviderCapability::Simple(false))
            ),
            DefinitionKind::Implementation => !matches!(
                capabilities.implementation_provider,
                None | Some(ImplementationProviderCapability::Simple(false))
            ),
        };
        let targets: Vec<ServerId> = in_step
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| self.capabilities(*id).is_some_and(supported))
            .collect();
        if targets.is_empty() {
            let names: Vec<String> = in_step
                .iter()
                .filter_map(|(id, _)| {
                    let server = self.servers.get(id)?;
                    Some(match &server.version {
                        Some(version) => format!("{} {version}", server.name),
                        None => server.name.to_string(),
                    })
                })
                .collect();
            self.updates.push(StoreEvent::Unsupported(format!(
                "{} does not support {}",
                names.join(", "),
                kind.label()
            )));
            return None;
        }
        let uri = self.documents.get(path)?.uri.clone();
        let params = json!({
            "textDocument": {"uri": uri},
            "position": char_to_position(&synced.rope, offset),
        });
        let gathered = Gathered::Definitions {
            origin: None,
            targets: Vec::new(),
        };
        self.ask(targets, kind.method(), params, gathered, synced)
    }

    pub fn poll(&mut self) -> Vec<StoreEvent> {
        let ids: Vec<ServerId> = self.servers.keys().copied().collect();
        for id in ids {
            let events = match self
                .servers
                .get_mut(&id)
                .and_then(|server| server.server.as_mut())
            {
                Some(server) => server.poll(),
                None => continue,
            };
            for event in events {
                self.handle_event(id, event);
            }
        }
        self.expire_completions();
        std::mem::take(&mut self.updates)
    }

    /// Completions still waiting on a server past the settings' timeout go out with what has come.
    fn expire_completions(&mut self) {
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                matches!(
                    pending.gathered,
                    Gathered::Completions { timeout: Some(timeout), .. }
                        if pending.started.elapsed() >= timeout
                )
            })
            .map(|(request, _)| *request)
            .collect();
        for request in expired {
            self.calls.retain(|_, waiting_on| *waiting_on != request);
            self.finish(request);
        }
    }

    /// One server's answer (or its absence) to a gathered request; the last one sends the whole.
    fn answered(&mut self, id: ServerId, call: i64, result: Option<Value>) -> bool {
        let Some(request) = self.calls.remove(&(id, call)) else {
            return false;
        };
        let Some(pending) = self.pending.get_mut(&request) else {
            return true;
        };
        let rope = pending.synced.rope.clone();
        match &mut pending.gathered {
            Gathered::Hover { markdown, range } => {
                let hover = result
                    .and_then(|value| serde_json::from_value::<Option<Hover>>(value).ok())
                    .flatten();
                if let Some(hover) = hover {
                    if range.is_none() {
                        *range = hover.range.map(|found| {
                            position_to_char(&rope, found.start)..position_to_char(&rope, found.end)
                        });
                    }
                    let text = combine_hover_contents(hover.contents);
                    if !text.trim().is_empty() && !markdown.contains(&text) {
                        markdown.push(text);
                    }
                }
            }
            Gathered::Completions {
                offset,
                items,
                is_incomplete,
                ..
            } => {
                let (mut found, incomplete) = crate::completion::parse_completions(
                    result.unwrap_or(Value::Null),
                    &rope,
                    *offset,
                );
                for item in &mut found {
                    item.server = Some(id);
                }
                items.extend(found);
                *is_incomplete |= incomplete;
            }
            Gathered::Definitions { origin, targets } => {
                let (found_origin, found) = parse_definitions(result, &rope);
                if origin.is_none() {
                    *origin = found_origin;
                }
                for target in found {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
        }
        pending.waiting = pending.waiting.saturating_sub(1);
        if pending.waiting == 0 {
            self.finish(request);
        }
        true
    }

    fn finish(&mut self, request: u64) {
        let Some(pending) = self.pending.remove(&request) else {
            return;
        };
        let synced = pending.synced;
        self.updates.push(match pending.gathered {
            Gathered::Hover { markdown, range } => StoreEvent::Hover(HoverResponse {
                request,
                markdown: (!markdown.is_empty()).then(|| markdown.join("\n\n---\n\n")),
                range,
                synced,
            }),
            Gathered::Completions {
                items,
                is_incomplete,
                ..
            } => StoreEvent::Completions(crate::CompletionsResponse {
                request,
                items,
                is_incomplete,
                synced,
            }),
            Gathered::Definitions { origin, targets } => {
                StoreEvent::Definitions(DefinitionsResponse {
                    request,
                    origin,
                    targets,
                    synced,
                })
            }
        });
    }

    fn handle_event(&mut self, id: ServerId, event: ServerEvent) {
        match event {
            ServerEvent::Response {
                id: call, result, ..
            } => {
                if self.answered(id, call, result.as_ref().ok().cloned()) {
                    return;
                }
                if let Some((request, synced)) = self.resolves.remove(&(id, call)) {
                    let result = result.ok();
                    let documentation = result
                        .as_ref()
                        .and_then(|value| value.get("documentation"))
                        .and_then(|value| {
                            serde_json::from_value::<lsp_types::Documentation>(value.clone()).ok()
                        })
                        .map(crate::CompletionDocumentation::from);
                    let detail = result
                        .as_ref()
                        .and_then(|value| value.get("detail"))
                        .and_then(Value::as_str)
                        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                        .filter(|text| !text.is_empty());
                    let additional_edits = result
                        .and_then(|value| {
                            serde_json::from_value::<Vec<lsp_types::TextEdit>>(
                                value.get("additionalTextEdits")?.clone(),
                            )
                            .ok()
                        })
                        .map(|edits| crate::completion::text_edits(&synced.rope, &edits))
                        .unwrap_or_default();
                    self.updates
                        .push(StoreEvent::CompletionResolved(crate::ResolvedCompletion {
                            request,
                            additional_edits,
                            documentation,
                            detail,
                            synced,
                        }));
                    return;
                }
                let Some(server) = self.servers.get_mut(&id) else {
                    return;
                };
                let ServerState::Starting { initialize_id } = server.state else {
                    return;
                };
                if call != initialize_id {
                    return;
                }
                let result = result.ok();
                server.version = result
                    .as_ref()
                    .and_then(|result| result.pointer("/serverInfo/version"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let capabilities = result.and_then(|result| {
                    serde_json::from_value::<ServerCapabilities>(
                        result.get("capabilities")?.clone(),
                    )
                    .ok()
                });
                match (capabilities, server.server.as_mut()) {
                    (Some(capabilities), Some(language_server)) => {
                        language_server.notify("initialized", json!({}));
                        server.state = ServerState::Running {
                            capabilities: Box::new(capabilities),
                        };
                    }
                    _ => {
                        eprintln!("lsp: {} failed to initialize", server.name);
                        self.server_gone(id, "failed to initialize");
                    }
                }
            }
            ServerEvent::Notification { method, params } => {
                if method == "textDocument/publishDiagnostics" {
                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                        Ok(params) => self.receive_diagnostics(id, params),
                        Err(error) => eprintln!("lsp: bad diagnostics: {error}"),
                    }
                } else if method == "$/progress" {
                    self.receive_progress(id, &params);
                } else if method == "window/showMessage" {
                    self.log_message(id, &params);
                    self.receive_message(id, &params, None);
                } else if method == "window/logMessage" {
                    self.log_message(id, &params);
                }
            }
            ServerEvent::MessageRequest { id: call, params } => {
                self.receive_message(id, &params, Some(call));
            }
            ServerEvent::ProgressCreated { token } => {
                if let Some(server) = self.servers.get_mut(&id) {
                    server.progress_tokens.insert(token);
                }
            }
            ServerEvent::Exited => {
                let name = self.servers.get(&id).map_or("server", |server| server.name);
                eprintln!("lsp: {name} exited");
                self.server_gone(id, "the server exited");
            }
        }
    }

    /// A server that exited or failed: dropped with the diagnostics it had shown, and not retried.
    fn server_gone(&mut self, id: ServerId, message: &str) {
        self.fail(id, message.to_string());
    }

    /// Stop a server and forget its documents and diagnostics; its files open on it again on their next sync.
    fn drop_server(&mut self, id: ServerId) {
        if let Some(server) = self.servers.remove(&id).and_then(|server| server.server) {
            server.shutdown();
        }
        let answered: Vec<i64> = self
            .calls
            .keys()
            .filter(|(server, _)| *server == id)
            .map(|(_, call)| *call)
            .collect();
        for call in answered {
            self.answered(id, call, None);
        }
        self.resolves.retain(|(server, _), _| *server != id);
        let paths: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|(_, document)| document.server(id).is_some())
            .map(|(path, _)| path.clone())
            .collect();
        for path in &paths {
            if let Some(document) = self.documents.get_mut(path) {
                document.servers.retain(|server| server.id != id);
            }
        }
        let published: Vec<PathBuf> = self
            .published
            .iter()
            .filter(|(_, by_server)| by_server.contains_key(&id))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths.into_iter().chain(published) {
            self.clear_diagnostics(path, id);
        }
        self.unopened_diagnostics
            .retain(|(_, server), _| *server != id);
    }

    /// Every server this folder has started, stopped or failed to find, by name.
    pub fn servers(&self) -> Vec<ServerSummary> {
        let root_of = |id: &ServerId| {
            self.slot(*id)
                .map(|(_, root)| root.clone())
                .unwrap_or_else(|| self.root.clone())
        };
        let mut summaries: Vec<ServerSummary> = self
            .servers
            .iter()
            .map(|(id, server)| ServerSummary {
                key: *id,
                name: server.name,
                root: root_of(id),
                status: match &server.state {
                    ServerState::Locating(Locating {
                        status: Some(BinaryStatus::CheckingForUpdate),
                        ..
                    }) => ServerStatus::CheckingForUpdate,
                    ServerState::Locating(Locating {
                        status: Some(BinaryStatus::Downloading),
                        ..
                    }) => ServerStatus::Downloading,
                    ServerState::Locating(_) | ServerState::Starting { .. } => {
                        ServerStatus::Starting
                    }
                    ServerState::Running { .. } => ServerStatus::Running,
                },
                message: None,
                version: server.version.clone(),
                binary: server.binary.clone(),
                process_id: server.server.as_ref().and_then(LanguageServer::process_id),
            })
            .collect();
        summaries.extend(self.stopped.iter().map(|(id, name)| ServerSummary {
            key: *id,
            name,
            root: root_of(id),
            status: ServerStatus::Stopped,
            message: None,
            version: None,
            binary: None,
            process_id: None,
        }));
        summaries.extend(
            self.unavailable
                .iter()
                .map(|(id, unavailable)| ServerSummary {
                    key: *id,
                    name: unavailable.name,
                    root: root_of(id),
                    status: ServerStatus::Failed,
                    message: Some(unavailable.message.clone()),
                    version: None,
                    binary: None,
                    process_id: None,
                }),
        );
        summaries.sort_by(|a, b| (&a.root, a.name).cmp(&(&b.root, b.name)));
        summaries
    }

    /// The work servers report progress on, the most recently updated first.
    pub fn work(&self) -> Vec<ServerWork> {
        let mut work: Vec<ServerWork> = self
            .servers
            .values()
            .flat_map(|server| server.work.values().cloned())
            .collect();
        work.sort_by_key(|work| std::cmp::Reverse(work.updated));
        work
    }

    /// Start a server again: stopped, failed or running, it looks for its binary anew on the next sync.
    pub fn restart_server(&mut self, id: ServerId) {
        self.drop_server(id);
        self.stopped.remove(&id);
        self.unavailable.remove(&id);
        (self.waker)();
    }

    pub fn stop_server(&mut self, id: ServerId) {
        let name = self
            .servers
            .get(&id)
            .map(|server| server.name)
            .or_else(|| {
                self.unavailable
                    .get(&id)
                    .map(|unavailable| unavailable.name)
            })
            .or_else(|| self.slot(id).map(|(name, _)| *name))
            .unwrap_or("server");
        self.drop_server(id);
        self.unavailable.remove(&id);
        self.stopped.insert(id, name);
    }

    pub fn restart_all(&mut self) {
        for id in self.known_ids() {
            self.restart_server(id);
        }
    }

    pub fn stop_all(&mut self) {
        for id in self.known_ids() {
            if !self.stopped.contains_key(&id) {
                self.stop_server(id);
            }
        }
    }

    /// Ask a server to cancel work it said can be (`window/workDoneProgress/cancel`).
    fn log_message(&mut self, id: ServerId, params: &Value) {
        let level = match params.get("type").and_then(Value::as_u64) {
            Some(1) => "ERROR",
            Some(2) => "WARN",
            Some(3) => "INFO",
            _ => "LOG",
        };
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Ok(mut log) = self.logs.entry(id).or_default().lock() {
            log.push(format!("[{level}] {message}"));
        }
    }

    /// The lines of `id`'s log after the first `seen`, and how many there have been; none before it started.
    pub fn server_log(&self, id: ServerId, seen: u64) -> Option<(Vec<String>, u64)> {
        let log = self.logs.get(&id)?.lock().ok()?;
        Some(log.since(seen))
    }

    pub fn has_log(&self, id: ServerId) -> bool {
        self.logs.contains_key(&id)
    }

    /// Whether a view shows `id`'s log, so its new lines wake the UI.
    pub fn watch_log(&mut self, id: ServerId, watched: bool) {
        if let Ok(mut log) = self.logs.entry(id).or_default().lock() {
            log.watched = watched;
        }
    }

    fn receive_message(&mut self, id: ServerId, params: &Value, request: Option<Value>) {
        let name = self.servers.get(&id).map_or("server", |server| server.name);
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let level = match params.get("type").and_then(Value::as_u64) {
            Some(1) => MessageLevel::Error,
            Some(2) => MessageLevel::Warning,
            _ => MessageLevel::Info,
        };
        let actions = params
            .get("actions")
            .and_then(Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .filter_map(|action| action.get("title")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let switch = suggested_switch(name, &message);
        self.updates.push(StoreEvent::Message(ServerMessage {
            server: id,
            name,
            level,
            message,
            actions,
            request,
            switch,
            missing: false,
        }));
    }

    /// Answers a `window/showMessageRequest` with the action picked, or none when it was dismissed.
    pub fn answer_message(&mut self, id: ServerId, request: Value, action: Option<&str>) {
        if let Some(server) = self
            .servers
            .get_mut(&id)
            .and_then(|server| server.server.as_mut())
        {
            let result = action.map_or(Value::Null, |title| json!({"title": title}));
            server.reply(request, result);
        }
    }

    pub fn cancel_work(&mut self, id: ServerId, token: &str) {
        if let Some(server) = self
            .servers
            .get_mut(&id)
            .and_then(|server| server.server.as_mut())
        {
            server.notify("window/workDoneProgress/cancel", json!({"token": token}));
        }
    }

    fn known_ids(&self) -> Vec<ServerId> {
        let mut ids: Vec<ServerId> = self
            .servers
            .keys()
            .chain(self.stopped.keys())
            .chain(self.unavailable.keys())
            .copied()
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Track a server's `$/progress`: begun, reported on (throttled) and ended, for tokens it created.
    fn receive_progress(&mut self, id: ServerId, params: &Value) {
        let Some(server) = self.servers.get_mut(&id) else {
            return;
        };
        let Some(token) = params.get("token").map(crate::progress_token) else {
            return;
        };
        if !server.progress_tokens.contains(&token) {
            return;
        }
        let value = params.get("value").cloned().unwrap_or(Value::Null);
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let percentage = value
            .get("percentage")
            .and_then(Value::as_u64)
            .map(|percentage| percentage as u32);
        let cancellable = value.get("cancellable").and_then(Value::as_bool);
        let now = Instant::now();
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                server.work.insert(
                    token.clone(),
                    ServerWork {
                        server: id,
                        title: text("title").unwrap_or_else(|| token.clone()),
                        token,
                        message: text("message"),
                        percentage,
                        cancellable: cancellable.unwrap_or(false),
                        updated: now,
                    },
                );
            }
            Some("report") => {
                if let Some(work) = server.work.get_mut(&token) {
                    if now.duration_since(work.updated) < PROGRESS_THROTTLE {
                        return;
                    }
                    if let Some(message) = text("message") {
                        work.message = Some(message);
                    }
                    if percentage.is_some() {
                        work.percentage = percentage;
                    }
                    if let Some(cancellable) = cancellable {
                        work.cancellable = cancellable;
                    }
                    work.updated = now;
                }
            }
            Some("end") => {
                server.work.remove(&token);
                server.progress_tokens.remove(&token);
            }
            _ => {}
        }
    }

    /// Forget one server's diagnostics for `path`, and tell the file.
    fn clear_diagnostics(&mut self, path: PathBuf, id: ServerId) {
        self.summaries.remove(&(path.clone(), id));
        if let Some(by_server) = self.published.get_mut(&path) {
            if by_server.remove(&id).is_some() {
                self.published_generation += 1;
            }
            if by_server.is_empty() {
                self.published.remove(&path);
            }
        }
        self.updates
            .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                path,
                server: id,
                diagnostics: Vec::new(),
                synced: None,
            }));
    }

    /// Every file's latest diagnostics from all its servers, open or not, sorted by path, with the generation
    /// they are at.
    pub fn all_diagnostics(&self) -> (u64, Vec<(PathBuf, Vec<Diagnostic>)>) {
        let mut files: Vec<(PathBuf, Vec<Diagnostic>)> = self
            .published
            .iter()
            .map(|(path, by_server)| {
                let mut diagnostics: Vec<Diagnostic> =
                    by_server.values().flatten().cloned().collect();
                diagnostics.sort_by_key(|d| (d.range.start.line, d.range.start.character));
                (path.clone(), diagnostics)
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        (self.published_generation, files)
    }

    pub fn diagnostics_generation(&self) -> u64 {
        self.published_generation
    }

    pub fn diagnostic_summary(&self) -> (usize, usize) {
        self.summaries
            .values()
            .fold((0, 0), |(errors, warnings), (e, w)| {
                (errors + e, warnings + w)
            })
    }

    fn receive_diagnostics(&mut self, id: ServerId, params: PublishDiagnosticsParams) {
        let Ok(path) = params.uri.to_file_path() else {
            return;
        };
        let count = |severity: lsp_types::DiagnosticSeverity| {
            params
                .diagnostics
                .iter()
                .filter(|d| d.severity.unwrap_or(lsp_types::DiagnosticSeverity::ERROR) == severity)
                .count()
        };
        let (errors, warnings) = (
            count(lsp_types::DiagnosticSeverity::ERROR),
            count(lsp_types::DiagnosticSeverity::WARNING),
        );
        if errors + warnings == 0 {
            self.summaries.remove(&(path.clone(), id));
        } else {
            self.summaries
                .insert((path.clone(), id), (errors, warnings));
        }
        let mut diagnostics = params.diagnostics;
        diagnostics.sort_by(|a, b| {
            (a.range.start.line, a.range.start.character)
                .cmp(&(b.range.start.line, b.range.start.character))
                .then_with(|| {
                    (b.range.end.line, b.range.end.character)
                        .cmp(&(a.range.end.line, a.range.end.character))
                })
                .then_with(|| a.severity.cmp(&b.severity))
        });
        let by_server = self.published.entry(path.clone()).or_default();
        if diagnostics.is_empty() {
            by_server.remove(&id);
        } else {
            by_server.insert(id, diagnostics.clone());
        }
        if by_server.is_empty() {
            self.published.remove(&path);
        }
        self.published_generation += 1;
        let Some(synced_server) = self
            .documents
            .get(&path)
            .and_then(|document| document.server(id))
        else {
            self.unopened_diagnostics.insert((path, id), diagnostics);
            return;
        };
        let synced = match params.version {
            Some(version) => synced_server
                .versions
                .iter()
                .find(|v| v.lsp_version == version)
                .cloned(),
            None => synced_server.versions.back().cloned(),
        };
        // Diagnostics for a version no longer retained can't be placed; newer ones will follow.
        if synced.is_none() {
            return;
        }
        self.updates
            .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                path,
                server: id,
                diagnostics,
                synced,
            }));
    }
}

impl Drop for LspStore {
    fn drop(&mut self) {
        for server in self.servers.drain().filter_map(|(_, server)| server.server) {
            server.shutdown();
        }
    }
}

enum HoverBlock {
    Markdown(String),
    Code { language: String, text: String },
}

fn combine_hover_contents(contents: HoverContents) -> String {
    let from_marked = |marked: MarkedString| match marked {
        MarkedString::String(text) => HoverBlock::Markdown(text),
        MarkedString::LanguageString(code) => HoverBlock::Code {
            language: code.language,
            text: code.value,
        },
    };
    let blocks: Vec<HoverBlock> = match contents {
        HoverContents::Scalar(marked) => vec![from_marked(marked)],
        HoverContents::Array(marked) => marked.into_iter().map(from_marked).collect(),
        HoverContents::Markup(markup) => match markup.kind {
            MarkupKind::Markdown | MarkupKind::PlainText => {
                vec![HoverBlock::Markdown(markup.value)]
            }
        },
    };
    let mut combined = String::new();
    let mut truncated = false;
    for block in blocks {
        let piece = match block {
            HoverBlock::Markdown(text) => text.trim().to_string(),
            HoverBlock::Code { language, text } => {
                let text = text.trim();
                let fence = code_fence_for(text);
                let language = language.replace(['`', '\r', '\n'], "");
                format!("{fence}{language}\n{text}\n{fence}")
            }
        };
        if piece.is_empty() {
            continue;
        }
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }
        let room = MAX_HOVER_BYTES.saturating_sub(combined.len());
        if piece.len() > room {
            let mut cut = room;
            while !piece.is_char_boundary(cut) {
                cut -= 1;
            }
            combined.push_str(&piece[..cut]);
            truncated = true;
            break;
        }
        combined.push_str(&piece);
    }
    if truncated {
        combined.push_str("\n\n...");
    }
    combined
}

fn code_fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn parse_definitions(
    result: Option<Value>,
    rope: &Rope,
) -> (Option<std::ops::Range<usize>>, Vec<DefinitionTarget>) {
    use lsp_types::GotoDefinitionResponse;
    let response =
        result.and_then(|value| serde_json::from_value::<GotoDefinitionResponse>(value).ok());
    let target = |uri: &Url, range: lsp_types::Range| {
        Some(DefinitionTarget {
            path: uri.to_file_path().ok()?,
            range,
        })
    };
    match response {
        None => (None, Vec::new()),
        Some(GotoDefinitionResponse::Scalar(location)) => (
            None,
            target(&location.uri, location.range).into_iter().collect(),
        ),
        Some(GotoDefinitionResponse::Array(locations)) => (
            None,
            locations
                .iter()
                .filter_map(|location| target(&location.uri, location.range))
                .collect(),
        ),
        Some(GotoDefinitionResponse::Link(links)) => {
            let origin = links.iter().find_map(|link| {
                let range = link.origin_selection_range?;
                Some(position_to_char(rope, range.start)..position_to_char(rope, range.end))
            });
            let targets = links
                .iter()
                .filter_map(|link| target(&link.target_uri, link.target_selection_range))
                .collect();
            (origin, targets)
        }
    }
}

fn sync_kind(capabilities: &ServerCapabilities) -> Option<TextDocumentSyncKind> {
    match capabilities.text_document_sync.as_ref()? {
        TextDocumentSyncCapability::Kind(kind) => Some(*kind),
        TextDocumentSyncCapability::Options(options) => options.change,
    }
}

/// `None` when the server doesn't want to hear about saves; else whether it wants the text with them.
fn save_includes_text(capabilities: &ServerCapabilities) -> Option<bool> {
    match capabilities.text_document_sync.as_ref()? {
        TextDocumentSyncCapability::Options(options) => match options.save.as_ref()? {
            TextDocumentSyncSaveOptions::Supported(true) => Some(false),
            TextDocumentSyncSaveOptions::Supported(false) => None,
            TextDocumentSyncSaveOptions::SaveOptions(options) => {
                Some(options.include_text.unwrap_or(false))
            }
        },
        TextDocumentSyncCapability::Kind(_) => None,
    }
}

fn full_change(buffer: &EditorBuffer) -> Vec<TextDocumentContentChangeEvent> {
    vec![TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: buffer.rope.to_string(),
    }]
}

/// The buffer's edits since `last` as ranged changes, each positioned in the text the ones before it left.
/// `None` if they don't reproduce the buffer (it was replaced wholesale), so the caller sends it all.
fn incremental_changes(
    last: &SyncedText,
    buffer: &EditorBuffer,
) -> Option<Vec<TextDocumentContentChangeEvent>> {
    if buffer.version() < last.buffer_version {
        return None;
    }
    let mut mirror = last.rope.clone();
    let mut changes = Vec::new();
    for (edits, texts) in buffer.text_edits_since(last.buffer_version) {
        // A batch's ranges are all relative to the text before it; applying from the end keeps the earlier
        // ones valid, and the server applies changes in the order sent.
        for (edit, text) in edits.iter().zip(texts).rev() {
            if edit.old.end > mirror.len_chars() {
                return None;
            }
            let range = lsp_types::Range::new(
                char_to_position(&mirror, edit.old.start),
                char_to_position(&mirror, edit.old.end),
            );
            mirror.remove(edit.old.clone());
            mirror.insert(edit.old.start, text);
            changes.push(TextDocumentContentChangeEvent {
                range: Some(range),
                range_length: None,
                text: text.clone(),
            });
        }
    }
    (mirror == buffer.rope).then_some(changes)
}

/// What the client supports, limited to the features it handles.
fn initialize_params(root: &Path, options: Value) -> Value {
    let root_uri = Url::from_directory_path(root).ok();
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({
        "processId": std::process::id(),
        "rootPath": root.to_string_lossy(),
        "rootUri": root_uri,
        "workspaceFolders": root_uri.as_ref().map(|uri| json!([{"uri": uri, "name": name}])),
        "clientInfo": {"name": "Pomelo", "version": env!("CARGO_PKG_VERSION")},
        "initializationOptions": options,
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeConfiguration": {"dynamicRegistration": true},
            },
            "textDocument": {
                "synchronization": {"didSave": true, "dynamicRegistration": true},
                "hover": {"contentFormat": ["markdown"], "dynamicRegistration": true},
                "definition": {"linkSupport": true, "dynamicRegistration": true},
                "declaration": {"linkSupport": true, "dynamicRegistration": true},
                "typeDefinition": {"linkSupport": true, "dynamicRegistration": true},
                "implementation": {"linkSupport": true, "dynamicRegistration": true},
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "resolveSupport": {"properties": ["additionalTextEdits", "detail", "documentation"]},
                        "deprecatedSupport": true,
                        "tagSupport": {"valueSet": [1]},
                        "insertReplaceSupport": true,
                        "labelDetailsSupport": true,
                        "insertTextModeSupport": {"valueSet": [1, 2]},
                        "documentationFormat": ["markdown", "plaintext"],
                    },
                    "insertTextMode": 2,
                    "completionList": {
                        "itemDefaults": ["commitCharacters", "editRange", "insertTextMode", "insertTextFormat", "data"],
                    },
                    "contextSupport": true,
                    "dynamicRegistration": true,
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "versionSupport": true,
                    "dataSupport": true,
                    "tagSupport": {"valueSet": [1, 2]},
                    "codeDescriptionSupport": true,
                },
            },
            "window": {"workDoneProgress": true},
        },
    })
}

/// solargraph stops indexing past its file limit, so nothing resolves; ruby-lsp has no such limit.
fn suggested_switch(name: &str, message: &str) -> Option<ServerSwitch> {
    (name == "solargraph" && message.contains("too large to index")).then_some(ServerSwitch {
        language: "Ruby",
        from: "solargraph",
        to: "ruby-lsp",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_running(key: &'static str) -> (LspStore, ServerId) {
        let waker: Waker = std::sync::Arc::new(|| {});
        let mut store =
            LspStore::new(PathBuf::from("/tmp"), waker).with_environment(HashMap::new());
        let id = store.server_id(key, Path::new("/tmp"));
        let (_, found) = channel();
        let mut server = Server::new(
            ServerState::Locating(Locating {
                found,
                fallback: None,
                status: Some(BinaryStatus::Downloading),
                started: Instant::now(),
            }),
            vec!["vtsls"],
            "vtsls",
        );
        server.progress_tokens.insert("1".into());
        store.servers.insert(id, server);
        (store, id)
    }

    #[test]
    fn server_messages_and_questions_reach_the_ui() {
        let (mut store, id) = store_with_running("typescript");
        store.handle_event(
            id,
            ServerEvent::Notification {
                method: "window/showMessage".into(),
                params: json!({"type": 2, "message": "Using the bundled TypeScript"}),
            },
        );
        store.handle_event(
            id,
            ServerEvent::MessageRequest {
                id: json!(4),
                params: json!({"type": 1, "message": "Reload?", "actions": [{"title": "Yes"}, {"title": "No"}]}),
            },
        );
        let messages: Vec<&ServerMessage> = store
            .updates
            .iter()
            .filter_map(|event| match event {
                StoreEvent::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        match messages.as_slice() {
            [told, asked] => {
                assert_eq!(
                    (told.name, told.level, told.request.clone()),
                    ("vtsls", MessageLevel::Warning, None)
                );
                assert!(told.actions.is_empty());
                assert_eq!(asked.level, MessageLevel::Error);
                assert_eq!(asked.actions, ["Yes", "No"]);
                assert_eq!(asked.request, Some(json!(4)));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn log_and_show_messages_land_in_the_servers_log() {
        let (mut store, id) = store_with_running("typescript");
        for (method, kind, message) in [
            ("window/logMessage", 4, "indexing"),
            ("window/showMessage", 1, "crashed"),
        ] {
            store.handle_event(
                id,
                ServerEvent::Notification {
                    method: method.into(),
                    params: json!({"type": kind, "message": message}),
                },
            );
        }
        let (lines, seen) = store.server_log(id, 0).expect("log");
        assert_eq!(lines, ["[LOG] indexing", "[ERROR] crashed"]);
        assert_eq!(
            store.server_log(id, seen).map(|(lines, _)| lines.len()),
            Some(0)
        );
    }

    #[test]
    fn a_workspace_too_large_for_solargraph_suggests_ruby_lsp() {
        let message = "The workspace is too large to index (5716 files, 5000 max)";
        assert_eq!(
            suggested_switch("solargraph", message).map(|switch| switch.to),
            Some("ruby-lsp")
        );
        assert_eq!(suggested_switch("solargraph", "Indexing"), None);
        assert_eq!(suggested_switch("vtsls", message), None);
    }

    #[test]
    fn progress_is_kept_from_begin_to_end_for_created_tokens() {
        let (mut store, id) = store_with_running("typescript");
        store.receive_progress(
            id,
            &json!({"token": 1, "value": {"kind": "begin", "title": "Indexing", "percentage": 5}}),
        );
        store.receive_progress(
            id,
            &json!({"token": "other", "value": {"kind": "begin", "title": "Ignored"}}),
        );
        let work = store.work();
        assert_eq!(work.len(), 1, "a token the server never created is ignored");
        assert_eq!(
            (work[0].title.as_str(), work[0].percentage),
            ("Indexing", Some(5))
        );
        store.receive_progress(id, &json!({"token": 1, "value": {"kind": "end"}}));
        assert!(store.work().is_empty());
    }

    #[test]
    fn a_stopped_server_stays_stopped_until_restarted() {
        let (mut store, id) = store_with_running("typescript");
        assert_eq!(store.servers()[0].status, ServerStatus::Downloading);
        store.stop_server(id);
        let servers = store.servers();
        assert_eq!(
            (servers[0].name, servers[0].status),
            ("vtsls", ServerStatus::Stopped)
        );
        store.restart_server(id);
        assert!(
            store.servers().is_empty(),
            "it looks for its binary again on the next sync"
        );
    }

    #[test]
    fn a_request_to_several_servers_answers_once_with_all_of_them() {
        let (mut store, first) = store_with_running("typescript");
        let second = store.server_id("tailwindcss", Path::new("/tmp"));
        let synced = SyncedText {
            lsp_version: 0,
            buffer_version: 0,
            rope: Rope::from_str("const a = 1;\n"),
        };
        store.calls.insert((first, 7), 1);
        store.calls.insert((second, 3), 1);
        store.pending.insert(
            1,
            Pending {
                gathered: Gathered::Hover {
                    markdown: Vec::new(),
                    range: None,
                },
                waiting: 2,
                synced,
                started: Instant::now(),
            },
        );
        store.answered(first, 7, Some(json!({"contents": "a number"})));
        assert!(store.updates.is_empty(), "waits for the other server");
        store.answered(second, 3, Some(json!({"contents": "a class"})));
        match store.updates.as_slice() {
            [StoreEvent::Hover(hover)] => assert_eq!(
                hover.markdown.as_deref(),
                Some("a number\n\n---\n\na class")
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_server_keeps_its_own_diagnostics_for_a_file() {
        let (mut store, first) = store_with_running("typescript");
        let second = store.server_id("tailwindcss", Path::new("/tmp"));
        let publish = |store: &mut LspStore, id: ServerId, count: usize| {
            let diagnostics = (0..count)
                .map(|line| Diagnostic {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(line as u32, 0),
                        lsp_types::Position::new(line as u32, 1),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: "bad".into(),
                    ..Diagnostic::default()
                })
                .collect();
            store.receive_diagnostics(
                id,
                PublishDiagnosticsParams {
                    uri: Url::from_file_path("/tmp/a.ts").unwrap(),
                    diagnostics,
                    version: None,
                },
            );
        };
        publish(&mut store, first, 2);
        publish(&mut store, second, 1);
        assert_eq!(store.diagnostic_summary(), (3, 0));
        publish(&mut store, first, 0);
        assert_eq!(
            store.diagnostic_summary(),
            (1, 0),
            "only the first server's went"
        );
        assert_eq!(store.all_diagnostics().1[0].1.len(), 1);
    }

    #[test]
    fn a_manifest_picks_the_folder_a_server_runs_in() {
        let workspace = tempfile::tempdir().expect("temp");
        let root = workspace.path();
        std::fs::create_dir_all(root.join("api/app/models")).unwrap();
        std::fs::create_dir_all(root.join("web/apps/portal/src")).unwrap();
        std::fs::write(root.join("api/Gemfile"), "").unwrap();
        std::fs::write(root.join("web/package.json"), "{}").unwrap();
        std::fs::write(root.join("web/apps/portal/package.json"), "{}").unwrap();
        let gemfile = crate::adapters::Manifest {
            names: &["Gemfile"],
            outermost: false,
        };
        let node = crate::adapters::Manifest {
            names: &["package.json"],
            outermost: true,
        };
        assert_eq!(
            gemfile.root_for(&root.join("api/app/models/user.rb"), root),
            root.join("api")
        );
        assert_eq!(
            node.root_for(&root.join("web/apps/portal/src/a.tsx"), root),
            root.join("web"),
            "one server for the monorepo"
        );
        assert_eq!(
            gemfile.root_for(&root.join("web/x.rb"), root),
            root.to_path_buf()
        );
    }

    fn synced(buffer: &EditorBuffer) -> SyncedText {
        SyncedText {
            lsp_version: 0,
            buffer_version: buffer.version(),
            rope: buffer.rope.clone(),
        }
    }

    fn apply(text: &str, changes: &[TextDocumentContentChangeEvent]) -> String {
        let mut rope = Rope::from_str(text);
        for change in changes {
            let Some(range) = change.range else {
                rope = Rope::from_str(&change.text);
                continue;
            };
            let start = crate::position_to_char(&rope, range.start);
            let end = crate::position_to_char(&rope, range.end);
            rope.remove(start..end);
            rope.insert(start, &change.text);
        }
        rope.to_string()
    }

    #[test]
    fn incremental_changes_replay_to_the_buffer() {
        let mut buffer = EditorBuffer::from_text("fn main() {\n    let x = 1;\n}\n");
        let last = synced(&buffer);
        buffer.place_cursor(16);
        buffer.add_cursor(0);
        buffer.insert_text("é");
        buffer.place_cursor(buffer.rope.len_chars());
        buffer.insert_text("// end\n");
        buffer.undo();
        buffer.insert_text("x");
        let changes = incremental_changes(&last, &buffer).unwrap();
        assert!(changes.iter().all(|c| c.range.is_some()));
        assert_eq!(apply(&last.rope.to_string(), &changes), buffer.text());
    }

    #[test]
    fn a_replaced_buffer_needs_the_full_text() {
        let buffer = EditorBuffer::from_text("abc");
        let last = SyncedText {
            lsp_version: 3,
            buffer_version: 7,
            rope: Rope::from_str("xyz"),
        };
        assert!(incremental_changes(&last, &buffer).is_none());
    }

    #[test]
    fn hover_contents_become_one_markdown_document() {
        let contents = HoverContents::Array(vec![
            MarkedString::LanguageString(lsp_types::LanguageString {
                language: "rust".into(),
                value: "fn a() -> u8".into(),
            }),
            MarkedString::String("  Doc text.  ".into()),
        ]);
        assert_eq!(
            combine_hover_contents(contents),
            "```rust\nfn a() -> u8\n```\n\nDoc text."
        );
        assert_eq!(code_fence_for("uses ``` inside"), "````");
    }

    #[test]
    fn definitions_come_from_locations_or_links() {
        let rope = Rope::from_str("let a = b;\n");
        let links = json!([{
            "originSelectionRange": {"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 9}},
            "targetUri": "file:///tmp/x.rs",
            "targetRange": {"start": {"line": 3, "character": 0}, "end": {"line": 5, "character": 1}},
            "targetSelectionRange": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 5}},
        }]);
        let (origin, targets) = parse_definitions(Some(links), &rope);
        assert_eq!(origin, Some(8..9));
        assert_eq!(targets[0].path, PathBuf::from("/tmp/x.rs"));
        assert_eq!(targets[0].range.start.character, 4);
        let location = json!({"uri": "file:///tmp/y.rs", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}});
        let (origin, targets) = parse_definitions(Some(location), &rope);
        assert_eq!((origin, targets.len()), (None, 1));
        assert!(parse_definitions(Some(Value::Null), &rope).1.is_empty());
    }

    #[test]
    fn save_text_follows_the_server_options() {
        let mut capabilities = ServerCapabilities::default();
        assert_eq!(save_includes_text(&capabilities), None);
        capabilities.text_document_sync = Some(TextDocumentSyncCapability::Options(
            lsp_types::TextDocumentSyncOptions {
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(
                    lsp_types::SaveOptions {
                        include_text: Some(true),
                    },
                )),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                ..Default::default()
            },
        ));
        assert_eq!(save_includes_text(&capabilities), Some(true));
        assert_eq!(
            sync_kind(&capabilities),
            Some(TextDocumentSyncKind::INCREMENTAL)
        );
    }

    /// Needs clangd on the login shell's PATH: `cargo test -p lsp -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_server_reports_an_error_and_clears_it_after_a_fix() {
        let root = std::env::temp_dir().join(format!("pomelo-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.c");
        let text = "int main(void) {\n    return missing;\n}\n";
        std::fs::write(&path, text).unwrap();
        let env = crate::capture_login_env(&root).unwrap();
        let mut store =
            LspStore::new(root.clone(), std::sync::Arc::new(|| {})).with_environment(env);
        let mut buffer = EditorBuffer::from_text(text);
        let wait_for =
            |store: &mut LspStore, buffer: &EditorBuffer, want: &dyn Fn(&[Diagnostic]) -> bool| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                while std::time::Instant::now() < deadline {
                    store.sync_document(&path, Lang::C, buffer);
                    for event in store.poll() {
                        let StoreEvent::Diagnostics(update) = event else {
                            continue;
                        };
                        if update.path == path && want(&update.diagnostics) {
                            return true;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                false
            };
        let has_error = |d: &[Diagnostic]| {
            d.iter()
                .any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
        };
        assert!(wait_for(&mut store, &buffer, &has_error));
        let token = store
            .hover(&path, Lang::C, &buffer, text.find("main").unwrap() + 1)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut answer = None;
        while answer.is_none() && std::time::Instant::now() < deadline {
            answer = store.poll().into_iter().find_map(|event| match event {
                StoreEvent::Hover(hover) if hover.request == token => Some(hover),
                _ => None,
            });
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let answer = answer.unwrap();
        assert!(answer.markdown.unwrap().contains("main"));
        assert_eq!(answer.range, Some(4..8));
        let at = buffer.text().find("missing").unwrap();
        buffer.place_cursor(at);
        buffer.extend_cursor(at + "missing".len());
        buffer.insert_text("0");
        assert!(wait_for(&mut store, &buffer, &|d| !has_error(d)));
        drop(store);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore]
    fn a_real_server_completes_a_keyword() {
        let root = std::env::temp_dir().join(format!("pomelo-lsp-complete-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.c");
        let text = "int main(void) {\n    ret\n}\n";
        std::fs::write(&path, text).unwrap();
        let env = crate::capture_login_env(&root).unwrap();
        let mut store =
            LspStore::new(root.clone(), std::sync::Arc::new(|| {})).with_environment(env);
        let buffer = EditorBuffer::from_text(text);
        let offset = text.find("ret").unwrap() + 3;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut token = None;
        let mut answer = None;
        while answer.is_none() && std::time::Instant::now() < deadline {
            if token.is_none() {
                token = store.completion(&path, Lang::C, &buffer, offset, None);
            }
            for event in store.poll() {
                if let StoreEvent::Completions(response) = event {
                    if Some(response.request) == token {
                        // The first answer can come before the server has parsed the file.
                        if response.items.is_empty() {
                            token = None;
                            std::thread::sleep(std::time::Duration::from_millis(200));
                        } else {
                            answer = Some(response);
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let answer = answer.unwrap();
        let item = answer
            .items
            .iter()
            .find(|item| item.filter_text == "return")
            .unwrap();
        assert_eq!(item.replace_range, offset - 3..offset);
        drop(store);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore]
    fn a_real_server_finds_a_definition() {
        let root =
            std::env::temp_dir().join(format!("pomelo-lsp-definition-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.c");
        let text = "int helper(void) { return 0; }\nint main(void) { return helper(); }\n";
        std::fs::write(&path, text).unwrap();
        let env = crate::capture_login_env(&root).unwrap();
        let mut store =
            LspStore::new(root.clone(), std::sync::Arc::new(|| {})).with_environment(env);
        let buffer = EditorBuffer::from_text(text);
        let offset = text.rfind("helper").unwrap() + 2;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut token = None;
        let mut answer = None;
        while answer.is_none() && std::time::Instant::now() < deadline {
            if token.is_none() {
                token =
                    store.definitions(&path, Lang::C, &buffer, offset, DefinitionKind::Definition);
            }
            for event in store.poll() {
                if let StoreEvent::Definitions(response) = event {
                    if Some(response.request) == token {
                        answer = Some(response);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let answer = answer.unwrap();
        let target = answer.targets.first().unwrap();
        assert_eq!(target.path.file_name(), path.file_name());
        assert_eq!(
            (target.range.start.line, target.range.start.character),
            (0, 4)
        );
        drop(store);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
