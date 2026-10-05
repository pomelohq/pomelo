use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pom_db::object_storage::{Listing, ObjectEntry, PrefixStats, PAGE_SIZE};
use pom_db::{
    ConnectError, ConnectErrorKind, Connector, Console, ConsoleKind, Database, Engine, Schema,
    Table, TableKind,
};
use ui::measure;
use workspace::keymap::Action as KeyAction;
use workspace::list_nav::{ListNav, NavMove};
use workspace::persistence::SerializedItem;
use workspace::text_field::TextField;
use workspace::{
    side_panel_base, AgentFix, EditKey, Item, MenuItem, PaneKind, PanelRequest, SidePanelView,
};

use crate::bucket_item::BucketItem;
use crate::console::{console_item, console_item_id, new_console, restore_console};
use crate::failure::{Failure, FailureAction, ACTION_STRIDE};
use crate::keyspace_item::KeyspaceItem;
use crate::menu::{self, Action, Entry, Facts};
use crate::object_item::{object_item_id, ObjectItem};
use crate::tree::{
    bucket_key, database_key, folder_key, group_key, prefix_key, readable, repo_key, table_key,
    Folder, Loaded, Model, Opened, Row, Section, CONSOLES_KEY,
};
use crate::{DatabaseContext, Pending, TableItem};

pub(crate) const ROW_H: f32 = 22.0;
pub(crate) const HEADER_H: f32 = 34.0;
pub(crate) const FILTER_H: f32 = 34.0;
pub(crate) const ROW_STRIDE: u64 = 4;
/// The second click target of a row: its chevron.
pub(crate) const CHEVRON: u64 = 1;
pub(crate) const REFRESH: u64 = 9_000_000;
pub(crate) const NEW_CONSOLE: u64 = REFRESH + 1;
pub(crate) const FILTER: u64 = REFRESH + 2;
pub(crate) const COLLAPSE_ALL: u64 = REFRESH + 3;
pub(crate) const RENAME_FIELD: u64 = REFRESH + 4;
pub(crate) const FAILURE: u64 = REFRESH + 16;
const MENU: u64 = 9_500_000;
pub(crate) const FAILURE_BLOCK_LEFT: f32 = 44.0;
pub(crate) const FAILURE_BLOCK_RIGHT: f32 = 8.0;
pub(crate) const FAILURE_BLOCK_TOP: f32 = 2.0;
pub(crate) const FAILURE_BLOCK_BOTTOM: f32 = 6.0;
const SERVER_START_WAIT: Duration = Duration::from_secs(30);
const CONFIRM_PROMPT: u64 = 1;
const SELECT_LIMIT: usize = 100;
const PRESIGNED_SECONDS: u64 = 3600;

/// What a background action says when it is done, and what to load again after it.
enum Outcome {
    Done {
        message: String,
        reload: Reload,
    },
    Text {
        id: String,
        title: String,
        text: String,
    },
}

#[derive(Clone)]
enum Reload {
    Nothing,
    Database(String),
    Folder {
        database: String,
        bucket: String,
        prefix: String,
    },
}

/// Folder totals by `folder_key`; `None` where counting failed.
type Counted = Vec<(String, Option<PrefixStats>)>;

/// What a database row says under its name and in its tooltip.
#[derive(Clone, Default)]
pub(crate) struct Detail {
    pub subtitle: String,
    pub tooltip: String,
}

pub struct DatabasePanel {
    pub(crate) context: DatabaseContext,
    pub(crate) model: Model,
    pub(crate) rows: Vec<Row>,
    pub(crate) found: usize,
    pub(crate) details: HashMap<String, Detail>,
    scroll: f32,
    viewport_h: f32,
    pub(crate) hover: Option<u64>,
    requests: Vec<PanelRequest>,
    pub(crate) filter: TextField,
    pub(crate) filter_focused: bool,
    /// The console being renamed in place, and its field.
    pub(crate) rename: Option<(String, TextField)>,
    pub(crate) selected: Option<String>,
    /// The tree has the keyboard.
    pub(crate) keyboard: bool,
    menu: Option<(Row, Vec<Entry>)>,
    confirm: Option<(Action, Row)>,
    /// Fixes running for a failed database, by its name; each answers with what to tell the person.
    fixes: HashMap<String, Pending<String>>,
    jobs: Vec<Pending<Outcome>>,
    counting: Vec<Pending<Counted>>,
    pub(crate) width: f32,
}

impl DatabasePanel {
    pub fn new(context: DatabaseContext) -> DatabasePanel {
        let mut panel = DatabasePanel {
            model: Model::default(),
            context,
            rows: Vec::new(),
            found: 0,
            details: HashMap::new(),
            scroll: 0.0,
            viewport_h: 0.0,
            hover: None,
            requests: Vec::new(),
            filter: {
                let mut field = TextField::default();
                field.set_font_size(12.5);
                field
            },
            filter_focused: false,
            rename: None,
            selected: None,
            keyboard: false,
            menu: None,
            confirm: None,
            fixes: HashMap::new(),
            jobs: Vec::new(),
            counting: Vec::new(),
            width: 300.0,
        };
        panel.reload_config();
        panel.load_consoles();
        let first = panel
            .model
            .databases
            .iter()
            .find(|database| database.engine.is_sql())
            .map(|database| (database.repo.clone(), database.name.clone()));
        if let Some((repo, name)) = first {
            panel.toggle_database(&repo, &name);
        }
        panel
    }

    pub(crate) fn base() -> u64 {
        side_panel_base(PaneKind::Database)
    }

    pub(crate) fn id(row: usize) -> u64 {
        Self::base() + row as u64 * ROW_STRIDE
    }

    fn decode(id: u64) -> Option<(usize, u64)> {
        let offset = id.checked_sub(Self::base())?;
        (offset < REFRESH).then_some(((offset / ROW_STRIDE) as usize, offset % ROW_STRIDE))
    }

    fn reload_config(&mut self) {
        self.model.databases = self.context.databases();
        self.model.repos = self.context.repos();
        let details: Option<HashMap<String, Detail>> = self.context.run_now(|connector| {
            self.model
                .databases
                .iter()
                .map(|database| (database.name.clone(), detail(connector, database)))
                .collect()
        });
        self.details = details.unwrap_or_default();
    }

    fn load_consoles(&mut self) {
        let databases: Vec<String> = self
            .context
            .databases()
            .into_iter()
            .map(|database| database.name)
            .collect();
        let branch = self.context.branch.clone();
        self.model.consoles = self
            .context
            .consoles()
            .into_iter()
            .filter(|console| console.kind == ConsoleKind::Query)
            .filter(|console| belongs_to(console, &branch, &databases))
            .collect();
    }

    fn database(&self, name: &str) -> Option<&Database> {
        self.model.database(name)
    }

    fn database_at(&self, index: usize) -> Option<Database> {
        self.model.databases.get(index).cloned()
    }

    fn open_console(&mut self, console: Console) {
        let context = self.context.clone();
        self.requests.push(PanelRequest::Reveal {
            id: console_item_id(&console),
            open: Box::new(move || Some(console_item(context, console))),
        });
    }

    /// A console on the selected database (else the first one that runs queries), with `sql` in it.
    fn create_console(&mut self, database: Option<Database>, title: Option<String>, sql: &str) {
        let queryable =
            |database: &Database| matches!(database.engine, Engine::Postgres | Engine::Redis);
        let Some(database) = database.filter(queryable).or_else(|| {
            self.model
                .databases
                .iter()
                .find(|database| database.engine.is_sql())
                .or_else(|| {
                    self.model
                        .databases
                        .iter()
                        .find(|database| queryable(database))
                })
                .cloned()
        }) else {
            self.toast("No database here runs queries".into());
            return;
        };
        let mut saved = self.context.consoles();
        let mut console = new_console(&self.model.consoles, &database);
        console.workspace = self.context.branch.clone();
        if let Some(title) = title {
            console.title = title;
        }
        console.sql = sql.to_string();
        saved.push(console.clone());
        self.context.save_consoles(&saved);
        self.load_consoles();
        self.model.set_open(CONSOLES_KEY, true);
        self.open_console(console);
    }

    fn selected_database(&self) -> Option<Database> {
        let key = self.selected.as_ref()?;
        let row = self.rows.iter().find(|row| row.key() == *key)?;
        match row {
            Row::Console { console, .. } => self.database(&console.database).cloned(),
            _ => row.database().and_then(|index| self.database_at(index)),
        }
    }

    fn update_console(&mut self, id: &str, change: impl FnOnce(&mut Console)) {
        let mut saved = self.context.consoles();
        if let Some(console) = saved.iter_mut().find(|console| console.id == id) {
            change(console);
            self.context.save_consoles(&saved);
        }
        self.load_consoles();
    }

    fn delete_console(&mut self, id: &str) {
        let mut saved = self.context.consoles();
        saved.retain(|console| console.id != id);
        self.context.save_consoles(&saved);
        self.load_consoles();
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter.set_text(filter);
        self.filter.move_to_end();
        self.filter_changed();
    }

    fn filter_changed(&mut self) {
        self.scroll = 0.0;
        self.model.filter = self.filter.text();
        if self.model.filter.trim().is_empty() {
            return;
        }
        let unloaded: Vec<String> = self
            .model
            .databases
            .iter()
            .filter(|database| database.engine.is_sql())
            .filter(|database| !self.model.loaded.contains_key(&database.name))
            .map(|database| database.name.clone())
            .collect();
        for name in unloaded {
            self.load(&name);
        }
    }

