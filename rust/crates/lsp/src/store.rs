//! The language servers of one project folder: each started on first need (after the login-shell environment
//! is known), sharing one server per adapter, with every open document mirrored to its server and the
//! diagnostics it publishes handed back per file.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use editor::{EditorBuffer, Lang};
use lsp_types::{
    Diagnostic, Hover, HoverContents, HoverProviderCapability, MarkedString, MarkupKind,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncSaveOptions, Url,
};
use ropey::Rope;
use serde_json::{json, Value};

use crate::adapters::adapter_for;
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
}

#[derive(Clone, Debug)]
pub struct HoverResponse {
    pub request: u64,
    pub markdown: Option<String>,
    pub range: Option<std::ops::Range<usize>>,
    pub synced: SyncedText,
}

const MAX_HOVER_BYTES: usize = 100_000;

enum ServerState {
    Locating(Receiver<Option<(PathBuf, Vec<String>)>>),
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
    /// Adapters with no binary on the PATH, or whose server failed; not retried.
    unavailable: HashSet<&'static str>,
    documents: HashMap<PathBuf, Document>,
    /// Diagnostics for files that aren't open, delivered when they are.
    unopened_diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    updates: Vec<StoreEvent>,
    hovers: HashMap<(&'static str, i64), (u64, SyncedText)>,
    completions: HashMap<(&'static str, i64), (u64, SyncedText, usize)>,
    resolves: HashMap<(&'static str, i64), (u64, SyncedText)>,
    next_request: u64,
}

impl LspStore {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self {
            root,
            waker,
            environment: Environment::Unrequested,
            servers: HashMap::new(),
            unavailable: HashSet::new(),
            documents: HashMap::new(),
            unopened_diagnostics: HashMap::new(),
            updates: Vec::new(),
            hovers: HashMap::new(),
            completions: HashMap::new(),
            resolves: HashMap::new(),
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
        if self.unavailable.contains(adapter.name) {
            return false;
        }
        let located = match self.servers.get(adapter.name).map(|server| &server.state) {
            Some(ServerState::Running { .. }) => return true,
            Some(ServerState::Starting { .. }) => return false,
            Some(ServerState::Locating(receiver)) => match receiver.try_recv() {
                Ok(located) => located,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => None,
            },
            None => {
                let Some(env) = self.environment().cloned() else {
                    return false;
                };
                let (sender, receiver) = channel();
                let waker = self.waker.clone();
                let finder = adapter.clone();
                std::thread::spawn(move || {
                    if sender.send(finder.locate(&env)).is_ok() {
                        waker();
                    }
                });
                self.servers.insert(
                    adapter.name,
                    Server {
                        server: None,
                        state: ServerState::Locating(receiver),
                    },
                );
                return false;
            }
        };
        let (Some((binary, args)), Some(env)) = (located, self.environment().cloned()) else {
            self.servers.remove(adapter.name);
            self.unavailable.insert(adapter.name);
            return false;
        };
        let mut server = match LanguageServer::spawn(
            adapter.name,
            &binary,
            &args,
            &self.root,
            &env,
            self.waker.clone(),
        ) {
            Ok(server) => server,
            Err(error) => {
                eprintln!("lsp: could not start {}: {error}", binary.display());
                self.servers.remove(adapter.name);
                self.unavailable.insert(adapter.name);
                return false;
            }
        };
        let initialize_id = server.request("initialize", initialize_params(&self.root));
        self.servers.insert(
            adapter.name,
            Server {
                server: Some(server),
                state: ServerState::Starting { initialize_id },
            },
        );
        false
    }

    /// Keep `path`'s document open on its language's server and in step with `buffer`: opened once the
    /// server is up, then sent each change (just the edited ranges when the server accepts that).
    pub fn sync_document(&mut self, path: &Path, lang: Lang, buffer: &EditorBuffer) {
        let Some((adapter, language_id)) = adapter_for(lang) else {
            return;
        };
        if !self.ensure_server(adapter.clone()) {
            return;
        }
        let Some(Server {
            server: Some(server),
            state: ServerState::Running { capabilities },
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
                if let Some((request, synced)) = self.resolves.remove(&(name, id)) {
                    let additional_edits = result
                        .ok()
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
                let capabilities = result.ok().and_then(|result| {
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
                        self.server_gone(name);
                    }
                }
            }
            ServerEvent::Notification { method, params } => {
                if method == "textDocument/publishDiagnostics" {
                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                        Ok(params) => self.receive_diagnostics(params),
                        Err(error) => eprintln!("lsp: bad diagnostics from {name}: {error}"),
                    }
                }
            }
            ServerEvent::Exited => {
                eprintln!("lsp: {name} exited");
                self.server_gone(name);
            }
        }
    }

    /// Drop a server that exited or failed, and the diagnostics it had shown.
    fn server_gone(&mut self, name: &'static str) {
        self.servers.remove(name);
        self.unavailable.insert(name);
        let closed: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|(_, document)| document.adapter == name)
            .map(|(path, _)| path.clone())
            .collect();
        for path in closed {
            self.documents.remove(&path);
            self.updates
                .push(StoreEvent::Diagnostics(DiagnosticsUpdate {
                    path,
                    diagnostics: Vec::new(),
                    synced: None,
                }));
        }
    }

    fn receive_diagnostics(&mut self, params: PublishDiagnosticsParams) {
        let Ok(path) = params.uri.to_file_path() else {
            return;
        };
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
}
