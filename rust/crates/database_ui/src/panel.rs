use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pom_db::{
    ConnectError, ConnectErrorKind, Connector, Console, ConsoleKind, Database, Engine, Table,
    TableKind,
};
use ui::{div, icon, label, measure, theme, IconKind, Node, Rgba};
use workspace::persistence::SerializedItem;
use workspace::text_field::{FieldFont, TextField};
use workspace::{
    side_panel_base, AgentFix, EditKey, Item, MenuItem, PaneKind, PanelRequest, SidePanelView,
};

use crate::console::{console_item, console_item_id, new_console, restore_console};
use crate::failure::{Failure, FailureAction, ACTION_STRIDE};
use crate::{DatabaseContext, Pending, TableItem};

const ROW_H: f32 = 22.0;
const INDENT: f32 = 16.0;
const HEADER_H: f32 = 30.0;
const FILTER_H: f32 = 28.0;
const BUTTON: f32 = 20.0;
const ROW_STRIDE: u64 = 4;
const REFRESH: u64 = 9_000_000;
const NEW_CONSOLE: u64 = REFRESH + 1;
const FILTER: u64 = REFRESH + 2;
const DELETE_CONSOLE: u64 = REFRESH + 3;
const FAILURE: u64 = REFRESH + 16;
const FAILURE_BLOCK_LEFT: f32 = 36.0;
const FAILURE_BLOCK_RIGHT: f32 = 4.0;
const FAILURE_BLOCK_TOP: f32 = 2.0;
const FAILURE_BLOCK_BOTTOM: f32 = 6.0;
const SERVER_START_WAIT: Duration = Duration::from_secs(30);
const DELETE_PROMPT: u64 = 1;

enum Tables {
    Loading(Pending<Opened>),
    Loaded(Vec<Table>),
    Failed(Box<Failure>),
}

enum Opened {
    Tables(Vec<Table>),
    Failed(Box<Failure>),
}

#[derive(Clone)]
enum Row {
    Consoles {
        collapsed: bool,
    },
    Console(Console),
    Repo {
        repo: String,
        collapsed: bool,
    },
    Database {
        index: usize,
        open: bool,
    },
    Table {
        index: usize,
        table: Table,
    },
    Note {
        depth: usize,
        text: String,
        error: bool,
    },
    Failure {
        index: usize,
    },
}

pub struct DatabasePanel {
    context: DatabaseContext,
    databases: Vec<Database>,
    tables: HashMap<String, Tables>,
    collapsed_repos: HashSet<String>,
    open: HashSet<String>,
    rows: Vec<Row>,
    scroll: f32,
    viewport_h: f32,
    hover: Option<u64>,
    requests: Vec<PanelRequest>,
    consoles: Vec<Console>,
    consoles_collapsed: bool,
    filter: TextField,
    filter_focused: bool,
    menu_console: Option<String>,
    /// Fixes running for a failed database, by its name; each answers with what to tell the person.
    fixes: HashMap<String, Pending<String>>,
    width: f32,
}

impl DatabasePanel {
    pub fn new(context: DatabaseContext) -> DatabasePanel {
        let databases = context.databases();
        let mut panel = DatabasePanel {
            context,
            databases,
            tables: HashMap::new(),
            collapsed_repos: HashSet::new(),
            open: HashSet::new(),
            rows: Vec::new(),
            scroll: 0.0,
            viewport_h: 0.0,
            hover: None,
            requests: Vec::new(),
            consoles: Vec::new(),
            consoles_collapsed: false,
            filter: {
                let mut field = TextField::default();
                field.set_font_size(12.0);
                field
            },
            filter_focused: false,
            menu_console: None,
            fixes: HashMap::new(),
            width: 320.0,
        };
        panel.load_consoles();
        if let Some(first) = panel.databases.first().map(|db| db.name.clone()) {
            panel.toggle_database(&first);
        }
        panel
    }

    fn base() -> u64 {
        side_panel_base(PaneKind::Database)
    }

    fn id(row: usize) -> u64 {
        Self::base() + row as u64 * ROW_STRIDE
    }

