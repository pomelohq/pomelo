//! A table tab: one page of a table's rows in the grid, with WHERE / ORDER BY fields, a page size, paging
//! ("1-500 of 1234"), sorting by clicking a column header, copying a cell and exporting the whole result as CSV.
//! A Redis keyspace tab lists its keys instead (no filter or paging).

use pom_db::{Database, Engine, QueryResult, Table};
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

use crate::grid::{Grid, GridEvent};
use crate::{DatabaseContext, Pending};

const TOOLBAR_H: f32 = 36.0;
const STATUS_H: f32 = 28.0;
const FIELD_H: f32 = 24.0;
const FIELD_FONT: f32 = 12.0;
const PAGE_SIZES: [usize; 4] = [100, 500, 1000, 5000];
const DEFAULT_PAGE: usize = 500;

const WHERE_FIELD: u64 = 1;
const ORDER_FIELD: u64 = 2;
const PAGE_SIZE: u64 = 3;
const REFRESH: u64 = 4;
const FIRST_PAGE: u64 = 5;
const PREVIOUS_PAGE: u64 = 6;
const NEXT_PAGE: u64 = 7;
const EXPORT: u64 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Grid,
    Where,
    Order,
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
    focus: Focus,
    focused: bool,
    page_size: usize,
    offset: usize,
    total: Option<u64>,
    shown: usize,
    grid: Grid,
    loading: Pending<Page>,
    exporting: Pending<(u64, std::path::PathBuf)>,
    status: Option<(String, bool)>,
    hits: Vec<(Rect, u64)>,
}

impl TableItem {
    pub fn item_id(database: &Database, table: &Table) -> String {
        format!("db-table:{}:{}", database.name, table.qualified())
    }

    pub fn new(context: DatabaseContext, database: Database, table: Table) -> TableItem {
        let field = || {
            let mut field = TextField::default();
            field.set_font_size(FIELD_FONT);
            field
        };
        let mut item = TableItem {
            context,
            database,
            table,
            filter: field(),
            order: field(),
            focus: Focus::Grid,
            focused: false,
            page_size: DEFAULT_PAGE,
            offset: 0,
            total: None,
            shown: 0,
            grid: Grid::default(),
            loading: Pending::idle(),
            exporting: Pending::idle(),
            status: None,
            hits: Vec::new(),
        };
        item.run();
        item
    }

    fn is_redis(&self) -> bool {
        self.database.engine == Engine::Redis
    }

