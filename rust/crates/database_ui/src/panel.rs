//! The Database panel: the workspace's databases grouped by repo, each opening to its tables (schema-qualified
//! outside `public`), views and Redis keyspaces. Tables load when a database is first opened; clicking one opens
//! its table tab.

use std::collections::{HashMap, HashSet};

use pom_db::{Database, Engine, Table, TableKind};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::{side_panel_base, PaneKind, PanelRequest, SidePanelView};

use crate::{DatabaseContext, Pending, TableItem};

const ROW_H: f32 = 22.0;
const INDENT: f32 = 16.0;
const HEADER_H: f32 = 30.0;
const BUTTON: f32 = 20.0;
const ROW_STRIDE: u64 = 4;
const REFRESH: u64 = 9_000_000;

enum Tables {
    Loading(Pending<Vec<Table>>),
    Loaded(Vec<Table>),
    Failed(String),
}

#[derive(Clone)]
enum Row {
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
        };
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

    fn database(&self, name: &str) -> Option<&Database> {
        self.databases.iter().find(|db| db.name == name)
    }

    fn load_tables(&mut self, name: &str) {
        let Some(database) = self.database(name).cloned() else {
            return;
        };
        let pending = self
            .context
            .run(move |connector| connector.tables(&database));
        self.tables
            .insert(name.to_string(), Tables::Loading(pending));
    }

    /// Shows these tables for a database as if they had loaded (previews and tests).
    pub fn show_tables(&mut self, database: &str, tables: Vec<Table>) {
        self.open.insert(database.to_string());
        self.tables
            .insert(database.to_string(), Tables::Loaded(tables));
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
        self.databases = self.context.databases();
        self.tables.clear();
        let open: Vec<String> = self.open.iter().cloned().collect();
        for name in open {
            self.load_tables(&name);
        }
    }

    fn poll(&mut self) {
        for tables in self.tables.values_mut() {
            if let Tables::Loading(pending) = tables {
                match pending.poll() {
                    Some(Ok(loaded)) => *tables = Tables::Loaded(loaded),
                    Some(Err(error)) => *tables = Tables::Failed(error),
                    None => {}
                }
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
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
                        rows.extend(tables.iter().map(|table| Row::Table {
                            index,
                            table: table.clone(),
                        }))
                    }
                    Some(Tables::Failed(error)) => rows.push(Row::Note {
                        depth: 2,
                        text: error.lines().next().unwrap_or_default().to_string(),
                        error: true,
                    }),
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

    fn content_height(&self) -> f32 {
        HEADER_H + self.rows.len() as f32 * ROW_H
    }
}

impl SidePanelView for DatabasePanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Database
    }

    fn render(&mut self, width: f32, height: f32) -> Node {
        self.poll();
        self.viewport_h = height;
        self.rebuild_rows();
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let colors = theme();
        let refresh_id = Self::base() + REFRESH;
        let refresh_hot = self.hover == Some(refresh_id);
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
            .child(
                div()
                    .w_px(BUTTON)
                    .h_px(BUTTON)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(refresh_id)
                    .bg(if refresh_hot {
                        colors.element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(icon(IconKind::RotateCw).size(12.0).color(if refresh_hot {
                        colors.icon
                    } else {
                        colors.icon_muted
                    })),
            );
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
        let first = (self.scroll / ROW_H).floor() as usize;
        let visible = (height / ROW_H).ceil() as usize + 2;
        for (index, row) in self.rows.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.render_row(index, row));
        }
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(list)
            .into()
    }

    fn click(&mut self, id: u64) {
        if id == Self::base() + REFRESH {
            self.refresh();
            return;
        }
        let Some(row) = Self::decode(id).and_then(|index| self.rows.get(index).cloned()) else {
            return;
        };
        match row {
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
            Row::Note { .. } => {}
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

    fn open_menu(&mut self, _id: u64) -> bool {
        false
    }

    fn menu_items(&self) -> Vec<workspace::MenuItem> {
        Vec::new()
    }

    fn menu_action(&mut self, _item: u64) {}

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }
}
