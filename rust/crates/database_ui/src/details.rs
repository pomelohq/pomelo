//! What a table tab shows beside and instead of its grid: the Details side (the selected cell's whole value, or
//! its row as a record with the tables that point at it), the Structure view and the DDL view.

use std::collections::HashSet;

use pom_db::TableStructure;
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rect, Rgba};

use crate::json::{self, Json};

pub(crate) const SIDE_VALUE: u64 = 30;
pub(crate) const SIDE_ROW: u64 = 31;
pub(crate) const SIDE_CLOSE: u64 = 32;
pub(crate) const SET_NULL: u64 = 33;
pub(crate) const REVERT: u64 = 34;
pub(crate) const COPY_VALUE: u64 = 35;
pub(crate) const OPEN_IN_TAB: u64 = 36;
pub(crate) const EXPAND_ALL: u64 = 37;
pub(crate) const COLLAPSE: u64 = 38;
pub(crate) const OPEN_TARGET: u64 = 39;
pub(crate) const SAVE_VALUE: u64 = 41;
pub(crate) const CANCEL_VALUE: u64 = 42;
pub(crate) const COPY_DDL: u64 = 44;
pub(crate) const VALUE_AREA: u64 = 45;
pub(crate) const FOLD_BASE: u64 = 2000;
pub(crate) const FOLD_END: u64 = 4000;
pub(crate) const FIELD_BASE: u64 = 4000;
pub(crate) const FIELD_LINK_BASE: u64 = 4500;
pub(crate) const REFERENCE_BASE: u64 = 5000;
pub(crate) const STRUCTURE_REFERENCE_BASE: u64 = 5500;
pub(crate) const STRUCTURE_LINK_BASE: u64 = 6000;
pub(crate) const RANGE_END: u64 = 6500;
pub(crate) const ROW_EXPAND_BASE: u64 = 6500;
pub(crate) const ROW_COLLAPSE_BASE: u64 = 6700;
pub(crate) const ROW_COPY_BASE: u64 = 6900;
pub(crate) const ROW_OPEN_BASE: u64 = 7100;
pub(crate) const ROW_END: u64 = 7300;

const JSON_LINE_H: f32 = 18.0;
/// Children an open object or array lists before "show N more".
const SHOWN_CHILDREN: usize = 50;
const TEXT: f32 = 12.0;

/// A scrolled region whose content taller than the view is cut to it.
#[derive(Default)]
pub(crate) struct Pane {
    pub scroll: f32,
    content_h: f32,
    view_h: f32,
    rect: Option<Rect>,
}

impl Pane {
    pub fn paint(&mut self, tree: &Node, area: Rect) -> ui::Painted {
        let scale = ui::ui_text_scale();
        self.rect = Some(area);
        self.content_h = ui::measure(tree).1 * scale;
        self.view_h = area.h;
        self.scroll = self
            .scroll
            .clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let shifted = Rect::new(
            area.x,
            area.y - self.scroll,
            area.w,
            area.h.max(self.content_h),
            Rgba::TRANSPARENT,
        );
        let mut painted = ui::render(tree, shifted);
        let (top, bottom) = (area.y, area.y + area.h);
        painted.rects.retain_mut(|rect| {
            let (start, end) = (rect.y.max(top), (rect.y + rect.h).min(bottom));
            if end <= start {
                return false;
            }
            rect.h = end - start;
            rect.y = start;
            true
        });
        painted
            .texts
            .retain(|text| text.y >= top - 1.0 && text.y + text.size <= bottom + 1.0);
        painted
            .icons
            .retain(|quad| quad.y >= top - 1.0 && quad.y + quad.h <= bottom + 1.0);
        painted.tris.clear();
        painted
            .hits
            .retain(|(rect, _)| rect.y + rect.h > top && rect.y < bottom);
        painted
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.is_some_and(|rect| {
            x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
        })
    }

