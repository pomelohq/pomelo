//! The language servers of one project folder: each started on first need (after the login-shell environment
//! is known), sharing one server per adapter, with every open document mirrored to its server and the
//! diagnostics it publishes handed back per file.

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

use crate::adapters::adapter_for;
use crate::install::{BinaryStatus, Found, Located};
use crate::position::{char_to_position, position_to_char};
use crate::{LanguageServer, ServerEvent, Waker};

/// Versions of a document kept after sending them, so diagnostics a server computed for an older version can
/// still be placed.
const RETAINED_VERSIONS: usize = 10;

/// A document's text as sent to its server under `lsp_version`, and the buffer version it matched.
#[derive(Clone, Debug)]
pub struct SyncedText {
    pub lsp_version: i32,
    pub buffer_version: u64,
    pub rope: Rope,
}

/// A file's latest diagnostics, with the text their positions refer to when the file is open.
#[derive(Clone, Debug)]
pub struct DiagnosticsUpdate {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub synced: Option<SyncedText>,
}

#[derive(Clone, Debug)]
pub enum StoreEvent {
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
    pub key: &'static str,
    pub name: &'static str,
    pub status: ServerStatus,
    pub message: Option<String>,
    pub version: Option<String>,
    pub binary: Option<Located>,
    pub process_id: Option<u32>,
}

/// Work a server reports progress on (`$/progress`).
#[derive(Clone, Debug)]
pub struct ServerWork {
    pub server: &'static str,
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

struct Document {
    uri: Url,
    adapter: &'static str,
    /// Oldest first; the last is what the server has now.
    versions: VecDeque<SyncedText>,
}

pub struct LspStore {
    root: PathBuf,
    waker: Waker,
    environment: Environment,
    servers: HashMap<&'static str, Server>,
    unavailable: HashMap<&'static str, Unavailable>,
    /// Servers stopped from the menu, by adapter, with the name they had; started again only on restart.
    stopped: HashMap<&'static str, &'static str>,
    documents: HashMap<PathBuf, Document>,
    /// Diagnostics for files that aren't open, delivered when they are.
    unopened_diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    updates: Vec<StoreEvent>,
    hovers: HashMap<(&'static str, i64), (u64, SyncedText)>,
    completions: HashMap<(&'static str, i64), (u64, SyncedText, usize)>,
    summaries: HashMap<PathBuf, (&'static str, usize, usize)>,
    resolves: HashMap<(&'static str, i64), (u64, SyncedText)>,
    definitions: HashMap<(&'static str, i64), (u64, SyncedText)>,
    next_request: u64,
}

impl LspStore {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self {
            root,
            waker,
            environment: Environment::Unrequested,
            servers: HashMap::new(),
            unavailable: HashMap::new(),
            stopped: HashMap::new(),
            documents: HashMap::new(),
            unopened_diagnostics: HashMap::new(),
            updates: Vec::new(),
            hovers: HashMap::new(),
            completions: HashMap::new(),
            summaries: HashMap::new(),
            resolves: HashMap::new(),
            definitions: HashMap::new(),
            next_request: 0,
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

    /// Start the adapter's server if it isn't running (finding its binary off-thread first); `true` once it
    /// is initialized.
    fn ensure_server(&mut self, adapter: crate::Adapter) -> bool {
        let candidates: Vec<&'static str> = adapter
            .candidates
            .iter()
            .map(|candidate| candidate.name)
            .collect();
        if self.stopped.contains_key(adapter.name)
            || self
                .unavailable
                .get(adapter.name)
                .is_some_and(|unavailable| unavailable.candidates == candidates)
        {
            return false;
        }
        self.unavailable.remove(adapter.name);
        if self
            .servers
            .get(adapter.name)
            .is_some_and(|server| server.candidates != candidates)
        {
            self.drop_server(adapter.name);
        }
        let located = match self.servers.get_mut(adapter.name) {
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
                    adapter.name,
                    Server::new(ServerState::Locating(locating), candidates, first),
                );
                return false;
            }
        };
        let located = match located {
            Ok(located) => located,
            Err(message) => {
                eprintln!("lsp: {}: {message}", adapter.name);
                self.fail(adapter.name, message);
                return false;
            }
        };
        let Some(env) = self.environment().cloned() else {
            return false;
        };
        let mut server = match LanguageServer::spawn(
            located.name,
            &located.binary,
            &located.args,
            &self.root,
            &env,
            self.waker.clone(),
        ) {
            Ok(server) => server,
            Err(error) => {
                let message = format!("could not start {}: {error}", located.binary.display());
                eprintln!("lsp: {message}");
                self.fail(adapter.name, message);
                return false;
            }
        };
        server.set_configuration(crate::adapters::workspace_configuration(
            adapter.name,
            &self.root,
        ));
        let initialize_id = server.request("initialize", initialize_params(&self.root));
        if let Some(entry) = self.servers.get_mut(adapter.name) {
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

    fn fail(&mut self, key: &'static str, message: String) {
        let (candidates, name) = self
            .servers
            .get(key)
            .map(|server| (server.candidates.clone(), server.name))
            .unwrap_or((Vec::new(), key));
        self.drop_server(key);
        self.unavailable.insert(
            key,
            Unavailable {
                candidates,
                name,
                message,
            },
        );
    }

    /// Keep `path`'s document open on its language's server and in step with `buffer`: opened once the
    /// server is up, then sent each change (just the edited ranges when the server accepts that).
    pub fn sync_document(&mut self, path: &Path, lang: Lang, buffer: &EditorBuffer) {
        let Some((adapter, language_id)) = adapter_for(lang) else {
            if self.documents.contains_key(path) {
                self.close_document(path);
                self.clear_diagnostics(path.to_path_buf());
            }
            return;
        };
        if !self.ensure_server(adapter.clone()) {
            return;
        }
        let Some(Server {
            server: Some(server),
            state: ServerState::Running { capabilities },
            ..
        }) = self.servers.get_mut(adapter.name)
        else {
            return;
        };
        let Some(document) = self.documents.get_mut(path) else {
            let Ok(uri) = Url::from_file_path(path) else {
                return;
            };
            server.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 0,
                    "text": buffer.rope.to_string(),
                }}),
            );
            let synced = SyncedText {
                lsp_version: 0,
                buffer_version: buffer.version(),
                rope: buffer.rope.clone(),
            };
            if let Some(diagnostics) = self.unopened_diagnostics.remove(path) {
                self.updates
                    .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                        path: path.to_path_buf(),
                        diagnostics,
                        synced: Some(synced.clone()),
                    }));
            }
            self.documents.insert(
                path.to_path_buf(),
                Document {
                    uri,
                    adapter: adapter.name,
                    versions: VecDeque::from([synced]),
                },
            );
            return;
        };
        let Some(last) = document.versions.back() else {
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
        document.versions.push_back(SyncedText {
            lsp_version: version,
            buffer_version: buffer.version(),
            rope: buffer.rope.clone(),
        });
        while document.versions.len() > RETAINED_VERSIONS {
            document.versions.pop_front();
        }
    }