    fn decode(id: u64) -> Option<usize> {
        let offset = id.checked_sub(Self::base())?;
        (offset < REFRESH).then_some((offset / ROW_STRIDE) as usize)
    }

    fn load_consoles(&mut self) {
        self.consoles = self
            .context
            .consoles()
            .into_iter()
            .filter(|console| console.kind == ConsoleKind::Query)
            .collect();
    }

    fn open_console(&mut self, console: Console) {
        let context = self.context.clone();
        self.requests.push(PanelRequest::Reveal {
            id: console_item_id(&console),
            open: Box::new(move || Some(console_item(context, console))),
        });
    }

    fn create_console(&mut self) {
        let Some(database) = self
            .databases
            .iter()
            .find(|database| database.engine == Engine::Postgres)
            .or(self.databases.first())
        else {
            return;
        };
        let mut saved = self.context.consoles();
        let console = new_console(&saved, database);
        saved.push(console.clone());
        self.context.save_consoles(&saved);
        self.load_consoles();
        self.open_console(console);
    }

    fn delete_console(&mut self, id: &str) {
        let mut saved = self.context.consoles();
        saved.retain(|console| console.id != id);
        self.context.save_consoles(&saved);
        self.load_consoles();
    }

    fn filter_matches(&self, text: &str) -> bool {
        let filter = self.filter.text();
        filter.is_empty() || text.to_lowercase().contains(&filter.trim().to_lowercase())
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter.set_text(filter);
        self.filter.move_to_end();
    }

    fn database(&self, name: &str) -> Option<&Database> {
        self.databases.iter().find(|db| db.name == name)
    }

    fn load_tables(&mut self, name: &str) {
        let Some(database) = self.database(name).cloned() else {
            return;
        };
        let pending = self
            .context
            .run(move |connector| Ok(open_database(connector, &database)));
        self.tables
            .insert(name.to_string(), Tables::Loading(pending));
    }

    /// Shows these tables for a database as if they had loaded (previews and tests).
    pub fn show_tables(&mut self, database: &str, tables: Vec<Table>) {
        self.open.insert(database.to_string());
        self.tables
            .insert(database.to_string(), Tables::Loaded(tables));
    }

    /// Shows this failure for a database as if opening it had failed (previews and tests).
    pub fn show_failure(&mut self, database: &str, error: ConnectError, main_copy: Option<String>) {
        let repo = self.repo_key(database);
        let mut failure = Failure::new(error, repo);
        failure.main_copy = main_copy;
        self.open.insert(database.to_string());
        self.tables
            .insert(database.to_string(), Tables::Failed(Box::new(failure)));
    }

    pub fn toggle_full_error(&mut self, database: &str) {
        if let Some(failure) = self.failure_mut(database) {
            failure.raw_open = !failure.raw_open;
        }
    }

    /// Folds a database row without dropping what it loaded.
    pub fn fold(&mut self, database: &str) {
        self.open.remove(database);
    }