    pub fn scroll_by(&mut self, delta: f32) -> bool {
        let next = (self.scroll - delta).clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    /// The scrolled-off part in design px, and the view's height.
    pub fn window(&self) -> (f32, f32) {
        let scale = ui::ui_text_scale();
        (self.scroll / scale, self.view_h / scale)
    }
}

/// Drops what spilled past `right` (a grid's last column under the side beside it).
pub(crate) fn clip_right(painted: &mut ui::Painted, right: f32) {
    painted.rects.retain_mut(|rect| {
        if rect.x >= right {
            return false;
        }
        rect.w = rect.w.min(right - rect.x);
        true
    });
    painted.texts.retain(|text| text.x < right - 4.0);
    painted.icons.retain(|quad| quad.x + quad.w <= right + 1.0);
    painted.hits.retain_mut(|(rect, _)| {
        if rect.x >= right {
            return false;
        }
        rect.w = rect.w.min(right - rect.x);
        true
    });
}

pub(crate) fn merge(into: &mut ui::Painted, part: ui::Painted) {
    into.rects.extend(part.rects);
    into.tris.extend(part.tris);
    into.texts.extend(part.texts);
    into.icons.extend(part.icons);
    into.hits.extend(part.hits);
}

fn small(text: impl Into<String>, color: Rgba) -> ui::Label {
    label(text.into()).label_size(LabelSize::Small).color(color)
}

fn link(id: u64, text: &str, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut part = div()
        .row()
        .px(4.0)
        .h_px(18.0)
        .items_center()
        .rounded(3.0)
        .on_click(id)
        .child(small(text, colors.text_accent));
    if hovered == Some(id) {
        part = part.bg(colors.ghost_element_hover);
    }
    part.into()
}

fn section(title: &str) -> Node {
    div()
        .row()
        .pt(12.0)
        .pb(4.0)
        .child(
            label(title.to_uppercase())
                .size(10.5)
                .weight(600)
                .color(theme().text_placeholder),
        )
        .into()
}

/// Which folds of the value's tree are open; reset when another cell is shown.
#[derive(Default)]
pub(crate) struct JsonFold {
    pub open: HashSet<String>,
    pub closed: HashSet<String>,
    pub more: HashSet<String>,
}

impl JsonFold {
    fn is_open(&self, path: &str, depth: usize) -> bool {
        if self.closed.contains(path) {
            return false;
        }
        depth == 0 || self.open.contains(path)
    }

    pub fn expand_all(&mut self, value: &Json) {
        self.closed.clear();
        fn walk(value: &Json, path: String, open: &mut HashSet<String>) {
            match value {
                Json::Object(entries) => {
                    open.insert(path.clone());
                    for (key, child) in entries {
                        walk(child, format!("{path}.{key}"), open);
                    }
                }
                Json::Array(items) => {
                    open.insert(path.clone());
                    for (index, child) in items.iter().enumerate() {
                        walk(child, format!("{path}.{index}"), open);
                    }
                }
                _ => {}
            }
        }
        walk(value, "$".into(), &mut self.open);
    }

