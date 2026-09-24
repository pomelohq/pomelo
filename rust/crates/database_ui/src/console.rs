//! A SQL console: an editor (the files editor, text kept only in the saved consoles) with the results docked
//! under it. Cmd+Enter runs the selection or the statement at the caret, Cmd+Shift+Enter the whole text; the
//! footer's top edge drags to share the height between the two.

use std::time::{Duration, Instant};

use pom_db::{Console, ConsoleKind, Database, Engine, QueryResult};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::persistence::SerializedItem;
use workspace::{EditKey, Item, ItemFooter, RunRequest};

use crate::grid::Grid;
use crate::{DatabaseContext, Pending};

const TOOLBAR_H: f32 = 32.0;
const DEFAULT_HEIGHT: f32 = 280.0;
const MIN_HEIGHT: f32 = 80.0;
const MIN_EDITOR: f32 = 60.0;
const EDGE_HIT: f32 = 4.0;
const SAVE_AFTER: Duration = Duration::from_millis(700);
const ROW_LIMIT: usize = 500;

const CONSOLE_KIND: &str = "db-console";

const DATABASE_PICKER: u64 = 1;
const RUN: u64 = 2;

pub struct ConsoleFooter {
    context: DatabaseContext,
    console: Console,
    databases: Vec<Database>,
    height: f32,
    grid: Grid,
    running: Pending<QueryResult>,
    note: Option<(String, bool)>,
    save_due: Option<Instant>,
    resizing: Option<(f32, f32)>,
    run_requested: bool,
    area: Rect,
    hits: Vec<(Rect, u64)>,
}

impl ConsoleFooter {
    pub fn new(context: DatabaseContext, console: Console) -> ConsoleFooter {
        let databases = context.databases();
        ConsoleFooter {
            context,
            console,
            databases,
            height: DEFAULT_HEIGHT,
            grid: Grid::default(),
            running: Pending::idle(),
            note: None,
            save_due: None,
            resizing: None,
            run_requested: false,
            area: Rect::new(0.0, 0.0, 0.0, 0.0, Rgba::TRANSPARENT),
            hits: Vec::new(),
        }
    }

    /// Shows a result as if a run had returned it (previews and tests).
    pub fn show_result(&mut self, result: Result<QueryResult, String>) {
        self.running = Pending::idle();
        self.apply(result);
    }

    /// The status line under the toolbar and the grid's shape (tests).
    pub fn summary(&self) -> (Option<(String, bool)>, usize, usize) {
        (
            self.note.clone(),
            self.grid.columns().len(),
            self.grid.rows().len(),
        )
    }

    fn database(&self) -> Option<&Database> {
        self.databases
            .iter()
            .find(|database| database.name == self.console.database)
    }

    /// A console deleted from the panel while its tab is open is not brought back.
    fn save(&mut self) {
        self.save_due = None;
        let mut consoles = self.context.consoles();
        if let Some(saved) = consoles
            .iter_mut()
            .find(|saved| saved.id == self.console.id)
        {
            *saved = self.console.clone();
            self.context.save_consoles(&consoles);
        }
    }

    fn next_database(&mut self) {
        if self.databases.is_empty() {
            return;
        }
        let at = self
            .databases
            .iter()
            .position(|database| database.name == self.console.database)
            .map_or(0, |at| (at + 1) % self.databases.len());
        self.console.database = self.databases[at].name.clone();
        self.save();
    }

    fn start(&mut self, sql: String) {
        let Some(database) = self.database().cloned() else {
            self.note = Some(("Pick a database first".into(), true));
            return;
        };
        if sql.trim().is_empty() {
            self.note = Some(("Nothing to run".into(), true));
            return;
        }
        self.note = None;
        self.running = self
            .context
            .run(move |connector| connector.query(&database, &sql, ROW_LIMIT));
    }