    /// Tell the server `path` was saved, with its text if it asked for that.
    pub fn did_save(&mut self, path: &Path, buffer: &EditorBuffer) {
        let Some(document) = self.documents.get(path) else {
            return;
        };
        let Some(Server {
            server: Some(server),
            state: ServerState::Running { capabilities },
            ..
        }) = self.servers.get_mut(document.adapter)
        else {
            return;
        };
        let Some(include_text) = save_includes_text(capabilities) else {
            return;
        };
        let mut params = json!({"textDocument": {"uri": document.uri}});
        if include_text {
            params["text"] = Value::String(buffer.rope.to_string());
        }
        server.notify("textDocument/didSave", params);
    }

    pub fn close_document(&mut self, path: &Path) {
        let Some(document) = self.documents.remove(path) else {
            return;
        };
        if let Some(server) = self
            .servers
            .get_mut(document.adapter)
            .and_then(|server| server.server.as_mut())
        {
            server.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": document.uri}}),
            );
        }
    }

    pub fn hover(
        &mut self,
        path: &Path,
        lang: Lang,
        buffer: &EditorBuffer,
        offset: usize,
    ) -> Option<u64> {
        self.sync_document(path, lang, buffer);
        let document = self.documents.get(path)?;
        let synced = document.versions.back()?.clone();
        if synced.buffer_version != buffer.version() {
            return None;
        }
        let Some(Server {
            server: Some(server),
            state: ServerState::Running { capabilities },
            ..
        }) = self.servers.get_mut(document.adapter)
        else {
            return None;
        };
        let supported = matches!(
            capabilities.hover_provider,
            Some(HoverProviderCapability::Simple(true) | HoverProviderCapability::Options(_))
        );
        if !supported {
            return None;
        }
        let id = server.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": document.uri},
                "position": char_to_position(&synced.rope, offset),
            }),
        );
        let request = self.next_request;
        self.next_request += 1;
        self.hovers
            .insert((document.adapter, id), (request, synced));
        Some(request)
    }

    fn document_server(
        &mut self,
        path: &Path,
    ) -> Option<(&mut LanguageServer, &ServerCapabilities, &Document)> {
        let document = self.documents.get(path)?;
        match self.servers.get_mut(document.adapter) {
            Some(Server {
                server: Some(server),
                state: ServerState::Running { capabilities },
                ..
            }) => Some((server, capabilities, document)),
            _ => None,
        }
    }

    /// `None` when no running server completes `path`; else the characters that ask it to without a word.
    pub fn completion_triggers(&mut self, path: &Path) -> Option<Vec<String>> {
        let (_, capabilities, _) = self.document_server(path)?;
        let provider = capabilities.completion_provider.as_ref()?;
        Some(provider.trigger_characters.clone().unwrap_or_default())
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
        if !crate::adapters::choice_for(lang).completions {
            return None;
        }
        let (server, capabilities, document) = self.document_server(path)?;
        let triggers = capabilities
            .completion_provider
            .as_ref()?
            .trigger_characters
            .clone()
            .unwrap_or_default();
        let synced = document.versions.back()?.clone();
        if synced.buffer_version != buffer.version() {
            return None;
        }
        let context = match trigger.filter(|t| triggers.iter().any(|known| known == t)) {
            Some(character) => json!({"triggerKind": 2, "triggerCharacter": character}),
            None => json!({"triggerKind": 1}),
        };
        let adapter = document.adapter;
        let id = server.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": document.uri},
                "position": char_to_position(&synced.rope, offset),
                "context": context,
            }),
        );
        let request = self.next_request;
        self.next_request += 1;
        self.completions
            .insert((adapter, id), (request, synced, offset));
        Some(request)
    }

    pub fn resolve_completion(&mut self, path: &Path, raw: &Value) -> Option<u64> {
        let (server, capabilities, document) = self.document_server(path)?;
        let resolves = capabilities
            .completion_provider
            .as_ref()?
            .resolve_provider
            .unwrap_or(false);
        if !resolves {
            return None;
        }
        let synced = document.versions.back()?.clone();
        let adapter = document.adapter;
        let id = server.request("completionItem/resolve", raw.clone());
        let request = self.next_request;
        self.next_request += 1;
        self.resolves.insert((adapter, id), (request, synced));
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
        let (server, capabilities, document) = self.document_server(path)?;
        let supported = match kind {
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
        let synced = document.versions.back()?.clone();
        if !supported || synced.buffer_version != buffer.version() {
            return None;
        }
        let adapter = document.adapter;
        let id = server.request(
            kind.method(),
            json!({
                "textDocument": {"uri": document.uri},
                "position": char_to_position(&synced.rope, offset),
            }),
        );
        let request = self.next_request;
        self.next_request += 1;
        self.definitions.insert((adapter, id), (request, synced));
        Some(request)
    }

    pub fn poll(&mut self) -> Vec<StoreEvent> {
        let names: Vec<&'static str> = self.servers.keys().copied().collect();
        for name in names {
            let events = match self
                .servers
                .get_mut(name)
                .and_then(|server| server.server.as_mut())
            {
                Some(server) => server.poll(),
                None => continue,
            };
            for event in events {
                self.handle_event(name, event);
            }
        }
        std::mem::take(&mut self.updates)
    }

    fn handle_event(&mut self, name: &'static str, event: ServerEvent) {
        match event {
            ServerEvent::Response { id, result, .. } => {
                if let Some((request, synced, offset)) = self.completions.remove(&(name, id)) {
                    let (items, is_incomplete) = crate::completion::parse_completions(
                        result.unwrap_or(Value::Null),
                        &synced.rope,
                        offset,
                    );
                    self.updates
                        .push(StoreEvent::Completions(crate::CompletionsResponse {
                            request,
                            items,
                            is_incomplete,
                            synced,
                        }));
                    return;
                }
                if let Some((request, synced)) = self.definitions.remove(&(name, id)) {
                    let (origin, targets) = parse_definitions(result.ok(), &synced.rope);
                    self.updates
                        .push(StoreEvent::Definitions(DefinitionsResponse {
                            request,
                            origin,
                            targets,
                            synced,
                        }));
                    return;
                }
                if let Some((request, synced)) = self.resolves.remove(&(name, id)) {
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
                if let Some((request, synced)) = self.hovers.remove(&(name, id)) {
                    let hover = result
                        .ok()
                        .and_then(|value| serde_json::from_value::<Option<Hover>>(value).ok())
                        .flatten();
                    let range = hover.as_ref().and_then(|hover| hover.range).map(|range| {
                        position_to_char(&synced.rope, range.start)
                            ..position_to_char(&synced.rope, range.end)
                    });
                    let markdown = hover
                        .map(|hover| combine_hover_contents(hover.contents))
                        .filter(|markdown| !markdown.trim().is_empty());
                    self.updates.push(StoreEvent::Hover(HoverResponse {
                        request,
                        markdown,
                        range,
                        synced,
                    }));
                    return;
                }
                let Some(server) = self.servers.get_mut(name) else {
                    return;
                };
                let ServerState::Starting { initialize_id } = server.state else {
                    return;
                };
                if id != initialize_id {
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
                        eprintln!("lsp: {name} failed to initialize");
                        self.server_gone(name, "failed to initialize");
                    }
                }
            }
            ServerEvent::Notification { method, params } => {
                if method == "textDocument/publishDiagnostics" {
                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                        Ok(params) => self.receive_diagnostics(name, params),
                        Err(error) => eprintln!("lsp: bad diagnostics from {name}: {error}"),
                    }
                } else if method == "$/progress" {
                    self.receive_progress(name, &params);
                }
            }
            ServerEvent::ProgressCreated { token } => {
                if let Some(server) = self.servers.get_mut(name) {
                    server.progress_tokens.insert(token);
                }
            }
            ServerEvent::Exited => {
                eprintln!("lsp: {name} exited");
                self.server_gone(name, "the server exited");
            }
        }
    }

    /// A server that exited or failed: dropped with the diagnostics it had shown, and not retried.
    fn server_gone(&mut self, name: &'static str, message: &str) {
        self.fail(name, message.to_string());
    }

    /// Stop a server and forget its documents and diagnostics; its files open on it again on their next sync.
    fn drop_server(&mut self, name: &'static str) {
        if let Some(server) = self.servers.remove(name).and_then(|server| server.server) {
            server.shutdown();
        }
        self.summaries.retain(|_, (adapter, _, _)| *adapter != name);
        let closed: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|(_, document)| document.adapter == name)
            .map(|(path, _)| path.clone())
            .collect();
        for path in closed {
            self.documents.remove(&path);
            self.clear_diagnostics(path);
        }
    }

    /// Every server this folder has started, stopped or failed to find, by name.
    pub fn servers(&self) -> Vec<ServerSummary> {
        let mut summaries: Vec<ServerSummary> = self
            .servers
            .iter()
            .map(|(key, server)| ServerSummary {
                key,
                name: server.name,
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
        summaries.extend(self.stopped.iter().map(|(key, name)| ServerSummary {
            key,
            name,
            status: ServerStatus::Stopped,
            message: None,
            version: None,
            binary: None,
            process_id: None,
        }));
        summaries.extend(
            self.unavailable
                .iter()
                .map(|(key, unavailable)| ServerSummary {
                    key,
                    name: unavailable.name,
                    status: ServerStatus::Failed,
                    message: Some(unavailable.message.clone()),
                    version: None,
                    binary: None,
                    process_id: None,
                }),
        );
        summaries.sort_by_key(|summary| summary.name);
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
    pub fn restart_server(&mut self, key: &str) {
        let Some(key) = self.known_key(key) else {
            return;
        };
        self.drop_server(key);
        self.stopped.remove(key);
        self.unavailable.remove(key);
        (self.waker)();
    }

    pub fn stop_server(&mut self, key: &str) {
        let Some(key) = self.known_key(key) else {
            return;
        };
        let name = self.servers.get(key).map_or(key, |server| server.name);
        self.drop_server(key);
        self.unavailable.remove(key);
        self.stopped.insert(key, name);
    }

    pub fn restart_all(&mut self) {
        for key in self.known_keys() {
            self.restart_server(key);
        }
    }

    pub fn stop_all(&mut self) {
        for key in self.known_keys() {
            if !self.stopped.contains_key(key) {
                self.stop_server(key);
            }
        }
    }

    /// Ask a server to cancel work it said can be (`window/workDoneProgress/cancel`).
    pub fn cancel_work(&mut self, key: &str, token: &str) {
        let Some(server) = self
            .servers
            .iter_mut()
            .find(|(name, _)| **name == key)
            .map(|(_, server)| server)
        else {
            return;
        };
        if let Some(language_server) = server.server.as_mut() {
            language_server.notify("window/workDoneProgress/cancel", json!({"token": token}));
        }
    }

    fn known_keys(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = self
            .servers
            .keys()
            .chain(self.stopped.keys())
            .chain(self.unavailable.keys())
            .copied()
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn known_key(&self, key: &str) -> Option<&'static str> {
        self.known_keys().into_iter().find(|known| *known == key)
    }

    /// Track a server's `$/progress`: begun, reported on (throttled) and ended, for tokens it created.
    fn receive_progress(&mut self, name: &'static str, params: &Value) {
        let Some(server) = self.servers.get_mut(name) else {
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
                        server: name,
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

    fn clear_diagnostics(&mut self, path: PathBuf) {
        self.summaries.remove(&path);
        self.updates
            .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                path,
                diagnostics: Vec::new(),
                synced: None,
            }));
    }

    pub fn diagnostic_summary(&self) -> (usize, usize) {
        self.summaries
            .values()
            .fold((0, 0), |(errors, warnings), (_, e, w)| {
                (errors + e, warnings + w)
            })
    }

    fn receive_diagnostics(&mut self, name: &'static str, params: PublishDiagnosticsParams) {
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
            self.summaries.remove(&path);
        } else {
            self.summaries
                .insert(path.clone(), (name, errors, warnings));
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
        let Some(document) = self.documents.get(&path) else {
            self.unopened_diagnostics.insert(path, diagnostics);
            return;
        };
        let synced = match params.version {
            Some(version) => document
                .versions
                .iter()
                .find(|v| v.lsp_version == version)
                .cloned(),
            None => document.versions.back().cloned(),
        };
        // Diagnostics for a version no longer retained can't be placed; newer ones will follow.
        if synced.is_none() {
            return;
        }
        self.updates
            .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                path,
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
fn initialize_params(root: &Path) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_running(key: &'static str) -> LspStore {
        let waker: Waker = std::sync::Arc::new(|| {});
        let mut store =
            LspStore::new(PathBuf::from("/tmp"), waker).with_environment(HashMap::new());
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
        store.servers.insert(key, server);
        store
    }

    #[test]
    fn progress_is_kept_from_begin_to_end_for_created_tokens() {
        let mut store = store_with_running("typescript");
        store.receive_progress(
            "typescript",
            &json!({"token": 1, "value": {"kind": "begin", "title": "Indexing", "percentage": 5}}),
        );
        store.receive_progress(
            "typescript",
            &json!({"token": "other", "value": {"kind": "begin", "title": "Ignored"}}),
        );
        let work = store.work();
        assert_eq!(work.len(), 1, "a token the server never created is ignored");
        assert_eq!(
            (work[0].title.as_str(), work[0].percentage),
            ("Indexing", Some(5))
        );
        store.receive_progress("typescript", &json!({"token": 1, "value": {"kind": "end"}}));
        assert!(store.work().is_empty());
    }

    #[test]
    fn a_stopped_server_stays_stopped_until_restarted() {
        let mut store = store_with_running("typescript");
        assert_eq!(store.servers()[0].status, ServerStatus::Downloading);
        store.stop_server("typescript");
        let servers = store.servers();
        assert_eq!(
            (servers[0].name, servers[0].status),
            ("vtsls", ServerStatus::Stopped)
        );
        store.restart_server("typescript");
        assert!(
            store.servers().is_empty(),
            "it looks for its binary again on the next sync"
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
