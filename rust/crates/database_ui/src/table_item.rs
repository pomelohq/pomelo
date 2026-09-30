//! A table tab: one page of a table's rows in the grid, with WHERE / ORDER BY fields and a filter box per column,
//! a page size, paging ("1-500 of 1234"), sorting by clicking a column header, copying a cell and exporting the
//! whole result as CSV. Beside the grid a Details side shows the selected cell's whole value (JSON as a tree) or
//! its row as a record with the tables pointing at it; a foreign key opens the row it points at. Cells are edited
//! in place and saved together in one transaction. Structure and DDL views show the table's shape.
//! A Redis keyspace tab lists its keys instead (no filter, paging or editing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use pom_db::{Database, Engine, QueryResult, Table, TableKind, TableStructure};
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextArea, TextField};
use workspace::{segmented, EditKey, Item, ItemTick, PanelRequest, TerminalKeyOutcome};

use crate::details::{
    self, CellView, FoldAction, FoldTarget, JsonFold, Pane, RowField, Target, CANCEL_VALUE,
    COLLAPSE, COPY_DDL, COPY_VALUE, EXPAND_ALL, FIELD_BASE, FIELD_LINK_BASE, FOLD_BASE, FOLD_END,
    OPEN_IN_TAB, OPEN_TARGET, RANGE_END, REFERENCE_BASE, REVERT, ROW_COLLAPSE_BASE, ROW_COPY_BASE,
    ROW_END, ROW_EXPAND_BASE, ROW_OPEN_BASE, SAVE_VALUE, SET_NULL, SIDE_CLOSE, SIDE_ROW,
    SIDE_VALUE, STRUCTURE_LINK_BASE, STRUCTURE_REFERENCE_BASE, VALUE_AREA,
};
use crate::grid::{Grid, GridEvent};
use crate::json::{self, Json};
use crate::{DatabaseContext, Pending};

const TOOLBAR_H: f32 = 36.0;
const QUERY_H: f32 = 32.0;
/// Narrower than this (design px), the tab gives the whole width to the grid and hides the Details side.
const SIDE_NEEDS: f32 = 620.0;
/// Narrower than this, WHERE and ORDER BY take a row each.
const QUERY_STACK_BELOW: f32 = 520.0;
const STATUS_H: f32 = 28.0;
const PENDING_H: f32 = 34.0;
const FIELD_H: f32 = 24.0;
const FILTER_H: f32 = 26.0;
const FIELD_FONT: f32 = 12.0;
const SIDE_W: f32 = 360.0;
const SIDE_HEAD_H: f32 = 36.0;
const PAGE_SIZES: [usize; 4] = [100, 500, 1000, 5000];
const DEFAULT_PAGE: usize = 500;
/// A value longer than this, or on several lines, is edited in the Details side instead of in its cell.
const INLINE_EDIT_MAX: usize = 200;
const FILE_CHECK: Duration = Duration::from_millis(500);
/// Columns that name a row, the first one a table has standing for its rows next to foreign keys.
const LABEL_COLUMNS: [&str; 9] = [
    "name",
    "title",
    "label",
    "display_name",
    "full_name",
    "email",
    "username",
    "slug",
    "code",
];

const WHERE_FIELD: u64 = 1;
const ORDER_FIELD: u64 = 2;
const PAGE_SIZE: u64 = 3;
const REFRESH: u64 = 4;
const FIRST_PAGE: u64 = 5;
const PREVIOUS_PAGE: u64 = 6;
const NEXT_PAGE: u64 = 7;
const EXPORT: u64 = 8;
const VIEW_DATA: u64 = 10;
const VIEW_STRUCTURE: u64 = 11;
const VIEW_DDL: u64 = 12;
const DETAILS: u64 = 13;
const REVIEW: u64 = 14;
const DISCARD: u64 = 15;
const APPLY: u64 = 16;
const FILTER_BASE: u64 = 100;
const FILTER_END: u64 = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Grid,
    Where,
    Order,
    /// The filter box of this grid column.
    Column(usize),
    /// The selected cell's in-place editor.
    Cell,
    /// The Details side's text box.
    Value,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Data,
    Structure,
    Ddl,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Value,
    Row,
}

type RowKey = Vec<(String, Option<String>)>;

/// A cell changed but not saved yet, found again by its row's primary key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Edit {
    key: RowKey,
    column: String,
    value: Option<String>,
}

/// A value opened in an editor tab; saving the file stages its text as an edit.
struct OpenedValue {
    path: PathBuf,
    key: RowKey,
    column: String,
    json: bool,
    written: String,
    modified: Option<SystemTime>,
}

struct Page {
    result: QueryResult,
    total: Option<u64>,
}

pub struct TableItem {
    context: DatabaseContext,
    database: Database,
    table: Table,
    filter: TextField,
    order: TextField,
    column_filters: HashMap<String, TextField>,
    focus: Focus,
    focused: bool,
    page_size: usize,
    offset: usize,
    total: Option<u64>,
    shown: usize,
    /// The page as loaded, before staged edits are laid over it.
    loaded: Vec<Vec<Option<String>>>,
    grid: Grid,
    loading: Pending<Page>,
    exporting: Pending<(u64, PathBuf)>,
    status: Option<(String, bool)>,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    view: View,
    structure: Option<TableStructure>,
    loading_structure: Pending<TableStructure>,
    ddl: Option<Result<String, String>>,
    loading_ddl: Pending<String>,
    view_pane: Pane,
    details: bool,
    side: Side,
    side_pane: Pane,
    /// The cell the side last showed, whose parsed value and folds it keeps.
    shown_cell: Option<(usize, usize)>,
    shown_json: Option<Json>,
    fold: JsonFold,
    /// The Row side's JSON fields' folds, by column.
    row_folds: HashMap<usize, JsonFold>,
    fold_targets: Vec<FoldTarget>,
    /// Per foreign key column, each value's name in the table it points at.
    labels: HashMap<usize, HashMap<String, String>>,
    loading_labels: Pending<Vec<(usize, HashMap<String, String>)>>,
    target: Option<((String, String), Target)>,
    loading_target: Pending<Target>,
    counts: Option<(RowKey, Vec<Option<u64>>)>,
    loading_counts: Pending<Vec<Option<u64>>>,
    edits: Vec<Edit>,
    cell_editor: TextField,
    value_editor: TextArea,
    value_one_line: bool,
    applying: Pending<u64>,
    opened: Vec<OpenedValue>,
    files_checked: Instant,
    requests: Vec<PanelRequest>,
    clipboard: Option<String>,
    /// The tab was wide enough for the Details side when last painted.
    side_fits: bool,
}

impl TableItem {
    pub fn item_id(database: &Database, table: &Table) -> String {
        format!("db-table:{}:{}", database.name, table.qualified())
    }

    pub fn new(context: DatabaseContext, database: Database, table: Table) -> TableItem {
        TableItem::filtered(context, database, table, "")
    }

    /// The table with its WHERE field already holding `filter`.
    pub fn filtered(
        context: DatabaseContext,
        database: Database,
        table: Table,
        filter: &str,
    ) -> TableItem {
        let mut item = TableItem {
            context,
            database,
            table,
            filter: field(),
            order: field(),
            column_filters: HashMap::new(),
            focus: Focus::Grid,
            focused: false,
            page_size: DEFAULT_PAGE,
            offset: 0,
            total: None,
            shown: 0,
            loaded: Vec::new(),
            grid: Grid::default(),
            loading: Pending::idle(),
            exporting: Pending::idle(),
            status: None,
            hits: Vec::new(),
            hovered: None,
            view: View::Data,
            structure: None,
            loading_structure: Pending::idle(),
            ddl: None,
            loading_ddl: Pending::idle(),
            view_pane: Pane::default(),
            details: true,
            side: Side::Value,
            side_pane: Pane::default(),
            shown_cell: None,
            shown_json: None,
            fold: JsonFold::default(),
            row_folds: HashMap::new(),
            fold_targets: Vec::new(),
            labels: HashMap::new(),
            loading_labels: Pending::idle(),
            target: None,
            loading_target: Pending::idle(),
            counts: None,
            loading_counts: Pending::idle(),
            edits: Vec::new(),
            cell_editor: field(),
            value_editor: TextArea::default(),
            value_one_line: false,
            applying: Pending::idle(),
            opened: Vec::new(),
            files_checked: Instant::now(),
            requests: Vec::new(),
            clipboard: None,
            side_fits: true,
        };
        if !filter.is_empty() {
            item.filter.set_text(filter);
            item.filter.move_to_end();
        }
        item.value_editor.set_font_size(FIELD_FONT);
        item.run();
        if !item.is_redis() {
            let (database, table) = (item.database.clone(), item.table.clone());
            item.loading_structure = item
                .context
                .run(move |connector| connector.table_structure(&database, &table));
        }
        item
    }

    fn is_redis(&self) -> bool {
        self.database.engine == Engine::Redis
    }

    fn where_clause(&self) -> String {
        let conditions: Vec<String> = self
            .grid
            .columns()
            .iter()
            .filter_map(|name| {
                let typed = self.column_filters.get(name)?.text();
                pom_db::filter_condition(name, &typed)
            })
            .collect();
        pom_db::combined_filter(&self.filter.text(), &conditions)
    }

    fn run(&mut self) {
        let database = self.database.clone();
        let (limit, offset) = (self.page_size, self.offset);
        let (query, count) = if self.is_redis() {
            (format!("{}:*", self.table.name), None)
        } else {
            let (filter, order) = (self.where_clause(), self.order.text());
            (
                pom_db::table_query(&self.table, &filter, &order, limit, offset),
                Some(pom_db::count_query(&self.table, &filter)),
            )
        };
        self.status = None;
        self.loading = self.context.run(move |connector| {
            let result = connector.query(&database, &query, limit)?;
            let total = count.and_then(|count| {
                connector
                    .query(&database, &count, 1)
                    .ok()
                    .and_then(|counted| counted.rows.first()?.first()?.clone()?.parse().ok())
            });
            Ok(Page { result, total })
        });
    }

    fn rerun_from_start(&mut self) {
        self.offset = 0;
        self.run();
    }

    fn can_go_next(&self) -> bool {
        match self.total {
            Some(total) => (self.offset + self.shown) < total as usize,
            None => self.shown >= self.page_size,
        }
    }