    fn run(&mut self) {
        let database = self.database.clone();
        let (limit, offset) = (self.page_size, self.offset);
        let (query, count) = if self.is_redis() {
            (format!("{}:*", self.table.name), None)
        } else {
            let (filter, order) = (self.filter.text(), self.order.text());
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
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
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
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let stem = self.table.name.replace(['/', '\\'], "_");
        let mut path = downloads.join(format!("{stem}.csv"));
        let mut copy = 1;
        while path.exists() {
            path = downloads.join(format!("{stem}-{copy}.csv"));
            copy += 1;
        }
        let sql = pom_db::table_select(&self.table, &self.filter.text(), &self.order.text());
        let database = self.database.clone();
        self.status = Some(("Exporting...".into(), false));
        self.exporting = self.context.run(move |connector| {
            connector
                .export_csv(&database, &sql, &path)
                .map(|rows| (rows, path))
        });
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::Where => Some(&mut self.filter),
            Focus::Order => Some(&mut self.order),
            Focus::Grid => None,
        }
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
            _ => {}
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
        }
        button.into()
    }

    fn text_button(&self, id: u64, text: String) -> Node {
        let colors = theme();
        div()
            .row()
            .h_px(22.0)
            .px(6.0)
            .rounded(4.0)
            .items_center()
            .border(1.0, colors.border_variant)
            .on_click(id)
            .child(label(text).size(12.0).color(colors.text))
            .into()
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

    fn toolbar(&self) -> Node {
        let colors = theme();
        let title = div()
            .row()
            .gap(6.0)
            .items_center()
            .child(icon(IconKind::Table).size(12.0).color(colors.icon_muted))
            .child(label(self.table.qualified()).size(13.0).color(colors.text))
            .child(
                label(self.database.label.clone())
                    .size(11.0)
                    .color(colors.text_placeholder),
            );
        let mut bar = div()
            .row()
            .h_px(TOOLBAR_H)
            .px(8.0)
            .gap(8.0)
            .items_center()
            .child(title);
        if self.is_redis() {
            bar = bar.child(div().row().flex(1.0));
        } else {
            bar = bar
                .child(self.input(
                    WHERE_FIELD,
                    "WHERE",
                    &self.filter,
                    "id > 10",
                    self.focus == Focus::Where,
                ))
                .child(self.input(
                    ORDER_FIELD,
                    "ORDER BY",
                    &self.order,
                    "created_at DESC",
                    self.focus == Focus::Order,
                ))
                .child(self.text_button(PAGE_SIZE, format!("{} rows", self.page_size)));
        }
        bar.child(self.icon_button(REFRESH, IconKind::RotateCw, true))
            .into()
    }

    fn status_bar(&self) -> Node {
        let colors = theme();
        let mut bar = div().row().h_px(STATUS_H).px(8.0).gap(4.0).items_center();
        if !self.is_redis() {
            bar = bar
                .child(self.icon_button(FIRST_PAGE, IconKind::ChevronLeft, self.offset > 0))
                .child(self.icon_button(PREVIOUS_PAGE, IconKind::ArrowLeft, self.offset > 0))
                .child(label(self.page_label()).size(12.0).color(colors.text_muted))
                .child(self.icon_button(NEXT_PAGE, IconKind::ArrowRight, self.can_go_next()));
        } else {
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
        if !self.is_redis() {
            bar = bar.child(self.text_button(EXPORT, "Export CSV".into()));
        }
        bar.into()
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

    /// Shows a page as if the query had returned it (previews and tests).
    pub fn show_page(&mut self, result: QueryResult, total: Option<u64>) {
        self.loading = Pending::idle();
        self.status = None;
        self.apply_page(Ok(Page { result, total }));
    }

    fn apply_page(&mut self, page: Result<Page, String>) {
        match page {
            Ok(page) => {
                self.shown = page.result.rows.len();
                self.total = page.total;
                let affected = page.result.rows_affected;
                self.grid.set_data(page.result.columns, page.result.rows);
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
}

impl Item for TableItem {
    fn id(&self) -> Option<String> {
        Some(Self::item_id(&self.database, &self.table))
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

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let grid_area = Rect::new(
            body.x,
            body.y + (TOOLBAR_H + 1.0) * scale,
            body.w,
            (body.h - (TOOLBAR_H + STATUS_H + 2.0) * scale).max(0.0),
            Rgba::TRANSPARENT,
        );
        let colors = theme();
        let empty = self.grid.columns().is_empty();
        let middle: Node = if empty {
            let text = if self.loading.busy() {
                "Loading..."
            } else {
                "No rows"
            };
            div()
                .row()
                .h_px(grid_area.h / scale)
                .justify_center()
                .pt(24.0)
                .child(label(text).color(colors.text_muted))
                .into()
        } else {
            let grid = self.grid.render(grid_area);
            div().col().h_px(grid_area.h / scale).child(grid).into()
        };
        let tree: Node = div()
            .col()
            .w_px(body.w / scale)
            .h_px(body.h / scale)
            .bg(colors.editor_background)
            .child(self.toolbar())
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(middle)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(self.status_bar())
            .into();
        let painted = ui::render(&tree, body);
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
        if self.focus != Focus::Grid {
            match keystroke.key.as_str() {
                "enter" => {
                    self.focus = Focus::Grid;
                    self.grid.sort = None;
                    self.rerun_from_start();
                }
                "escape" => self.focus = Focus::Grid,
                "tab" => {
                    self.focus = if self.focus == Focus::Where {
                        Focus::Order
                    } else {
                        Focus::Where
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
            return TerminalKeyOutcome::Handled;
        }
        let page = self.grid.page_rows();
        match keystroke.key.as_str() {
            "c" if modifiers.cmd => {
                return self
                    .grid
                    .selected_text()
                    .map_or(TerminalKeyOutcome::Ignored, TerminalKeyOutcome::Copy)
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
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = self.field() {
            field.insert(&typed);
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
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
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        if let Some(id) = hit {
            self.click(id);
            return true;
        }
        self.focus = Focus::Grid;
        if let GridEvent::Sort(column) = self.grid.pointer_down(x, y) {
            self.sort_by(column);
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
        self.grid.pointer_move(x, y)
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, modifiers: Modifiers) -> bool {
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
        if let Some(exported) = self.exporting.poll() {
            self.status = Some(match exported {
                Ok((rows, path)) => (format!("Exported {rows} rows to {}", path.display()), false),
                Err(error) => (format!("Export failed: {error}"), true),
            });
            changed = true;
        }
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.loading.busy() || self.exporting.busy()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