    pub fn collapse(&mut self) {
        self.open.clear();
        self.more.clear();
        self.closed.insert("$".into());
    }
}

/// A click inside a value's tree, by the id its line was drawn with; `column` is the Row side's field the tree
/// belongs to (the Value side has one tree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FoldTarget {
    pub column: Option<usize>,
    pub action: FoldAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FoldAction {
    Toggle(String, bool),
    More(String),
}

struct JsonLine {
    depth: usize,
    /// The fold arrow: in the gutter for the root, right after the key for a nested object or array.
    toggle: Option<(String, bool)>,
    key: Option<(String, &'static str)>,
    parts: Vec<(String, &'static str)>,
    more: Option<(String, usize)>,
}

fn json_lines(value: &Json, fold: &JsonFold) -> Vec<JsonLine> {
    let mut lines = Vec::new();
    push_lines(value, "$".into(), 0, None, fold, &mut lines);
    lines
}

fn scalar(value: &Json) -> (String, &'static str) {
    match value {
        Json::Null => ("null".into(), "constant"),
        Json::Bool(value) => (value.to_string(), "boolean"),
        Json::Number(number) => (number.clone(), "number"),
        Json::String(text) => (json::quote(text), "string"),
        Json::Array(_) | Json::Object(_) => (String::new(), ""),
    }
}

fn push_lines(
    value: &Json,
    path: String,
    depth: usize,
    key: Option<(String, &'static str)>,
    fold: &JsonFold,
    lines: &mut Vec<JsonLine>,
) {
    let line = |toggle, parts| JsonLine {
        depth,
        toggle,
        key: key.clone(),
        parts,
        more: None,
    };
    let (children, open_mark, close_mark): (Vec<(Option<String>, &Json)>, &str, &str) = match value
    {
        Json::Object(entries) if !entries.is_empty() => (
            entries
                .iter()
                .map(|(key, child)| (Some(key.clone()), child))
                .collect(),
            "{",
            "}",
        ),
        Json::Array(items) if !items.is_empty() => {
            (items.iter().map(|child| (None, child)).collect(), "[", "]")
        }
        Json::Object(_) => {
            lines.push(line(None, vec![("{}".into(), "")]));
            return;
        }
        Json::Array(_) => {
            lines.push(line(None, vec![("[]".into(), "")]));
            return;
        }
        scalar_value => {
            lines.push(line(None, vec![scalar(scalar_value)]));
            return;
        }
    };
    if !fold.is_open(&path, depth) {
        let summary = json::summary(value).unwrap_or_default();
        lines.push(line(Some((path, false)), vec![(summary, "summary")]));
        return;
    }
    lines.push(line(
        Some((path.clone(), true)),
        vec![(open_mark.into(), "")],
    ));
    let shown = if fold.more.contains(&path) {
        children.len()
    } else {
        children.len().min(SHOWN_CHILDREN)
    };
    for (index, (key, child)) in children.iter().take(shown).enumerate() {
        let child_path = match key {
            Some(key) => format!("{path}.{key}"),
            None => format!("{path}.{index}"),
        };
        let label = key.as_ref().map(|key| (json::quote(key), "property"));
        push_lines(child, child_path, depth + 1, label, fold, lines);
    }
    if shown < children.len() {
        lines.push(JsonLine {
            depth: depth + 1,
            toggle: None,
            key: None,
            parts: Vec::new(),
            more: Some((path, children.len() - shown)),
        });
    }
    lines.push(JsonLine {
        depth,
        toggle: None,
        key: None,
        parts: vec![(close_mark.into(), "")],
        more: None,
    });
}

/// The value's tree, only the lines inside `window` (top, height in design px, relative to the tree's top)
/// built, with spacers keeping the full height; clicks land in `targets`.
pub(crate) fn json_tree(
    value: &Json,
    fold: &JsonFold,
    window: (f32, f32),
    column: Option<usize>,
    targets: &mut Vec<FoldTarget>,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let palette = workspace::syntax_theme();
    let syntax = |capture: &str| {
        let [r, g, b] = palette.syntax_color(capture).0;
        Rgba::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        )
    };
    let lines = json_lines(value, fold);
    let first = ((window.0 / JSON_LINE_H).floor().max(0.0) as usize).min(lines.len());
    let count = ((window.1 / JSON_LINE_H).ceil() as usize).saturating_add(2);
    let last = (first + count).min(lines.len());
    let arrow = |path: &str, open: bool, targets: &mut Vec<FoldTarget>| -> Node {
        let id = FOLD_BASE + targets.len() as u64;
        targets.push(FoldTarget {
            column,
            action: FoldAction::Toggle(path.to_string(), open),
        });
        div()
            .row()
            .w_px(12.0)
            .h_px(JSON_LINE_H)
            .items_center()
            .on_click(id)
            .child(
                icon(if open {
                    IconKind::ChevronDown
                } else {
                    IconKind::ChevronRight
                })
                .size(9.0)
                .color(if hovered == Some(id) {
                    colors.icon
                } else {
                    colors.icon_muted
                }),
            )
            .into()
    };
    let mut tree = div()
        .col()
        .py(6.0)
        .child(div().h_px(first as f32 * JSON_LINE_H));
    for line in &lines[first..last] {
        let mut row = div()
            .row()
            .h_px(JSON_LINE_H)
            .items_center()
            .pl(line.depth as f32 * 14.0);
        match (&line.toggle, line.depth) {
            (Some((path, open)), 0) => row = row.child(arrow(path, *open, targets)),
            _ => row = row.child(div().w_px(12.0)),
        }
        if let Some((key, capture)) = &line.key {
            row = row
                .child(label(key.clone()).size(TEXT).mono().color(syntax(capture)))
                .child(label(": ").size(TEXT).mono().color(colors.text));
        }
        if let (Some((path, open)), depth) = (&line.toggle, line.depth) {
            if depth > 0 {
                row = row.child(arrow(path, *open, targets));
            }
        }
        if let Some((path, hidden)) = &line.more {
            let id = FOLD_BASE + targets.len() as u64;
            targets.push(FoldTarget {
                column,
                action: FoldAction::More(path.clone()),
            });
            row = row.child(link(id, &format!("show {hidden} more"), hovered));
        }
        for (text, capture) in &line.parts {
            let color = match *capture {
                "" => colors.text,
                "summary" => colors.text_placeholder,
                capture => syntax(capture),
            };
            row = row.child(label(text.clone()).size(TEXT).mono().color(color));
        }
        tree = tree.child(row);
    }
    tree.child(div().h_px((lines.len() - last) as f32 * JSON_LINE_H))
        .into()
}

/// The tree inside its box, with a bar above it: `head` on the left, then the folds' and the value's actions.
pub(crate) fn json_box(
    tree: Node,
    head: String,
    actions: &[(u64, &str)],
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let mut bar = div()
        .row()
        .h_px(26.0)
        .px(8.0)
        .gap(4.0)
        .items_center()
        .child(small(head, colors.text_placeholder))
        .child(div().row().flex(1.0));
    for (id, text) in actions {
        bar = bar.child(link(*id, text, hovered));
    }
    div()
        .col()
        .rounded(6.0)
        .border(1.0, colors.border_variant)
        .bg(colors.editor_background)
        .child(bar)
        .child(div().h_px(1.0).bg(colors.border_variant))
        .child(div().col().pl(8.0).pr(6.0).child(tree))
        .into()
}

/// Where a foreign key's value leads, as the Value side shows it.
pub(crate) enum Target {
    None,
    Loading,
    Found {
        table: String,
        fields: Vec<(String, Option<String>)>,
    },
    Missing(String),
}

pub(crate) struct CellView<'a> {
    pub column: &'a str,
    pub data_type: &'a str,
    pub primary_key: bool,
    pub value: Option<&'a str>,
    pub edited: bool,
    /// The row's primary key value (else its number), as the side's corner names it.
    pub row_name: String,
    /// Why the value can't be changed here, when it can't.
    pub read_only: Option<&'static str>,
    pub references: Option<&'a (String, String)>,
    pub target: &'a Target,
}

pub(crate) fn value_title(cell: &CellView<'_>) -> Node {
    let colors = theme();
    let mut head = div()
        .row()
        .gap(6.0)
        .items_center()
        .child(
            label(cell.column.to_string())
                .size(13.0)
                .mono()
                .color(colors.text),
        )
        .child(
            label(cell.data_type.to_string())
                .size(11.0)
                .mono()
                .color(colors.text_placeholder),
        );
    if cell.primary_key {
        head = head.child(
            div()
                .row()
                .px(6.0)
                .h_px(16.0)
                .items_center()
                .rounded(8.0)
                .border(1.0, colors.border_variant)
                .child(label("primary key").size(10.5).color(colors.warning)),
        );
    }
    head.child(div().row().flex(1.0))
        .child(small(
            format!("row {}", cell.row_name),
            colors.text_placeholder,
        ))
        .into()
}

/// A text value in its box (a text box while editing).
pub(crate) fn value_box(cell: &CellView<'_>, editor: Option<Node>, width: f32) -> Node {
    let colors = theme();
    if let Some(editor) = editor {
        return editor;
    }
    let body: Node = match cell.value {
        None => label("NULL")
            .size(TEXT)
            .mono()
            .italic()
            .color(colors.text_placeholder)
            .into(),
        Some(text) => label(text.to_string())
            .size(TEXT)
            .mono()
            .color(if cell.edited {
                colors.warning
            } else {
                colors.text
            })
            .wrap(width - 22.0)
            .into(),
    };
    div()
        .col()
        .p(10.0)
        .rounded(6.0)
        .border(1.0, colors.border_variant)
        .bg(colors.editor_background)
        .on_click(VALUE_AREA)
        .child(body)
        .into()
}

/// Under the value: its size, and what can be done with it.
pub(crate) fn value_actions(
    cell: &CellView<'_>,
    editing: bool,
    json: bool,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let size = match cell.value {
        Some(text) if json => pom_db::object_storage::format_size(text.len() as u64),
        Some(text) => format!("{} chars", text.chars().count()),
        None => String::new(),
    };
    let mut actions = div()
        .row()
        .gap(2.0)
        .items_center()
        .child(small(size, colors.text_placeholder));
    if cell.edited {
        actions = actions.child(small("  edited", colors.warning));
    }
    actions = actions.child(div().row().flex(1.0));
    if editing {
        return actions
            .child(small("cmd-enter keeps it", colors.text_placeholder))
            .child(link(CANCEL_VALUE, "Cancel", hovered))
            .child(link(SAVE_VALUE, "Keep", hovered))
            .into();
    }
    match cell.read_only {
        Some(reason) => actions = actions.child(small(reason, colors.text_placeholder)),
        None => {
            if !cell.primary_key && cell.value.is_some() {
                actions = actions.child(link(SET_NULL, "Set NULL", hovered));
            }
            if cell.edited {
                actions = actions.child(link(REVERT, "Revert", hovered));
            }
        }
    }
    if cell.value.is_some() && !json {
        actions = actions.child(link(OPEN_IN_TAB, "Open in tab", hovered));
    }
    actions.child(link(COPY_VALUE, "Copy", hovered)).into()
}

/// The row a foreign key points at: a few of its fields and a way to open it.
pub(crate) fn value_target(cell: &CellView<'_>, hovered: Option<u64>) -> Option<Node> {
    let colors = theme();
    let (table, target) = cell.references?;
    let value = cell.value?;
    let heading = section(&format!("{table} where {target} = {value}"));
    Some(match cell.target {
        Target::None => return None,
        Target::Loading => div()
            .col()
            .child(heading)
            .child(small("Loading...", colors.text_muted))
            .into(),
        Target::Missing(reason) => div()
            .col()
            .child(heading)
            .child(small(reason.clone(), colors.text_muted))
            .into(),
        Target::Found { table, fields } => {
            let mut record = div().col().child(heading);
            for (name, value) in fields.iter().take(4) {
                record = record.child(field_line(
                    name,
                    value.as_deref(),
                    None,
                    false,
                    None,
                    false,
                    hovered,
                ));
            }
            let mut open = div()
                .row()
                .h_px(26.0)
                .px(4.0)
                .items_center()
                .rounded(4.0)
                .on_click(OPEN_TARGET)
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(format!("Open {table} #{value}"))
                            .size(12.0)
                            .color(colors.hint),
                    ),
                )
                .child(
                    icon(IconKind::ArrowUpRight)
                        .size(10.0)
                        .color(colors.icon_muted),
                );
            if hovered == Some(OPEN_TARGET) {
                open = open.bg(colors.ghost_element_hover);
            }
            record.child(open).into()
        }
    })
}