    fn load(&mut self, name: &str) {
        let Some(database) = self.database(name).cloned() else {
            return;
        };
        if !database.engine.browsable() {
            return;
        }
        let engine = database.engine;
        let transport = self.context.objects.clone();
        let pending = self.context.run(move |connector| {
            if database.engine == Engine::Minio {
                return Ok(
                    match connector
                        .object_store(&database.name)
                        .buckets(transport.as_ref())
                    {
                        Ok(buckets) => Opened::Buckets(buckets),
                        Err(raw) => {
                            Opened::Failed(Box::new(storage_failure(connector, &database, raw)))
                        }
                    },
                );
            }
            Ok(open_database(connector, &database))
        });
        self.model
            .loaded
            .insert(name.to_string(), Loaded::Loading(pending));
        if engine == Engine::Minio {
            let stale = format!("{name}\u{1f}");
            self.model.folders.retain(|key, _| !key.starts_with(&stale));
            self.model.stats.retain(|key, _| !key.starts_with(&stale));
        }
    }

    fn load_folder(&mut self, name: &str, bucket: &str, prefix: &str, token: Option<String>) {
        let key = folder_key(name, bucket, prefix);
        if self
            .model
            .folders
            .get(&key)
            .is_some_and(|folder| folder.loading.is_some())
        {
            return;
        }
        let (database, bucket_name, prefix_name, transport) = (
            name.to_string(),
            bucket.to_string(),
            prefix.to_string(),
            self.context.objects.clone(),
        );
        let pending = self.context.run(move |connector| {
            connector.object_store(&database).list(
                transport.as_ref(),
                &bucket_name,
                &prefix_name,
                token.as_deref(),
                PAGE_SIZE,
            )
        });
        self.model.folders.entry(key).or_default().loading = Some(pending);
    }

    fn count_prefixes(&mut self, name: &str, bucket: &str, prefixes: Vec<String>) {
        if prefixes.is_empty() {
            return;
        }
        for prefix in &prefixes {
            self.model
                .stats
                .insert(folder_key(name, bucket, prefix), None);
        }
        let (database, bucket, transport) = (
            name.to_string(),
            bucket.to_string(),
            self.context.objects.clone(),
        );
        self.counting.push(self.context.run(move |connector| {
            let store = connector.object_store(&database);
            Ok(prefixes
                .into_iter()
                .map(|prefix| {
                    let stats = store
                        .prefix_stats(transport.as_ref(), &bucket, &prefix)
                        .ok();
                    (folder_key(&database, &bucket, &prefix), stats)
                })
                .collect())
        }));
    }

    /// Shows these tables for a database as if they had loaded (previews and tests).
    pub fn show_tables(&mut self, database: &str, tables: Vec<Table>) {
        self.show_schema(
            database,
            Schema {
                tables,
                columns: Vec::new(),
            },
        );
    }

    /// Shows this schema for a database as if it had loaded, and opens it (previews and tests).
    pub fn show_schema(&mut self, database: &str, schema: Schema) {
        self.expand_everywhere(database);
        self.model
            .loaded
            .insert(database.to_string(), Loaded::Schema(schema));
    }

    /// Shows these buckets of an object storage as if they had loaded (previews and tests).
    pub fn show_buckets(&mut self, database: &str, buckets: Vec<String>) {
        self.expand_everywhere(database);
        self.model
            .loaded
            .insert(database.to_string(), Loaded::Buckets(buckets));
    }

    /// Shows one listed page of a bucket's prefix, opened (previews and tests).
    pub fn show_folder(&mut self, database: &str, bucket: &str, prefix: &str, listing: Listing) {
        for repo in self.repos_showing(database) {
            let key = if prefix.is_empty() {
                bucket_key(&repo, database, bucket)
            } else {
                prefix_key(&repo, database, bucket, prefix)
            };
            self.model.set_open(&key, true);
        }
        self.model.folders.insert(
            folder_key(database, bucket, prefix),
            Folder::listed(listing),
        );
    }

    pub fn show_prefix_stats(
        &mut self,
        database: &str,
        bucket: &str,
        prefix: &str,
        stats: PrefixStats,
    ) {
        self.model
            .stats
            .insert(folder_key(database, bucket, prefix), Some(stats));
    }

    /// Opens a table's columns wherever the database is listed (previews and tests).
    pub fn expand_table(&mut self, database: &str, table: &Table) {
        for repo in self.repos_showing(database) {
            self.model
                .set_open(&table_key(&repo, database, table), true);
        }
    }

    /// Shows this failure for a database as if opening it had failed (previews and tests).
    pub fn show_failure(&mut self, database: &str, error: ConnectError, main_copy: Option<String>) {
        let repo = self.repo_key_of(database);
        let mut failure = Failure::new(error, repo);
        failure.main_copy = main_copy;
        self.expand_everywhere(database);
        self.model
            .loaded
            .insert(database.to_string(), Loaded::Failed(Box::new(failure)));
    }

    pub fn toggle_full_error(&mut self, database: &str) {
        if let Some(failure) = self.failure_mut(database) {
            failure.raw_open = !failure.raw_open;
        }
    }

    /// Folds a database row without dropping what it loaded.
    pub fn fold(&mut self, database: &str) {
        for repo in self.repos_showing(database) {
            self.model.set_open(&database_key(&repo, database), false);
        }
    }

    /// The click id of the first row whose name is `name` (previews and tests).
    pub fn row_named(&mut self, name: &str) -> Option<u64> {
        self.rebuild_rows();
        self.rows
            .iter()
            .position(|row| crate::render::row_name(self, row).as_deref() == Some(name))
            .map(Self::id)
    }

    fn repos_showing(&self, database: &str) -> Vec<String> {
        match self.database(database) {
            Some(found) if found.is_shared() && found.used_by.is_empty() => {
                vec![crate::tree::OTHER_SERVICES.to_string()]
            }
            Some(found) if found.is_shared() => found.used_by.clone(),
            Some(found) => vec![found.repo.clone()],
            None => Vec::new(),
        }
    }

    fn expand_everywhere(&mut self, database: &str) {
        for repo in self.repos_showing(database) {
            self.model.set_open(&repo_key(&repo), true);
            self.model.set_open(&database_key(&repo, database), true);
        }
    }

    /// The repo's key in pom.yml (its checkout folder), for a repo alias.
    fn repo_folder(&self, alias: &str) -> String {
        (self.context.config)()
            .and_then(|config| {
                config
                    .repos
                    .iter()
                    .find(|(key, dir)| {
                        (dir.alias.is_empty() && key.as_str() == alias) || dir.alias == alias
                    })
                    .map(|(key, _)| key.clone())
            })
            .unwrap_or_default()
    }

    fn repo_key_of(&self, name: &str) -> String {
        match self.database(name) {
            Some(database) if !database.is_shared() => self.repo_folder(&database.repo),
            _ => String::new(),
        }
    }

    pub(crate) fn failure(&self, name: &str) -> Option<&Failure> {
        match self.model.loaded.get(name) {
            Some(Loaded::Failed(failure)) => Some(failure),
            _ => None,
        }
    }

    fn failure_mut(&mut self, name: &str) -> Option<&mut Failure> {
        match self.model.loaded.get_mut(name) {
            Some(Loaded::Failed(failure)) => Some(failure),
            _ => None,
        }
    }

    pub(crate) fn failure_id(index: usize, action: FailureAction) -> u64 {
        Self::base() + FAILURE + index as u64 * ACTION_STRIDE + action.offset()
    }

    fn decode_failure(id: u64) -> Option<(usize, FailureAction)> {
        let offset = id.checked_sub(Self::base() + FAILURE)?;
        if offset >= MENU - FAILURE {
            return None;
        }
        let action = FailureAction::from_offset(offset % ACTION_STRIDE)?;
        Some(((offset / ACTION_STRIDE) as usize, action))
    }

    fn checkout(&self, repo: &str) -> PathBuf {
        let checkout = self.context.workspace_root.join(repo);
        if !repo.is_empty() && checkout.is_dir() {
            checkout
        } else {
            self.context.workspace_root.clone()
        }
    }

    fn toast(&mut self, message: String) {
        self.requests.push(PanelRequest::Toast(message));
    }

    fn copy(&mut self, text: String, message: String) {
        self.requests.push(PanelRequest::Copy(text));
        self.toast(message);
    }

    fn failure_action(&mut self, index: usize, action: FailureAction) {
        let Some(database) = self.database_at(index) else {
            return;
        };
        let name = database.name.clone();
        let Some(failure) = self.failure(&name) else {
            return;
        };
        let (repo, main_copy, report, prompt) = (
            failure.repo.clone(),
            failure.main_copy.clone(),
            failure.report(),
            failure.agent_prompt(),
        );
        let place = readable(&database);
        match action {
            FailureAction::ToggleRaw => self.toggle_full_error(&name),
            FailureAction::CopyError => self.copy(report, "Copied the error".into()),
            FailureAction::FixWithAgent => {
                self.requests.push(PanelRequest::FixWithAgent(AgentFix {
                    prompt,
                    cwd: self.checkout(&repo),
                }));
            }
            FailureAction::EditConfig => self
                .requests
                .push(PanelRequest::OpenFile(self.context.config_path.clone())),
            FailureAction::Retry => self.load(&name),
            FailureAction::CreateDatabase => {
                self.start_fix(&name, "Creating database...", move |connector| {
                    connector.create_database(&database.name)?;
                    Ok(format!("Created an empty {place} database"))
                })
            }
            FailureAction::CopyFromMain => {
                let Some(main) = main_copy else {
                    return;
                };
                self.start_fix(&name, "Copying from main...", move |connector| {
                    connector.copy_database(&main, &database.name)?;
                    Ok(format!("Copied main's {place} data into this workspace"))
                })
            }
            FailureAction::StartShared => {
                self.start_fix(&name, "Starting shared services...", move |connector| {
                    connector.start_shared()?;
                    let deadline = Instant::now() + SERVER_START_WAIT;
                    while Instant::now() < deadline {
                        match connector.open(&database) {
                            Err(error)
                                if !error.server_answered()
                                    || error.raw.contains("starting up") =>
                            {
                                std::thread::sleep(Duration::from_millis(500))
                            }
                            _ => break,
                        }
                    }
                    Ok("Started the shared services".to_string())
                })
            }
        }
    }