    fn page_label(&self) -> String {
        if self.shown == 0 {
            return "0 rows".into();
        }
        let range = format!("{}-{}", self.offset + 1, self.offset + self.shown);
        match self.total {
            Some(total) => format!("{range} of {total}"),
            None => range,
        }
    }

    /// A header click cycles that column through unsorted, ascending and descending.
    fn sort_by(&mut self, column: usize) {
        if self.is_redis() {
            return;
        }
        let Some(name) = self.grid.columns().get(column).cloned() else {
            return;
        };
        let next = match self.grid.sort {
            Some((sorted, true)) if sorted == column => Some((column, false)),
            Some((sorted, false)) if sorted == column => None,
            _ => Some((column, true)),
        };
        let quoted = pom_db::quote_identifier(&name);
        let order = match next {
            Some((_, true)) => format!("{quoted} ASC"),
            Some((_, false)) => format!("{quoted} DESC"),
            None => String::new(),
        };
        self.order.set_text(&order);
        self.order.move_to_end();
        self.grid.sort = next;
        self.rerun_from_start();
    }

    fn export(&mut self) {
        if self.is_redis() || self.exporting.busy() {
            return;
        }
        let downloads = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let stem = self.table.name.replace(['/', '\\'], "_");
        let mut path = downloads.join(format!("{stem}.csv"));
        let mut copy = 1;
        while path.exists() {
            path = downloads.join(format!("{stem}-{copy}.csv"));
            copy += 1;
        }
        let sql = pom_db::table_select(&self.table, &self.where_clause(), &self.order.text());
        let database = self.database.clone();
        self.status = Some(("Exporting...".into(), false));
        self.exporting = self.context.run(move |connector| {
            connector
                .export_csv(&database, &sql, &path)
                .map(|rows| (rows, path))
        });
    }

    fn column_name(&self, column: usize) -> Option<&str> {
        self.grid.columns().get(column).map(String::as_str)
    }

    fn column_info(&self, column: usize) -> Option<&pom_db::ColumnInfo> {
        let name = self.column_name(column)?;
        self.structure.as_ref()?.column(name)
    }

    /// The row's primary key values as loaded, which is how its edits find it again.
    fn row_key(&self, row: usize) -> Option<RowKey> {
        let structure = self.structure.as_ref()?;
        let keys = structure.primary_key();
        if keys.is_empty() {
            return None;
        }
        let cells = self.loaded.get(row)?;
        keys.iter()
            .map(|name| {
                let at = self
                    .grid
                    .columns()
                    .iter()
                    .position(|column| column == name)?;
                Some((name.to_string(), cells.get(at).cloned().flatten()))
            })
            .collect()
    }

    fn staged(&self, key: &RowKey, column: &str) -> Option<&Edit> {
        self.edits
            .iter()
            .find(|edit| edit.key == *key && edit.column == column)
    }