    fn toolbar(&self) -> Node {
        let colors = theme();
        let database = self.database();
        let picker = div()
            .row()
            .h_px(22.0)
            .px(6.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .border(1.0, colors.border_variant)
            .on_click(DATABASE_PICKER)
            .child(icon(IconKind::Cylinder).size(12.0).color(colors.icon_muted))
            .child(
                label(database.map_or_else(
                    || "Select database".to_string(),
                    |database| format!("{} - {}", database.repo, database.label),
                ))
                .size(12.0)
                .color(colors.text),
            )
            .child(
                icon(IconKind::ChevronUpDown)
                    .size(10.0)
                    .color(colors.icon_muted),
            );
        let running = self.running.busy();
        let run = div()
            .row()
            .h_px(22.0)
            .px(6.0)
            .gap(4.0)
            .items_center()
            .rounded(4.0)
            .on_click(RUN)
            .child(icon(IconKind::Play).size(12.0).color(if running {
                colors.text_disabled
            } else {
                colors.success
            }))
            .child(label("Run").size(12.0).color(colors.text))
            .child(workspace::render_keystroke("cmd-enter", 11.0));
        let (status, error) = if running {
            ("Running...".to_string(), false)
        } else {
            self.note.clone().unwrap_or_default()
        };
        div()
            .row()
            .h_px(TOOLBAR_H)
            .px(8.0)
            .gap(8.0)
            .items_center()
            .child(picker)
            .child(run)
            .child(
                div().row().flex(1.0).items_center().child(
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
            .into()
    }

    fn apply(&mut self, answer: Result<QueryResult, String>) {
        match answer {
            Ok(result) => {
                let count = result.rows.len();
                let note = match (result.rows_affected, result.columns.is_empty()) {
                    (Some(affected), true) => format!("{affected} rows affected"),
                    _ if result.truncated => format!("{count} rows (capped at {ROW_LIMIT})"),
                    _ => format!("{count} rows"),
                };
                self.note = Some((note, false));
                self.grid.set_data(result.columns, result.rows);
            }
            Err(error) => {
                self.note = Some((error.lines().next().unwrap_or_default().to_string(), true))
            }
        }
    }
}

impl ItemFooter for ConsoleFooter {
    fn height(&mut self, body_h: f32) -> f32 {
        let scale = ui::ui_text_scale();
        (self.height * scale).clamp(
            MIN_HEIGHT * scale,
            (body_h - MIN_EDITOR * scale).max(MIN_HEIGHT * scale),
        )
    }

    fn paint(&mut self, area: Rect) -> Option<ui::Painted> {
        self.area = area;
        let scale = ui::ui_text_scale();
        let colors = theme();
        let grid_area = Rect::new(
            area.x,
            area.y + (TOOLBAR_H + 1.0) * scale,
            area.w,
            (area.h - (TOOLBAR_H + 1.0) * scale).max(0.0),
            Rgba::TRANSPARENT,
        );
        let results: Node = if self.grid.columns().is_empty() {
            let hint = match self.database().map(|database| database.engine) {
                Some(Engine::Redis) => "Run a Redis command (GET key) or a key pattern (user:*)",
                _ => "Run a query with Cmd+Enter",
            };
            div()
                .row()
                .h_px(grid_area.h / scale)
                .justify_center()
                .pt(16.0)
                .child(label(hint).size(12.0).color(colors.text_muted))
                .into()
        } else {
            let grid = self.grid.render(grid_area);
            div().col().h_px(grid_area.h / scale).child(grid).into()
        };
        let tree: Node = div()
            .col()
            .w_px(area.w / scale)
            .h_px(area.h / scale)
            .bg(colors.editor_background)
            .child(self.toolbar())
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(results)
            .into();
        let painted = ui::render(&tree, area);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32) -> bool {
        if (y - self.area.y).abs() <= EDGE_HIT * ui::ui_text_scale() {
            self.resizing = Some((y, self.height));
            return true;
        }
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        match hit {
            Some(DATABASE_PICKER) => self.next_database(),
            Some(RUN) => self.run_requested = true,
            _ => {
                self.grid.pointer_down(x, y);
            }
        }
        true
    }

    fn pointer_drag(&mut self, x: f32, y: f32) -> bool {
        if let Some((start_y, start_height)) = self.resizing {
            self.height = (start_height - (y - start_y) / ui::ui_text_scale()).max(MIN_HEIGHT);
            return true;
        }
        self.grid.pointer_drag(x)
    }

    fn pointer_up(&mut self) {
        self.resizing = None;
        self.grid.pointer_up();
    }

    fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        self.grid.pointer_move(x, y)
    }

    fn scroll(&mut self, delta_x: f32, delta_y: f32, shift: bool) -> bool {
        let sideways = if shift { delta_y } else { delta_x };
        let moved_x = sideways != 0.0 && self.grid.scroll_columns(sideways);
        let moved_y = !shift && delta_y != 0.0 && self.grid.scroll_rows(delta_y);
        moved_x || moved_y
    }

    fn key(&mut self, key: EditKey, _shift: bool) -> bool {
        let page = self.grid.page_rows();
        match key {
            EditKey::Up => self.grid.move_selection(-1, 0),
            EditKey::Down => self.grid.move_selection(1, 0),
            EditKey::Left => self.grid.move_selection(0, -1),
            EditKey::Right => self.grid.move_selection(0, 1),
            EditKey::PageUp => self.grid.move_selection(-page, 0),
            EditKey::PageDown => self.grid.move_selection(page, 0),
            _ => return false,
        };
        true
    }

    fn copy(&self) -> Option<String> {
        self.grid.selected_text()
    }

    fn run(&mut self, request: RunRequest) {
        let sql = match request.selection {
            Some(selection) => selection,
            None if request.all => request.text.trim().to_string(),
            None => pom_db::statement_at(&request.text, request.caret),
        };
        self.start(sql);
    }

    fn take_run(&mut self) -> Option<bool> {
        std::mem::take(&mut self.run_requested).then_some(false)
    }

    fn text_changed(&mut self, text: &str) {
        if self.console.sql != text {
            self.console.sql = text.to_string();
            self.save_due = Some(Instant::now() + SAVE_AFTER);
        }
    }

    fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(answer) = self.running.poll() {
            self.apply(answer);
            changed = true;
        }
        if self.save_due.is_some_and(|due| Instant::now() >= due) {
            self.save();
        }
        changed
    }

    fn busy(&self) -> bool {
        self.running.busy() || self.save_due.is_some()
    }

    fn serialize(&self) -> Option<SerializedItem> {
        Some(SerializedItem {
            kind: CONSOLE_KIND.into(),
            data: serde_json::json!({ "id": self.console.id }),
        })
    }

    fn title(&self) -> Option<String> {
        let database = self.database().map_or_else(
            || self.console.database.clone(),
            |database| database.label.clone(),
        );
        Some(format!("{} [{database}]", self.console.title))
    }
}

/// A new query console on `database`, numbered after the saved ones ("query 3").
pub fn new_console(existing: &[Console], database: &Database) -> Console {
    let queries = existing
        .iter()
        .filter(|console| console.kind == ConsoleKind::Query)
        .count();
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    Console {
        id: format!("C{stamp:X}-{sequence}"),
        title: format!("query {}", queries + 1),
        database: database.name.clone(),
        kind: ConsoleKind::Query,
        limit: ROW_LIMIT,
        ..Console::default()
    }
}

pub(crate) fn console_item_id(console: &Console) -> String {
    format!("db-console:{}", console.id)
}

/// The console as an editor tab with its results under it.
pub(crate) fn console_item(context: DatabaseContext, console: Console) -> Box<dyn Item> {
    let (id, title, sql) = (
        console_item_id(&console),
        console.title.clone(),
        console.sql.clone(),
    );
    files_ui::scratch_editor(
        id,
        title,
        &sql,
        Box::new(ConsoleFooter::new(context, console)),
    )
}

/// A saved console tab, rebuilt from the consoles file (gone if the console was deleted since).
pub(crate) fn restore_console(
    context: &DatabaseContext,
    item: &SerializedItem,
) -> Option<Box<dyn Item>> {
    if item.kind != CONSOLE_KIND {
        return None;
    }
    let id = item.data.get("id")?.as_str()?;
    let console = context
        .consoles()
        .into_iter()
        .find(|console| console.id == id)?;
    Some(console_item(context.clone(), console))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_consoles_are_numbered_after_the_saved_queries() {
        let database = Database {
            name: "demo_api_main".into(),
            engine: Engine::Postgres,
            repo: "api".into(),
            label: "main".into(),
        };
        let first = new_console(&[], &database);
        assert_eq!(first.title, "query 1");
        assert_eq!(first.database, "demo_api_main");
        let table = Console {
            kind: ConsoleKind::Table,
            ..Console::default()
        };
        let second = new_console(&[first.clone(), table], &database);
        assert_eq!(second.title, "query 2");
        assert_ne!(first.id, second.id);
    }

    fn footer(context: &crate::tests::TestContext) -> ConsoleFooter {
        let database = context.context.databases().remove(0);
        let console = new_console(&[], &database);
        context
            .context
            .save_consoles(std::slice::from_ref(&console));
        ConsoleFooter::new(context.context.clone(), console)
    }

    #[test]
    fn results_say_how_many_rows_came_back_or_were_changed() {
        let context = crate::tests::context();
        let mut footer = footer(&context);
        footer.show_result(Ok(QueryResult {
            columns: vec!["id".into()],
            rows: vec![vec![Some("1".into())], vec![Some("2".into())]],
            ..QueryResult::default()
        }));
        assert_eq!(footer.summary(), (Some(("2 rows".into(), false)), 1, 2));
        footer.show_result(Ok(QueryResult {
            rows_affected: Some(3),
            ..QueryResult::default()
        }));
        assert_eq!(footer.summary().0, Some(("3 rows affected".into(), false)));
        footer.show_result(Err("ERROR: relation \"nope\" does not exist\nLINE 1".into()));
        assert_eq!(
            footer.summary().0,
            Some(("ERROR: relation \"nope\" does not exist".into(), true))
        );
    }

    #[test]
    fn a_blank_statement_is_not_sent() {
        let context = crate::tests::context();
        let mut footer = footer(&context);
        footer.run(RunRequest {
            text: "  \n\n   \n".into(),
            selection: None,
            caret: 4,
            all: false,
        });
        assert!(!footer.busy());
        assert_eq!(footer.summary().0, Some(("Nothing to run".into(), true)));
        footer.run(RunRequest {
            text: "select 1;".into(),
            selection: None,
            caret: 3,
            all: false,
        });
        assert!(footer.busy());
    }

    #[test]
    fn typing_saves_the_sql_and_the_tab_comes_back_from_it() {
        let context = crate::tests::context();
        let mut footer = footer(&context);
        footer.text_changed("select * from users;");
        assert!(footer.busy());
        footer.save_due = Some(Instant::now());
        footer.tick();
        assert!(!footer.busy());
        let saved = context.context.consoles();
        assert_eq!(saved[0].sql, "select * from users;");
        let Some(item) = footer.serialize() else {
            panic!("a console tab should save its state");
        };
        let Some(restored) = restore_console(&context.context, &item) else {
            panic!("a saved console should come back");
        };
        assert_eq!(restored.title(), "query 1 [main]");
        context.context.save_consoles(&[]);
        assert!(restore_console(&context.context, &item).is_none());
    }
}