    fn start_fix(
        &mut self,
        name: &str,
        busy: &str,
        work: impl FnOnce(&Connector<'_>) -> Result<String, String> + Send + 'static,
    ) {
        if self.fixes.contains_key(name) {
            return;
        }
        if let Some(failure) = self.failure_mut(name) {
            failure.busy = Some(busy.to_string());
            failure.action_error = None;
        }
        let pending = self.context.run(work);
        self.fixes.insert(name.to_string(), pending);
    }

    fn toggle_database(&mut self, repo: &str, name: &str) {
        let key = database_key(repo, name);
        self.model.toggle(&key);
        if self.model.is_open(&key) && !self.model.loaded.contains_key(name) {
            self.load(name);
        }
    }

    fn refresh(&mut self) {
        self.load_consoles();
        self.reload_config();
        let loaded: Vec<String> = self.model.loaded.keys().cloned().collect();
        self.model.folders.clear();
        self.model.stats.clear();
        for name in loaded {
            self.load(&name);
        }
    }

    fn poll(&mut self) {
        let engines: HashMap<String, Engine> = self
            .model
            .databases
            .iter()
            .map(|database| (database.name.clone(), database.engine))
            .collect();
        let mut opened_buckets: Vec<(String, Vec<String>)> = Vec::new();
        for (name, loaded) in self.model.loaded.iter_mut() {
            if let Loaded::Loading(pending) = loaded {
                match pending.poll() {
                    Some(Ok(Opened::Schema(schema))) => *loaded = Loaded::Schema(schema),
                    Some(Ok(Opened::Buckets(buckets))) => {
                        opened_buckets.push((name.clone(), buckets.clone()));
                        *loaded = Loaded::Buckets(buckets);
                    }
                    Some(Ok(Opened::Failed(failure))) => *loaded = Loaded::Failed(failure),
                    Some(Err(error)) => {
                        let error = ConnectError {
                            kind: ConnectErrorKind::Other,
                            engine: engines.get(name).copied().unwrap_or(Engine::Postgres),
                            database: name.clone(),
                            host: String::new(),
                            port: 0,
                            user: String::new(),
                            raw: error,
                            container: None,
                        };
                        *loaded = Loaded::Failed(Box::new(Failure::new(error, String::new())));
                    }
                    None => {}
                }
            }
        }
        for (name, buckets) in opened_buckets {
            for bucket in buckets {
                let open = self
                    .repos_showing(&name)
                    .iter()
                    .any(|repo| self.model.is_open(&bucket_key(repo, &name, &bucket)));
                if open {
                    self.load_folder(&name, &bucket, "", None);
                }
            }
        }
        self.poll_folders();
        self.poll_fixes();
        self.poll_jobs();
    }

    fn poll_folders(&mut self) {
        let mut listed: Vec<(String, Listing, bool)> = Vec::new();
        for (key, folder) in self.model.folders.iter_mut() {
            let Some(pending) = folder.loading.as_mut() else {
                continue;
            };
            match pending.poll() {
                Some(Ok(listing)) => {
                    folder.loading = None;
                    folder.error = None;
                    let more = folder.next.is_some() && !folder.is_empty();
                    listed.push((key.clone(), listing, more));
                }
                Some(Err(error)) => {
                    folder.loading = None;
                    folder.error = Some(error);
                }
                None => {}
            }
        }
        for (key, listing, more) in listed {
            let prefixes = listing.prefixes.clone();
            if let Some(folder) = self.model.folders.get_mut(&key) {
                if more {
                    folder.prefixes.extend(listing.prefixes);
                    folder.objects.extend(listing.objects);
                    folder.next = listing.next;
                } else {
                    *folder = Folder::listed(listing);
                }
            }
            let mut parts = key.split('\u{1f}');
            if let (Some(name), Some(bucket)) = (parts.next(), parts.next()) {
                let (name, bucket) = (name.to_string(), bucket.to_string());
                self.count_prefixes(&name, &bucket, prefixes);
            }
        }
        let mut counted = Vec::new();
        self.counting.retain_mut(|pending| match pending.poll() {
            Some(answer) => {
                counted.push(answer);
                false
            }
            None => true,
        });
        for answer in counted.into_iter().flatten() {
            for (key, stats) in answer {
                self.model.stats.insert(key, stats);
            }
        }
    }

    fn poll_fixes(&mut self) {
        let mut finished = Vec::new();
        for (name, pending) in self.fixes.iter_mut() {
            if let Some(answer) = pending.poll() {
                finished.push((name.clone(), answer));
            }
        }
        for (name, answer) in finished {
            self.fixes.remove(&name);
            match answer {
                Ok(message) => {
                    self.toast(message);
                    self.load(&name);
                }
                Err(error) => {
                    if let Some(failure) = self.failure_mut(&name) {
                        failure.busy = None;
                        failure.action_error = Some(error);
                    }
                }
            }
        }
    }

    fn poll_jobs(&mut self) {
        let mut finished = Vec::new();
        self.jobs.retain_mut(|pending| match pending.poll() {
            Some(answer) => {
                finished.push(answer);
                false
            }
            None => true,
        });
        for answer in finished {
            match answer {
                Ok(Outcome::Done { message, reload }) => {
                    self.toast(message);
                    self.reload(reload);
                }
                Ok(Outcome::Text { id, title, text }) => {
                    self.requests.push(PanelRequest::Reveal {
                        id: id.clone(),
                        open: Box::new(move || Some(files_ui::text_tab(id, title, &text))),
                    });
                }
                Err(error) => self.toast(error),
            }
        }
    }

    fn reload(&mut self, reload: Reload) {
        match reload {
            Reload::Nothing => {}
            Reload::Database(name) => self.load(&name),
            Reload::Folder {
                database,
                bucket,
                prefix,
            } => {
                self.model
                    .folders
                    .remove(&folder_key(&database, &bucket, &prefix));
                self.load_folder(&database, &bucket, &prefix, None);
            }
        }
    }

    fn job(
        &mut self,
        work: impl FnOnce(&Connector<'_>) -> Result<Outcome, String> + Send + 'static,
    ) {
        self.jobs.push(self.context.run(work));
    }

    pub(crate) fn rebuild_rows(&mut self) {
        let (rows, found) = self.model.rows();
        self.rows = rows;
        self.found = found;
    }

    fn row_height(&self, index: usize, row: &Row) -> f32 {
        match row {
            Row::Failure { .. } => measure(&crate::render::render_row(self, index, row)).1,
            Row::Section { .. } => ROW_H + 4.0,
            _ => ROW_H,
        }
    }

    fn content_height(&self) -> f32 {
        HEADER_H
            + FILTER_H
            + self
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| self.row_height(index, row))
                .sum::<f32>()
            + 8.0
    }

    fn on_main(&self) -> bool {
        (self.context.config)()
            .is_some_and(|config| self.context.branch == config.global_default_branch())
    }

    fn menu_facts(&self, row: &Row) -> (Option<Database>, bool, Option<Database>) {
        let database = match row {
            Row::Console { console, .. } => self.database(&console.database).cloned(),
            _ => row.database().and_then(|index| self.database_at(index)),
        };
        let has_main_copy = database.as_ref().is_some_and(|database| {
            (self.context.config)()
                .and_then(|config| pom_db::main_database(&config, database))
                .is_some()
        });
        let repo_database = match row {
            Row::Repo { repo, .. } => self
                .model
                .databases
                .iter()
                .find(|database| database.engine.is_sql() && database.repo == *repo)
                .cloned(),
            _ => database.clone(),
        };
        (database, has_main_copy, repo_database)
    }

    fn row_database(&self, row: &Row) -> Option<Database> {
        self.menu_facts(row).0
    }

    /// Runs a menu item, asking first when it loses data.
    fn choose(&mut self, action: Action, row: Row) {
        let database = self.row_database(&row);
        if let Some(asked) = menu::confirmation(&action, &row, database.as_ref()) {
            self.confirm = Some((action, row));
            self.requests.push(PanelRequest::Prompt {
                tag: CONFIRM_PROMPT,
                message: asked.message,
                detail: Some(asked.detail),
                buttons: vec![asked.button.to_string(), "Cancel".into()],
            });
            return;
        }
        self.act(action, row);
    }

    fn act(&mut self, action: Action, row: Row) {
        let (database, _, repo_database) = self.menu_facts(&row);
        let place = database.as_ref().map(readable).unwrap_or_default();
        match action {
            Action::Header => {}
            Action::Refresh => self.refresh_row(&row),
            Action::CollapseAll => self.model.collapse_all(),
            Action::Collapse => {
                if let (Row::Group { repo, kind, .. }, Some(database)) = (&row, &database) {
                    self.model
                        .set_open(&group_key(repo, &database.name, *kind), false);
                }
            }
            Action::CopyUrl => {
                let target = repo_database.or(database);
                if let Some(target) = target {
                    if let Some(url) = self.connection_url(&target) {
                        self.copy(
                            url,
                            format!("Copied the connection URL of {}", readable(&target)),
                        );
                    }
                }
            }
            Action::OpenClient => {
                let target = repo_database.or(database);
                if let Some(target) = target {
                    self.open_client(&target);
                }
            }
            Action::NewConsole => self.create_console(database, None, ""),
            Action::CopyName => {
                let name = match &row {
                    Row::Table { table, .. } => table.qualified(),
                    Row::Column { column, .. } => column.name.clone(),
                    _ => database
                        .as_ref()
                        .map(|database| database.name.clone())
                        .unwrap_or_default(),
                };
                self.copy(name.clone(), format!("Copied {name}"));
            }
            Action::CopyFromMain => {
                let Some(database) = database else {
                    return;
                };
                let main = (self.context.config)()
                    .and_then(|config| pom_db::main_database(&config, &database));
                let Some(main) = main else {
                    return;
                };
                self.job(move |connector| {
                    connector.copy_database(&main, &database.name)?;
                    Ok(Outcome::Done {
                        message: format!("Copied main's data into {place}"),
                        reload: Reload::Database(database.name.clone()),
                    })
                });
            }
            Action::ResetDatabase => {
                let Some(database) = database else {
                    return;
                };
                self.job(move |connector| {
                    connector.reset_database(&database.name)?;
                    Ok(Outcome::Done {
                        message: format!("Reset {place}: it is empty now"),
                        reload: Reload::Database(database.name.clone()),
                    })
                });
            }
            Action::AskAboutSchema => {
                if let Some(database) = database {
                    let prompt = self.schema_prompt(&database);
                    self.ask(&database, prompt);
                }
            }
            Action::AskAboutTable => {
                if let (Row::Table { table, .. }, Some(database)) = (&row, database) {
                    let prompt = self.table_prompt(&database, table);
                    self.ask(&database, prompt);
                }
            }
            Action::OpenData | Action::OpenKeys => match (&row, database) {
                (Row::Table { table, .. }, Some(database)) => {
                    self.open_table(database, table.clone(), "")
                }
                (Row::Keyspace { table, .. }, Some(database)) => {
                    self.open_keyspace(database, table.clone())
                }
                _ => {}
            },
            Action::NewConsoleWithSelect => {
                if let Row::Table { table, .. } = &row {
                    let sql = format!("SELECT * FROM {} LIMIT {SELECT_LIMIT};\n", table.sql_name());
                    self.create_console(database, Some(table.qualified()), &sql);
                }
            }
            Action::CopySelect => {
                if let Row::Table { table, .. } = &row {
                    let sql = format!("SELECT * FROM {} LIMIT {SELECT_LIMIT};", table.sql_name());
                    self.copy(sql, format!("Copied a SELECT of {}", table.qualified()));
                }
            }
            Action::ShowDdl => {
                if let (Row::Table { table, .. }, Some(database)) = (&row, database) {
                    let table = table.clone();
                    self.job(move |connector| {
                        let text = connector.table_ddl(&database, &table)?;
                        Ok(Outcome::Text {
                            id: format!("db-ddl:{}:{}", database.name, table.qualified()),
                            title: format!("{}.sql", table.qualified()),
                            text,
                        })
                    });
                }
            }
            Action::Truncate | Action::DropTable => {
                let (Row::Table { table, .. }, Some(database)) = (&row, database) else {
                    return;
                };
                let truncate = action == Action::Truncate;
                let sql = if truncate {
                    format!("TRUNCATE TABLE {}", table.sql_name())
                } else {
                    format!("DROP TABLE {}", table.sql_name())
                };
                let name = table.qualified();
                self.job(move |connector| {
                    connector.query(&database, &sql, 1)?;
                    Ok(Outcome::Done {
                        message: if truncate {
                            format!("Truncated {name}: it has no rows now")
                        } else {
                            format!("Dropped {name} from {place}")
                        },
                        reload: Reload::Database(database.name.clone()),
                    })
                });
            }
            Action::FilterByColumn => {
                if let (Row::Column { table, column, .. }, Some(database)) = (&row, database) {
                    let filter = format!("{} IS NOT NULL", pom_db::quote_identifier(&column.name));
                    self.open_table(database, table.clone(), &filter);
                }
            }
            Action::DistinctValues => {
                if let Row::Column { table, column, .. } = &row {
                    let quoted = pom_db::quote_identifier(&column.name);
                    let sql = format!(
                        "SELECT {quoted}, count(*) AS rows FROM {} GROUP BY 1 ORDER BY 2 DESC LIMIT 500;\n",
                        table.sql_name()
                    );
                    let title = format!("{}.{} values", table.qualified(), column.name);
                    self.create_console(database, Some(title), &sql);
                }
            }
            Action::OpenConsole => {
                if let Row::Console { console, .. } = row {
                    self.open_console(console);
                }
            }
            Action::RenameConsole => {
                if let Row::Console { console, .. } = &row {
                    let mut field = TextField::default();
                    field.set_font_size(12.5);
                    field.set_text(&console.title);
                    field.select_all();
                    self.filter_focused = false;
                    self.rename = Some((console.id.clone(), field));
                }
            }
            Action::ChangeDatabase => {
                if let Row::Console { console, .. } = &row {
                    let mut entries = vec![Entry {
                        label: format!("Run {} on", console.title),
                        action: Action::Header,
                        danger: false,
                        disabled: true,
                        sep: false,
                    }];
                    for database in &self.model.databases {
                        if !matches!(database.engine, Engine::Postgres | Engine::Redis) {
                            continue;
                        }
                        entries.push(Entry {
                            label: readable(database),
                            action: Action::MoveConsole(database.name.clone()),
                            danger: false,
                            disabled: database.name == console.database,
                            sep: false,
                        });
                    }
                    self.menu = Some((row.clone(), entries));
                    self.requests.push(PanelRequest::OpenMenu);
                }
            }
            Action::MoveConsole(name) => {
                if let Row::Console { console, .. } = &row {
                    let place = self.model.place_of(&name);
                    self.update_console(&console.id, |saved| saved.database = name);
                    let title = self
                        .model
                        .consoles
                        .iter()
                        .find(|saved| saved.id == console.id)
                        .map_or_else(|| console.title.clone(), |saved| saved.title.clone());
                    self.toast(format!("{title} now runs on {place}"));
                }
            }
            Action::DeleteConsole => {
                if let Row::Console { console, .. } = &row {
                    self.delete_console(&console.id);
                    self.toast(format!("Deleted the console {}", console.title));
                }
            }
            Action::CopyPattern => {
                if let Row::Keyspace { table, .. } = &row {
                    let pattern = format!("{}:*", table.name);
                    self.copy(pattern.clone(), format!("Copied {pattern}"));
                }
            }
            Action::DeleteKeys => {
                if let (Row::Keyspace { table, .. }, Some(database)) = (&row, database) {
                    let pattern = format!("{}:*", table.name);
                    self.job(move |connector| {
                        let deleted = connector.delete_keys(&database, &pattern)?;
                        Ok(Outcome::Done {
                            message: format!("Deleted {deleted} {pattern} keys from {place}"),
                            reload: Reload::Database(database.name.clone()),
                        })
                    });
                }
            }
            Action::CopyPath => {
                let path = match &row {
                    Row::Bucket { bucket, .. } => bucket.clone(),
                    Row::Prefix { bucket, prefix, .. } => format!("{bucket}/{prefix}"),
                    Row::Object { bucket, object, .. } => format!("{bucket}/{}", object.key),
                    _ => return,
                };
                self.copy(path.clone(), format!("Copied {path}"));
            }
            Action::DeleteFolder => {
                if let (Row::Prefix { bucket, prefix, .. }, Some(database)) = (&row, database) {
                    let (bucket, prefix) = (bucket.clone(), prefix.clone());
                    let transport = self.context.objects.clone();
                    self.job(move |connector| {
                        let deleted = connector.object_store(&database.name).delete_prefix(
                            transport.as_ref(),
                            &bucket,
                            &prefix,
                        )?;
                        Ok(Outcome::Done {
                            message: format!("Deleted {deleted} objects under {bucket}/{prefix}"),
                            reload: Reload::Folder {
                                database: database.name.clone(),
                                bucket,
                                prefix: parent_prefix(&prefix),
                            },
                        })
                    });
                }
            }
            Action::OpenObject => {
                if let (Row::Object { bucket, object, .. }, Some(database)) = (&row, database) {
                    self.open_object(database, bucket.clone(), object.clone());
                }
            }
            Action::DownloadObject => {
                if let (Row::Object { bucket, object, .. }, Some(database)) = (&row, database) {
                    self.download(database, bucket.clone(), object.clone());
                }
            }
            Action::CopyPresignedUrl => {
                if let (Row::Object { bucket, object, .. }, Some(database)) = (&row, database) {
                    let url = self.context.run_now(|connector| {
                        connector.object_store(&database.name).presigned_url(
                            bucket,
                            &object.key,
                            PRESIGNED_SECONDS,
                        )
                    });
                    if let Some(url) = url {
                        self.copy(url, "Copied a link that works for 1 hour".into());
                    }
                }
            }
            Action::DeleteObject => {
                if let (Row::Object { bucket, object, .. }, Some(database)) = (&row, database) {
                    let (bucket, key) = (bucket.clone(), object.key.clone());
                    let transport = self.context.objects.clone();
                    self.job(move |connector| {
                        connector.object_store(&database.name).delete(
                            transport.as_ref(),
                            &bucket,
                            &key,
                        )?;
                        Ok(Outcome::Done {
                            message: format!("Deleted {bucket}/{key}"),
                            reload: Reload::Folder {
                                database: database.name.clone(),
                                bucket,
                                prefix: parent_prefix(&key),
                            },
                        })
                    });
                }
            }
        }
    }

    fn refresh_row(&mut self, row: &Row) {
        match row {
            Row::Repo { repo, .. } => {
                let names: Vec<String> = self
                    .model
                    .groups()
                    .into_iter()
                    .filter(|(group, _, _)| group == repo)
                    .flat_map(|(_, _, indexes)| indexes)
                    .filter_map(|index| self.database_at(index).map(|database| database.name))
                    .collect();
                for name in names {
                    self.load(&name);
                }
            }
            Row::Bucket { index, bucket, .. } | Row::Prefix { index, bucket, .. } => {
                let prefix = match row {
                    Row::Prefix { prefix, .. } => prefix.clone(),
                    _ => String::new(),
                };
                if let Some(database) = self.database_at(*index) {
                    self.reload(Reload::Folder {
                        database: database.name,
                        bucket: bucket.clone(),
                        prefix,
                    });
                }
            }
            _ => match row.database().and_then(|index| self.database_at(index)) {
                Some(database) => self.load(&database.name),
                None => self.refresh(),
            },
        }
    }

    fn connection_url(&self, database: &Database) -> Option<String> {
        self.context.run_now(|connector| match database.engine {
            Engine::Minio => Some(connector.object_store(&database.name).base),
            engine => connector.login(database).map(|login| login.url(engine)),
        })?
    }

    fn open_client(&mut self, database: &Database) {
        let Some(Some(login)) = self.context.run_now(|connector| connector.login(database)) else {
            return;
        };
        let (program, argv, env) = match database.engine {
            Engine::Redis => (
                "redis-cli",
                vec![
                    "-h".to_string(),
                    login.host.clone(),
                    "-p".into(),
                    login.port.to_string(),
                    "-n".into(),
                    login.database.clone(),
                ],
                Vec::new(),
            ),
            _ => (
                "psql",
                vec![
                    "-h".to_string(),
                    login.host.clone(),
                    "-p".into(),
                    login.port.to_string(),
                    "-U".into(),
                    login.user.clone(),
                    "-d".into(),
                    login.database.clone(),
                ],
                vec![("PGPASSWORD".to_string(), login.password.clone())],
            ),
        };
        let mut command = vec![find_program(program)];
        command.extend(argv);
        let mut env = env;
        env.push(("PATH".into(), pom_services::tool_path().to_string()));
        let folder = self.repo_key_of(&database.name);
        self.requests.push(PanelRequest::RunCommand {
            title: format!("{program} {}", readable(database)),
            cwd: self.checkout(&folder),
            argv: command,
            env,
        });
    }

    fn open_table(&mut self, database: Database, table: Table, filter: &str) {
        let context = self.context.clone();
        let filter = filter.to_string();
        let id = if filter.is_empty() {
            TableItem::item_id(&database, &table)
        } else {
            format!("{}:{filter}", TableItem::item_id(&database, &table))
        };
        self.requests.push(PanelRequest::Reveal {
            id,
            open: Box::new(move || {
                Some(Box::new(TableItem::filtered(
                    context, database, table, &filter,
                )))
            }),
        });
    }

    fn open_keyspace(&mut self, database: Database, table: Table) {
        let context = self.context.clone();
        self.requests.push(PanelRequest::Reveal {
            id: KeyspaceItem::item_id(&database, &table),
            open: Box::new(move || Some(Box::new(KeyspaceItem::new(context, database, table)))),
        });
    }

    fn open_bucket(
        &mut self,
        database: Database,
        bucket: String,
        prefix: &str,
        selected: Option<String>,
    ) {
        let id = BucketItem::item_id(&database, &bucket);
        let (context, prefix) = (self.context.clone(), prefix.to_string());
        self.requests.push(PanelRequest::Reveal {
            id,
            open: Box::new(move || {
                Some(Box::new(BucketItem::new(
                    context, database, bucket, &prefix, selected,
                )))
            }),
        });
    }

    fn open_object(&mut self, database: Database, bucket: String, object: ObjectEntry) {
        let context = self.context.clone();
        self.requests.push(PanelRequest::Reveal {
            id: object_item_id(&database, &bucket, &object.key),
            open: Box::new(move || {
                Some(Box::new(ObjectItem::new(context, database, bucket, object)))
            }),
        });
    }

    fn download(&mut self, database: Database, bucket: String, object: ObjectEntry) {
        let downloads = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let name = object.name().replace(['/', '\\'], "_");
        let mut path = downloads.join(&name);
        let mut copy = 1;
        while path.exists() {
            path = downloads.join(format!("{copy}-{name}"));
            copy += 1;
        }
        let transport = self.context.objects.clone();
        self.job(move |connector| {
            connector.object_store(&database.name).download(
                transport.as_ref(),
                &bucket,
                &object.key,
                &path,
            )?;
            Ok(Outcome::Done {
                message: format!("Downloaded {name} to {}", path.display()),
                reload: Reload::Nothing,
            })
        });
    }

    fn ask(&mut self, database: &Database, prompt: String) {
        let folder = self.repo_key_of(&database.name);
        self.requests.push(PanelRequest::AskAgent(AgentFix {
            prompt,
            cwd: self.checkout(&folder),
        }));
    }

    fn describe_table(schema: &Schema, table: &Table) -> String {
        let columns: Vec<String> = schema
            .columns_of(table)
            .map(|column| {
                let mut text = format!("{} {}", column.name, column.data_type);
                if column.primary_key {
                    text.push_str(" primary key");
                }
                if let Some(target) = &column.references {
                    text.push_str(&format!(" references {target}"));
                }
                text
            })
            .collect();
        let rows = table
            .count
            .map(|count| format!(" (about {count} rows)"))
            .unwrap_or_default();
        let kind = if table.kind == TableKind::View {
            "view "
        } else {
            ""
        };
        format!(
            "- {kind}{}{rows}: {}",
            table.qualified(),
            columns.join(", ")
        )
    }

    fn about(&self, database: &Database) -> String {
        let detail = self
            .details
            .get(&database.name)
            .map(|detail| detail.tooltip.clone())
            .unwrap_or_else(|| database.name.clone());
        format!("the {} database ({detail})", readable(database))
    }

    fn schema_prompt(&self, database: &Database) -> String {
        let tables = match self.model.schema(&database.name) {
            Some(schema) if !schema.tables.is_empty() => schema
                .tables
                .iter()
                .map(|table| Self::describe_table(schema, table))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => "(the panel has not loaded its tables; list them with the database tools)".into(),
        };
        format!(
            "I have a question about {}. Its tables:\n\n{tables}\n\nRead the schema and find where the code \
             uses these tables, then wait for my question. Do not change data.",
            self.about(database)
        )
    }

    fn table_prompt(&self, database: &Database, table: &Table) -> String {
        let columns = self
            .model
            .schema(&database.name)
            .map(|schema| Self::describe_table(schema, table))
            .unwrap_or_else(|| format!("- {}", table.qualified()));
        format!(
            "I have a question about the {} table of {}:\n\n{columns}\n\nFind where the code reads and writes \
             it, then wait for my question. Do not change data.",
            table.qualified(),
            self.about(database)
        )
    }

    fn commit_rename(&mut self) {
        let Some((id, field)) = self.rename.take() else {
            return;
        };
        let title = field.text().trim().to_string();
        if title.is_empty() {
            return;
        }
        self.update_console(&id, |console| console.title = title);
    }

    fn click_row(&mut self, index: usize, part: u64) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        self.selected = Some(row.key());
        match row {
            Row::Section {
                section: Section::Consoles,
                ..
            } => self.model.toggle(CONSOLES_KEY),
            Row::Section { .. } | Row::Note { .. } | Row::Failure { .. } | Row::Column { .. } => {}
            Row::Console { console, .. } => self.open_console(console),
            Row::Repo { repo, .. } => self.model.toggle(&repo_key(&repo)),
            Row::Database { index, repo, .. } => {
                if let Some(database) = self.database_at(index) {
                    self.toggle_database(&repo, &database.name);
                }
            }
            Row::Group {
                index, repo, kind, ..
            } => {
                if let Some(database) = self.database_at(index) {
                    self.model.toggle(&group_key(&repo, &database.name, kind));
                }
            }
            Row::Table {
                index,
                repo,
                table,
                has_columns,
                ..
            } => {
                let Some(database) = self.database_at(index) else {
                    return;
                };
                if part == CHEVRON && has_columns {
                    self.model.toggle(&table_key(&repo, &database.name, &table));
                } else {
                    self.open_table(database, table, "");
                }
            }
            Row::Keyspace { index, table, .. } => {
                if let Some(database) = self.database_at(index) {
                    self.open_keyspace(database, table);
                }
            }
            Row::Bucket {
                index,
                repo,
                bucket,
                ..
            } => {
                let Some(database) = self.database_at(index) else {
                    return;
                };
                if part != CHEVRON {
                    self.open_bucket(database, bucket, "", None);
                    return;
                }
                let key = bucket_key(&repo, &database.name, &bucket);
                self.model.toggle(&key);
                if self.model.is_open(&key)
                    && !self
                        .model
                        .folders
                        .contains_key(&folder_key(&database.name, &bucket, ""))
                {
                    self.load_folder(&database.name, &bucket, "", None);
                }
            }
            Row::Prefix {
                index,
                repo,
                bucket,
                prefix,
                ..
            } => {
                let Some(database) = self.database_at(index) else {
                    return;
                };
                if part != CHEVRON {
                    self.open_bucket(database, bucket, &prefix, None);
                    return;
                }
                let key = prefix_key(&repo, &database.name, &bucket, &prefix);
                self.model.toggle(&key);
                if self.model.is_open(&key)
                    && !self.model.folders.contains_key(&folder_key(
                        &database.name,
                        &bucket,
                        &prefix,
                    ))
                {
                    self.load_folder(&database.name, &bucket, &prefix, None);
                }
            }
            Row::Object {
                index,
                bucket,
                object,
                ..
            } => {
                if let Some(database) = self.database_at(index) {
                    let folder = parent_prefix(&object.key);
                    self.open_bucket(database, bucket, &folder, Some(object.key));
                }
            }
            Row::More {
                index,
                bucket,
                prefix,
                ..
            } => {
                let Some(database) = self.database_at(index) else {
                    return;
                };
                let next = self
                    .model
                    .folders
                    .get(&folder_key(&database.name, &bucket, &prefix))
                    .and_then(|folder| folder.next.clone());
                if next.is_some() {
                    self.load_folder(&database.name, &bucket, &prefix, next);
                }
            }
        }
    }
}