    /// Why a cell can't be changed here, if it can't.
    fn read_only(&self, column: usize) -> Option<&'static str> {
        if self.is_redis() {
            return Some("Redis values are read-only here");
        }
        if self.table.kind != TableKind::Table {
            return Some("A view is read-only");
        }
        let Some(structure) = self.structure.as_ref() else {
            return Some("Reading the table's structure...");
        };
        if structure.primary_key().is_empty() {
            return Some("No primary key; edit it in a console");
        }
        if self
            .column_info(column)
            .is_some_and(|info| info.primary_key)
        {
            return Some("Primary key");
        }
        None
    }

    /// Lays the staged edits over the loaded page, marking the cells they change.
    fn lay_edits(&mut self) {
        self.grid.edited.clear();
        for row in 0..self.loaded.len() {
            let key = self.row_key(row);
            for column in 0..self.grid.columns().len() {
                let name = self.grid.columns()[column].clone();
                let staged = key
                    .as_ref()
                    .and_then(|key| self.staged(key, &name))
                    .map(|edit| edit.value.clone());
                let want = match &staged {
                    Some(value) => value.clone(),
                    None => self.loaded[row].get(column).cloned().flatten(),
                };
                if self.grid.rows()[row].get(column).cloned().flatten() != want {
                    self.grid.set_cell(row, column, want);
                }
                if staged.is_some() {
                    self.grid.edited.insert((row, column));
                }
            }
        }
    }

    fn stage(
        &mut self,
        key: RowKey,
        column: String,
        value: Option<String>,
        original: Option<String>,
    ) {
        self.edits
            .retain(|edit| !(edit.key == key && edit.column == column));
        if value != original {
            self.edits.push(Edit { key, column, value });
        }
        self.lay_edits();
    }

    fn stage_cell(&mut self, row: usize, column: usize, value: Option<String>) {
        let (Some(key), Some(name)) = (self.row_key(row), self.column_name(column)) else {
            return;
        };
        let name = name.to_string();
        let original = self
            .loaded
            .get(row)
            .and_then(|cells| cells.get(column))
            .cloned()
            .flatten();
        self.stage(key, name, value, original);
    }

    fn current(&self, row: usize, column: usize) -> Option<String> {
        self.grid.rows().get(row)?.get(column).cloned().flatten()
    }

    /// Starts editing the selected cell: in place for a short one-line value, else in the Details side.
    fn edit_selected(&mut self) {
        let Some((row, column)) = self.grid.selected() else {
            return;
        };
        if let Some(reason) = self.read_only(column) {
            self.status = Some((reason.to_string(), false));
            return;
        }
        let value = self.current(row, column).unwrap_or_default();
        let long = value.len() > INLINE_EDIT_MAX
            || value.contains('\n')
            || self
                .column_info(column)
                .is_some_and(|info| info.data_type.contains("json"));
        if long {
            self.details = true;
            self.side = Side::Value;
            let parsed = self
                .column_info(column)
                .filter(|info| info.data_type.contains("json"))
                .and_then(|_| json::parse(&value));
            match parsed {
                Some(parsed) => {
                    self.value_editor.set_text(&json::pretty(&parsed));
                    self.value_editor.set_code(Some(json::highlight));
                    self.value_one_line = !value.contains('\n');
                }
                None => {
                    self.value_editor.set_text(&value);
                    self.value_editor.set_code(None);
                    self.value_one_line = false;
                }
            }
            self.focus = Focus::Value;
        } else {
            self.cell_editor.set_text(&value);
            self.cell_editor.select_all();
            self.grid.editing = Some((row, column));
            self.focus = Focus::Cell;
        }
    }

    fn keep_cell_edit(&mut self) {
        if let Some((row, column)) = self.grid.editing.take() {
            let text = self.cell_editor.text();
            self.stage_cell(row, column, Some(text));
        }
        self.focus = Focus::Grid;
    }

    fn keep_value_edit(&mut self) {
        if let Some((row, column)) = self.grid.selected() {
            let mut text = self.value_editor.text();
            // Edited as indented lines; saved on one line like it was, so only the change shows.
            if self.value_one_line {
                if let Some(parsed) = json::parse(&text) {
                    text = json::compact(&parsed);
                }
            }
            self.stage_cell(row, column, Some(text));
        }
        self.focus = Focus::Grid;
    }

    fn statements(&self) -> Vec<String> {
        self.edits
            .iter()
            .map(|edit| {
                pom_db::update_statement(
                    &self.table,
                    &edit.key,
                    &edit.column,
                    edit.value.as_deref(),
                )
            })
            .collect()
    }

    fn apply(&mut self) {
        if self.edits.is_empty() || self.applying.busy() {
            return;
        }
        let statements = self.statements();
        let database = self.database.clone();
        self.status = Some(("Saving...".into(), false));
        self.applying = self
            .context
            .run(move |connector| connector.apply(&database, &statements));
    }

    fn reveal_table(&mut self, name: &str, filter: String) {
        let table = Table {
            schema: self.table.schema.clone(),
            name: name.to_string(),
            kind: TableKind::Table,
            count: None,
        };
        let id = if filter.is_empty() {
            TableItem::item_id(&self.database, &table)
        } else {
            format!("{}:{filter}", TableItem::item_id(&self.database, &table))
        };
        let (context, database) = (self.context.clone(), self.database.clone());
        self.requests.push(PanelRequest::Reveal {
            id,
            open: Box::new(move || {
                Some(Box::new(TableItem::filtered(
                    context, database, table, &filter,
                )))
            }),
        });
    }

    /// Opens the row a foreign key cell points at, in that table's tab.
    fn follow(&mut self, row: usize, column: usize) {
        let Some((table, target)) = self
            .column_info(column)
            .and_then(|info| info.references.clone())
        else {
            return;
        };
        let Some(value) = self.current(row, column) else {
            return;
        };
        let filter = format!(
            "{} = {}",
            pom_db::quote_identifier(&target),
            pom_db::quote_literal(&value)
        );
        self.reveal_table(&table, filter);
    }

    fn open_reference(&mut self, index: usize, filtered: bool) {
        let Some(reference) = self
            .structure
            .as_ref()
            .and_then(|structure| structure.referenced_by.get(index))
            .cloned()
        else {
            return;
        };
        let value = filtered
            .then(|| {
                let (row, _) = self.grid.selected()?;
                let key = self.row_key(row)?;
                key.into_iter().next()?.1
            })
            .flatten();
        let filter = match value {
            Some(value) => format!(
                "{} = {}",
                pom_db::quote_identifier(&reference.column),
                pom_db::quote_literal(&value)
            ),
            None => String::new(),
        };
        self.reveal_table(&reference.table, filter);
    }

    /// Writes the selected value to a file and opens it in an editor tab; saving the file stages it.
    /// The staged statements in an editor tab, one per paragraph, to read before Apply runs them.
    fn review_in_tab(&mut self) {
        if self.edits.is_empty() {
            return;
        }
        let count = self.edits.len();
        let mut text = format!(
            "-- {count} {} to {} in {}, not applied yet.\n-- Apply (cmd-s) in the table tab runs them in one transaction; any error saves nothing.\n-- Editing this file does not change what runs.\n",
            if count == 1 { "change" } else { "changes" },
            self.table.qualified(),
            self.database.label,
        );
        for edit in &self.edits {
            text.push('\n');
            text.push_str(&pom_db::update_statement_lines(
                &self.table,
                &edit.key,
                &edit.column,
                edit.value.as_deref(),
            ));
            text.push_str(";\n");
        }
        let safe: String = format!("{}-{}-pending", self.database.name, self.table.name)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self
            .context
            .state
            .path("db-values")
            .join(format!("{safe}.sql"));
        let written = std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|()| std::fs::write(&path, &text));
        match written {
            Ok(()) => self.requests.push(PanelRequest::OpenFile(path)),
            Err(error) => {
                self.status = Some((format!("Could not open the SQL in a tab: {error}"), true))
            }
        }
    }

    fn open_in_tab(&mut self) {
        let Some((row, column)) = self.grid.selected() else {
            return;
        };
        let Some(value) = self.current(row, column) else {
            return;
        };
        let name = self.column_name(column).unwrap_or_default().to_string();
        let parsed = json::parse(&value);
        let text = parsed.as_ref().map_or(value.clone(), json::pretty);
        let key = self.row_key(row).unwrap_or_default();
        let key_text: Vec<String> = key
            .iter()
            .map(|(_, value)| value.clone().unwrap_or_else(|| "null".into()))
            .collect();
        let stem = format!(
            "{}-{}-{}-{}",
            self.database.name,
            self.table.name,
            if key_text.is_empty() {
                format!("row{}", row + 1)
            } else {
                key_text.join("-")
            },
            name
        );
        let safe: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.context.state.path("db-values").join(format!(
            "{safe}.{}",
            if parsed.is_some() { "json" } else { "txt" }
        ));
        let written = std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|()| std::fs::write(&path, &text));
        if let Err(error) = written {
            self.status = Some((format!("Could not open it in a tab: {error}"), true));
            return;
        }
        let modified = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        self.opened.retain(|opened| opened.path != path);
        if self.read_only(column).is_none() {
            self.opened.push(OpenedValue {
                path: path.clone(),
                key,
                column: name,
                json: parsed.is_some(),
                written: text,
                modified,
            });
            self.status = Some(("Saving the tab stages the change here".into(), false));
        }
        self.requests.push(PanelRequest::OpenFile(path));
    }

    /// Stages what was saved in a value's editor tab.
    fn check_opened(&mut self) -> bool {
        if self.opened.is_empty() || self.files_checked.elapsed() < FILE_CHECK {
            return false;
        }
        self.files_checked = Instant::now();
        let mut staged = Vec::new();
        for opened in &mut self.opened {
            let modified = std::fs::metadata(&opened.path)
                .and_then(|meta| meta.modified())
                .ok();
            if modified == opened.modified {
                continue;
            }
            opened.modified = modified;
            let Ok(text) = std::fs::read_to_string(&opened.path) else {
                continue;
            };
            if text == opened.written {
                continue;
            }
            opened.written = text.clone();
            staged.push((opened.key.clone(), opened.column.clone(), text, opened.json));
        }
        let mut changed = false;
        for (key, column, text, is_json) in staged {
            if is_json && json::parse(&text).is_none() {
                self.status = Some((format!("{column}: the saved file is not valid JSON"), true));
                changed = true;
                continue;
            }
            let row = (0..self.loaded.len()).find(|row| self.row_key(*row).as_ref() == Some(&key));
            let original = row
                .and_then(|row| {
                    let at = self
                        .grid
                        .columns()
                        .iter()
                        .position(|name| *name == column)?;
                    self.loaded[row].get(at).cloned()
                })
                .flatten();
            self.stage(
                key,
                column.clone(),
                Some(text.trim_end().to_string()),
                original,
            );
            self.status = Some((format!("Staged {column} from its tab"), false));
            changed = true;
        }
        changed
    }

    /// Loads what the side needs for the selected cell: its parsed value, the row a foreign key points at, and
    /// how many rows of other tables point at its row.
    fn follow_selection(&mut self) {
        let selected = self.grid.selected();
        if selected != self.shown_cell {
            if selected.map(|(row, _)| row) != self.shown_cell.map(|(row, _)| row) {
                self.row_folds.clear();
            }
            self.shown_cell = selected;
            self.fold = JsonFold::default();
            self.side_pane.scroll = 0.0;
            self.shown_json = selected
                .and_then(|(row, column)| self.current(row, column))
                .filter(|value| value.starts_with(['{', '[']))
                .and_then(|value| json::parse(&value));
            if self.focus == Focus::Value {
                self.focus = Focus::Grid;
            }
        }
        if !self.details || self.is_redis() {
            return;
        }
        let Some((row, column)) = selected else {
            return;
        };
        if self.side == Side::Value {
            let references = self
                .column_info(column)
                .and_then(|info| info.references.clone());
            if let (Some((table, target)), Some(value)) = (references, self.current(row, column)) {
                let wanted = (table.clone(), value.clone());
                if self.target.as_ref().map(|(key, _)| key) != Some(&wanted) {
                    self.target = Some((wanted, Target::Loading));
                    let (database, schema) = (self.database.clone(), self.table.schema.clone());
                    self.loading_target = self.context.run(move |connector| {
                        let sql = pom_db::referenced_row_query(&schema, &table, &target, &value);
                        let found = connector.query(&database, &sql, 1)?;
                        Ok(match found.rows.into_iter().next() {
                            Some(cells) => Target::Found {
                                table,
                                fields: found.columns.into_iter().zip(cells).collect(),
                            },
                            None => {
                                Target::Missing(format!("No {table} row has {target} = {value}"))
                            }
                        })
                    });
                }
            }
        } else if let (Some(key), Some(structure)) = (self.row_key(row), self.structure.as_ref()) {
            if !structure.referenced_by.is_empty()
                && self.counts.as_ref().map(|(counted, _)| counted) != Some(&key)
            {
                let value = key
                    .first()
                    .and_then(|(_, value)| value.clone())
                    .unwrap_or_default();
                let references = structure.referenced_by.clone();
                self.counts = Some((key, vec![None; references.len()]));
                let database = self.database.clone();
                self.loading_counts = self.context.run(move |connector| {
                    Ok(references
                        .iter()
                        .map(|reference| {
                            let sql = pom_db::reference_count_query(reference, &value);
                            connector
                                .query(&database, &sql, 1)
                                .ok()
                                .and_then(|counted| {
                                    counted.rows.first()?.first()?.clone()?.parse().ok()
                                })
                        })
                        .collect())
                });
            }
        }
    }

    fn row_json_action(&mut self, id: u64) {
        let Some((row, _)) = self.grid.selected() else {
            return;
        };
        let (base, column) = [
            ROW_OPEN_BASE,
            ROW_COPY_BASE,
            ROW_COLLAPSE_BASE,
            ROW_EXPAND_BASE,
        ]
        .into_iter()
        .find(|base| id >= *base)
        .map(|base| (base, (id - base) as usize))
        .unwrap_or((ROW_EXPAND_BASE, 0));
        let value = self.current(row, column);
        match base {
            ROW_EXPAND_BASE => {
                if let Some(parsed) = value.as_deref().and_then(json::parse) {
                    self.row_folds
                        .entry(column)
                        .or_default()
                        .expand_all(&parsed);
                }
            }
            ROW_COLLAPSE_BASE => self.row_folds.entry(column).or_default().collapse(),
            ROW_COPY_BASE => self.clipboard = value,
            _ => {
                self.grid.select(row, column);
                self.open_in_tab();
            }
        }
    }

    /// Looks up the names foreign key values stand for (a user's email, an org's name) in the tables they
    /// point at, for the page shown.
    fn load_labels(&mut self) {
        let Some(structure) = self.structure.as_ref() else {
            return;
        };
        let mut asks = Vec::new();
        for (column, name) in self.grid.columns().iter().enumerate() {
            let Some((table, target)) = structure
                .column(name)
                .and_then(|info| info.references.clone())
            else {
                continue;
            };
            let mut values: Vec<String> = self
                .loaded
                .iter()
                .filter_map(|cells| cells.get(column).cloned().flatten())
                .collect();
            values.sort();
            values.dedup();
            values.truncate(500);
            if !values.is_empty() {
                asks.push((column, table, target, values));
            }
        }
        if asks.is_empty() {
            return;
        }
        let (database, schema) = (self.database.clone(), self.table.schema.clone());
        self.loading_labels = self.context.run(move |connector| {
            Ok(asks
                .into_iter()
                .filter_map(|(column, table, target, values)| {
                    let listed: Vec<String> = values
                        .iter()
                        .map(|value| pom_db::quote_literal(value))
                        .collect();
                    let sql = format!(
                        "SELECT * FROM {}.{} WHERE {}::text IN ({})",
                        pom_db::quote_identifier(if schema.is_empty() {
                            "public"
                        } else {
                            &schema
                        }),
                        pom_db::quote_identifier(&table),
                        pom_db::quote_identifier(&target),
                        listed.join(", ")
                    );
                    let found = connector.query(&database, &sql, values.len()).ok()?;
                    let key = found.columns.iter().position(|name| *name == target)?;
                    let shown = LABEL_COLUMNS
                        .iter()
                        .find_map(|wanted| found.columns.iter().position(|name| name == wanted))?;
                    let names = found
                        .rows
                        .into_iter()
                        .filter_map(|cells| {
                            Some((cells.get(key)?.clone()?, cells.get(shown)?.clone()?))
                        })
                        .collect();
                    Some((column, names))
                })
                .collect())
        });
    }

    fn click(&mut self, id: u64) {
        match id {
            WHERE_FIELD => self.focus = Focus::Where,
            ORDER_FIELD => self.focus = Focus::Order,
            PAGE_SIZE => {
                let at = PAGE_SIZES
                    .iter()
                    .position(|size| *size == self.page_size)
                    .unwrap_or(0);
                self.page_size = PAGE_SIZES[(at + 1) % PAGE_SIZES.len()];
                self.rerun_from_start();
            }
            REFRESH => self.run(),
            FIRST_PAGE if self.offset > 0 => self.rerun_from_start(),
            PREVIOUS_PAGE if self.offset > 0 => {
                self.offset = self.offset.saturating_sub(self.page_size);
                self.run();
            }
            NEXT_PAGE if self.can_go_next() => {
                self.offset += self.page_size;
                self.run();
            }
            EXPORT => self.export(),
            VIEW_DATA => self.view = View::Data,
            VIEW_STRUCTURE => self.view = View::Structure,
            VIEW_DDL => {
                self.view = View::Ddl;
                if self.ddl.is_none() && !self.loading_ddl.busy() {
                    let (database, table) = (self.database.clone(), self.table.clone());
                    self.loading_ddl = self
                        .context
                        .run(move |connector| connector.table_ddl(&database, &table));
                }
            }
            DETAILS | SIDE_CLOSE => self.details = !self.details,
            SIDE_VALUE => self.side = Side::Value,
            SIDE_ROW => self.side = Side::Row,
            REVIEW => self.review_in_tab(),
            DISCARD => {
                self.edits.clear();
                self.lay_edits();
            }
            APPLY => self.apply(),
            VALUE_AREA
                if self.focus != Focus::Value
                    && self
                        .grid
                        .selected()
                        .is_some_and(|(_, column)| self.read_only(column).is_none()) =>
            {
                self.edit_selected()
            }
            SAVE_VALUE => self.keep_value_edit(),
            CANCEL_VALUE => self.focus = Focus::Grid,
            SET_NULL => {
                if let Some((row, column)) = self.grid.selected() {
                    self.stage_cell(row, column, None);
                }
            }
            REVERT => {
                if let Some((row, column)) = self.grid.selected() {
                    let original = self
                        .loaded
                        .get(row)
                        .and_then(|cells| cells.get(column))
                        .cloned()
                        .flatten();
                    self.stage_cell(row, column, original);
                }
            }
            COPY_VALUE => {
                if let Some((row, column)) = self.grid.selected() {
                    self.clipboard = Some(self.current(row, column).unwrap_or_default());
                }
            }
            OPEN_IN_TAB => self.open_in_tab(),
            EXPAND_ALL => {
                if let Some(value) = &self.shown_json {
                    self.fold.expand_all(value);
                }
            }
            COLLAPSE => self.fold.collapse(),
            OPEN_TARGET => {
                if let Some((row, column)) = self.grid.selected() {
                    self.follow(row, column);
                }
            }
            COPY_DDL => {
                if let Some(Ok(ddl)) = &self.ddl {
                    self.clipboard = Some(ddl.clone());
                }
            }
            id if (FOLD_BASE..FOLD_END).contains(&id) => {
                let Some(target) = self.fold_targets.get((id - FOLD_BASE) as usize).cloned() else {
                    return;
                };
                let fold = match target.column {
                    Some(column) => self.row_folds.entry(column).or_default(),
                    None => &mut self.fold,
                };
                match target.action {
                    FoldAction::Toggle(path, true) => {
                        fold.open.remove(&path);
                        fold.closed.insert(path);
                    }
                    FoldAction::Toggle(path, false) => {
                        fold.closed.remove(&path);
                        fold.open.insert(path);
                    }
                    FoldAction::More(path) => {
                        fold.more.insert(path);
                    }
                }
            }
            id if (ROW_EXPAND_BASE..ROW_END).contains(&id) => self.row_json_action(id),
            id if (FIELD_LINK_BASE..REFERENCE_BASE).contains(&id) => {
                if let Some((row, _)) = self.grid.selected() {
                    self.follow(row, (id - FIELD_LINK_BASE) as usize);
                }
            }
            id if (FIELD_BASE..FIELD_LINK_BASE).contains(&id) => {
                if let Some((row, _)) = self.grid.selected() {
                    self.grid.select(row, (id - FIELD_BASE) as usize);
                }
            }
            id if (REFERENCE_BASE..STRUCTURE_REFERENCE_BASE).contains(&id) => {
                self.open_reference((id - REFERENCE_BASE) as usize, true);
            }
            id if (STRUCTURE_REFERENCE_BASE..STRUCTURE_LINK_BASE).contains(&id) => {
                self.open_reference((id - STRUCTURE_REFERENCE_BASE) as usize, false);
            }
            id if (STRUCTURE_LINK_BASE..RANGE_END).contains(&id) => {
                let target = self
                    .structure
                    .as_ref()
                    .and_then(|structure| {
                        structure.columns.get((id - STRUCTURE_LINK_BASE) as usize)
                    })
                    .and_then(|column| column.references.clone());
                if let Some((table, _)) = target {
                    self.reveal_table(&table, String::new());
                }
            }
            id if (FILTER_BASE..FILTER_END).contains(&id) => {
                let column = (id - FILTER_BASE) as usize;
                if let Some(name) = self.column_name(column).map(str::to_string) {
                    self.column_filters.entry(name).or_insert_with(field);
                    self.focus = Focus::Column(column);
                }
            }
            _ => {}
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::Where => Some(&mut self.filter),
            Focus::Order => Some(&mut self.order),
            Focus::Cell => Some(&mut self.cell_editor),
            Focus::Column(column) => {
                let name = self.grid.columns().get(column)?.clone();
                Some(self.column_filters.entry(name).or_insert_with(field))
            }
            Focus::Grid | Focus::Value => None,
        }
    }

    fn icon_button(&self, id: u64, kind: IconKind, enabled: bool) -> Node {
        let colors = theme();
        let mut button = div()
            .row()
            .w_px(22.0)
            .h_px(22.0)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .child(icon(kind).size(12.0).color(if enabled {
                colors.icon_muted
            } else {
                colors.text_disabled
            }));
        if enabled {
            button = button.on_click(id);
            if self.hovered == Some(id) {
                button = button.bg(colors.ghost_element_hover);
            }
        }
        button.into()
    }

    fn text_button(&self, id: u64, text: String, on: bool) -> Node {
        let colors = theme();
        let mut button = div()
            .row()
            .h_px(22.0)
            .px(6.0)
            .rounded(4.0)
            .items_center()
            .border(1.0, colors.border_variant)
            .on_click(id)
            .child(label(text).size(12.0).color(colors.text));
        if on {
            button = button.bg(colors.element_selected);
        } else if self.hovered == Some(id) {
            button = button.bg(colors.ghost_element_hover);
        }
        button.into()
    }

    fn keyed_button(&self, id: u64, text: &str, keys: &str, on: bool) -> Node {
        let colors = theme();
        let mut button = div()
            .row()
            .h_px(22.0)
            .px(6.0)
            .gap(6.0)
            .rounded(4.0)
            .items_center()
            .border(1.0, colors.border_variant)
            .on_click(id)
            .child(label(text.to_string()).size(12.0).color(colors.text))
            .child(workspace::render_keystroke(keys, 10.5));
        if on {
            button = button.bg(colors.element_selected);
        } else if self.hovered == Some(id) {
            button = button.bg(colors.ghost_element_hover);
        }
        button.into()
    }

    fn input(
        &self,
        id: u64,
        prefix: &str,
        field: &TextField,
        placeholder: &str,
        focused: bool,
    ) -> Node {
        let colors = theme();
        div()
            .row()
            .flex(1.0)
            .h_px(FIELD_H)
            .px(6.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .bg(colors.editor_background)
            .border(
                1.0,
                if focused && self.focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                },
            )
            .on_click(id)
            .child(
                label(prefix.to_string())
                    .size(11.0)
                    .mono()
                    .color(colors.text_muted),
            )
            .child(field.render(
                placeholder,
                focused && self.focused,
                colors.text,
                FIELD_H - 6.0,
                FieldFont::Mono,
            ))
            .into()
    }

    /// Where this is (repo / database / table, on the workspace's branch) and what the tab shows; in a
    /// narrow tab the branch goes first, then the repo, then the database, and the table always stays.
    fn toolbar(&self, width: f32) -> Node {
        let colors = theme();
        let text_w =
            |text: &str, size: f32, mono: bool| ui::measure_text_width(text, size, mono, 400);
        let controls = if self.is_redis() { 0.0 } else { 196.0 }
            + if self.view == View::Data && self.side_fits {
                130.0
            } else {
                0.0
            }
            + 22.0
            + 44.0;
        let mut room = width - controls - 20.0 - text_w(&self.table.qualified(), 12.5, false);
        let mut take = |w: f32| {
            let fits = w <= room;
            if fits {
                room -= w;
            }
            fits
        };
        let slash_w = text_w("/", 12.5, false) + 12.0;
        let show_label = take(text_w(&self.database.label, 12.5, false) + slash_w);
        let show_repo = show_label
            && !self.database.repo.is_empty()
            && take(text_w(&self.database.repo, 12.5, false) + slash_w);
        let show_branch = !self.context.branch.is_empty()
            && take(text_w("on", 12.0, false) + text_w(&self.context.branch, 12.0, true) + 34.0);
        let muted = |text: String| label(text).size(12.5).color(colors.text_muted);
        let slash = || label("/").size(12.5).color(colors.text_placeholder);
        let mut crumb = div()
            .row()
            .flex(1.0)
            .gap(6.0)
            .items_center()
            .child(icon(IconKind::Table).size(12.0).color(colors.icon_muted));
        if show_repo {
            crumb = crumb
                .child(muted(self.database.repo.clone()))
                .child(slash());
        }
        if show_label {
            crumb = crumb
                .child(muted(self.database.label.clone()))
                .child(slash());
        }
        crumb = crumb.child(
            label(self.table.qualified())
                .size(12.5)
                .medium()
                .color(colors.text)
                .truncate(),
        );
        if show_branch {
            crumb = crumb
                .child(div().w_px(2.0))
                .child(label("on").size(12.0).color(colors.text_placeholder))
                .child(icon(IconKind::Branch).size(11.0).color(colors.text_accent))
                .child(
                    label(self.context.branch.clone())
                        .size(12.0)
                        .mono()
                        .color(colors.text_accent),
                );
        }
        let mut bar = div()
            .row()
            .h_px(TOOLBAR_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .child(crumb);
        if !self.is_redis() {
            let views = [
                (VIEW_DATA, "Data"),
                (VIEW_STRUCTURE, "Structure"),
                (VIEW_DDL, "DDL"),
            ];
            let selected = match self.view {
                View::Data => 0,
                View::Structure => 1,
                View::Ddl => 2,
            };
            bar = bar.child(segmented(&views, selected, self.hovered));
        }
        if self.view == View::Data && self.side_fits {
            bar = bar.child(self.keyed_button(DETAILS, "Details", "shift-enter", self.details));
        }
        bar.child(self.icon_button(REFRESH, IconKind::RotateCw, true))
            .into()
    }

    /// The typed WHERE and ORDER BY, and how many rows a page holds.
    fn query_bar(&self, width: f32) -> Node {
        let filter = self.input(
            WHERE_FIELD,
            "WHERE",
            &self.filter,
            "id > 10",
            self.focus == Focus::Where,
        );
        let order = self.input(
            ORDER_FIELD,
            "ORDER BY",
            &self.order,
            "id",
            self.focus == Focus::Order,
        );
        let rows = self.text_button(PAGE_SIZE, format!("{} rows", self.page_size), false);
        let line = || div().row().h_px(QUERY_H).px(8.0).gap(8.0).items_center();
        if width < QUERY_STACK_BELOW {
            return div()
                .col()
                .child(line().child(filter))
                .child(line().child(order).child(rows))
                .into();
        }
        line().child(filter).child(order).child(rows).into()
    }

    fn query_height(width: f32) -> f32 {
        if width < QUERY_STACK_BELOW {
            2.0 * QUERY_H
        } else {
            QUERY_H
        }
    }

    /// A filter box under each visible column, lined up with the grid below.
    fn filter_row(&self) -> Node {
        let colors = theme();
        let (number_width, slices) = self.grid.column_slices();
        let mut row = div()
            .row()
            .h_px(FILTER_H)
            .items_center()
            .bg(colors.editor_background)
            .child(
                div()
                    .w_px(number_width)
                    .h_px(FILTER_H)
                    .bg(colors.panel_background),
            );
        for (column, _, width) in slices {
            let name = &self.grid.columns()[column];
            let focused = self.focus == Focus::Column(column) && self.focused;
            let box_: Node = match self.column_filters.get(name) {
                Some(filter) if focused || !filter.text().is_empty() => filter.render(
                    "filter",
                    focused,
                    colors.text,
                    FIELD_H - 8.0,
                    FieldFont::Mono,
                ),
                _ => label("filter")
                    .size(11.0)
                    .mono()
                    .color(colors.text_placeholder)
                    .truncate()
                    .into(),
            };
            row = row.child(
                div()
                    .row()
                    .w_px(width)
                    .h_px(FILTER_H)
                    .px(2.0)
                    .items_center()
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .h_px(FIELD_H - 4.0)
                            .px(6.0)
                            .items_center()
                            .rounded(3.0)
                            .border(
                                1.0,
                                if focused {
                                    colors.border_focused
                                } else {
                                    colors.border_variant
                                },
                            )
                            .on_click(FILTER_BASE + column as u64)
                            .child(box_),
                    ),
            );
        }
        row.into()
    }

    fn status_bar(&self) -> Node {
        let colors = theme();
        let mut bar = div().row().h_px(STATUS_H).px(8.0).gap(4.0).items_center();
        let data = self.view == View::Data;
        if !self.is_redis() && data {
            bar = bar
                .child(self.icon_button(FIRST_PAGE, IconKind::ChevronLeft, self.offset > 0))
                .child(self.icon_button(PREVIOUS_PAGE, IconKind::ArrowLeft, self.offset > 0))
                .child(label(self.page_label()).size(12.0).color(colors.text_muted))
                .child(self.icon_button(NEXT_PAGE, IconKind::ArrowRight, self.can_go_next()));
        } else if data {
            bar = bar.child(label(self.page_label()).size(12.0).color(colors.text_muted));
        }
        let (status, error) = if self.loading.busy() {
            ("Loading...".to_string(), false)
        } else {
            self.status.clone().unwrap_or_default()
        };
        bar = bar.child(
            div().row().flex(1.0).pl(8.0).items_center().child(
                label(status)
                    .size(12.0)
                    .color(if error {
                        colors.error
                    } else {
                        colors.text_muted
                    })
                    .truncate(),
            ),
        );
        if !self.is_redis() && data {
            bar = bar.child(self.text_button(EXPORT, "Export CSV".into(), false));
        }
        bar.into()
    }

    /// Staged edits waiting to be saved: how many, into which database, and the three ways out.
    fn pending_bar(&self) -> Node {
        let colors = theme();
        let count = self.edits.len();
        let (status, error) = if self.applying.busy() {
            ("Saving...".to_string(), false)
        } else {
            self.status.clone().unwrap_or_default()
        };
        div()
            .row()
            .h_px(PENDING_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .bg(colors.warning.alpha(0.07))
            .child(
                label(format!(
                    "{count} {}",
                    if count == 1 { "change" } else { "changes" }
                ))
                .size(12.5)
                .medium()
                .color(colors.warning),
            )
            .child(
                label(format!("not saved - into {}", self.database.name))
                    .size(12.0)
                    .color(colors.text_muted),
            )
            .child(
                div().row().flex(1.0).pl(8.0).items_center().child(
                    label(status)
                        .size(12.0)
                        .color(if error {
                            colors.error
                        } else {
                            colors.text_muted
                        })
                        .truncate(),
                ),
            )
            .child(self.text_button(REVIEW, "Review SQL".into(), false))
            .child(self.text_button(DISCARD, "Discard".into(), false))
            .child(self.keyed_button(APPLY, "Apply", "cmd-s", false))
            .into()
    }

    fn side_head(&self) -> Node {
        let colors = theme();
        let modes = [(SIDE_VALUE, "Value"), (SIDE_ROW, "Row")];
        div()
            .row()
            .h_px(SIDE_HEAD_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .child(segmented(
                &modes,
                usize::from(self.side == Side::Row),
                self.hovered,
            ))
            .child(div().row().flex(1.0))
            .child(
                div()
                    .row()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(SIDE_CLOSE)
                    .bg(if self.hovered == Some(SIDE_CLOSE) {
                        colors.ghost_element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(icon(IconKind::Close).size(11.0).color(colors.icon_muted)),
            )
            .into()
    }

    fn side_content(&mut self, width: f32) -> Node {
        let colors = theme();
        self.fold_targets.clear();
        let Some((row, column)) = self.grid.selected() else {
            return div()
                .col()
                .p(12.0)
                .child(
                    label("Select a cell to see its whole value")
                        .size(12.0)
                        .color(colors.text_muted)
                        .wrap(width - 24.0),
                )
                .into();
        };
        if self.side == Side::Row {
            return self.row_side(row, column);
        }
        let info = self.column_info(column).cloned();
        let name = self.column_name(column).unwrap_or_default().to_string();
        let value = self.current(row, column);
        let read_only = self.read_only(column);
        let no_target = Target::None;
        let target = match (
            &self.target,
            info.as_ref().and_then(|info| info.references.as_ref()),
        ) {
            (Some(((table, wanted), target)), Some((references, _)))
                if table == references && value.as_deref() == Some(wanted.as_str()) =>
            {
                target
            }
            _ => &no_target,
        };
        let editing = self.focus == Focus::Value;
        let (_, view_h) = self.side_pane.window();
        let rows = ((view_h - 150.0) / self.value_editor.line_height()).max(8.0) as usize;
        let editor = editing.then(|| {
            self.value_editor
                .render("value", self.focused, width - 42.0, rows)
        });
        let row_name = self
            .row_key(row)
            .and_then(|key| key.into_iter().next()?.1)
            .unwrap_or_else(|| (row + 1).to_string());
        let cell = CellView {
            column: &name,
            data_type: info.as_ref().map_or("", |info| info.data_type.as_str()),
            primary_key: info.as_ref().is_some_and(|info| info.primary_key),
            value: value.as_deref(),
            edited: self.grid.edited.contains(&(row, column)),
            row_name,
            read_only,
            references: info.as_ref().and_then(|info| info.references.as_ref()),
            target,
        };
        let title = details::value_title(&cell);
        let head_h = ui::measure(&title).1 + 10.0 + 8.0 + 27.0;
        let mut content = div().col().p(10.0).gap(8.0).child(title);
        let json = self.shown_json.is_some() && !editing;
        match (&self.shown_json, editing) {
            (Some(parsed), false) => {
                let (scroll, view) = self.side_pane.window();
                let tree = details::json_tree(
                    parsed,
                    &self.fold,
                    (scroll - head_h, view),
                    None,
                    &mut self.fold_targets,
                    self.hovered,
                );
                content = content.child(details::json_box(
                    tree,
                    json::summary(parsed).unwrap_or_default(),
                    &[
                        (EXPAND_ALL, "Expand all"),
                        (COLLAPSE, "Collapse"),
                        (OPEN_IN_TAB, "Open in tab"),
                    ],
                    self.hovered,
                ));
            }
            _ => content = content.child(details::value_box(&cell, editor, width - 20.0)),
        }
        content = content.child(details::value_actions(&cell, editing, json, self.hovered));
        if let Some(target) = details::value_target(&cell, self.hovered) {
            content = content.child(target);
        }
        content.into()
    }

    fn row_side(&mut self, row: usize, column: usize) -> Node {
        let key = self.row_key(row);
        let columns: Vec<String> = self.grid.columns().to_vec();
        let mut fields = Vec::with_capacity(columns.len());
        let empty_fold = JsonFold::default();
        for (index, name) in columns.iter().enumerate() {
            let value = self.grid.rows()[row].get(index).cloned().flatten();
            let info = self
                .structure
                .as_ref()
                .and_then(|structure| structure.column(name))
                .cloned();
            let link =
                match (&info, &value) {
                    (Some(info), Some(value)) => info.references.as_ref().map(|(table, _)| {
                        match self.labels.get(&index).and_then(|names| names.get(value)) {
                            Some(label) => format!("{value} - {label} ({table})"),
                            None => format!("{value} ({table})"),
                        }
                    }),
                    _ => None,
                };
            let parsed = value
                .as_deref()
                .filter(|text| text.starts_with(['{', '[']))
                .and_then(json::parse);
            let json = parsed.map(|parsed| {
                let fold = self.row_folds.get(&index).unwrap_or(&empty_fold);
                let tree = details::json_tree(
                    &parsed,
                    fold,
                    (0.0, f32::MAX / 4.0),
                    Some(index),
                    &mut self.fold_targets,
                    self.hovered,
                );
                let text = value.as_deref().unwrap_or_default();
                let head = format!(
                    "{} - {}",
                    info.as_ref().map_or("json", |info| info.data_type.as_str()),
                    pom_db::object_storage::format_size(text.len() as u64)
                );
                let index = index as u64;
                let mut actions = Vec::new();
                if text.len() > 2048 {
                    actions.push((ROW_EXPAND_BASE + index, "Expand all"));
                    actions.push((ROW_COLLAPSE_BASE + index, "Collapse"));
                }
                actions.push((ROW_COPY_BASE + index, "Copy"));
                actions.push((ROW_OPEN_BASE + index, "Open in tab"));
                details::json_box(tree, head, &actions, self.hovered)
            });
            fields.push((name.clone(), value, link, json));
        }
        let references: Vec<(String, Option<u64>)> = self
            .structure
            .as_ref()
            .map(|structure| {
                structure
                    .referenced_by
                    .iter()
                    .enumerate()
                    .map(|(index, reference)| {
                        let count = self
                            .counts
                            .as_ref()
                            .filter(|(counted, _)| key.as_ref() == Some(counted))
                            .and_then(|(_, counts)| counts.get(index).copied().flatten());
                        (format!("{}.{}", reference.table, reference.column), count)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let fields: Vec<RowField> = fields
            .into_iter()
            .enumerate()
            .map(|(index, (name, value, link, json))| RowField {
                name,
                value,
                edited: self.grid.edited.contains(&(row, index)),
                link,
                json,
            })
            .collect();
        div()
            .col()
            .p(10.0)
            .child(details::row_view(
                fields,
                Some(column),
                &references,
                self.hovered,
            ))
            .into()
    }

    /// Rows shown, the total when counted, and the first row's first cell (tests).
    pub fn page_summary(&self) -> (usize, Option<u64>, Option<String>) {
        let first = self
            .grid
            .rows()
            .first()
            .and_then(|row| row.first().cloned().flatten());
        (self.shown, self.total, first)
    }

    /// What a click on a column's header does (tests).
    pub fn sort_by_column(&mut self, column: usize) {
        self.sort_by(column);
    }

    /// Types a WHERE clause and runs it, as Enter in the field does (tests).
    pub fn set_filter(&mut self, filter: &str) {
        self.filter.set_text(filter);
        self.grid.sort = None;
        self.rerun_from_start();
    }

    /// Types into a column's filter box and runs it, as Enter there does (tests).
    pub fn set_column_filter(&mut self, column: &str, typed: &str) {
        let mut filter = field();
        filter.set_text(typed);
        self.column_filters.insert(column.to_string(), filter);
        self.rerun_from_start();
    }

    /// Stages a new value for a cell, as editing it does (tests).
    pub fn edit_cell(&mut self, row: usize, column: usize, value: Option<&str>) {
        self.stage_cell(row, column, value.map(str::to_string));
    }

    /// Saves the staged edits, as Apply does (tests).
    pub fn apply_edits(&mut self) {
        self.apply();
    }

    /// The staged edits as the statements Apply would run (tests).
    pub fn pending_statements(&self) -> Vec<String> {
        self.statements()
    }

    /// The table's shape once read (tests).
    pub fn structure(&self) -> Option<&TableStructure> {
        self.structure.as_ref()
    }

    /// Shows a page as if the query had returned it (previews and tests).
    pub fn show_page(&mut self, result: QueryResult, total: Option<u64>) {
        self.loading = Pending::idle();
        self.status = None;
        self.apply_page(Ok(Page { result, total }));
    }

    /// Selects a cell, as clicking it does (previews and tests).
    pub fn select_cell(&mut self, row: usize, column: usize) {
        self.grid.select(row, column);
    }

    /// Shows names for a foreign key column's values as if they had been looked up (previews and tests).
    pub fn show_labels(&mut self, column: usize, names: &[(&str, &str)]) {
        self.labels.insert(
            column,
            names
                .iter()
                .map(|(value, name)| (value.to_string(), name.to_string()))
                .collect(),
        );
        self.grid.labels = (0..self.grid.columns().len())
            .map(|column| self.labels.get(&column).cloned().unwrap_or_default())
            .collect();
    }

    /// Switches the Details side to the selected row's record (previews and tests).
    pub fn show_side_row(&mut self) {
        self.side = Side::Row;
    }

    /// Switches to the Structure view (previews and tests).
    pub fn show_view_structure(&mut self) {
        self.view = View::Structure;
    }

    /// Switches to the DDL view showing `ddl` (previews and tests).
    pub fn show_ddl(&mut self, ddl: &str) {
        self.view = View::Ddl;
        self.ddl = Some(Ok(ddl.to_string()));
    }

    /// Shows the table's structure as if it had been read (previews and tests).
    pub fn show_structure(&mut self, structure: TableStructure) {
        self.loading_structure = Pending::idle();
        self.set_structure(structure);
    }

    fn set_structure(&mut self, structure: TableStructure) {
        self.structure = Some(structure);
        self.mark_links();
        self.lay_edits();
        self.load_labels();
    }

    fn mark_links(&mut self) {
        let infos: Vec<Option<pom_db::ColumnInfo>> = self
            .grid
            .columns()
            .iter()
            .map(|name| {
                self.structure
                    .as_ref()
                    .and_then(|structure| structure.column(name))
                    .cloned()
            })
            .collect();
        let fitted = self.grid.types.iter().any(|kind| !kind.is_empty());
        self.grid.types = infos
            .iter()
            .map(|info| {
                info.as_ref()
                    .map(|info| info.data_type.clone())
                    .unwrap_or_default()
            })
            .collect();
        self.grid.primary = infos
            .iter()
            .map(|info| info.as_ref().is_some_and(|info| info.primary_key))
            .collect();
        if !fitted && infos.iter().any(Option::is_some) {
            self.grid.refit();
        }
        let links: Vec<bool> = self
            .grid
            .columns()
            .iter()
            .map(|name| {
                self.structure
                    .as_ref()
                    .and_then(|structure| structure.column(name))
                    .is_some_and(|info| info.references.is_some())
            })
            .collect();
        self.grid.links = links;
    }

    fn apply_page(&mut self, page: Result<Page, String>) {
        match page {
            Ok(page) => {
                self.shown = page.result.rows.len();
                self.total = page.total;
                let affected = page.result.rows_affected;
                self.loaded = page.result.rows.clone();
                self.grid.set_data(page.result.columns, page.result.rows);
                self.shown_cell = None;
                self.mark_links();
                self.lay_edits();
                self.labels.clear();
                self.grid.labels.clear();
                self.load_labels();
                if let Some(count) = affected {
                    self.status = Some((format!("{count} rows affected"), false));
                }
            }
            Err(error) => {
                self.status = Some((error.lines().next().unwrap_or_default().to_string(), true));
            }
        }
    }

    fn edit_key(keystroke: &Keystroke) -> Option<(EditKey, bool)> {
        let Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "up" => EditKey::Up,
            "down" => EditKey::Down,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "home" => EditKey::Home,
            "end" => EditKey::End,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            "z" if cmd && shift => EditKey::Redo,
            "z" if cmd => EditKey::Undo,
            _ => return None,
        };
        Some((key, shift))
    }

    fn field_keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" => match self.focus {
                Focus::Cell => self.keep_cell_edit(),
                _ => {
                    self.focus = Focus::Grid;
                    self.grid.sort = None;
                    self.rerun_from_start();
                }
            },
            "escape" => {
                if self.focus == Focus::Cell {
                    self.grid.editing = None;
                }
                self.focus = Focus::Grid;
            }
            "tab" => {
                self.focus = match self.focus {
                    Focus::Where => Focus::Order,
                    Focus::Order => Focus::Where,
                    Focus::Column(column) => {
                        let count = self.grid.columns().len().max(1);
                        let next = if modifiers.shift {
                            (column + count - 1) % count
                        } else {
                            (column + 1) % count
                        };
                        if let Some(name) = self.column_name(next).map(str::to_string) {
                            self.column_filters.entry(name).or_insert_with(field);
                        }
                        Focus::Column(next)
                    }
                    other => other,
                }
            }
            "c" if modifiers.cmd => {
                return self
                    .field()
                    .and_then(|field| field.selected_text())
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
            }
            "v" if modifiers.cmd => return TerminalKeyOutcome::Paste,
            _ => match Self::edit_key(keystroke) {
                Some((key, shift)) => {
                    if let Some(field) = self.field() {
                        field.key(key, shift);
                    }
                }
                None => return TerminalKeyOutcome::Ignored,
            },
        }
        TerminalKeyOutcome::Handled
    }

    fn value_keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" if modifiers.cmd => self.keep_value_edit(),
            "enter" => self.value_editor.insert("\n"),
            "escape" => self.focus = Focus::Grid,
            "tab" => self.value_editor.insert("  "),
            "c" if modifiers.cmd => {
                return self
                    .value_editor
                    .selected_text()
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
            }
            "v" if modifiers.cmd => return TerminalKeyOutcome::Paste,
            _ => match Self::edit_key(keystroke) {
                Some((key, shift)) => {
                    self.value_editor.key(key, shift);
                }
                None => return TerminalKeyOutcome::Ignored,
            },
        }
        TerminalKeyOutcome::Handled
    }

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }
}