    fn repo_key(&self, name: &str) -> String {
        let config = (self.context.config)();
        match (config, self.database(name)) {
            (Some(config), Some(database)) => pom_db::repo_of(&config, database)
                .map(|(key, _)| key.to_string())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn failure(&self, name: &str) -> Option<&Failure> {
        match self.tables.get(name) {
            Some(Tables::Failed(failure)) => Some(failure),
            _ => None,
        }
    }

    fn failure_mut(&mut self, name: &str) -> Option<&mut Failure> {
        match self.tables.get_mut(name) {
            Some(Tables::Failed(failure)) => Some(failure),
            _ => None,
        }
    }

    fn failure_id(index: usize, action: FailureAction) -> u64 {
        Self::base() + FAILURE + index as u64 * ACTION_STRIDE + action.offset()
    }

    fn decode_failure(id: u64) -> Option<(usize, FailureAction)> {
        let offset = id.checked_sub(Self::base() + FAILURE)?;
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

    fn failure_action(&mut self, index: usize, action: FailureAction) {
        let Some(database) = self.databases.get(index).cloned() else {
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
        match action {
            FailureAction::ToggleRaw => self.toggle_full_error(&name),
            FailureAction::CopyError => {
                self.requests.push(PanelRequest::Copy(report));
                self.requests
                    .push(PanelRequest::Toast("Copied the error".into()));
            }
            FailureAction::FixWithAgent => {
                self.requests.push(PanelRequest::FixWithAgent(AgentFix {
                    prompt,
                    cwd: self.checkout(&repo),
                }));
            }
            FailureAction::EditConfig => self
                .requests
                .push(PanelRequest::OpenFile(self.context.config_path.clone())),
            FailureAction::Retry => self.load_tables(&name),
            FailureAction::CreateDatabase => {
                self.start_fix(&name, "Creating database...", move |connector| {
                    connector.create_database(&database.name)?;
                    Ok(format!(
                        "Created an empty {} > {} database",
                        database.repo, database.label
                    ))
                })
            }
            FailureAction::CopyFromMain => {
                let Some(main) = main_copy else {
                    return;
                };
                self.start_fix(&name, "Copying from main...", move |connector| {
                    connector.copy_database(&main, &database.name)?;
                    Ok(format!(
                        "Copied main's {} > {} data into this workspace",
                        database.repo, database.label
                    ))
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
                    Ok("Started shared services".to_string())
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

    fn toggle_database(&mut self, name: &str) {
        if !self.open.remove(name) {
            self.open.insert(name.to_string());
            if !self.tables.contains_key(name) {
                self.load_tables(name);
            }
        }
    }

    fn refresh(&mut self) {
        self.load_consoles();
        self.databases = self.context.databases();
        self.tables.clear();
        let open: Vec<String> = self.open.iter().cloned().collect();
        for name in open {
            self.load_tables(&name);
        }
    }

    fn poll(&mut self) {
        let engines: HashMap<String, Engine> = self
            .databases
            .iter()
            .map(|database| (database.name.clone(), database.engine))
            .collect();
        for (name, tables) in self.tables.iter_mut() {
            if let Tables::Loading(pending) = tables {
                match pending.poll() {
                    Some(Ok(Opened::Tables(loaded))) => *tables = Tables::Loaded(loaded),
                    Some(Ok(Opened::Failed(failure))) => *tables = Tables::Failed(failure),
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
                        *tables = Tables::Failed(Box::new(Failure::new(error, String::new())));
                    }
                    None => {}
                }
            }
        }
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
                    self.requests.push(PanelRequest::Toast(message));
                    self.load_tables(&name);
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

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let consoles: Vec<Console> = self
            .consoles
            .iter()
            .filter(|console| self.filter_matches(&console.title))
            .cloned()
            .collect();
        if !consoles.is_empty() {
            rows.push(Row::Consoles {
                collapsed: self.consoles_collapsed,
            });
            if !self.consoles_collapsed {
                rows.extend(consoles.into_iter().map(Row::Console));
            }
        }
        let mut repos: Vec<&str> = Vec::new();
        for database in &self.databases {
            if !repos.contains(&database.repo.as_str()) {
                repos.push(&database.repo);
            }
        }
        for repo in repos {
            let collapsed = self.collapsed_repos.contains(repo);
            rows.push(Row::Repo {
                repo: repo.to_string(),
                collapsed,
            });
            if collapsed {
                continue;
            }
            for (index, database) in self.databases.iter().enumerate() {
                if database.repo != repo {
                    continue;
                }
                let open = self.open.contains(&database.name);
                rows.push(Row::Database { index, open });
                if !open {
                    continue;
                }
                match self.tables.get(&database.name) {
                    Some(Tables::Loaded(tables)) if tables.is_empty() => rows.push(Row::Note {
                        depth: 2,
                        text: if database.engine == Engine::Redis {
                            "No keys".into()
                        } else {
                            "No tables".into()
                        },
                        error: false,
                    }),
                    Some(Tables::Loaded(tables)) => {
                        let before = rows.len();
                        rows.extend(
                            tables
                                .iter()
                                .filter(|table| self.filter_matches(&table.qualified()))
                                .map(|table| Row::Table {
                                    index,
                                    table: table.clone(),
                                }),
                        );
                        if rows.len() == before {
                            rows.push(Row::Note {
                                depth: 2,
                                text: "No matches".into(),
                                error: false,
                            });
                        }
                    }
                    Some(Tables::Failed(_)) => rows.push(Row::Failure { index }),
                    Some(Tables::Loading(_)) | None => rows.push(Row::Note {
                        depth: 2,
                        text: "Loading...".into(),
                        error: false,
                    }),
                }
            }
        }
        self.rows = rows;
    }

    fn guide(&self) -> Node {
        div()
            .row()
            .w_px(INDENT)
            .h_px(ROW_H)
            .justify_center()
            .child(div().w_px(1.0).h_px(ROW_H).bg(theme().panel_indent_guide))
            .into()
    }

    fn slot(&self, kind: IconKind, color: Rgba) -> Node {
        div()
            .row()
            .w_px(INDENT)
            .h_px(ROW_H)
            .items_center()
            .justify_center()
            .child(icon(kind).size(12.0).color(color))
            .into()
    }

    fn render_row(&self, index: usize, row: &Row) -> Node {
        let colors = theme();
        let id = Self::id(index);
        let mut line = div()
            .row()
            .h_px(ROW_H)
            .pl(8.0)
            .pr(8.0)
            .items_center()
            .rounded(4.0)
            .on_click(id);
        if self.hover == Some(id) {
            line = line.bg(colors.ghost_element_hover);
        }
        let name = |text: String, color: Rgba| {
            div()
                .row()
                .flex(1.0)
                .pl(4.0)
                .items_center()
                .child(label(text).color(color).truncate())
        };
        match row {
            Row::Consoles { collapsed } => line
                .child(self.slot(
                    if *collapsed {
                        IconKind::ChevronRight
                    } else {
                        IconKind::ChevronDown
                    },
                    colors.icon_muted,
                ))
                .child(name("Consoles".into(), colors.text))
                .child(
                    label(self.consoles.len().to_string())
                        .size(11.0)
                        .color(colors.text_placeholder),
                )
                .into(),
            Row::Console(console) => {
                let database = self.database(&console.database).map_or_else(
                    || console.database.clone(),
                    |database| database.label.clone(),
                );
                line.child(self.guide())
                    .child(self.slot(IconKind::File, colors.icon_muted))
                    .child(name(console.title.clone(), colors.text_muted))
                    .child(label(database).size(11.0).color(colors.text_placeholder))
                    .into()
            }
            Row::Repo { repo, collapsed } => {
                let engine = self
                    .databases
                    .iter()
                    .find(|db| db.repo == *repo)
                    .map_or("", |db| match db.engine {
                        Engine::Postgres => "postgres",
                        Engine::Redis => "redis",
                    });
                line.child(self.slot(
                    if *collapsed {
                        IconKind::ChevronRight
                    } else {
                        IconKind::ChevronDown
                    },
                    colors.icon_muted,
                ))
                .child(name(repo.clone(), colors.text))
                .children(self.repo_dot(repo).map(dot))
                .child(label(engine).size(11.0).color(colors.text_placeholder))
                .into()
            }
            Row::Database { index, open } => {
                let database = &self.databases[*index];
                let count = match self.tables.get(&database.name) {
                    Some(Tables::Loaded(tables)) => tables.len().to_string(),
                    _ => String::new(),
                };
                line.child(self.guide())
                    .child(self.slot(
                        if *open {
                            IconKind::ChevronDown
                        } else {
                            IconKind::ChevronRight
                        },
                        colors.icon_muted,
                    ))
                    .child(self.slot(IconKind::Cylinder, colors.icon_muted))
                    .child(name(database.label.clone(), colors.text))
                    .children(self.failure(&database.name).map(|failure| {
                        dot(if failure.is_warning() {
                            colors.warning
                        } else {
                            colors.error
                        })
                    }))
                    .child(label(count).size(11.0).color(colors.text_placeholder))
                    .into()
            }
            Row::Table { table, .. } => {
                let kind = match table.kind {
                    TableKind::Table => IconKind::Table,
                    TableKind::View => IconKind::Eye,
                    TableKind::Keyspace => IconKind::Key,
                };
                let trailing = table
                    .count
                    .map(|count| count.to_string())
                    .unwrap_or_default();
                line.child(self.guide())
                    .child(self.guide())
                    .child(self.slot(kind, colors.icon_muted))
                    .child(name(table.qualified(), colors.text_muted))
                    .child(label(trailing).size(11.0).color(colors.text_placeholder))
                    .into()
            }
            Row::Failure { index } => {
                let Some(failure) = self
                    .databases
                    .get(*index)
                    .and_then(|database| self.failure(&database.name))
                else {
                    return div().into();
                };
                let block_width = self.width - 8.0 - FAILURE_BLOCK_LEFT - FAILURE_BLOCK_RIGHT;
                let row = *index;
                div()
                    .row()
                    .pl(FAILURE_BLOCK_LEFT)
                    .pr(FAILURE_BLOCK_RIGHT)
                    .pt(FAILURE_BLOCK_TOP)
                    .pb(FAILURE_BLOCK_BOTTOM)
                    .child(failure.render(
                        block_width,
                        |action| Self::failure_id(row, action),
                        self.hover,
                    ))
                    .into()
            }
            Row::Note { depth, text, error } => {
                let mut note = div().row().h_px(ROW_H).pl(8.0).items_center();
                for _ in 0..*depth {
                    note = note.child(self.guide());
                }
                note.child(div().w_px(INDENT))
                    .child(
                        label(text.clone())
                            .size(12.0)
                            .color(if *error {
                                colors.error
                            } else {
                                colors.text_muted
                            })
                            .truncate(),
                    )
                    .into()
            }
        }
    }

    /// The color of the dot on a repo's header: red when one of its databases failed, yellow for a warning only.
    fn repo_dot(&self, repo: &str) -> Option<Rgba> {
        let colors = theme();
        let failures: Vec<&Failure> = self
            .databases
            .iter()
            .filter(|database| database.repo == repo)
            .filter_map(|database| self.failure(&database.name))
            .collect();
        if failures.is_empty() {
            None
        } else if failures.iter().all(|failure| failure.is_warning()) {
            Some(colors.warning)
        } else {
            Some(colors.error)
        }
    }

    fn row_height(&self, index: usize, row: &Row) -> f32 {
        match row {
            Row::Failure { .. } => measure(&self.render_row(index, row)).1,
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
    }

    fn header_button(&self, offset: u64, kind: IconKind) -> Node {
        let colors = theme();
        let id = Self::base() + offset;
        let hot = self.hover == Some(id);
        div()
            .w_px(BUTTON)
            .h_px(BUTTON)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .bg(if hot {
                colors.element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .child(
                icon(kind)
                    .size(12.0)
                    .color(if hot { colors.icon } else { colors.icon_muted }),
            )
            .into()
    }

    fn filter_box(&self) -> Node {
        let colors = theme();
        div()
            .row()
            .h_px(FILTER_H)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .on_click(Self::base() + FILTER)
            .child(icon(IconKind::Search).size(12.0).color(colors.icon_muted))
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .items_center()
                    .child(self.filter.render(
                        "Filter tables",
                        self.filter_focused,
                        colors.text,
                        FILTER_H - 8.0,
                        FieldFont::Ui,
                    )),
            )
            .into()
    }
}

/// Wide enough for an error block's facts and its buttons two to a line.
const MIN_WIDTH: f32 = 260.0;

impl SidePanelView for DatabasePanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Database
    }

    fn min_width(&self) -> f32 {
        MIN_WIDTH
    }

    fn render(&mut self, width: f32, height: f32) -> Node {
        self.poll();
        self.width = width;
        self.viewport_h = height;
        self.rebuild_rows();
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let colors = theme();
        let header = div()
            .row()
            .h_px(HEADER_H)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(label("Database").size(12.0).color(colors.text_muted))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(self.context.branch.clone())
                        .size(11.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            )
            .child(self.header_button(NEW_CONSOLE, IconKind::Plus))
            .child(self.header_button(REFRESH, IconKind::RotateCw));
        let mut list = div().col().px(4.0);
        if self.rows.is_empty() {
            list = list.child(
                div().row().h_px(ROW_H).px(8.0).items_center().child(
                    label("No databases in pom.yml")
                        .size(12.0)
                        .color(colors.text_muted),
                ),
            );
        }
        let mut top = 0.0;
        for (index, row) in self.rows.iter().enumerate() {
            let row_height = self.row_height(index, row);
            let bottom = top + row_height;
            if bottom > self.scroll && top < self.scroll + height {
                list = list.child(self.render_row(index, row));
            }
            top = bottom;
        }
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(self.filter_box())
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(list)
            .into()
    }

    fn click(&mut self, id: u64) {
        self.filter_focused = id == Self::base() + FILTER;
        match id.checked_sub(Self::base()) {
            Some(REFRESH) => return self.refresh(),
            Some(NEW_CONSOLE) => return self.create_console(),
            Some(FILTER) => return,
            _ => {}
        }
        if let Some((index, action)) = Self::decode_failure(id) {
            return self.failure_action(index, action);
        }
        let Some(row) = Self::decode(id).and_then(|index| self.rows.get(index).cloned()) else {
            return;
        };
        match row {
            Row::Consoles { .. } => self.consoles_collapsed = !self.consoles_collapsed,
            Row::Console(console) => self.open_console(console),
            Row::Repo { repo, .. } => {
                if !self.collapsed_repos.remove(&repo) {
                    self.collapsed_repos.insert(repo);
                }
            }
            Row::Database { index, .. } => {
                let name = self.databases[index].name.clone();
                self.toggle_database(&name);
            }
            Row::Table { index, table } => {
                let database = self.databases[index].clone();
                let context = self.context.clone();
                self.requests.push(PanelRequest::Reveal {
                    id: TableItem::item_id(&database, &table),
                    open: Box::new(move || {
                        Some(Box::new(TableItem::new(context, database, table)))
                    }),
                });
            }
            Row::Note { .. } | Row::Failure { .. } => {}
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
        let row = Self::decode(id).and_then(|index| self.rows.get(index));
        self.menu_console = match row {
            Some(Row::Console(console)) => Some(console.id.clone()),
            _ => None,
        };
        self.menu_console.is_some()
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        if self.menu_console.is_none() {
            return Vec::new();
        }
        vec![MenuItem {
            id: Self::base() + DELETE_CONSOLE,
            label: "Delete Console".into(),
            checked: false,
            sep: false,
            disabled: false,
        }]
    }

    fn menu_action(&mut self, item: u64) {
        if item != Self::base() + DELETE_CONSOLE {
            self.menu_console = None;
            return;
        }
        let Some(title) = self
            .menu_console
            .as_ref()
            .and_then(|id| self.consoles.iter().find(|console| console.id == *id))
            .map(|console| console.title.clone())
        else {
            return;
        };
        self.requests.push(PanelRequest::Prompt {
            tag: DELETE_PROMPT,
            message: format!("Delete the console \"{title}\"?"),
            detail: Some("Its SQL is removed and cannot be recovered.".into()),
            buttons: vec!["Delete".into(), "Cancel".into()],
        });
    }

    fn prompt_answered(&mut self, tag: u64, answer: usize) {
        let Some(id) = self.menu_console.take() else {
            return;
        };
        if tag == DELETE_PROMPT && answer == 0 {
            self.delete_console(&id);
        }
    }

    fn text_focused(&self) -> bool {
        self.filter_focused
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        self.filter.insert(&typed);
        self.scroll = 0.0;
        true
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape if self.filter.text().is_empty() => self.filter_focused = false,
            EditKey::Escape => self.filter.set_text(""),
            EditKey::Enter => self.filter_focused = false,
            _ => {
                self.scroll = 0.0;
                return self.filter.key(key, shift);
            }
        }
        true
    }

    fn blur(&mut self) {
        self.filter_focused = false;
    }

    fn restore_item(&mut self, item: &SerializedItem) -> Option<Box<dyn Item>> {
        restore_console(&self.context, item)
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }
}

fn dot(color: Rgba) -> Node {
    div()
        .row()
        .w_px(11.0)
        .h_px(ROW_H)
        .items_center()
        .justify_center()
        .child(div().w_px(7.0).h_px(7.0).rounded(3.5).bg(color))
        .into()
}

/// The database's tables, or why it failed with the fixes that apply to it.
fn open_database(connector: &Connector<'_>, database: &Database) -> Opened {
    let error = match connector.open(database) {
        Ok(tables) => return Opened::Tables(tables),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row_id(panel: &mut DatabasePanel, wanted: impl Fn(&Row) -> bool) -> u64 {
        panel.render(320.0, 600.0);
        match panel.rows.iter().position(wanted) {
            Some(index) => DatabasePanel::id(index),
            None => panic!("row not shown"),
        }
    }

    #[test]
    fn new_console_is_saved_listed_and_opened() {
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        panel.click(DatabasePanel::base() + NEW_CONSOLE);
        let saved = context.context.consoles();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].title, "query 1");
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Reveal { id, .. }] if *id == console_item_id(&saved[0])
        ));
        let console = row_id(&mut panel, |row| matches!(row, Row::Console(_)));
        panel.click(console);
        assert_eq!(panel.take_requests().len(), 1);
    }

    #[test]
    fn a_console_is_deleted_only_after_confirming() {
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        panel.click(DatabasePanel::base() + NEW_CONSOLE);
        panel.take_requests();
        let console = row_id(&mut panel, |row| matches!(row, Row::Console(_)));
        assert!(panel.open_menu(console));
        let delete = panel.menu_items()[0].id;
        panel.menu_action(delete);
        assert!(matches!(
            panel.take_requests().as_slice(),
            [PanelRequest::Prompt {
                tag: DELETE_PROMPT,
                ..
            }]
        ));
        panel.prompt_answered(DELETE_PROMPT, 1);
        assert_eq!(context.context.consoles().len(), 1);
        assert!(panel.open_menu(console));
        panel.menu_action(delete);
        panel.prompt_answered(DELETE_PROMPT, 0);
        assert!(context.context.consoles().is_empty());
        panel.render(320.0, 600.0);
        assert!(!panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Consoles { .. })));
    }

    #[test]
    fn the_filter_narrows_tables_and_takes_typing_while_focused() {
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        let table = |name: &str| Table {
            schema: "public".into(),
            name: name.into(),
            kind: TableKind::Table,
            count: None,
        };
        let name = context.context.databases()[0].name.clone();
        panel.show_tables(&name, vec![table("users"), table("orders")]);
        panel.click(DatabasePanel::base() + FILTER);
        assert!(panel.text_focused());
        assert!(panel.text("ORD"));
        panel.render(320.0, 600.0);
        let tables: Vec<String> = panel
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Table { table, .. } => Some(table.name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tables, ["orders"]);
        assert!(panel.key(EditKey::Escape, false));
        assert!(panel.filter.text().is_empty());
        assert!(panel.text_focused());
        panel.key(EditKey::Escape, false);
        assert!(!panel.text_focused());
        panel.click(DatabasePanel::base() + FILTER);
        panel.blur();
        assert!(!panel.text_focused());
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
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        let name = context.context.databases()[0].name.clone();
        show_missing(&mut panel, &name);
        panel.render(320.0, 600.0);
        assert!(panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Failure { index: 0 })));
        assert_eq!(panel.repo_dot("api"), Some(theme().error));
        assert_eq!(panel.repo_dot("web"), None);
        panel.fold(&name);
        panel.render(320.0, 600.0);
        assert!(!panel
            .rows
            .iter()
            .any(|row| matches!(row, Row::Failure { .. })));
        assert_eq!(panel.repo_dot("api"), Some(theme().error));
    }

    #[test]
    fn failure_controls_ask_the_app() {
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        let name = context.context.databases()[0].name.clone();
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
        let context = crate::tests::context();
        let mut panel = DatabasePanel::new(context.context.clone());
        let name = context.context.databases()[0].name.clone();
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