impl DatabasePanel {
    fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.rows.iter().position(|row| row.key() == *key)
    }

    fn selectable(row: &Row) -> bool {
        !matches!(row, Row::Note { .. } | Row::Failure { .. })
    }

    fn select_index(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        self.selected = Some(row.key());
        let top: f32 = self
            .rows
            .iter()
            .enumerate()
            .take(index)
            .map(|(at, row)| self.row_height(at, row))
            .sum();
        let height = self.row_height(index, row);
        let shown_h = self.viewport_h - HEADER_H - FILTER_H;
        if top < self.scroll {
            self.scroll = top;
        } else if top + height > self.scroll + shown_h {
            self.scroll = top + height - shown_h;
        }
    }

    fn move_selection(&mut self, movement: NavMove) {
        let mut nav = ListNav {
            selected: self.selected_index(),
        };
        let rows = &self.rows;
        nav.apply(movement, rows.len(), false, |index| {
            rows.get(index).is_some_and(Self::selectable)
        });
        if let Some(index) = nav.selected {
            self.select_index(index);
        }
    }

    fn take_keyboard(&mut self, on: bool) {
        self.keyboard = on;
        if on {
            self.filter_focused = false;
            if self.selected_index().is_none() {
                self.move_selection(NavMove::First);
            }
        }
    }

    fn row_open(row: &Row) -> Option<bool> {
        match row {
            Row::Section {
                section: Section::Consoles,
                open,
                ..
            }
            | Row::Repo { open, .. }
            | Row::Database { open, .. }
            | Row::Group { open, .. }
            | Row::Bucket { open, .. }
            | Row::Prefix { open, .. } => Some(*open),
            Row::Table {
                open,
                has_columns: true,
                ..
            } => Some(*open),
            _ => None,
        }
    }

    /// Folds an open row; from one that is closed or does not fold, goes to the row it sits under.
    fn collapse_selected(&mut self) -> bool {
        let Some(index) = self.selected_index() else {
            return false;
        };
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        if Self::row_open(row) == Some(true) {
            self.click_row(index, CHEVRON);
            return true;
        }
        let depth = row.depth();
        let parent = (0..index)
            .rev()
            .find(|above| self.rows.get(*above).is_some_and(|row| row.depth() < depth));
        if let Some(parent) = parent {
            self.select_index(parent);
        }
        true
    }

    fn expand_selected(&mut self) -> bool {
        let Some(index) = self.selected_index() else {
            return false;
        };
        match self.rows.get(index).and_then(Self::row_open) {
            Some(false) => self.click_row(index, CHEVRON),
            Some(true) => self.move_selection(NavMove::Next),
            None => return false,
        }
        true
    }

    /// Runs the selected row's menu item for `action`, when its menu offers it.
    fn run_row_entry(&mut self, action: Action) -> bool {
        let Some(row) = self
            .selected_index()
            .and_then(|index| self.rows.get(index).cloned())
        else {
            return false;
        };
        let (database, has_main_copy, repo_database) = self.menu_facts(&row);
        let offered = menu::entries(
            &row,
            &Facts {
                database: database.as_ref(),
                has_main_copy,
                repo_database: repo_database.as_ref(),
                on_main: self.on_main(),
            },
        )
        .into_iter()
        .any(|entry| entry.action == action && !entry.disabled);
        if offered {
            self.choose(action, row);
        }
        offered
    }

    fn run_key_action(&mut self, action: KeyAction) -> bool {
        match action {
            KeyAction::DatabaseOpen => {
                let Some(index) = self.selected_index() else {
                    return false;
                };
                self.click_row(index, 0);
                true
            }
            KeyAction::DatabaseCollapse => self.collapse_selected(),
            KeyAction::DatabaseExpand => self.expand_selected(),
            KeyAction::DatabaseCopyUrl => self.run_row_entry(Action::CopyUrl),
            KeyAction::DatabaseNewConsole => {
                let database = self.selected_database();
                self.create_console(database, None, "");
                true
            }
            _ => false,
        }
    }
}