fn field_line(
    name: &str,
    value: Option<&str>,
    id: Option<u64>,
    focused: bool,
    link_id: Option<u64>,
    edited: bool,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let shown: Node = match value {
        None => label("NULL")
            .size(TEXT)
            .mono()
            .italic()
            .color(colors.text_placeholder)
            .into(),
        Some(text) => {
            let color = if link_id.is_some() {
                colors.hint
            } else if edited {
                colors.warning
            } else {
                colors.text
            };
            let mut line = div().row().flex(1.0).gap(4.0).items_center().child(
                label(text.lines().next().unwrap_or_default().to_string())
                    .size(TEXT)
                    .mono()
                    .color(color)
                    .truncate(),
            );
            if let Some(link_id) = link_id {
                line = line
                    .on_click(link_id)
                    .child(icon(IconKind::ArrowUpRight).size(10.0).color(colors.hint));
            }
            line.into()
        }
    };
    let mut row = div()
        .row()
        .h_px(26.0)
        .pr(4.0)
        .gap(8.0)
        .items_center()
        .rounded(4.0)
        .child(div().w_px(2.0).h_px(26.0).bg(if focused {
            colors.text_accent
        } else {
            Rgba::TRANSPARENT
        }))
        .child(
            div().row().w_px(110.0).items_center().child(
                label(name.to_string())
                    .size(TEXT)
                    .mono()
                    .color(colors.text_muted)
                    .truncate(),
            ),
        )
        .child(div().row().flex(1.0).items_center().child(shown));
    if let Some(id) = id {
        row = row.on_click(id);
        if focused {
            row = row.bg(colors.element_selected);
        } else if edited {
            row = row.bg(colors.warning.alpha(0.12));
        } else if hovered == Some(id) {
            row = row.bg(colors.ghost_element_hover);
        }
    }
    row.into()
}