fn field() -> TextField {
    let mut field = TextField::default();
    field.set_font_size(FIELD_FONT);
    field
}

const TABLE_KIND: &str = "db-table";

/// A table tab saved at quit, opened again the next session on the same database.
pub(crate) fn restore_table(
    context: &DatabaseContext,
    item: &workspace::persistence::SerializedItem,
) -> Option<Box<dyn Item>> {
    if item.kind != TABLE_KIND {
        return None;
    }
    let data = &item.data;
    let name = data.get("database")?.as_str()?;
    let database = context
        .databases()
        .into_iter()
        .find(|database| database.name == name)?;
    let table: Table = serde_json::from_value(data.get("table")?.clone()).ok()?;
    let text = |key: &str| {
        data.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    };
    let mut restored = TableItem::filtered(context.clone(), database, table, text("filter"));
    restored.order.set_text(text("order"));
    if let Some(filters) = data
        .get("column_filters")
        .and_then(|value| value.as_object())
    {
        for (column, typed) in filters {
            let mut filter = field();
            filter.set_text(typed.as_str().unwrap_or_default());
            restored.column_filters.insert(column.clone(), filter);
        }
    }
    restored.view = match text("view") {
        "structure" => View::Structure,
        "ddl" => View::Ddl,
        _ => View::Data,
    };
    restored.details = data
        .get("details")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    restored.side = if text("side") == "row" {
        Side::Row
    } else {
        Side::Value
    };
    if let Some(size) = data.get("page_size").and_then(|value| value.as_u64()) {
        restored.page_size = size as usize;
    }
    restored.focus = Focus::Grid;
    restored.run();
    Some(Box::new(restored))
}