/// Whether `console` is one of this workspace's: saved for its branch, or, from before consoles were per
/// workspace, on one of its databases.
fn belongs_to(console: &Console, branch: &str, databases: &[String]) -> bool {
    if console.workspace.is_empty() {
        databases.contains(&console.database)
    } else {
        console.workspace == branch
    }
}

/// `uploads/avatars/u1.jpg` -> `uploads/avatars/`, `uploads/avatars/` -> `uploads/`.
fn parent_prefix(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(at) => trimmed[..=at].to_string(),
        None => String::new(),
    }
}

/// The program's full path on the tool PATH, or its bare name for the shell to report.
fn find_program(name: &str) -> String {
    pom_services::tool_path()
        .split(':')
        .map(|dir| std::path::Path::new(dir).join(name))
        .find(|path| path.is_file())
        .map_or_else(
            || name.to_string(),
            |path| path.to_string_lossy().into_owned(),
        )
}

fn detail(connector: &Connector<'_>, database: &Database) -> Detail {
    let engine = database.engine.title();
    match database.engine {
        Engine::Postgres | Engine::Redis => {
            let Some(login) = connector.login(database) else {
                return Detail::default();
            };
            let (subtitle, what) = if database.engine == Engine::Redis {
                let subtitle = format!("db {}", login.database);
                (subtitle.clone(), format!("{} {subtitle}", database.name))
            } else {
                (database.name.clone(), database.name.clone())
            };
            Detail {
                subtitle,
                tooltip: format!("{engine} - {what} on {}:{}", login.host, login.port),
            }
        }
        Engine::Minio => Detail {
            subtitle: String::new(),
            tooltip: format!(
                "{engine} - {} on {}",
                database.name,
                connector.object_store(&database.name).host
            ),
        },
        _ => Detail {
            subtitle: String::new(),
            tooltip: format!("{engine} - {}", database.name),
        },
    }
}