pub(crate) struct RowField {
    pub name: String,
    pub value: Option<String>,
    pub edited: bool,
    /// `2 - Globex (orgs)` for a foreign key whose row was found.
    pub link: Option<String>,
    /// A JSON value's tree in its box, drawn under the field's name.
    pub json: Option<Node>,
}

/// The selected row as a record: every field, the chosen column's lit; then the tables whose foreign keys
/// point at it with how many of their rows do.
pub(crate) fn row_view(
    fields: Vec<RowField>,
    focused: Option<usize>,
    references: &[(String, Option<u64>)],
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let mut column = div().col().gap(2.0);
    for (index, field) in fields.into_iter().enumerate() {
        let id = FIELD_BASE + index as u64;
        if let Some(tree) = field.json {
            let mut name = div()
                .row()
                .h_px(26.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(div().w_px(2.0).h_px(26.0).bg(if focused == Some(index) {
                    colors.text_accent
                } else {
                    Rgba::TRANSPARENT
                }))
                .child(
                    div().row().pl(8.0).items_center().child(
                        label(field.name.clone())
                            .size(TEXT)
                            .mono()
                            .color(colors.text_muted),
                    ),
                );
            if focused == Some(index) {
                name = name.bg(colors.element_selected);
            }
            column = column
                .child(name)
                .child(div().col().pl(10.0).pb(4.0).child(tree));
            continue;
        }
        let linked = field.link.is_some();
        column = column.child(field_line(
            &field.name,
            field.link.as_deref().or(field.value.as_deref()),
            Some(id),
            focused == Some(index),
            linked.then_some(FIELD_LINK_BASE + index as u64),
            field.edited,
            hovered,
        ));
    }
    if !references.is_empty() {
        column = column.child(section("Referenced by"));
        for (index, (name, count)) in references.iter().enumerate() {
            let id = REFERENCE_BASE + index as u64;
            let counted = match count {
                Some(1) => "1 row".to_string(),
                Some(count) => format!("{count} rows"),
                None => "...".to_string(),
            };
            let mut line = div()
                .row()
                .h_px(28.0)
                .px(6.0)
                .gap(8.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(name.clone())
                            .size(TEXT)
                            .mono()
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(
                    div()
                        .row()
                        .px(7.0)
                        .h_px(18.0)
                        .items_center()
                        .rounded(9.0)
                        .border(1.0, colors.border_variant)
                        .child(label(counted).size(10.5).color(colors.text_muted)),
                )
                .child(
                    icon(IconKind::ArrowUpRight)
                        .size(10.0)
                        .color(colors.icon_muted),
                );
            if hovered == Some(id) {
                line = line.bg(colors.ghost_element_hover);
            }
            column = column.child(line);
        }
    }
    column
        .child(div().row().pt(10.0).child(small(
            "Double-click a value to edit it.",
            colors.text_placeholder,
        )))
        .into()
}

fn table_head(columns: &[(&str, f32)]) -> Node {
    let colors = theme();
    let mut row = div().row().h_px(26.0).items_center().px(8.0).gap(8.0);
    for (name, width) in columns {
        let cell = div().row().items_center().child(
            label(name.to_uppercase())
                .size(10.5)
                .weight(600)
                .color(colors.text_placeholder),
        );
        row = row.child(if *width > 0.0 {
            cell.w_px(*width)
        } else {
            cell.flex(1.0)
        });
    }
    div()
        .col()
        .child(row)
        .child(div().h_px(1.0).bg(colors.border_variant))
        .into()
}

fn table_line(cells: Vec<(Node, f32)>, id: Option<u64>, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut row = div().row().h_px(26.0).items_center().px(8.0).gap(8.0);
    for (cell, width) in cells {
        let slot = div().row().items_center().child(cell);
        row = row.child(if width > 0.0 {
            slot.w_px(width)
        } else {
            slot.flex(1.0)
        });
    }
    if let Some(id) = id {
        row = row.on_click(id);
        if hovered == Some(id) {
            row = row.bg(colors.ghost_element_hover);
        }
    }
    div()
        .col()
        .child(row)
        .child(div().h_px(1.0).bg(colors.border_variant))
        .into()
}

fn mono(text: impl Into<String>, color: Rgba) -> Node {
    label(text.into())
        .size(TEXT)
        .mono()
        .color(color)
        .truncate()
        .into()
}

/// Columns (type, NULL, default, key), indexes and the foreign keys that point at the table.
pub(crate) fn structure_view(structure: &TableStructure, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut view = div().col().p(12.0).gap(4.0).child(table_head(&[
        ("Column", 180.0),
        ("Type", 160.0),
        ("Null", 50.0),
        ("Default", 0.0),
        ("Key", 200.0),
    ]));
    for (index, column) in structure.columns.iter().enumerate() {
        let key: Node = if column.primary_key {
            small("primary", colors.warning).into()
        } else if let Some((table, target)) = &column.references {
            div()
                .row()
                .gap(4.0)
                .items_center()
                .child(
                    icon(IconKind::ArrowUpRight)
                        .size(10.0)
                        .color(colors.text_accent),
                )
                .child(small(format!("{table}.{target}"), colors.text_accent))
                .into()
        } else {
            div().into()
        };
        let id = column
            .references
            .as_ref()
            .map(|_| STRUCTURE_LINK_BASE + index as u64);
        view = view.child(table_line(
            vec![
                (mono(column.name.clone(), colors.text), 180.0),
                (mono(column.data_type.clone(), colors.text_muted), 160.0),
                (
                    mono(
                        if column.nullable { "yes" } else { "no" },
                        colors.text_muted,
                    ),
                    50.0,
                ),
                (
                    mono(
                        column.default.clone().unwrap_or_default(),
                        colors.text_placeholder,
                    ),
                    0.0,
                ),
                (key, 200.0),
            ],
            id,
            hovered,
        ));
    }
    view = view.child(div().h_px(12.0)).child(table_head(&[
        ("Index", 260.0),
        ("Columns", 0.0),
        ("Kind", 120.0),
    ]));
    for index in &structure.indexes {
        let kind = if index.primary {
            "primary"
        } else if index.unique {
            "unique"
        } else {
            ""
        };
        view = view.child(table_line(
            vec![
                (mono(index.name.clone(), colors.text), 260.0),
                (mono(index.columns.clone(), colors.text_muted), 0.0),
                (mono(kind, colors.text_muted), 120.0),
            ],
            None,
            hovered,
        ));
    }
    if !structure.referenced_by.is_empty() {
        view = view.child(div().h_px(12.0)).child(table_head(&[
            ("Referenced by", 260.0),
            ("Column", 0.0),
            ("On delete", 120.0),
        ]));
        for (index, reference) in structure.referenced_by.iter().enumerate() {
            view = view.child(table_line(
                vec![
                    (mono(reference.table.clone(), colors.text_accent), 260.0),
                    (mono(reference.column.clone(), colors.text_muted), 0.0),
                    (mono(reference.on_delete.clone(), colors.text_muted), 120.0),
                ],
                Some(STRUCTURE_REFERENCE_BASE + index as u64),
                hovered,
            ));
        }
    }
    view.into()
}

const SQL_KEYWORDS: [&str; 44] = [
    "create",
    "table",
    "view",
    "index",
    "unique",
    "on",
    "using",
    "not",
    "null",
    "default",
    "constraint",
    "primary",
    "key",
    "foreign",
    "references",
    "check",
    "as",
    "select",
    "from",
    "where",
    "and",
    "or",
    "in",
    "is",
    "cascade",
    "restrict",
    "set",
    "delete",
    "update",
    "no",
    "action",
    "with",
    "without",
    "time",
    "zone",
    "varying",
    "if",
    "exists",
    "alter",
    "add",
    "column",
    "generated",
    "always",
    "identity",
];

/// A line of SQL as the editor colors it: keywords, strings, numbers, quoted names.
fn sql_line(line: &str, syntax: &dyn Fn(&str) -> Rgba, plain: Rgba) -> Node {
    let mut row = div().row().h_px(18.0).items_center();
    let bytes = line.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let capture = match bytes[at] {
            b'\'' | b'"' => {
                let quote = bytes[at];
                at += 1;
                while at < bytes.len() && bytes[at] != quote {
                    at += 1;
                }
                at = (at + 1).min(bytes.len());
                if quote == b'\'' {
                    "string"
                } else {
                    "property"
                }
            }
            byte if byte.is_ascii_digit() => {
                while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                    at += 1;
                }
                "number"
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                    at += 1;
                }
                if SQL_KEYWORDS.contains(&line[start..at].to_ascii_lowercase().as_str()) {
                    "keyword"
                } else {
                    ""
                }
            }
            _ => {
                at += 1;
                while at < bytes.len()
                    && !bytes[at].is_ascii_alphanumeric()
                    && !matches!(bytes[at], b'\'' | b'"' | b'_')
                {
                    at += 1;
                }
                ""
            }
        };
        let color = if capture.is_empty() {
            plain
        } else {
            syntax(capture)
        };
        row = row.child(
            label(line[start..at].to_string())
                .size(TEXT)
                .mono()
                .color(color),
        );
    }
    row.into()
}

