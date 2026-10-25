use components::key_chip;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::search::{SearchOptions, SearchQuery};
use files::search::{FileResult, SearchRequest};
use files::ProjectFs;
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, material_icon, theme, IconKind, Node, Rect};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

pub const ITEM_ID: &str = "project-search";
const INPUT_H: f32 = 28.0;
const BUTTON: f32 = 26.0;
/// The case, word and regex toggles inside the query box.
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 8.0;
const LINE_GAP: f32 = 6.0;
const HEADER_ROW_H: f32 = 30.0;
const LINE_H: f32 = 22.0;
const FIELD_FONT: f32 = 12.0;
const SEPARATOR_H: f32 = 10.0;
const CODE_FONT: f32 = 13.0;
const GUTTER_W: f32 = 56.0;
const TYPE_DEBOUNCE: Duration = Duration::from_millis(250);

const QUERY: u64 = 1;
const REPLACEMENT: u64 = 2;
const INCLUDE: u64 = 3;
const EXCLUDE: u64 = 4;
const CASE: u64 = 5;
const WORD: u64 = 6;
const REGEX: u64 = 7;
const FILTERS: u64 = 8;
const REPLACE: u64 = 9;
const PREVIOUS: u64 = 10;
const NEXT: u64 = 11;
const REPLACE_ALL: u64 = 12;
const ROW_BASE: u64 = 1_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Query,
    Replacement,
    Include,
    Exclude,
}

enum Message {
    File(FileResult),
    Done { limited: bool },
    Failed(String),
}

struct Running {
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Message>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Header(usize),
    Line {
        file: usize,
        excerpt: usize,
        line: usize,
    },
    Separator,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    Searching,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: String,
    pub row: u32,
    pub column: u32,
}

pub struct ReplaceRequest {
    pub paths: Vec<String>,
    pub query: String,
    pub replacement: String,
    pub options: SearchOptions,
}

pub struct ProjectSearch {
    fs: Arc<dyn ProjectFs>,
    query: TextField,
    replacement: TextField,
    include: TextField,
    exclude: TextField,
    focus: Option<Field>,
    focused: bool,
    options: SearchOptions,
    filters_enabled: bool,
    replace_enabled: bool,
    include_ignored: bool,
    running: Option<Running>,
    phase: Phase,
    results: Vec<FileResult>,
    limit_reached: bool,
    error: Option<String>,
    /// The query, options and scope the results came from; typing searches again once this differs.
    searched: Option<String>,
    typed_at: Option<Instant>,
    /// (file, excerpt, line, hit) of every match, in result order.
    matches: Vec<(usize, usize, usize, usize)>,
    active_match: Option<usize>,
    rows: Vec<Row>,
    scroll: f32,
    body_h: f32,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    open: Option<OpenRequest>,
    replace: Option<ReplaceRequest>,
}

fn font_field() -> TextField {
    let mut field = TextField::default();
    field.set_font_size(FIELD_FONT);
    field
}

impl ProjectSearch {
    pub fn new(fs: Arc<dyn ProjectFs>, query: &str) -> ProjectSearch {
        let mut search = ProjectSearch {
            fs,
            query: font_field(),
            replacement: font_field(),
            include: font_field(),
            exclude: font_field(),
            focus: Some(Field::Query),
            focused: true,
            options: SearchOptions::default(),
            filters_enabled: false,
            replace_enabled: false,
            include_ignored: false,
            running: None,
            phase: Phase::Idle,
            results: Vec::new(),
            limit_reached: false,
            error: None,
            searched: None,
            typed_at: None,
            matches: Vec::new(),
            active_match: None,
            rows: Vec::new(),
            scroll: 0.0,
            body_h: 0.0,
            hits: Vec::new(),
            hovered: None,
            open: None,
            replace: None,
        };
        search.set_query(query);
        search
    }

    /// Puts `query` in the field (selected, so typing replaces it) and searches for it.
    pub fn set_query(&mut self, query: &str) {
        self.focus = Some(Field::Query);
        if query.is_empty() {
            self.query.select_all();
            return;
        }
        self.query.set_text(query);
        self.search();
    }

    pub fn take_open(&mut self) -> Option<OpenRequest> {
        self.open.take()
    }

    pub fn take_replace(&mut self) -> Option<ReplaceRequest> {
        self.replace.take()
    }