impl Item for TableItem {
    fn id(&self) -> Option<String> {
        Some(Self::item_id(&self.database, &self.table))
    }

    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        let filters: serde_json::Map<String, serde_json::Value> = self
            .column_filters
            .iter()
            .filter(|(_, field)| !field.text().is_empty())
            .map(|(column, field)| (column.clone(), serde_json::Value::String(field.text())))
            .collect();
        Some(workspace::persistence::SerializedItem {
            kind: TABLE_KIND.into(),
            data: serde_json::json!({
                "database": self.database.name,
                "table": self.table,
                "filter": self.filter.text(),
                "order": self.order.text(),
                "column_filters": filters,
                "page_size": self.page_size,
                "view": match self.view {
                    View::Data => "data",
                    View::Structure => "structure",
                    View::Ddl => "ddl",
                },
                "details": self.details,
                "side": if self.side == Side::Row { "row" } else { "value" },
            }),
        })
    }

    fn title(&self) -> String {
        format!("{} [{}]", self.table.qualified(), self.database.label)
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Table)
    }

    /// The body is painted by `paint_body`.
    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, focused: bool) -> Option<ui::Painted> {
        // A tab that paints its own body is never told its focus any other way.
        self.focused = focused;
        let scale = ui::ui_text_scale();
        let colors = theme();
        self.follow_selection();
        self.side_fits = body.w / scale >= SIDE_NEEDS;
        let side_open = self.view == View::Data && self.details && self.side_fits;
        let side_w = if side_open {
            (SIDE_W * scale).min(body.w * 0.5)
        } else {
            0.0
        };
        let main = Rect::new(body.x, body.y, body.w - side_w, body.h, Rgba::TRANSPARENT);
        let bottom_h = if self.edits.is_empty() {
            STATUS_H
        } else {
            PENDING_H
        };
        let querying = self.view == View::Data && !self.is_redis();
        let query_h = if querying {
            Self::query_height(main.w / scale) + 1.0
        } else {
            0.0
        };
        self.grid.below_header = if querying { FILTER_H + 1.0 } else { 0.0 };
        let above = TOOLBAR_H + 1.0 + query_h;
        let middle_h = (main.h / scale - above - 1.0 - bottom_h).max(0.0);
        let middle_area = Rect::new(
            main.x,
            main.y + above * scale,
            main.w,
            middle_h * scale,
            Rgba::TRANSPARENT,
        );
        let mut tree = div()
            .col()
            .w_px(main.w / scale)
            .h_px(main.h / scale)
            .bg(colors.editor_background)
            .child(self.toolbar(main.w / scale))
            .child(div().h_px(1.0).bg(colors.border_variant));
        if querying {
            tree = tree
                .child(self.query_bar(main.w / scale))
                .child(div().h_px(1.0).bg(colors.border_variant));
        }
        let middle: Node = match self.view {
            View::Data if self.grid.columns().is_empty() => {
                let text = if self.loading.busy() {
                    "Loading..."
                } else {
                    "No rows"
                };
                div()
                    .row()
                    .h_px(middle_h)
                    .justify_center()
                    .pt(24.0)
                    .child(label(text).color(colors.text_muted))
                    .into()
            }
            View::Data => {
                self.grid.set_area(middle_area);
                let filters = querying.then(|| self.filter_row());
                let editor = self.grid.editing.map(|_| {
                    self.cell_editor.render(
                        "",
                        self.focused && self.focus == Focus::Cell,
                        colors.text,
                        FIELD_H - 6.0,
                        FieldFont::Mono,
                    )
                });
                let grid = self.grid.render(middle_area, editor, filters);
                div().col().h_px(middle_h).child(grid).into()
            }
            View::Structure | View::Ddl => div().h_px(middle_h).into(),
        };
        tree = tree
            .child(middle)
            .child(div().h_px(1.0).bg(colors.border_variant));
        tree = tree.child(if self.edits.is_empty() {
            self.status_bar()
        } else {
            self.pending_bar()
        });
        let tree: Node = tree.into();
        let mut painted = ui::render(&tree, main);
        details::clip_right(&mut painted, main.x + main.w);
        if self.view != View::Data {
            let content: Node = match (self.view, &self.structure, &self.ddl) {
                (View::Structure, Some(structure), _) => {
                    details::structure_view(structure, self.hovered)
                }
                (View::Ddl, _, Some(Ok(ddl))) => details::ddl_view(ddl, self.hovered),
                (View::Ddl, _, Some(Err(error))) => div()
                    .col()
                    .p(12.0)
                    .child(label(error.clone()).size(12.0).color(colors.error))
                    .into(),
                _ => div()
                    .col()
                    .p(12.0)
                    .child(label("Loading...").size(12.0).color(colors.text_muted))
                    .into(),
            };
            let view = self.view_pane.paint(&content, middle_area);
            details::merge(&mut painted, view);
        }
        if side_open {
            let side = Rect::new(
                body.x + body.w - side_w,
                body.y,
                side_w,
                body.h,
                Rgba::TRANSPARENT,
            );
            let rule = ui::render(
                &div().bg(colors.border_variant).into(),
                Rect::new(side.x, side.y, scale, side.h, Rgba::TRANSPARENT),
            );
            details::merge(&mut painted, rule);
            let background = ui::render(
                &div().bg(colors.panel_background).into(),
                Rect::new(
                    side.x + scale,
                    side.y,
                    side.w - scale,
                    side.h,
                    Rgba::TRANSPARENT,
                ),
            );
            details::merge(&mut painted, background);
            let head_h = SIDE_HEAD_H * scale;
            let head = ui::render(
                &self.side_head(),
                Rect::new(
                    side.x + scale,
                    side.y,
                    side.w - scale,
                    head_h,
                    Rgba::TRANSPARENT,
                ),
            );
            details::merge(&mut painted, head);
            let content = self.side_content(side.w / scale - 1.0);
            let body_area = Rect::new(
                side.x + scale,
                side.y + head_h,
                side.w - scale,
                side.h - head_h,
                Rgba::TRANSPARENT,
            );
            let mut content = self.side_pane.paint(&content, body_area);
            details::clip_right(&mut content, side.x + side.w - 2.0);
            details::merge(&mut painted, content);
        }
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let modifiers = keystroke.modifiers;
        if modifiers.cmd && keystroke.key == "r" {
            self.run();
            return TerminalKeyOutcome::Handled;
        }
        if modifiers.cmd && keystroke.key == "s" {
            if self.focus == Focus::Cell {
                self.keep_cell_edit();
            } else if self.focus == Focus::Value {
                self.keep_value_edit();
            }
            self.apply();
            return TerminalKeyOutcome::Handled;
        }
        match self.focus {
            Focus::Value => return self.value_keystroke(keystroke),
            Focus::Grid => {}
            _ => return self.field_keystroke(keystroke),
        }
        let page = self.grid.page_rows();
        match keystroke.key.as_str() {
            "c" if modifiers.cmd => {
                return self
                    .grid
                    .selected_text()
                    .map_or(TerminalKeyOutcome::Ignored, TerminalKeyOutcome::Copy)
            }
            "enter" if modifiers.shift => {
                self.details = !self.details;
                true
            }
            "enter" => {
                self.edit_selected();
                true
            }
            "up" => self.grid.move_selection(-1, 0),
            "down" => self.grid.move_selection(1, 0),
            "left" => self.grid.move_selection(0, -1),
            "right" => self.grid.move_selection(0, 1),
            "pageup" => self.grid.move_selection(-page, 0),
            "pagedown" => self.grid.move_selection(page, 0),
            _ => return TerminalKeyOutcome::Ignored,
        };
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        if self.focus == Focus::Value {
            self.value_editor.insert(text);
            return;
        }
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = self.field() {
            field.insert(&typed);
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        if self.focus == Focus::Value {
            self.value_editor.insert(text);
            return;
        }
        let line: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = self.field() {
            field.insert(&line);
        }
    }

    fn selected_text(&self) -> Option<String> {
        self.grid.selected_text()
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = self.hit(x, y) {
            if self.focus == Focus::Cell && !(FILTER_BASE..FILTER_END).contains(&id) {
                self.keep_cell_edit();
            }
            self.click(id);
            return true;
        }
        if self.side_pane.contains(x, y) {
            return true;
        }
        match self.focus {
            Focus::Cell => self.keep_cell_edit(),
            Focus::Value => {}
            _ => self.focus = Focus::Grid,
        }
        if self.view != View::Data {
            return true;
        }
        match self.grid.pointer_down(x, y) {
            GridEvent::Sort(column) => self.sort_by(column),
            GridEvent::Follow(row, column) => self.follow(row, column),
            GridEvent::Edit(_, _) => self.edit_selected(),
            GridEvent::None => {}
        }
        true
    }

    fn pointer_drag(&mut self, x: f32, _y: f32, _modifiers: Modifiers) -> bool {
        self.grid.pointer_drag(x)
    }

    fn pointer_up(&mut self, _x: f32, _y: f32, _modifiers: Modifiers) {
        self.grid.pointer_up();
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        self.grid.pointer_move(x, y) || changed
    }

    fn pointer_scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) -> bool {
        if self.side_pane.contains(x, y) {
            return self.side_pane.scroll_by(delta_y);
        }
        if self.view != View::Data {
            return self.view_pane.scroll_by(delta_y);
        }
        if modifiers.shift {
            self.grid.scroll_columns(delta_y)
        } else {
            self.grid.scroll_rows(delta_y)
        }
    }

    fn pointer_scroll_x(&mut self, _x: f32, _y: f32, delta_x: f32) -> bool {
        self.grid.scroll_columns(delta_x)
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = false;
        if let Some(page) = self.loading.poll() {
            self.apply_page(page);
            changed = true;
        }
        if let Some(structure) = self.loading_structure.poll() {
            match structure {
                Ok(structure) => self.set_structure(structure),
                Err(error) => eprintln!("database: read {}: {error}", self.table.qualified()),
            }
            changed = true;
        }
        if let Some(labels) = self.loading_labels.poll() {
            if let Ok(labels) = labels {
                self.labels = labels.into_iter().collect();
                self.grid.labels = (0..self.grid.columns().len())
                    .map(|column| self.labels.get(&column).cloned().unwrap_or_default())
                    .collect();
            }
            changed = true;
        }
        if let Some(ddl) = self.loading_ddl.poll() {
            self.ddl = Some(ddl);
            changed = true;
        }
        if let Some(target) = self.loading_target.poll() {
            if let Some((_, shown)) = self.target.as_mut() {
                *shown = target.unwrap_or_else(Target::Missing);
            }
            changed = true;
        }
        if let Some(counts) = self.loading_counts.poll() {
            if let (Some((_, shown)), Ok(counts)) = (self.counts.as_mut(), counts) {
                *shown = counts;
            }
            changed = true;
        }
        if let Some(applied) = self.applying.poll() {
            match applied {
                Ok(rows) => {
                    let count = self.edits.len();
                    self.edits.clear();
                    self.status = Some((
                        format!(
                            "Saved {count} {} ({rows} {})",
                            if count == 1 { "change" } else { "changes" },
                            if rows == 1 { "row" } else { "rows" }
                        ),
                        false,
                    ));
                    self.run();
                }
                Err(error) => self.status = Some((format!("Nothing saved: {error}"), true)),
            }
            changed = true;
        }
        if let Some(exported) = self.exporting.poll() {
            self.status = Some(match exported {
                Ok((rows, path)) => (format!("Exported {rows} rows to {}", path.display()), false),
                Err(error) => (format!("Export failed: {error}"), true),
            });
            changed = true;
        }
        changed |= self.check_opened();
        ItemTick {
            changed,
            clipboard_store: self.clipboard.take(),
            ..ItemTick::default()
        }
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }

    fn is_busy(&self) -> bool {
        self.loading.busy()
            || self.exporting.busy()
            || self.loading_structure.busy()
            || self.loading_ddl.busy()
            || self.loading_target.busy()
            || self.loading_counts.busy()
            || self.loading_labels.busy()
            || self.applying.busy()
            || !self.opened.is_empty()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users() -> Table {
        Table {
            schema: "public".into(),
            name: "users".into(),
            kind: TableKind::Table,
            count: None,
        }
    }

    fn column(
        name: &str,
        primary_key: bool,
        references: Option<(&str, &str)>,
    ) -> pom_db::ColumnInfo {
        pom_db::ColumnInfo {
            name: name.into(),
            data_type: if name == "settings" { "jsonb" } else { "text" }.into(),
            nullable: !primary_key,
            default: None,
            primary_key,
            references: references.map(|(table, column)| (table.into(), column.into())),
        }
    }

    fn structure() -> TableStructure {
        TableStructure {
            columns: vec![
                column("id", true, None),
                column("name", false, None),
                column("org_id", false, Some(("orgs", "id"))),
                column("settings", false, None),
            ],
            indexes: Vec::new(),
            referenced_by: vec![pom_db::Reference {
                schema: "public".into(),
                table: "orders".into(),
                column: "user_id".into(),
                on_delete: "cascade".into(),
            }],
        }
    }

    fn page(rows: &[(&str, &str)]) -> QueryResult {
        QueryResult {
            columns: ["id", "name", "org_id", "settings"]
                .map(str::to_string)
                .to_vec(),
            rows: rows
                .iter()
                .map(|(id, name)| {
                    vec![
                        Some(id.to_string()),
                        Some(name.to_string()),
                        Some("3".into()),
                        Some(r#"{"theme":"dark"}"#.into()),
                    ]
                })
                .collect(),
            ..QueryResult::default()
        }
    }

    fn item(context: &crate::tests::TestContext) -> TableItem {
        let database = context.context.databases().remove(0);
        let mut item = TableItem::new(context.context.clone(), database, users());
        item.show_page(page(&[("1", "Ann"), ("2", "Bob")]), Some(2));
        item.show_structure(structure());
        item
    }

    fn reveals(item: &mut TableItem) -> Vec<String> {
        item.take_requests()
            .into_iter()
            .filter_map(|request| match request {
                PanelRequest::Reveal { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn edits_find_their_row_by_primary_key_and_save_as_one_transaction() {
        let context = crate::tests::context();
        let mut item = item(&context);
        item.edit_cell(0, 1, Some("Anna"));
        item.edit_cell(1, 1, Some("Bob"));
        assert_eq!(
            item.pending_statements(),
            ["UPDATE \"public\".\"users\" SET \"name\" = 'Anna' WHERE \"id\" = '1'"],
            "an unchanged value stages nothing"
        );
        assert!(item.grid.edited.contains(&(0, 1)));
        item.show_page(page(&[("2", "Bob"), ("1", "Ann")]), Some(2));
        assert_eq!(item.current(1, 1).as_deref(), Some("Anna"));
        assert!(item.grid.edited.contains(&(1, 1)) && !item.grid.edited.contains(&(0, 1)));
        item.edit_cell(1, 1, Some("Ann"));
        assert!(
            item.edits.is_empty(),
            "typing the original back drops the edit"
        );
        assert_eq!(item.read_only(0), Some("Primary key"));
        assert_eq!(item.read_only(1), None);
        let mut keyless = structure();
        keyless.columns[0].primary_key = false;
        item.show_structure(keyless);
        assert_eq!(
            item.read_only(1),
            Some("No primary key; edit it in a console")
        );
    }

    #[test]
    fn a_foreign_key_and_the_tables_pointing_here_open_filtered_tabs() {
        let context = crate::tests::context();
        let mut item = item(&context);
        item.follow(0, 2);
        let opened = reveals(&mut item);
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains(":orgs:\"id\" = '3'"), "{opened:?}");
        item.select_cell(1, 1);
        item.open_reference(0, true);
        let opened = reveals(&mut item);
        assert!(
            opened[0].contains(":orders:\"user_id\" = '2'"),
            "{opened:?}"
        );
        item.open_reference(0, false);
        assert!(reveals(&mut item)[0].ends_with(":orders"));
    }

    #[test]
    fn column_filters_join_the_typed_where() {
        let context = crate::tests::context();
        let mut item = item(&context);
        item.filter.set_text("id > 0");
        item.set_column_filter("name", "= Ann");
        item.set_column_filter("settings", "null");
        assert_eq!(
            item.where_clause(),
            "(id > 0) AND \"name\"::text = 'Ann' AND \"settings\" IS NULL"
        );
    }

    #[test]
    fn a_value_saved_in_its_editor_tab_is_staged() {
        let context = crate::tests::context();
        let mut item = item(&context);
        item.select_cell(0, 3);
        item.open_in_tab();
        let path = match item.take_requests().pop() {
            Some(PanelRequest::OpenFile(path)) => path,
            _ => panic!("expected the value to open as a file"),
        };
        assert_eq!(
            std::fs::read_to_string(&path).expect("written"),
            "{\n  \"theme\": \"dark\"\n}"
        );
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "{\"theme\": ").expect("save");
        item.files_checked = Instant::now() - FILE_CHECK;
        assert!(item.check_opened());
        assert!(item.edits.is_empty());
        assert!(item
            .status
            .as_ref()
            .is_some_and(|(text, error)| *error && text.contains("not valid JSON")));
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "{\n  \"theme\": \"light\"\n}\n").expect("save");
        item.files_checked = Instant::now() - FILE_CHECK;
        assert!(item.check_opened());
        assert_eq!(
            item.pending_statements(),
            ["UPDATE \"public\".\"users\" SET \"settings\" = '{\n  \"theme\": \"light\"\n}' WHERE \"id\" = '1'"]
        );
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::*;

    #[test]
    fn toolbar_and_filter_boxes_take_clicks_and_typing() {
        let context = crate::tests::context();
        let database = context.context.databases().remove(0);
        let table = Table {
            schema: "public".into(),
            name: "users".into(),
            kind: TableKind::Table,
            count: None,
        };
        let mut item = TableItem::new(context.context.clone(), database, table);
        item.show_page(
            QueryResult {
                columns: vec!["id".into(), "name".into()],
                rows: vec![vec![Some("1".into()), Some("Ann".into())]],
                ..QueryResult::default()
            },
            Some(1),
        );
        let body = Rect::new(0.0, 0.0, 1200.0, 600.0, Rgba::TRANSPARENT);
        item.paint_body(body, true);
        let centre = |item: &TableItem, id: u64| {
            let (rect, _) = item
                .hits
                .iter()
                .find(|(_, hit)| *hit == id)
                .unwrap_or_else(|| {
                    panic!(
                        "no hit {id}: {:?}",
                        item.hits.iter().map(|(_, id)| id).collect::<Vec<_>>()
                    )
                });
            (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
        };
        for (id, focus) in [
            (WHERE_FIELD, Focus::Where),
            (ORDER_FIELD, Focus::Order),
            (FILTER_BASE + 1, Focus::Column(1)),
        ] {
            let (x, y) = centre(&item, id);
            item.pointer_down(x, y, 1, Modifiers::default());
            assert_eq!(item.focus, focus, "clicking {id}");
            item.input_text("ab");
            item.paint_body(body, true);
        }
        assert_eq!(item.filter.text(), "ab");
        assert_eq!(
            item.column_filters
                .get("name")
                .map(TextField::text)
                .as_deref(),
            Some("ab")
        );
        let (x, y) = centre(&item, DETAILS);
        item.pointer_down(x, y, 1, Modifiers::default());
        assert!(!item.details);
        item.paint_body(body, true);
        let (x, y) = centre(&item, VIEW_STRUCTURE);
        item.pointer_down(x, y, 1, Modifiers::default());
        assert_eq!(item.view, View::Structure);
    }
}

#[cfg(test)]
mod saved_tests {
    use super::*;

    #[test]
    fn a_table_tab_comes_back_with_its_filters_and_side() {
        let context = crate::tests::context();
        let database = context.context.databases().remove(0);
        let table = Table {
            schema: "public".into(),
            name: "users".into(),
            kind: TableKind::Table,
            count: None,
        };
        let mut item =
            TableItem::filtered(context.context.clone(), database.clone(), table, "id > 1");
        item.order.set_text("id DESC");
        item.set_column_filter("name", "= Ann");
        item.side = Side::Row;
        item.view = View::Structure;
        let saved = item.serialize().expect("saved");
        let restored = restore_table(&context.context, &saved).expect("restored");
        let restored = restored
            .as_any()
            .and_then(|item| item.downcast_ref::<TableItem>())
            .expect("a table tab");
        assert_eq!(restored.id(), item.id());
        assert_eq!(restored.filter.text(), "id > 1");
        assert_eq!(restored.order.text(), "id DESC");
        assert_eq!(
            restored
                .column_filters
                .get("name")
                .map(TextField::text)
                .as_deref(),
            Some("= Ann")
        );
        assert_eq!((restored.view, restored.side), (View::Structure, Side::Row));
        let mut other = saved.clone();
        other.data["database"] = "gone".into();
        assert!(
            restore_table(&context.context, &other).is_none(),
            "a database no longer there"
        );
    }
}