pub(crate) fn ddl_view(ddl: &str, hovered: Option<u64>) -> Node {
    let colors = theme();
    let palette = workspace::syntax_theme();
    let syntax = |capture: &str| {
        let [r, g, b] = palette.syntax_color(capture).0;
        Rgba::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        )
    };
    let mut code = div()
        .col()
        .p(10.0)
        .rounded(6.0)
        .border(1.0, colors.border_variant)
        .bg(colors.panel_background);
    for line in ddl.lines() {
        code = code.child(sql_line(line, &syntax, colors.text));
    }
    div()
        .col()
        .p(12.0)
        .gap(8.0)
        .child(code)
        .child(
            div()
                .row()
                .gap(4.0)
                .items_center()
                .child(link(COPY_DDL, "Copy", hovered)),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_opens_its_first_level_folds_the_rest_and_caps_long_lists() {
        let items: Vec<String> = (0..60).map(|index| index.to_string()).collect();
        let text = format!(
            r#"{{"theme":"dark","nested":{{"a":1}},"list":[{}]}}"#,
            items.join(",")
        );
        let value = json::parse(&text).expect("json");
        let mut fold = JsonFold::default();
        let lines = json_lines(&value, &fold);
        assert_eq!(
            lines.len(),
            5,
            "open root, three children folded, closing brace"
        );
        assert!(lines[2].parts.iter().any(|(text, _)| text == "{ 1 key }"));
        assert!(
            lines[1].parts.iter().all(|(text, _)| !text.ends_with(',')),
            "no commas"
        );
        fold.open.insert("$.list".into());
        let lines = json_lines(&value, &fold);
        assert!(lines
            .iter()
            .any(|line| line.more == Some(("$.list".into(), 10))));
        fold.expand_all(&value);
        fold.more.insert("$.list".into());
        assert_eq!(json_lines(&value, &fold).len(), 1 + 1 + 3 + 1 + 60 + 1 + 1);
        fold.collapse();
        assert_eq!(json_lines(&value, &fold).len(), 1);
    }
}