    /// Clicks the first result line (tests); false while none has arrived.
    #[cfg(test)]
    pub fn open_first_result(&mut self) -> bool {
        self.poll();
        let Some(row) = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Line { .. }))
        else {
            return false;
        };
        let row = self.rows[row];
        self.open_row(row);
        true
    }

    /// Types `replacement` and presses Replace All (tests).
    #[cfg(test)]
    pub fn replace_all_with(&mut self, replacement: &str) {
        self.poll();
        self.replace_enabled = true;
        self.replacement.set_text(replacement);
        self.request_replace_all();
    }

    /// Fills the replace and include boxes and opens both rows, as a person would before searching (previews).
    pub fn show_scope(&mut self, replacement: &str, include: &str) {
        self.replace_enabled = true;
        self.filters_enabled = true;
        self.replacement.set_text(replacement);
        self.include.set_text(include);
        self.search();
    }

    /// Blocks until the running search ends, with the filter row shown when `filters` (previews).
    pub fn wait_for_results(&mut self, filters: bool) {
        self.filters_enabled |= filters;
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.running.is_some() && Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn signature(&self) -> String {
        format!(
            "{}\u{0}{:?}\u{0}{}\u{0}{}\u{0}{}",
            self.query.text(),
            self.options,
            self.include.text(),
            self.exclude.text(),
            self.include_ignored
        )
    }

    pub fn search(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
        self.typed_at = None;
        self.searched = Some(self.signature());
        self.results.clear();
        self.matches.clear();
        self.active_match = None;
        self.limit_reached = false;
        self.error = None;
        self.scroll = 0.0;
        self.rebuild_rows();
        let text = self.query.text();
        if text.is_empty() {
            self.phase = Phase::Idle;
            return;
        }
        let request = SearchRequest {
            query: text,
            options: self.options,
            include: self.include.text(),
            exclude: self.exclude.text(),
            include_ignored: self.include_ignored,
        };
        if let Err(error) = request.query().and_then(|_| request.scope()) {
            self.error = Some(error);
            self.phase = Phase::Done;
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let (fs, stop) = (self.fs.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("project-search".into())
            .spawn(move || {
                let found = std::sync::Mutex::new(sender.clone());
                let outcome = fs.search(&request, &stop, &|file| {
                    if let Ok(sender) = found.lock() {
                        if sender.send(Message::File(file)).is_ok() {
                            ui::wake();
                        }
                    }
                });
                let message = match outcome {
                    Ok(limited) => Message::Done { limited },
                    Err(error) => Message::Failed(error.to_string()),
                };
                if sender.send(message).is_ok() {
                    ui::wake();
                }
            });
        match spawned {
            Ok(_) => {
                self.phase = Phase::Searching;
                self.running = Some(Running { cancel, receiver });
            }
            Err(error) => {
                self.error = Some(format!("search: {error}"));
                self.phase = Phase::Done;
            }
        }
    }

    /// Takes the results that arrived; returns whether any did.
    fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let message = match self
                .running
                .as_ref()
                .map(|running| running.receiver.try_recv())
            {
                Some(Ok(message)) => message,
                Some(Err(TryRecvError::Disconnected)) => Message::Done { limited: false },
                Some(Err(TryRecvError::Empty)) | None => break,
            };
            changed = true;
            match message {
                Message::File(file) => {
                    let at = self.results.partition_point(|known| known.path < file.path);
                    self.results.insert(at, file);
                }
                Message::Done { limited } => {
                    self.limit_reached = limited;
                    self.running = None;
                    self.phase = Phase::Done;
                }
                Message::Failed(error) => {
                    self.error = Some(error);
                    self.running = None;
                    self.phase = Phase::Done;
                }
            }
        }
        if changed {
            self.rebuild_rows();
        }
        changed
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut matches = Vec::new();
        for (file_index, file) in self.results.iter().enumerate() {
            rows.push(Row::Header(file_index));
            for (excerpt_index, excerpt) in file.excerpts.iter().enumerate() {
                if excerpt_index > 0 {
                    rows.push(Row::Separator);
                }
                for (line_index, line) in excerpt.iter().enumerate() {
                    rows.push(Row::Line {
                        file: file_index,
                        excerpt: excerpt_index,
                        line: line_index,
                    });
                    for hit in 0..line.hits.len() {
                        matches.push((file_index, excerpt_index, line_index, hit));
                    }
                }
            }
        }
        self.rows = rows;
        self.matches = matches;
        if self.active_match.is_none() && !self.matches.is_empty() {
            self.active_match = Some(0);
        }
        self.active_match = self
            .active_match
            .filter(|active| *active < self.matches.len());
    }

    fn row_height(row: Row) -> f32 {
        match row {
            Row::Header(_) => HEADER_ROW_H,
            Row::Line { .. } => LINE_H,
            Row::Separator => SEPARATOR_H,
        }
    }

    fn content_height(&self) -> f32 {
        self.rows.iter().map(|row| Self::row_height(*row)).sum()
    }

    fn step_match(&mut self, forward: bool) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        let next = match (self.active_match, forward) {
            (Some(active), true) => (active + 1) % count,
            (Some(active), false) => (active + count - 1) % count,
            (None, _) => 0,
        };
        self.active_match = Some(next);
        self.reveal_active();
    }

    fn reveal_active(&mut self) {
        let Some(&(file, excerpt, line, _)) = self
            .active_match
            .and_then(|active| self.matches.get(active))
        else {
            return;
        };
        let mut top = 0.0;
        for row in &self.rows {
            if *row
                == (Row::Line {
                    file,
                    excerpt,
                    line,
                })
            {
                break;
            }
            top += Self::row_height(*row);
        }
        let visible = (self.body_h - 120.0).max(LINE_H);
        if top < self.scroll {
            self.scroll = top;
        } else if top + LINE_H > self.scroll + visible {
            self.scroll = top + LINE_H - visible;
        }
    }

    fn open_row(&mut self, row: Row) {
        let (file, number, column) = match row {
            Row::Header(file) => {
                let first = self.results[file]
                    .excerpts
                    .iter()
                    .flatten()
                    .find(|line| !line.hits.is_empty());
                (
                    file,
                    first.map_or(0, |line| line.number),
                    first
                        .and_then(|line| line.hits.first())
                        .map_or(0, |hit| hit.start),
                )
            }
            Row::Line {
                file,
                excerpt,
                line,
            } => {
                let line = &self.results[file].excerpts[excerpt][line];
                (
                    file,
                    line.number,
                    line.hits.first().map_or(0, |hit| hit.start),
                )
            }
            Row::Separator => return,
        };
        let path = self.results[file].path.clone();
        let text = self.results[file]
            .excerpts
            .iter()
            .flatten()
            .find(|line| line.number == number)
            .map(|line| line.text.clone())
            .unwrap_or_default();
        let column = text
            .get(..column)
            .map_or(0, |prefix| prefix.chars().count());
        self.open = Some(OpenRequest {
            path,
            row: number as u32 + 1,
            column: column as u32 + 1,
        });
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus? {
            Field::Query => Some(&mut self.query),
            Field::Replacement => Some(&mut self.replacement),
            Field::Include => Some(&mut self.include),
            Field::Exclude => Some(&mut self.exclude),
        }
    }

    fn edited(&mut self) {
        if matches!(
            self.focus,
            Some(Field::Query | Field::Include | Field::Exclude)
        ) {
            self.typed_at = Some(Instant::now());
        }
    }

    fn fields_in_order(&self) -> Vec<Field> {
        let mut fields = vec![Field::Query];
        if self.replace_enabled {
            fields.push(Field::Replacement);
        }
        if self.filters_enabled {
            fields.extend([Field::Include, Field::Exclude]);
        }
        fields
    }

    fn request_replace_all(&mut self) {
        if self.results.is_empty() || self.query.text().is_empty() {
            return;
        }
        self.replace = Some(ReplaceRequest {
            paths: self.results.iter().map(|file| file.path.clone()).collect(),
            query: self.query.text(),
            replacement: self.replacement.text(),
            options: self.options,
        });
    }

    fn click(&mut self, id: u64) {
        match id {
            QUERY => self.focus = Some(Field::Query),
            REPLACEMENT => self.focus = Some(Field::Replacement),
            INCLUDE => self.focus = Some(Field::Include),
            EXCLUDE => self.focus = Some(Field::Exclude),
            CASE => {
                self.options.case_sensitive = !self.options.case_sensitive;
                self.search();
            }
            WORD => {
                self.options.whole_word = !self.options.whole_word;
                self.search();
            }
            REGEX => {
                self.options.regex = !self.options.regex;
                self.search();
            }
            FILTERS => {
                self.filters_enabled = !self.filters_enabled;
                if self.filters_enabled {
                    self.focus = Some(Field::Include);
                }
            }
            REPLACE => {
                self.replace_enabled = !self.replace_enabled;
                if self.replace_enabled {
                    self.focus = Some(Field::Replacement);
                }
            }
            PREVIOUS => self.step_match(false),
            NEXT => self.step_match(true),
            REPLACE_ALL => self.request_replace_all(),
            id if id >= ROW_BASE => {
                if let Some(row) = self.rows.get((id - ROW_BASE) as usize).copied() {
                    self.open_row(row);
                }
            }
            _ => {}
        }
    }

    fn icon_button(&self, id: u64, kind: IconKind, toggled: bool, enabled: bool) -> Node {
        components::icon_button(id, kind)
            .toggled(toggled)
            .enabled(enabled)
            .hovered(self.hovered)
            .into()
    }

    fn option_button(&self, id: u64, kind: IconKind, toggled: bool) -> Node {
        components::icon_button(id, kind)
            .size(components::ButtonSize::XSmall)
            .toggled(toggled)
            .hovered(self.hovered)
            .into()
    }

    fn input(&self, id: u64, which: Field, field: &TextField, placeholder: &str) -> ui::Div {
        let colors = theme();
        let focused = self.focused && self.focus == Some(which);
        let text_color = if which == Field::Query
            && self.phase == Phase::Done
            && self.matches.is_empty()
            && !self.query.text().is_empty()
        {
            colors.error
        } else {
            colors.text
        };
        let border = if which == Field::Query && self.error.is_some() {
            colors.error
        } else if focused {
            colors.border_focused
        } else {
            colors.border
        };
        div()
            .row()
            .h_px(INPUT_H)
            .pl(10.0)
            .pr(4.0)
            .gap(8.0)
            .items_center()
            .rounded(6.0)
            .bg(colors.background)
            .border(1.0, border)
            .on_click(id)
            .child(div().row().flex(1.0).items_center().child(field.render(
                placeholder,
                focused,
                text_color,
                INPUT_H - 8.0,
                FieldFont::Mono,
            )))
    }

    fn bar(&self, width: f32) -> Node {
        let colors = theme();
        let mode_w = 256.0_f32.min(width * 0.45);
        let query_w = (width - 2.0 * PAD_X - LINE_GAP - mode_w).max(160.0);
        let query_row = self
            .input(QUERY, Field::Query, &self.query, "Search all files...")
            .w_px(query_w)
            .child(
                div()
                    .row()
                    .pr(4.0)
                    .gap(6.0)
                    .items_center()
                    .child(self.option_button(
                        CASE,
                        IconKind::CaseSensitive,
                        self.options.case_sensitive,
                    ))
                    .child(self.option_button(WORD, IconKind::WholeWord, self.options.whole_word))
                    .child(self.option_button(REGEX, IconKind::Regex, self.options.regex)),
            );
        let has_match = self.active_match.is_some();
        let count = match self.active_match {
            Some(index) if self.limit_reached => format!("{}/{}+", index + 1, self.matches.len()),
            Some(index) => format!("{}/{}", index + 1, self.matches.len()),
            None => "0/0".to_string(),
        };
        let mut mode = div()
            .row()
            .w_px(mode_w)
            .gap(4.0)
            .items_center()
            .child(self.icon_button(FILTERS, IconKind::Filter, self.filters_enabled, true))
            .child(self.icon_button(REPLACE, IconKind::Replace, self.replace_enabled, true))
            .child(
                div()
                    .row()
                    .px(2.0)
                    .child(components::vertical_divider(18.0)),
            )
            .child(self.icon_button(PREVIOUS, IconKind::ChevronLeft, false, has_match))
            .child(self.icon_button(NEXT, IconKind::ChevronRight, false, has_match))
            .child(div().row().h_px(BUTTON).items_center().pl(6.0).child(
                label(count).size(12.0).color(if has_match {
                    colors.text_muted
                } else {
                    colors.text_placeholder
                }),
            ));
        if self.phase == Phase::Searching {
            mode = mode.child(
                icon(IconKind::RotateCw)
                    .size(14.0)
                    .color(colors.text_accent),
            );
        }
        let mut bar = div()
            .col()
            .gap(LINE_GAP)
            .px(PAD_X)
            .py(PAD_Y)
            .bg(colors.panel_background)
            .child(
                div()
                    .row()
                    .h_px(INPUT_H)
                    .gap(LINE_GAP + 2.0)
                    .items_center()
                    .child(query_row)
                    .child(mode),
            );
        if let Some(error) = &self.error {
            bar = bar.child(
                div()
                    .row()
                    .pl(10.0)
                    .child(label(error.clone()).size(12.0).color(colors.error)),
            );
        }
        if self.replace_enabled {
            let ready = !self.results.is_empty();
            bar = bar.child(
                div()
                    .row()
                    .h_px(INPUT_H)
                    .gap(LINE_GAP + 2.0)
                    .items_center()
                    .child(
                        self.input(
                            REPLACEMENT,
                            Field::Replacement,
                            &self.replacement,
                            "Replace in project...",
                        )
                        .w_px(query_w),
                    )
                    .child(
                        div()
                            .row()
                            .w_px(mode_w)
                            .gap(4.0)
                            .items_center()
                            .child(self.icon_button(
                                REPLACE_ALL,
                                IconKind::ReplaceAll,
                                false,
                                ready,
                            ))
                            .child(
                                div()
                                    .row()
                                    .pl(6.0)
                                    .gap(6.0)
                                    .items_center()
                                    .child(
                                        label("Replace all")
                                            .size(12.0)
                                            .color(colors.text_placeholder),
                                    )
                                    .child(key_chip("cmd-enter", false)),
                            ),
                    ),
            );
        }
        if self.filters_enabled {
            bar = bar.child(
                div()
                    .row()
                    .h_px(INPUT_H)
                    .gap(LINE_GAP + 2.0)
                    .items_center()
                    .child(
                        self.input(
                            INCLUDE,
                            Field::Include,
                            &self.include,
                            "Include: e.g. src/**/*.rs",
                        )
                        .flex(1.0),
                    )
                    .child(
                        self.input(
                            EXCLUDE,
                            Field::Exclude,
                            &self.exclude,
                            "Exclude: e.g. vendor/*, *.lock",
                        )
                        .flex(1.0),
                    ),
            );
        }
        div().col().child(bar).child(components::divider()).into()
    }

    fn landing(&self) -> Node {
        let (heading, detail) = match self.phase {
            Phase::Searching => ("Searching...", None),
            Phase::Done if self.error.is_none() => (
                "No Results",
                Some("No results found in this project for the provided query"),
            ),
            _ => (
                "Search All Files",
                Some("Include/exclude specific paths with the filter option. Matching exact word and/or casing is available too."),
            ),
        };
        let mut state = components::empty_state(heading);
        if let Some(detail) = detail {
            state = state.body(detail, 380.0);
        }
        if self.phase == Phase::Idle {
            state = state.action(
                div()
                    .row()
                    .gap(6.0)
                    .items_center()
                    .child(key_chip("enter", false))
                    .child(label("Search").size(12.0).color(theme().text_placeholder)),
            );
        }
        state.into()
    }

    fn line_node(
        &self,
        line: &files::search::SearchLine,
        file: usize,
        excerpt: usize,
        index: usize,
        id: u64,
    ) -> Node {
        let colors = theme();
        let active = self
            .active_match
            .and_then(|active| self.matches.get(active))
            .filter(|(f, e, l, _)| (*f, *e, *l) == (file, excerpt, index))
            .map(|(_, _, _, hit)| *hit);
        let mut text_row = div().row().items_center();
        let mut at = 0;
        let piece = |range: Range<usize>| line.text.get(range).unwrap_or_default().to_string();
        for (hit_index, hit) in line.hits.iter().enumerate() {
            if hit.start > at {
                text_row = text_row.child(
                    label(piece(at..hit.start))
                        .size(CODE_FONT)
                        .mono()
                        .color(colors.text),
                );
            }
            let background = if active == Some(hit_index) {
                colors.search_active_match_background
            } else {
                colors.search_match_background
            };
            text_row = text_row.child(
                div().bg(background).child(
                    label(piece(hit.start..hit.end.max(hit.start)))
                        .size(CODE_FONT)
                        .mono()
                        .color(colors.text),
                ),
            );
            at = hit.end.max(at);
        }
        if at < line.text.len() {
            text_row = text_row.child(
                label(piece(at..line.text.len()))
                    .size(CODE_FONT)
                    .mono()
                    .color(colors.text),
            );
        }
        let mut row = div()
            .row()
            .h_px(LINE_H)
            .items_center()
            .on_click(id)
            .child(
                div().row().w_px(GUTTER_W).pr(12.0).justify_end().child(
                    label((line.number + 1).to_string())
                        .size(CODE_FONT)
                        .mono()
                        .color(colors.editor_line_number),
                ),
            )
            .child(text_row);
        if active.is_some() || self.hovered == Some(id) {
            row = row.bg(colors.ghost_element_hover);
        }
        row.into()
    }

    fn results_node(&self, width: f32, height: f32) -> Node {
        let colors = theme();
        let mut column = div().col().w_px(width);
        let mut top = 0.0;
        for (index, row) in self.rows.iter().enumerate() {
            let row_h = Self::row_height(*row);
            if top + row_h <= self.scroll {
                top += row_h;
                continue;
            }
            // Whole rows only: text draws above every box, so a row cut off below would show past the tab.
            if top + row_h > self.scroll + height {
                break;
            }
            top += row_h;
            let id = ROW_BASE + index as u64;
            column = column.child(match *row {
                Row::Header(file) => {
                    let result = &self.results[file];
                    let (folder, name) = match result.path.rsplit_once('/') {
                        Some((folder, name)) => (folder.to_string(), name.to_string()),
                        None => (String::new(), result.path.clone()),
                    };
                    let mut header = div()
                        .row()
                        .h_px(HEADER_ROW_H)
                        .px(12.0)
                        .gap(6.0)
                        .items_center()
                        .bg(colors.panel_background)
                        .on_click(id)
                        .child(material_icon(crate::file_icon(&name)).size(14.0))
                        .child(label(name).size(13.0).color(colors.text))
                        .child(
                            label(folder)
                                .size(12.0)
                                .color(colors.text_muted)
                                .truncate_start(),
                        )
                        .child(div().row().flex(1.0))
                        .child(
                            label(result.match_count.to_string())
                                .size(12.0)
                                .color(colors.text_muted),
                        );
                    if self.hovered == Some(id) {
                        header = header.bg(colors.element_hover);
                    }
                    header.into()
                }
                Row::Line {
                    file,
                    excerpt,
                    line,
                } => self.line_node(
                    &self.results[file].excerpts[excerpt][line],
                    file,
                    excerpt,
                    line,
                    id,
                ),
                Row::Separator => div()
                    .row()
                    .h_px(SEPARATOR_H)
                    .pl(GUTTER_W)
                    .items_center()
                    .child(div().row().flex(1.0).h_px(1.0).bg(colors.border_variant))
                    .into(),
            });
        }
        column.into()
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

impl Drop for ProjectSearch {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Item for ProjectSearch {
    fn id(&self) -> Option<String> {
        Some(ITEM_ID.to_string())
    }

    fn title(&self) -> String {
        let query = self.query.text();
        if query.is_empty() {
            "Project Search".into()
        } else {
            format!("Search: {query}")
        }
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Search)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let (width, height) = (body.w / scale, body.h / scale);
        self.body_h = height;
        let bar = self.bar(width);
        let bar_h = INPUT_H
            + 2.0 * PAD_Y
            + 1.0
            + if self.error.is_some() {
                LINE_GAP + 16.0
            } else {
                0.0
            }
            + if self.replace_enabled {
                LINE_GAP + INPUT_H
            } else {
                0.0
            }
            + if self.filters_enabled {
                LINE_GAP + INPUT_H
            } else {
                0.0
            };
        let results_h = (height - bar_h).max(0.0);
        let max_scroll = (self.content_height() - results_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let middle = if self.rows.is_empty() {
            self.landing()
        } else {
            self.results_node(width, results_h)
        };
        let tree: Node = div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(theme().editor_background)
            .child(bar)
            .child(div().col().h_px(results_h).child(middle))
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
        let key = keystroke.key.as_str();
        match key {
            "enter" if modifiers.cmd && self.focus == Some(Field::Replacement) => {
                self.request_replace_all();
                return TerminalKeyOutcome::Handled;
            }
            "enter" => {
                if self.searched.as_deref() != Some(self.signature().as_str())
                    || self.running.is_some()
                {
                    self.search();
                } else {
                    self.step_match(!modifiers.shift);
                }
                return TerminalKeyOutcome::Handled;
            }
            "tab" => {
                let fields = self.fields_in_order();
                let at = self
                    .focus
                    .and_then(|focus| fields.iter().position(|field| *field == focus))
                    .unwrap_or(0);
                let next = if modifiers.shift {
                    (at + fields.len() - 1) % fields.len()
                } else {
                    (at + 1) % fields.len()
                };
                self.focus = fields.get(next).copied();
                return TerminalKeyOutcome::Handled;
            }
            "g" if modifiers.cmd => {
                self.step_match(!modifiers.shift);
                return TerminalKeyOutcome::Handled;
            }
            "c" if modifiers.alt && modifiers.cmd => {
                self.click(CASE);
                return TerminalKeyOutcome::Handled;
            }
            "w" if modifiers.alt && modifiers.cmd => {
                self.click(WORD);
                return TerminalKeyOutcome::Handled;
            }
            "x" if modifiers.alt && modifiers.cmd => {
                self.click(REGEX);
                return TerminalKeyOutcome::Handled;
            }
            "j" if modifiers.cmd && modifiers.shift => {
                self.click(FILTERS);
                return TerminalKeyOutcome::Handled;
            }
            "h" if modifiers.cmd && modifiers.shift => {
                self.click(REPLACE);
                return TerminalKeyOutcome::Handled;
            }
            "f" if modifiers.cmd && modifiers.shift => {
                self.focus = Some(Field::Query);
                self.query.select_all();
                return TerminalKeyOutcome::Handled;
            }
            "c" if modifiers.cmd => {
                return self
                    .field()
                    .and_then(|field| field.selected_text())
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
            }
            "v" if modifiers.cmd => return TerminalKeyOutcome::Paste,
            _ => {}
        }
        match Self::edit_key(keystroke) {
            Some((key, shift)) => {
                let changed = self.field().is_some_and(|field| field.key(key, shift));
                if changed {
                    self.edited();
                }
                TerminalKeyOutcome::Handled
            }
            None => TerminalKeyOutcome::Ignored,
        }
    }

    fn input_text(&mut self, text: &str) {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return;
        }
        if let Some(field) = self.field() {
            field.insert(&typed);
        }
        self.edited();
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(&text.replace('\n', " "));
    }

    fn selected_text(&self) -> Option<String> {
        None
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
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        let changed = hit != self.hovered;
        self.hovered = hit;
        changed
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let before = self.scroll;
        self.scroll = (self.scroll - delta_y / ui::ui_text_scale()).max(0.0);
        (self.scroll - before).abs() > 0.01
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = self.poll();
        if self
            .typed_at
            .is_some_and(|at| at.elapsed() >= TYPE_DEBOUNCE)
        {
            if self.searched.as_deref() != Some(self.signature().as_str()) {
                self.search();
                changed = true;
            } else {
                self.typed_at = None;
            }
        }
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.running.is_some() || self.typed_at.is_some()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// `text` broken at spaces into lines no wider than `width`, so each can be centered on its own.
/// What a replace over files no tab holds did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReplaceReport {
    pub replaced: usize,
    /// Files that changed between the read and the write, left as they were.
    pub conflicts: Vec<String>,
    pub failures: Vec<(String, String)>,
}

impl ReplaceReport {
    /// What to tell the person, when something was left undone.
    pub fn problem(&self) -> Option<String> {
        let list = |paths: &[&str]| {
            let mut shown = paths.iter().take(3).copied().collect::<Vec<_>>().join(", ");
            if paths.len() > 3 {
                shown.push_str(&format!(" and {} more", paths.len() - 3));
            }
            shown
        };
        if !self.conflicts.is_empty() {
            let paths: Vec<&str> = self.conflicts.iter().map(String::as_str).collect();
            return Some(format!(
                "Not replaced in {}: changed while replacing",
                list(&paths)
            ));
        }
        let (path, error) = self.failures.first()?;
        let paths: Vec<&str> = self
            .failures
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        Some(if paths.len() == 1 {
            format!("Not replaced in {path}: {error}")
        } else {
            format!("Not replaced in {}: {error}", list(&paths))
        })
    }
}

/// Replaces every match of `query` in `paths` through `fs`, each write expecting the file as it was read.
pub fn replace_in_files(
    fs: &dyn ProjectFs,
    paths: &[String],
    query: &SearchQuery,
) -> ReplaceReport {
    let mut report = ReplaceReport::default();
    for path in paths {
        let read = match fs.read(path) {
            Ok(read) => read,
            Err(error) => {
                report.failures.push((path.clone(), error.to_string()));
                continue;
            }
        };
        let Some(text) = read.text else {
            continue;
        };
        let Some(replaced) = replace_in_text(&text, query) else {
            continue;
        };
        let expect = files::FileStat {
            dir: false,
            size: read.size,
            mtime_ns: read.mtime_ns,
        };
        match fs.write(path, &replaced, Some(&expect)) {
            Ok(_) => report.replaced += 1,
            Err(files::fs::WriteError::Conflict(_)) => report.conflicts.push(path.clone()),
            Err(files::fs::WriteError::Failed(error)) => {
                report.failures.push((path.clone(), error.to_string()))
            }
        }
    }
    report
}

pub fn replace_in_text(text: &str, query: &SearchQuery) -> Option<String> {
    let hits = query.find_in(text);
    if hits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for hit in hits {
        let line_start = text[..hit.start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let line_end = text[hit.end..]
            .find('\n')
            .map_or(text.len(), |newline| hit.end + newline);
        let line = &text[line_start..line_end];
        let replacement = query
            .replacement_for(line, hit.start - line_start..hit.end - line_start)
            .unwrap_or_default();
        out.push_str(&text[at..hit.start]);
        out.push_str(&replacement);
        at = hit.end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).expect("dir");
        std::fs::write(
            root.join("src/a.rs"),
            "fn alpha() {}\nfn beta() { alpha(); }\n",
        )
        .expect("write");
        std::fs::write(root.join("src/b.rs"), "// alpha again\n").expect("write");
        std::fs::write(root.join("notes.md"), "nothing here\n").expect("write");
        temp
    }

    fn settle(search: &mut ProjectSearch) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while search.running.is_some() && Instant::now() < deadline {
            search.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn results_come_grouped_by_file_in_path_order() {
        let temp = project();
        let mut search = ProjectSearch::new(
            Arc::new(files::LocalFs::new(temp.path().to_path_buf())),
            "alpha",
        );
        settle(&mut search);
        let paths: Vec<&str> = search
            .results
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["src/a.rs", "src/b.rs"]);
        assert_eq!(search.matches.len(), 3);
        assert_eq!(search.active_match, Some(0));
        assert_eq!(search.title(), "Search: alpha");
        search.step_match(false);
        assert_eq!(search.active_match, Some(2));
    }

    #[test]
    fn include_filters_narrow_and_bad_regexes_say_why() {
        let temp = project();
        let mut search =
            ProjectSearch::new(Arc::new(files::LocalFs::new(temp.path().to_path_buf())), "");
        search.click(FILTERS);
        search.input_text("src/b.rs");
        search.focus = Some(Field::Query);
        search.input_text("alpha");
        search.search();
        settle(&mut search);
        assert_eq!(search.results.len(), 1);
        search.click(REGEX);
        search.query.set_text("(alpha");
        search.search();
        assert!(search.error.is_some());
    }

    #[test]
    fn a_click_on_a_line_asks_to_open_it_at_the_match() {
        let temp = project();
        let mut search = ProjectSearch::new(
            Arc::new(files::LocalFs::new(temp.path().to_path_buf())),
            "alpha",
        );
        settle(&mut search);
        let row = search
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 0,
                        line: 1,
                        ..
                    }
                )
            })
            .expect("second line of the first file");
        search.click(ROW_BASE + row as u64);
        assert_eq!(
            search.take_open(),
            Some(OpenRequest {
                path: "src/a.rs".into(),
                row: 2,
                column: 13
            })
        );
    }

    #[test]
    fn replacing_keeps_regex_groups() -> Result<(), String> {
        let query = SearchQuery::new(
            r"fn (\w+)",
            SearchOptions {
                regex: true,
                ..SearchOptions::default()
            },
            Some("pub fn $1".into()),
        )?;
        assert_eq!(
            replace_in_text("fn a() {}\nfn b() {}\n", &query).as_deref(),
            Some("pub fn a() {}\npub fn b() {}\n")
        );
        Ok(())
    }

    /// Files on disk whose writes are checked like the server checks them, with one file edited by someone else
    /// between the read and the write.
    struct CheckedFs {
        local: files::LocalFs,
        edited_meanwhile: String,
    }

    impl ProjectFs for CheckedFs {
        fn local_root(&self) -> Option<&std::path::Path> {
            None
        }
        fn watch(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn files::TreeFeed> {
            self.local.watch(wake)
        }
        fn stat(&self, path: &str) -> std::io::Result<files::FileStat> {
            self.local.stat(path)
        }
        fn read(&self, path: &str) -> std::io::Result<files::FileResult> {
            self.local.read(path)
        }
        fn read_bytes(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
            self.local.read_bytes(path, limit)
        }
        fn project_files(&self, include_ignored: bool) -> std::io::Result<Vec<String>> {
            self.local.project_files(include_ignored)
        }
        fn write(
            &self,
            path: &str,
            text: &str,
            expect: Option<&files::FileStat>,
        ) -> Result<files::FileStat, files::WriteError> {
            let now = self.local.stat(path)?;
            let unchanged = expect
                .is_some_and(|expect| expect.size == now.size && expect.mtime_ns == now.mtime_ns);
            if path == self.edited_meanwhile || !unchanged {
                return Err(files::WriteError::Conflict(now));
            }
            self.local.write(path, text, expect)
        }
    }

    #[test]
    fn replacing_leaves_a_file_that_changed_meanwhile_and_says_so() -> Result<(), String> {
        let temp = project();
        let fs = CheckedFs {
            local: files::LocalFs::new(temp.path().to_path_buf()),
            edited_meanwhile: "src/b.rs".into(),
        };
        let query = SearchQuery::new("alpha", SearchOptions::default(), Some("gamma".into()))?;
        let paths = vec![
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "notes.md".to_string(),
        ];
        let report = replace_in_files(&fs, &paths, &query);
        assert_eq!(report.replaced, 1);
        assert_eq!(report.conflicts, ["src/b.rs"]);
        assert!(report.failures.is_empty());
        let read = |path: &str| std::fs::read_to_string(temp.path().join(path)).unwrap_or_default();
        assert_eq!(read("src/a.rs"), "fn gamma() {}\nfn beta() { gamma(); }\n");
        assert_eq!(read("src/b.rs"), "// alpha again\n");
        assert_eq!(
            report.problem().as_deref(),
            Some("Not replaced in src/b.rs: changed while replacing")
        );
        Ok(())
    }

    #[test]
    fn files_that_cannot_be_searched_say_why_before_anything_runs() {
        let unserved = files::UnservedFs::new("Files need a newer pom on dev; update it there");
        assert_eq!(
            unserved.search_refusal().as_deref(),
            Some("Files need a newer pom on dev; update it there")
        );
        let temp = project();
        assert_eq!(
            files::LocalFs::new(temp.path().to_path_buf()).search_refusal(),
            None
        );
    }
}