impl SidePanelView for DatabasePanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Database
    }

    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.poll();
        self.width = width;
        self.viewport_h = height;
        self.rebuild_rows();
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut visible = Vec::new();
        let mut top = 0.0;
        for (index, row) in self.rows.iter().enumerate() {
            let bottom = top + self.row_height(index, row);
            if bottom > self.scroll && top < self.scroll + height {
                visible.push(index);
            }
            top = bottom;
        }
        crate::render::render_panel(self, width, height, &visible)
    }

    fn click(&mut self, id: u64) {
        let renaming = self.rename.as_ref().map(|(id, _)| id.clone());
        if renaming.is_some() && id != Self::base() + RENAME_FIELD {
            self.commit_rename();
        }
        self.filter_focused = id == Self::base() + FILTER;
        match id.checked_sub(Self::base()) {
            Some(REFRESH) => return self.refresh(),
            Some(NEW_CONSOLE) => {
                let database = self.selected_database();
                return self.create_console(database, None, "");
            }
            Some(COLLAPSE_ALL) => return self.model.collapse_all(),
            Some(FILTER) | Some(RENAME_FIELD) => return,
            _ => {}
        }
        if let Some((index, action)) = Self::decode_failure(id) {
            return self.failure_action(index, action);
        }
        if let Some((index, part)) = Self::decode(id) {
            self.click_row(index, part);
        }
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let changed = self.hover != id;
        self.hover = id;
        changed
    }

    fn scroll(&mut self, dy: f32) -> bool {
        let max = (self.content_height() - self.viewport_h).max(0.0);
        let next = (self.scroll - dy).clamp(0.0, max);
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn open_menu(&mut self, id: u64) -> bool {
        let Some(row) = Self::decode(id).and_then(|(index, _)| self.rows.get(index).cloned())
        else {
            self.menu = None;
            return false;
        };
        let (database, has_main_copy, repo_database) = self.menu_facts(&row);
        let entries = menu::entries(
            &row,
            &Facts {
                database: database.as_ref(),
                has_main_copy,
                repo_database: repo_database.as_ref(),
                on_main: self.on_main(),
            },
        );
        if entries.is_empty() {
            self.menu = None;
            return false;
        }
        self.selected = Some(row.key());
        self.menu = Some((row, entries));
        true
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        let Some((_, entries)) = &self.menu else {
            return Vec::new();
        };
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| MenuItem {
                id: Self::base() + MENU + index as u64,
                label: entry.label.clone().into(),
                checked: false,
                sep: entry.sep,
                disabled: entry.disabled,
                danger: entry.danger,
                icon: menu::icon(&entry.action),
                hint: None,
                color: None,
                header: false,
            })
            .collect()
    }

    fn menu_action(&mut self, item: u64) {
        let Some((row, entries)) = self.menu.take() else {
            return;
        };
        let Some(entry) = item
            .checked_sub(Self::base() + MENU)
            .and_then(|index| entries.get(index as usize))
        else {
            return;
        };
        if entry.disabled {
            return;
        }
        self.choose(entry.action.clone(), row);
    }

    fn prompt_answered(&mut self, tag: u64, answer: usize) {
        let Some((action, row)) = self.confirm.take() else {
            return;
        };
        if tag == CONFIRM_PROMPT && answer == 0 {
            self.act(action, row);
        }
    }

    fn text_focused(&self) -> bool {
        self.filter_focused || self.rename.is_some()
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        if let Some((_, field)) = self.rename.as_mut() {
            field.insert(&typed);
            return true;
        }
        self.filter.insert(&typed);
        self.filter_changed();
        true
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        if let Some((_, field)) = self.rename.as_mut() {
            match key {
                EditKey::Enter => self.commit_rename(),
                EditKey::Escape => self.rename = None,
                _ => return field.key(key, shift),
            }
            return true;
        }
        match key {
            EditKey::Escape if self.filter.text().is_empty() => self.filter_focused = false,
            EditKey::Escape => {
                self.filter.set_text("");
                self.filter_changed();
            }
            EditKey::Enter => self.filter_focused = false,
            _ => {
                let changed = self.filter.key(key, shift);
                self.filter_changed();
                return changed;
            }
        }
        true
    }

    fn blur(&mut self) {
        self.filter_focused = false;
        self.rename = None;
    }

    fn has_keyboard(&self) -> bool {
        self.keyboard
    }

    fn set_keyboard(&mut self, on: bool) {
        self.take_keyboard(on);
    }

    fn key_context(&self) -> Option<&'static str> {
        Some(workspace::keymap::DATABASE_PANEL)
    }

    fn list_key(&mut self, key: EditKey, shift: bool) -> bool {
        let Some(movement) = workspace::list_nav::nav_move(key, shift) else {
            return false;
        };
        self.move_selection(movement);
        true
    }

    fn panel_action(&mut self, action: KeyAction) -> bool {
        self.run_key_action(action)
    }

    fn restore_item(&mut self, item: &SerializedItem) -> Option<Box<dyn Item>> {
        restore_console(&self.context, item)
            .or_else(|| crate::table_item::restore_table(&self.context, item))
            .or_else(|| crate::keyspace_item::restore_keyspace(&self.context, item))
            .or_else(|| crate::bucket_item::restore_bucket(&self.context, item))
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// The database's schema, or why it failed with the fixes that apply to it.
fn open_database(connector: &Connector<'_>, database: &Database) -> Opened {
    let error = match connector.open(database) {
        Ok(schema) => return Opened::Schema(schema),
        Err(error) => error,
    };
    let repo = pom_db::repo_of(connector.config, database);
    let mut failure = Failure::new(
        *error,
        repo.map(|(key, _)| key.to_string()).unwrap_or_default(),
    );
    if failure.error.kind == ConnectErrorKind::DatabaseMissing {
        if let Some(main) = pom_db::main_database(connector.config, database) {
            match connector.database_exists(&main) {
                Ok(true) => failure.main_copy = Some(main),
                Ok(false) => failure.main_absent = true,
                Err(_) => {}
            }
        }
    }
    Opened::Failed(Box::new(failure))
}

fn storage_failure(connector: &Connector<'_>, database: &Database, raw: String) -> Failure {
    let store = connector.object_store(&database.name);
    let (host, port) = store
        .host
        .rsplit_once(':')
        .map(|(host, port)| (host.to_string(), port.parse().unwrap_or(0)))
        .unwrap_or((store.host.clone(), 0));
    Failure::new(
        ConnectError::new(
            Engine::Minio,
            &database.name,
            &host,
            port,
            &store.credentials.access_key,
            raw,
        ),
        String::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_shows_only_in_its_own_workspace() {
        let console = |workspace: &str, database: &str| Console {
            workspace: workspace.into(),
            database: database.into(),
            ..Console::default()
        };
        let here = vec!["demo_api_feat".to_string()];
        assert!(belongs_to(&console("feat", "demo_api_feat"), "feat", &here));
        assert!(!belongs_to(
            &console("main", "demo_api_main"),
            "feat",
            &here
        ));
        assert!(belongs_to(&console("", "demo_api_feat"), "feat", &here));
        assert!(!belongs_to(&console("", "demo_api_other"), "feat", &here));
    }
    use crate::tree::tests::{column, users_schema};
    use pom_db::object_storage::ObjectEntry;

    fn panel() -> (crate::tests::TestContext, DatabasePanel) {
        let context = crate::tests::context();
        let panel = DatabasePanel::new(context.context.clone());
        (context, panel)
    }

    fn row_id(panel: &mut DatabasePanel, wanted: impl Fn(&Row) -> bool) -> u64 {
        panel.render(320.0, 2000.0);
        match panel.rows.iter().position(wanted) {
            Some(index) => DatabasePanel::id(index),
            None => panic!("row not shown"),
        }
    }

    fn api_main(context: &crate::tests::TestContext) -> String {
        context.context.databases()[0].name.clone()
    }

    #[test]
    fn new_console_is_saved_listed_and_opened() {
        let (context, mut panel) = panel();
        panel.click(DatabasePanel::base() + NEW_CONSOLE);
        let saved = context.context.consoles();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].title, "query 1");
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Reveal { id, .. }] if *id == console_item_id(&saved[0])
        ));
        let console = row_id(&mut panel, |row| matches!(row, Row::Console { .. }));
        assert!(matches!(
            panel.rows.iter().find(|row| matches!(row, Row::Console { .. })),
            Some(Row::Console { place, .. }) if place == "api > main"
        ));
        panel.click(console);
        assert_eq!(panel.take_requests().len(), 1);
    }

    #[test]
    fn a_console_is_deleted_only_after_confirming() {
        let (context, mut panel) = panel();
        panel.click(DatabasePanel::base() + NEW_CONSOLE);
        panel.take_requests();
        let console = row_id(&mut panel, |row| matches!(row, Row::Console { .. }));
        assert!(panel.open_menu(console));
        let delete = panel
            .menu_items()
            .into_iter()
            .find(|item| item.label == "Delete Console...")
            .map(|item| item.id);
        let Some(delete) = delete else {
            panic!("the console menu deletes");
        };
        assert!(panel
            .menu_items()
            .iter()
            .any(|item| item.id == delete && item.danger));
        panel.menu_action(delete);
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Prompt { tag: CONFIRM_PROMPT, message, .. }] if message.contains("query 1")
        ));
        panel.prompt_answered(CONFIRM_PROMPT, 1);
        assert_eq!(context.context.consoles().len(), 1);
        assert!(panel.open_menu(console));
        panel.menu_action(delete);
        panel.prompt_answered(CONFIRM_PROMPT, 0);
        assert!(context.context.consoles().is_empty());
    }

    #[test]
    fn a_console_is_renamed_in_place_and_moved_to_another_database() {
        let (context, mut panel) = panel();
        panel.click(DatabasePanel::base() + NEW_CONSOLE);
        panel.take_requests();
        let console = row_id(&mut panel, |row| matches!(row, Row::Console { .. }));
        assert!(panel.open_menu(console));
        let item = |panel: &DatabasePanel, label: &str| {
            panel
                .menu_items()
                .into_iter()
                .find(|item| item.label == label)
                .map_or(0, |item| item.id)
        };
        panel.menu_action(item(&panel, "Rename"));
        assert!(panel.text_focused());
        panel.key(EditKey::Backspace, false);
        panel.text("orders by status");
        panel.key(EditKey::Enter, false);
        assert!(!panel.text_focused());
        assert_eq!(context.context.consoles()[0].title, "orders by status");
        assert!(panel.open_menu(console));
        panel.menu_action(item(&panel, "Change Database..."));
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::OpenMenu]
        ));
        let web = item(&panel, "web > main");
        assert_ne!(web, 0);
        panel.menu_action(web);
        let web_name = context.context.databases()[1].name.clone();
        assert_eq!(context.context.consoles()[0].database, web_name);
        let requests = panel.take_requests();
        let toasts: Vec<&String> = requests
            .iter()
            .filter_map(|request| match request {
                PanelRequest::Toast(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(toasts, ["orders by status now runs on web > main"]);
    }

    #[test]
    fn shared_services_show_under_each_repo_that_uses_them() {
        let (_context, mut panel) = panel();
        panel.render(320.0, 2000.0);
        let placed: Vec<(String, String, Vec<String>)> = panel
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Database {
                    index,
                    repo,
                    shared_with,
                    ..
                } => Some((
                    repo.clone(),
                    panel.model.databases[*index].label.clone(),
                    shared_with.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            placed,
            [
                ("api".to_string(), "main".to_string(), Vec::new()),
                ("api".into(), "redis".into(), vec!["web".to_string()]),
                ("api".into(), "files".into(), Vec::new()),
                ("web".into(), "main".into(), Vec::new()),
                ("web".into(), "redis".into(), vec!["api".to_string()]),
                ("Other services".into(), "queue".into(), Vec::new()),
            ]
        );
    }

    #[test]
    fn the_filter_counts_hits_and_takes_typing_while_focused() {
        let (context, mut panel) = panel();
        panel.show_schema(&api_main(&context), users_schema());
        panel.click(DatabasePanel::base() + FILTER);
        assert!(panel.text_focused());
        assert!(panel.text("TOKEN"));
        panel.render(320.0, 600.0);
        assert_eq!(panel.found, 1);
        let tables: Vec<String> = panel
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Table { table, hit, .. } => Some(format!("{} {hit:?}", table.name)),
                _ => None,
            })
            .collect();
        assert_eq!(tables, ["login_tokens Some(6..11)"]);
        assert!(panel.key(EditKey::Escape, false));
        assert!(panel.filter.text().is_empty());
        assert!(panel.text_focused());
        panel.key(EditKey::Escape, false);
        assert!(!panel.text_focused());
    }

    #[test]
    fn menus_come_from_the_row_under_the_pointer() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        panel.show_schema(&name, users_schema());
        let table = row_id(
            &mut panel,
            |row| matches!(row, Row::Table { table, .. } if table.name == "users"),
        );
        assert!(panel.open_menu(table));
        let labels: Vec<String> = panel
            .menu_items()
            .iter()
            .map(|item| item.label.to_string())
            .collect();
        assert!(labels.contains(&"Show DDL".to_string()), "{labels:?}");
        let section = row_id(&mut panel, |row| matches!(row, Row::Section { .. }));
        assert!(!panel.open_menu(section));
        assert!(panel.menu_items().is_empty());
    }

    #[test]
    fn destructive_items_ask_and_name_what_is_lost() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        panel.show_schema(&name, users_schema());
        let table = row_id(
            &mut panel,
            |row| matches!(row, Row::Table { table, .. } if table.name == "users"),
        );
        for label in ["Truncate...", "Drop Table..."] {
            assert!(panel.open_menu(table));
            let id = panel
                .menu_items()
                .into_iter()
                .find(|item| item.label == label)
                .map_or(0, |item| item.id);
            panel.menu_action(id);
            match panel.take_requests().as_slice() {
                [PanelRequest::Prompt { detail, .. }] => {
                    assert!(detail.as_deref().is_some_and(|text| text.contains("users")))
                }
                _ => panic!("{label} asks first"),
            }
            panel.prompt_answered(CONFIRM_PROMPT, 1);
            assert!(panel.jobs.is_empty(), "{label} waits for the answer");
        }
    }

    #[test]
    fn copy_and_client_items_use_the_login_without_putting_secrets_in_argv() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        panel.show_schema(&name, users_schema());
        let database = row_id(&mut panel, |row| {
            matches!(row, Row::Database { index: 0, .. })
        });
        let item = |panel: &DatabasePanel, label: &str| {
            panel
                .menu_items()
                .into_iter()
                .find(|item| item.label == label)
                .map_or(0, |item| item.id)
        };
        let Some(Some(login)) = context
            .context
            .run_now(|connector| connector.login(&context.context.databases()[0]))
        else {
            panic!("a Postgres login");
        };
        assert!(panel.open_menu(database));
        panel.menu_action(item(&panel, "Open psql in Terminal"));
        match panel.take_requests().as_slice() {
            [PanelRequest::RunCommand { argv, env, .. }] => {
                assert!(argv[0].ends_with("psql"));
                let port = login.port.to_string();
                assert_eq!(
                    argv[1..],
                    [
                        "-h",
                        "localhost",
                        "-p",
                        port.as_str(),
                        "-U",
                        &login.user,
                        "-d",
                        &name
                    ]
                );
                assert!(!argv
                    .iter()
                    .any(|arg| arg.contains('@') || arg.starts_with("postgres://")));
                assert!(env.contains(&("PGPASSWORD".to_string(), login.password.clone())));
            }
            _ => panic!("opens psql"),
        }
        assert!(panel.open_menu(database));
        panel.menu_action(item(&panel, "Copy Connection URL"));
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Copy(url), PanelRequest::Toast(_)] if *url == login.url(Engine::Postgres)
        ));
        assert!(panel.open_menu(database));
        panel.menu_action(item(&panel, "Ask Claude about this schema"));
        match panel.take_requests().as_slice() {
            [PanelRequest::AskAgent(fix)] => {
                assert!(fix.prompt.contains("login_tokens"), "{}", fix.prompt);
                assert!(fix.prompt.contains("user_id bigint references users"));
            }
            _ => panic!("asks the agent"),
        }
    }

    #[test]
    fn expanding_a_table_shows_its_columns_with_keys() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        let mut schema = users_schema();
        schema.columns.push(column("users", "name", false));
        panel.show_schema(&name, schema);
        let users = row_id(
            &mut panel,
            |row| matches!(row, Row::Table { table, .. } if table.name == "users"),
        );
        panel.click(users + CHEVRON);
        assert!(panel.take_requests().is_empty());
        panel.render(320.0, 2000.0);
        let columns: Vec<(String, bool)> = panel
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Column { column, .. } => Some((column.name.clone(), column.primary_key)),
                _ => None,
            })
            .collect();
        assert_eq!(
            columns,
            [
                ("id".to_string(), true),
                ("email".into(), false),
                ("name".into(), false)
            ]
        );
        panel.click(users);
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Reveal { id, .. }] if id.starts_with("db-table:")
        ));
    }

    #[test]
    fn the_tree_works_from_the_keyboard() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        let mut schema = users_schema();
        schema.columns.push(column("users", "name", false));
        panel.show_schema(&name, schema);
        panel.render(320.0, 2000.0);
        assert_eq!(panel.key_context(), Some(workspace::keymap::DATABASE_PANEL));
        panel.set_keyboard(true);
        assert!(
            panel.selected.is_some(),
            "taking the keyboard picks the first row"
        );
        let users = |panel: &DatabasePanel| {
            panel
                .rows
                .iter()
                .position(|row| matches!(row, Row::Table { table, .. } if table.name == "users"))
        };
        while panel.selected_index() != users(&panel) {
            assert!(panel.list_key(EditKey::Down, false), "reaches the table");
        }
        assert!(panel.panel_action(KeyAction::DatabaseExpand));
        panel.render(320.0, 2000.0);
        assert!(panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Column { .. })));
        assert!(panel.panel_action(KeyAction::DatabaseCollapse));
        panel.render(320.0, 2000.0);
        assert!(!panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Column { .. })));
        assert!(panel.panel_action(KeyAction::DatabaseOpen));
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Reveal { id, .. }] if id.starts_with("db-table:")
        ));
        assert!(panel.panel_action(KeyAction::DatabaseCollapse));
        assert!(
            matches!(
                panel
                    .selected_index()
                    .and_then(|index| panel.rows.get(index)),
                Some(Row::Group { .. })
            ),
            "from a closed row it goes up to its parent"
        );
        assert!(panel.panel_action(KeyAction::DatabaseNewConsole));
        assert_eq!(context.context.consoles().len(), 1);
    }

    #[test]
    fn a_bucket_loads_fifty_at_a_time() {
        let (_context, mut panel) = panel();
        let object = |key: &str| ObjectEntry {
            key: key.into(),
            size: 10,
            ..ObjectEntry::default()
        };
        panel.show_buckets("files", vec!["uploads".into()]);
        panel.show_folder(
            "files",
            "uploads",
            "",
            Listing {
                prefixes: Vec::new(),
                objects: (0..50).map(|n| object(&format!("f{n}.txt"))).collect(),
                next: Some("page-2".into()),
            },
        );
        let more = row_id(&mut panel, |row| matches!(row, Row::More { .. }));
        let page = r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>f50.txt</Key><Size>1</Size></Contents></ListBucketResult>"#;
        let transport = std::sync::Arc::new(crate::tests::FakeTransport::answering(&[page]));
        panel.context.objects = transport.clone();
        panel.click(more);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            panel.render(320.0, 4000.0);
            let loading = panel
                .model
                .folders
                .values()
                .any(|folder| folder.loading.is_some());
            if !loading || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let objects = panel
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Object { .. }))
            .count();
        assert_eq!(objects, 51);
        assert!(!panel.rows.iter().any(|row| matches!(row, Row::More { .. })));
        assert!(transport
            .urls()
            .first()
            .is_some_and(|url| url.contains("continuation-token=page-2")));
    }

    fn show_missing(panel: &mut DatabasePanel, name: &str) {
        panel.show_failure(
            name,
            ConnectError::new(
                Engine::Postgres,
                name,
                "localhost",
                5434,
                "postgres",
                format!("database \"{name}\" does not exist"),
            ),
            None,
        );
    }

    #[test]
    fn a_failed_database_shows_its_block_and_marks_its_rows() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        show_missing(&mut panel, &name);
        panel.render(320.0, 600.0);
        assert!(panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Failure { index: 0 })));
        let failed = |panel: &DatabasePanel, repo: &str| {
            panel.rows.iter().any(
                |row| matches!(row, Row::Repo { repo: shown, failed: true, .. } if shown == repo),
            )
        };
        assert!(failed(&panel, "api"));
        assert!(!failed(&panel, "web"));
        panel.fold(&name);
        panel.render(320.0, 600.0);
        assert!(!panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Failure { .. })));
        assert!(failed(&panel, "api"));
    }

    #[test]
    fn failure_controls_ask_the_app() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        show_missing(&mut panel, &name);
        panel.render(320.0, 600.0);
        panel.take_requests();
        panel.click(DatabasePanel::failure_id(0, FailureAction::CopyError));
        match panel.take_requests().as_slice() {
            [PanelRequest::Copy(text), PanelRequest::Toast(_)] => {
                assert!(text.contains("does not exist"), "{text}")
            }
            _ => panic!("copy the error"),
        }
        panel.click(DatabasePanel::failure_id(0, FailureAction::FixWithAgent));
        match panel.take_requests().as_slice() {
            [PanelRequest::FixWithAgent(fix)] => {
                assert!(fix.prompt.contains(&name), "{}", fix.prompt);
                assert_eq!(fix.cwd, context.context.workspace_root);
            }
            _ => panic!("start the agent"),
        }
        panel.click(DatabasePanel::failure_id(0, FailureAction::EditConfig));
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::OpenFile(path)] if *path == context.context.config_path
        ));
        panel.click(DatabasePanel::failure_id(0, FailureAction::ToggleRaw));
        assert!(panel.failure(&name).is_some_and(|failure| failure.raw_open));
    }

    #[test]
    fn a_fix_runs_off_the_ui_thread_and_reports_its_error() {
        let (context, mut panel) = panel();
        let name = api_main(&context);
        show_missing(&mut panel, &name);
        panel.click(DatabasePanel::failure_id(0, FailureAction::CreateDatabase));
        assert!(panel
            .failure(&name)
            .is_some_and(|failure| failure.busy.is_some()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while panel.fixes.contains_key(&name) && Instant::now() < deadline {
            panel.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        let failure = panel.failure(&name);
        assert!(failure.is_some_and(|failure| failure.busy.is_none()));
        assert!(failure.is_some_and(|failure| failure.action_error.is_some()));
    }
}
