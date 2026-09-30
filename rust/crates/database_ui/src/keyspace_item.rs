//! A Redis keyspace tab: the keys a pattern matches as a tree by prefix, each with its type and TTL, and the
//! selected key's value the way its type holds it (JSON as a tree, a hash as fields, a list with its indexes, a
//! sorted set with scores, a set's members). The TTL can be changed, and a key or every match deleted.

use std::collections::HashSet;

use pom_db::{Database, RedisKey, RedisValue, Table, VALUE_ITEMS};
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

use crate::details::{self, FoldAction, FoldTarget, JsonFold, Pane, FOLD_BASE, FOLD_END};
use crate::json::{self, Json};
use crate::{DatabaseContext, Pending};

const TOOLBAR_H: f32 = 36.0;
const STATUS_H: f32 = 28.0;
const FIELD_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const TREE_W: f32 = 300.0;
const KEYS_MAX: usize = 5000;
const TEXT: f32 = 12.0;

const PATTERN_FIELD: u64 = 1;
const REFRESH: u64 = 2;
const TTL_FIELD: u64 = 3;
const COPY_VALUE: u64 = 4;
const DELETE_KEY: u64 = 5;
const CONFIRM_DELETE: u64 = 6;
const CANCEL_DELETE: u64 = 7;
const DELETE_MATCHING: u64 = 8;
const CONFIRM_MATCHING: u64 = 9;
const CANCEL_MATCHING: u64 = 10;
const EXPAND_ALL: u64 = 11;
const COLLAPSE: u64 = 12;
const GROUP_BASE: u64 = 100;
const KEY_BASE: u64 = 10_000;
const KEY_END: u64 = 20_000;

const KIND: &str = "db-keyspace";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Pattern,
    Ttl,
}

enum Line {
    Group {
        name: String,
        count: usize,
        open: bool,
    },
    Key(usize),
}

pub struct KeyspaceItem {
    context: DatabaseContext,
    database: Database,
    table: Table,
    pattern: TextField,
    ttl: TextField,
    focus: Focus,
    focused: bool,
    keys: Vec<RedisKey>,
    truncated: bool,
    loading: Pending<(Vec<RedisKey>, bool)>,
    folded: HashSet<String>,
    selected: Option<String>,
    value: Option<(String, RedisValue)>,
    parsed: Option<Json>,
    loading_value: Pending<(String, RedisValue)>,
    fold: JsonFold,
    fold_targets: Vec<FoldTarget>,
    acting: Pending<String>,
    confirm_delete: bool,
    confirm_matching: bool,
    status: Option<(String, bool)>,
    tree: Pane,
    detail: Pane,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    clipboard: Option<String>,
}

fn field(text: &str) -> TextField {
    let mut field = TextField::default();
    field.set_font_size(TEXT);
    field.set_text(text);
    field.move_to_end();
    field
}

/// `3723` -> `1h 2m`, what a key's row shows of its TTL.
fn short_ttl(seconds: i64) -> String {
    match seconds {
        seconds if seconds >= 86_400 => format!("{}d", seconds / 86_400),
        seconds if seconds >= 3600 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        seconds if seconds >= 60 => format!("{}m", seconds / 60),
        seconds => format!("{seconds}s"),
    }
}

fn kind_color(kind: &str) -> Rgba {
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
    match kind {
        "string" => syntax("string"),
        "hash" => colors.hint,
        "list" => syntax("number"),
        "set" => colors.text_accent,
        "zset" => syntax("constant"),
        _ => colors.text_muted,
    }
}

fn badge(kind: &str) -> Node {
    let colors = theme();
    div()
        .row()
        .px(5.0)
        .h_px(16.0)
        .items_center()
        .rounded(3.0)
        .border(1.0, colors.border_variant)
        .child(
            label(kind.to_string())
                .size(10.0)
                .mono()
                .color(kind_color(kind)),
        )
        .into()
}

impl KeyspaceItem {
    pub fn item_id(database: &Database, table: &Table) -> String {
        format!("db-keyspace:{}:{}", database.name, table.name)
    }

    pub fn new(context: DatabaseContext, database: Database, table: Table) -> KeyspaceItem {
        let pattern = format!("{}:*", table.name);
        KeyspaceItem::matching(context, database, table, &pattern)
    }

    fn matching(
        context: DatabaseContext,
        database: Database,
        table: Table,
        pattern: &str,
    ) -> KeyspaceItem {
        let mut item = KeyspaceItem {
            context,
            database,
            table,
            pattern: field(pattern),
            ttl: field(""),
            focus: Focus::List,
            focused: false,
            keys: Vec::new(),
            truncated: false,
            loading: Pending::idle(),
            folded: HashSet::new(),
            selected: None,
            value: None,
            parsed: None,
            loading_value: Pending::idle(),
            fold: JsonFold::default(),
            fold_targets: Vec::new(),
            acting: Pending::idle(),
            confirm_delete: false,
            confirm_matching: false,
            status: None,
            tree: Pane::default(),
            detail: Pane::default(),
            hits: Vec::new(),
            hovered: None,
            clipboard: None,
        };
        item.load();
        item
    }

    fn load(&mut self) {
        let (database, pattern) = (self.database.clone(), self.pattern.text());
        self.loading = self
            .context
            .run(move |connector| connector.redis_keys(&database, &pattern, KEYS_MAX));
    }

    fn load_value(&mut self) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let database = self.database.clone();
        self.loading_value = self.context.run(move |connector| {
            connector
                .redis_value(&database, &key)
                .map(|value| (key, value))
        });
    }

    fn select(&mut self, index: usize) {
        let Some(key) = self.keys.get(index) else {
            return;
        };
        if self.selected.as_deref() == Some(key.key.as_str()) {
            return;
        }
        self.selected = Some(key.key.clone());
        self.ttl = field(&key.ttl.map(|ttl| ttl.to_string()).unwrap_or_default());
        self.confirm_delete = false;
        self.fold = JsonFold::default();
        self.detail.scroll = 0.0;
        self.load_value();
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.keys.iter().position(|key| key.key == selected)
    }

    /// The literal start of the pattern (`bull:` of `bull:*`), which every listed key shares.
    fn stem(&self) -> String {
        let pattern = self.pattern.text();
        let end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
        pattern[..end].to_string()
    }

    /// The group a key falls in: its next part after the stem, up to and with its `:`.
    fn group_of(&self, key: &str) -> String {
        let stem = self.stem();
        let rest = key.strip_prefix(stem.as_str()).unwrap_or(key);
        match rest.find(':') {
            Some(at) => format!("{stem}{}", &rest[..=at]),
            None => stem,
        }
    }

    fn lines(&self) -> Vec<Line> {
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (index, key) in self.keys.iter().enumerate() {
            let group = self.group_of(&key.key);
            match groups.iter_mut().find(|(name, _)| *name == group) {
                Some((_, members)) => members.push(index),
                None => groups.push((group, vec![index])),
            }
        }
        if groups.len() <= 1 {
            return (0..self.keys.len()).map(Line::Key).collect();
        }
        let mut lines = Vec::new();
        for (name, members) in groups {
            let open = !self.folded.contains(&name);
            lines.push(Line::Group {
                name,
                count: members.len(),
                open,
            });
            if open {
                lines.extend(members.into_iter().map(Line::Key));
            }
        }
        lines
    }

    fn group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for key in &self.keys {
            let group = self.group_of(&key.key);
            if !names.contains(&group) {
                names.push(group);
            }
        }
        names
    }

    fn apply_ttl(&mut self) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let typed = self.ttl.text();
        let seconds = match typed.trim() {
            "" | "none" | "no expiry" => None,
            text => match text.parse::<i64>() {
                Ok(seconds) if seconds > 0 => Some(seconds),
                _ => {
                    self.status = Some((
                        "TTL is a number of seconds, or empty to keep it".into(),
                        true,
                    ));
                    return;
                }
            },
        };
        let database = self.database.clone();
        self.acting = self.context.run(move |connector| {
            connector.redis_expire(&database, &key, seconds)?;
            Ok(match seconds {
                Some(seconds) => format!("{key} expires in {}", short_ttl(seconds)),
                None => format!("{key} no longer expires"),
            })
        });
        self.focus = Focus::List;
    }

    fn click(&mut self, id: u64) {
        if id != DELETE_KEY && id != CONFIRM_DELETE {
            self.confirm_delete = false;
        }
        if id != DELETE_MATCHING && id != CONFIRM_MATCHING {
            self.confirm_matching = false;
        }
        match id {
            PATTERN_FIELD => self.focus = Focus::Pattern,
            TTL_FIELD => self.focus = Focus::Ttl,
            REFRESH => {
                self.load();
                self.load_value();
            }
            COPY_VALUE => {
                self.clipboard = self.value.as_ref().map(|(_, value)| match value {
                    RedisValue::Text(text) | RedisValue::Other(text) => text.clone(),
                    RedisValue::List(items) | RedisValue::Set(items) => items.join("\n"),
                    RedisValue::Hash(pairs) | RedisValue::Sorted(pairs) => pairs
                        .iter()
                        .map(|(field, value)| format!("{field}\t{value}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    RedisValue::Missing => String::new(),
                });
            }
            DELETE_KEY => self.confirm_delete = true,
            CANCEL_DELETE => self.confirm_delete = false,
            CONFIRM_DELETE => {
                self.confirm_delete = false;
                if let Some(key) = self.selected.clone() {
                    let database = self.database.clone();
                    self.acting = self.context.run(move |connector| {
                        connector.redis_delete(&database, &key)?;
                        Ok(format!("Deleted {key}"))
                    });
                }
            }
            DELETE_MATCHING => self.confirm_matching = true,
            CANCEL_MATCHING => self.confirm_matching = false,
            CONFIRM_MATCHING => {
                self.confirm_matching = false;
                let (database, pattern) = (self.database.clone(), self.pattern.text());
                self.acting = self.context.run(move |connector| {
                    let deleted = connector.delete_keys(&database, &pattern)?;
                    Ok(format!("Deleted {deleted} keys matching {pattern}"))
                });
            }
            EXPAND_ALL => {
                if let Some(parsed) = &self.parsed {
                    self.fold.expand_all(parsed);
                }
            }
            COLLAPSE => self.fold.collapse(),
            id if (FOLD_BASE..FOLD_END).contains(&id) => {
                if let Some(target) = self.fold_targets.get((id - FOLD_BASE) as usize).cloned() {
                    match target.action {
                        FoldAction::Toggle(path, true) => {
                            self.fold.open.remove(&path);
                            self.fold.closed.insert(path);
                        }
                        FoldAction::Toggle(path, false) => {
                            self.fold.closed.remove(&path);
                            self.fold.open.insert(path);
                        }
                        FoldAction::More(path) => {
                            self.fold.more.insert(path);
                        }
                    }
                }
            }
            id if (KEY_BASE..KEY_END).contains(&id) => {
                self.focus = Focus::List;
                self.select((id - KEY_BASE) as usize);
            }
            id if (GROUP_BASE..KEY_BASE).contains(&id) => {
                if let Some(name) = self.group_names().get((id - GROUP_BASE) as usize).cloned() {
                    if !self.folded.remove(&name) {
                        self.folded.insert(name);
                    }
                }
            }
            _ => {}
        }
    }

    fn text_button(&self, id: u64, text: &str, danger: bool) -> Node {
        let colors = theme();
        let mut button = div()
            .row()
            .h_px(22.0)
            .px(7.0)
            .rounded(4.0)
            .items_center()
            .border(
                1.0,
                if danger {
                    colors.error.alpha(0.4)
                } else {
                    colors.border_variant
                },
            )
            .on_click(id)
            .child(label(text.to_string()).size(12.0).color(if danger {
                colors.error
            } else {
                colors.text
            }));
        if self.hovered == Some(id) {
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

    fn toolbar(&self, width: f32) -> Node {
        let colors = theme();
        let count = if self.truncated {
            format!("{}+ keys", self.keys.len())
        } else if self.keys.len() == 1 {
            "1 key".into()
        } else {
            format!("{} keys", self.keys.len())
        };
        let mut crumb = div().row().flex(1.0).gap(6.0).items_center().child(
            icon(IconKind::EngineRedis)
                .size(12.0)
                .color(colors.icon_muted),
        );
        let wide = width > 700.0;
        if wide && !self.database.repo.is_empty() {
            crumb = crumb
                .child(
                    label(self.database.repo.clone())
                        .size(12.5)
                        .color(colors.text_muted),
                )
                .child(label("/").size(12.5).color(colors.text_placeholder));
        }
        crumb = crumb
            .child(
                label(self.database.label.clone())
                    .size(12.5)
                    .color(colors.text_muted)
                    .truncate(),
            )
            .child(label("/").size(12.5).color(colors.text_placeholder))
            .child(
                label(format!("{}:*", self.table.name))
                    .size(12.5)
                    .medium()
                    .color(colors.text)
                    .truncate(),
            );
        if wide && !self.context.branch.is_empty() {
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
        let pattern_w = (width * 0.35).clamp(140.0, 280.0);
        div()
            .row()
            .h_px(TOOLBAR_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .child(crumb)
            .child(div().row().w_px(pattern_w).items_center().child(self.input(
                PATTERN_FIELD,
                "MATCH",
                &self.pattern,
                "session:*",
                self.focus == Focus::Pattern,
            )))
            .child(label(count).size(11.5).color(colors.text_placeholder))
            .child(
                div()
                    .row()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(REFRESH)
                    .bg(if self.hovered == Some(REFRESH) {
                        colors.ghost_element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(icon(IconKind::RotateCw).size(12.0).color(colors.icon_muted)),
            )
            .into()
    }

    fn tree_view(&self) -> Node {
        let colors = theme();
        let stem = self.stem();
        let names = self.group_names();
        let mut tree = div().col().py(4.0);
        if self.keys.is_empty() {
            let text = if self.loading.busy() {
                "Loading..."
            } else {
                "No key matches"
            };
            return tree
                .child(
                    div()
                        .row()
                        .px(12.0)
                        .h_px(ROW_H)
                        .items_center()
                        .child(label(text).size(12.0).color(colors.text_muted)),
                )
                .into();
        }
        for line in self.lines() {
            match line {
                Line::Group { name, count, open } => {
                    let index = names.iter().position(|group| *group == name).unwrap_or(0);
                    let id = GROUP_BASE + index as u64;
                    let mut row = div()
                        .row()
                        .h_px(ROW_H)
                        .px(8.0)
                        .gap(6.0)
                        .items_center()
                        .on_click(id)
                        .child(
                            icon(if open {
                                IconKind::ChevronDown
                            } else {
                                IconKind::ChevronRight
                            })
                            .size(10.0)
                            .color(colors.icon_muted),
                        )
                        .child(
                            div().row().flex(1.0).items_center().child(
                                label(name.clone())
                                    .size(TEXT)
                                    .mono()
                                    .color(colors.hint)
                                    .truncate(),
                            ),
                        )
                        .child(
                            label(count.to_string())
                                .size(11.0)
                                .color(colors.text_placeholder),
                        );
                    if self.hovered == Some(id) {
                        row = row.bg(colors.ghost_element_hover);
                    }
                    tree = tree.child(row);
                }
                Line::Key(index) => {
                    let key = &self.keys[index];
                    let id = KEY_BASE + index as u64;
                    let group = self.group_of(&key.key);
                    let grouped = names.len() > 1;
                    let shown = if grouped {
                        key.key.strip_prefix(group.as_str()).unwrap_or(&key.key)
                    } else {
                        key.key.strip_prefix(stem.as_str()).unwrap_or(&key.key)
                    };
                    let shown = if shown.is_empty() {
                        key.key.as_str()
                    } else {
                        shown
                    };
                    let chosen = self.selected.as_deref() == Some(key.key.as_str());
                    let mut row = div()
                        .row()
                        .h_px(ROW_H)
                        .pl(if grouped { 24.0 } else { 10.0 })
                        .pr(10.0)
                        .gap(8.0)
                        .items_center()
                        .on_click(id)
                        .child(badge(&key.kind))
                        .child(
                            div().row().flex(1.0).items_center().child(
                                label(shown.to_string())
                                    .size(TEXT)
                                    .mono()
                                    .color(colors.text)
                                    .truncate(),
                            ),
                        );
                    if let Some(ttl) = key.ttl {
                        row = row.child(
                            label(short_ttl(ttl))
                                .size(11.0)
                                .color(colors.text_placeholder),
                        );
                    }
                    if chosen {
                        row = row.bg(colors.element_selected);
                    } else if self.hovered == Some(id) {
                        row = row.bg(colors.ghost_element_hover);
                    }
                    tree = tree.child(row);
                }
            }
        }
        if self.truncated {
            tree = tree.child(
                div().row().px(12.0).h_px(ROW_H).items_center().child(
                    label(format!(
                        "The first {KEYS_MAX}; narrow the pattern for the rest"
                    ))
                    .size(11.5)
                    .color(colors.text_placeholder),
                ),
            );
        }
        tree.into()
    }

    fn table_of(&self, head: [&str; 2], rows: Vec<(String, String)>, numbered: bool) -> Node {
        let colors = theme();
        let head_row = |cells: [&str; 2]| {
            div()
                .row()
                .h_px(26.0)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .child(
                    div().row().w_px(if numbered { 50.0 } else { 160.0 }).child(
                        label(cells[0].to_uppercase())
                            .size(10.5)
                            .weight(600)
                            .color(colors.text_placeholder),
                    ),
                )
                .child(
                    div().row().flex(1.0).child(
                        label(cells[1].to_uppercase())
                            .size(10.5)
                            .weight(600)
                            .color(colors.text_placeholder),
                    ),
                )
        };
        let mut table = div()
            .col()
            .rounded(6.0)
            .border(1.0, colors.border_variant)
            .child(head_row(head))
            .child(div().h_px(1.0).bg(colors.border_variant));
        for (first, second) in rows {
            table = table.child(
                div()
                    .row()
                    .h_px(26.0)
                    .px(8.0)
                    .gap(8.0)
                    .items_center()
                    .child(
                        div()
                            .row()
                            .w_px(if numbered { 50.0 } else { 160.0 })
                            .items_center()
                            .child(
                                label(first)
                                    .size(TEXT)
                                    .mono()
                                    .color(if numbered {
                                        colors.text_placeholder
                                    } else {
                                        colors.hint
                                    })
                                    .truncate(),
                            ),
                    )
                    .child(
                        div().row().flex(1.0).items_center().child(
                            label(second)
                                .size(TEXT)
                                .mono()
                                .color(colors.text)
                                .truncate(),
                        ),
                    ),
            );
            table = table.child(div().h_px(1.0).bg(colors.border_variant));
        }
        table.into()
    }

    fn detail_view(&mut self, width: f32, view_h: f32) -> Node {
        let colors = theme();
        self.fold_targets.clear();
        let Some(index) = self.selected_index() else {
            return div()
                .col()
                .p(14.0)
                .child(
                    label("Select a key to see its value")
                        .size(12.0)
                        .color(colors.text_muted),
                )
                .into();
        };
        let key = self.keys[index].clone();
        let mut head = div()
            .row()
            .gap(8.0)
            .items_center()
            .child(badge(&key.kind))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(key.key.clone())
                        .size(13.0)
                        .mono()
                        .color(colors.text)
                        .truncate(),
                ),
            )
            .child(div().row().w_px(150.0).items_center().child(self.input(
                TTL_FIELD,
                "TTL",
                &self.ttl,
                "no expiry",
                self.focus == Focus::Ttl,
            )));
        head = head.child(self.text_button(COPY_VALUE, "Copy value", false));
        head = if self.confirm_delete {
            head.child(self.text_button(CONFIRM_DELETE, "Delete it", true))
                .child(self.text_button(CANCEL_DELETE, "Keep", false))
        } else {
            head.child(self.text_button(DELETE_KEY, "Delete key", true))
        };
        let mut content = div().col().p(14.0).gap(10.0).child(head);
        let value = match &self.value {
            Some((shown, value)) if *shown == key.key => value.clone(),
            _ => {
                return content
                    .child(label("Loading...").size(12.0).color(colors.text_muted))
                    .into();
            }
        };
        let capped = |count: usize| {
            (count >= VALUE_ITEMS).then(|| {
                details::small(
                    format!("The first {VALUE_ITEMS}; redis-cli reads the rest"),
                    colors.text_placeholder,
                )
            })
        };
        match value {
            RedisValue::Text(text) => match &self.parsed {
                Some(parsed) => {
                    let head_h = 14.0 + 26.0 + 10.0 + 27.0;
                    let (scroll, _) = self.detail.window();
                    let tree = details::json_tree(
                        parsed,
                        &self.fold,
                        (scroll - head_h, view_h),
                        None,
                        &mut self.fold_targets,
                        self.hovered,
                    );
                    content = content
                        .child(details::json_box(
                            tree,
                            json::summary(parsed).unwrap_or_default(),
                            &[(EXPAND_ALL, "Expand all"), (COLLAPSE, "Collapse")],
                            self.hovered,
                        ))
                        .child(details::small(
                            format!(
                                "JSON detected - {}",
                                pom_db::object_storage::format_size(text.len() as u64)
                            ),
                            colors.text_placeholder,
                        ));
                }
                None => {
                    content = content
                        .child(
                            div()
                                .col()
                                .p(10.0)
                                .rounded(6.0)
                                .border(1.0, colors.border_variant)
                                .bg(colors.editor_background)
                                .child(
                                    label(text.clone())
                                        .size(TEXT)
                                        .mono()
                                        .color(colors.text)
                                        .wrap(width - 50.0),
                                ),
                        )
                        .child(details::small(
                            format!("{} chars", text.chars().count()),
                            colors.text_placeholder,
                        ));
                }
            },
            RedisValue::Hash(pairs) => {
                let count = pairs.len();
                content = content.child(self.table_of(["Field", "Value"], pairs, false));
                if let Some(note) = capped(count) {
                    content = content.child(note);
                }
            }
            RedisValue::Sorted(pairs) => {
                let count = pairs.len();
                content = content.child(self.table_of(["Member", "Score"], pairs, false));
                if let Some(note) = capped(count) {
                    content = content.child(note);
                }
            }
            RedisValue::List(items) => {
                let count = items.len();
                let rows = items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| (index.to_string(), item))
                    .collect();
                content = content.child(self.table_of(["#", "Value"], rows, true));
                if let Some(note) = capped(count) {
                    content = content.child(note);
                }
            }
            RedisValue::Set(members) => {
                let count = members.len();
                let mut lines = div().col().gap(6.0);
                let mut line = div().row().gap(6.0);
                let mut used = 0.0;
                for member in members {
                    let chip_w = ui::measure_text_width(&member, TEXT, true, 400) + 18.0;
                    if used + chip_w > width - 40.0 && used > 0.0 {
                        lines = lines.child(line);
                        line = div().row().gap(6.0);
                        used = 0.0;
                    }
                    used += chip_w + 6.0;
                    line = line.child(
                        div()
                            .row()
                            .h_px(22.0)
                            .px(8.0)
                            .items_center()
                            .rounded(4.0)
                            .bg(colors.element_background)
                            .border(1.0, colors.border_variant)
                            .child(label(member).size(TEXT).mono().color(colors.text)),
                    );
                }
                content = content.child(lines.child(line));
                if let Some(note) = capped(count) {
                    content = content.child(note);
                }
            }
            RedisValue::Missing => {
                content = content.child(details::small(
                    "This key is gone: it expired or was deleted.",
                    colors.text_muted,
                ));
            }
            RedisValue::Other(text) => {
                content = content.child(details::small(text, colors.text_muted))
            }
        }
        content.into()
    }

    fn status_bar(&self) -> Node {
        let colors = theme();
        let groups = self.group_names().len();
        let (status, error) = if self.loading.busy() || self.acting.busy() {
            ("Working...".to_string(), false)
        } else {
            self.status.clone().unwrap_or_default()
        };
        let mut bar = div()
            .row()
            .h_px(STATUS_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .child(
                label(format!(
                    "{groups} {}",
                    if groups == 1 { "prefix" } else { "prefixes" }
                ))
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
            );
        bar = if self.confirm_matching {
            bar.child(
                label(format!(
                    "Delete every key matching {}?",
                    self.pattern.text()
                ))
                .size(12.0)
                .color(colors.error),
            )
            .child(self.text_button(CONFIRM_MATCHING, "Delete all", true))
            .child(self.text_button(CANCEL_MATCHING, "Keep", false))
        } else {
            bar.child(self.text_button(DELETE_MATCHING, "Delete all matching...", true))
        };
        bar.into()
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

    fn field_mut(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::Pattern => Some(&mut self.pattern),
            Focus::Ttl => Some(&mut self.ttl),
            Focus::List => None,
        }
    }

    /// Shows keys and a value as if Redis had answered (previews and tests).
    pub fn show(&mut self, keys: Vec<RedisKey>, selected: usize, value: RedisValue) {
        self.loading = Pending::idle();
        self.keys = keys;
        self.select(selected);
        self.loading_value = Pending::idle();
        self.set_value(self.selected.clone().unwrap_or_default(), value);
    }

    fn set_value(&mut self, key: String, value: RedisValue) {
        self.parsed = match &value {
            RedisValue::Text(text) if text.trim_start().starts_with(['{', '[']) => {
                json::parse(text)
            }
            _ => None,
        };
        self.value = Some((key, value));
    }
}

pub(crate) fn restore_keyspace(
    context: &DatabaseContext,
    item: &workspace::persistence::SerializedItem,
) -> Option<Box<dyn Item>> {
    if item.kind != KIND {
        return None;
    }
    let name = item.data.get("database")?.as_str()?;
    let database = context
        .databases()
        .into_iter()
        .find(|database| database.name == name)?;
    let table: Table = serde_json::from_value(item.data.get("table")?.clone()).ok()?;
    let pattern = item
        .data
        .get("pattern")
        .and_then(|pattern| pattern.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}:*", table.name));
    Some(Box::new(KeyspaceItem::matching(
        context.clone(),
        database,
        table,
        &pattern,
    )))
}

impl Item for KeyspaceItem {
    fn id(&self) -> Option<String> {
        Some(Self::item_id(&self.database, &self.table))
    }

    fn title(&self) -> String {
        format!("{}:* [{}]", self.table.name, self.database.label)
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::EngineRedis)
    }

    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        Some(workspace::persistence::SerializedItem {
            kind: KIND.into(),
            data: serde_json::json!({
                "database": self.database.name,
                "table": self.table,
                "pattern": self.pattern.text(),
            }),
        })
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, focused: bool) -> Option<ui::Painted> {
        // A tab that paints its own body is never told its focus any other way.
        self.focused = focused;
        let scale = ui::ui_text_scale();
        let colors = theme();
        let width = body.w / scale;
        let tree_w = TREE_W.min(width * 0.42) * scale;
        let top = body.y + (TOOLBAR_H + 1.0) * scale;
        let middle_h = (body.h - (TOOLBAR_H + STATUS_H + 2.0) * scale).max(0.0);
        let frame: Node = div()
            .col()
            .w_px(width)
            .h_px(body.h / scale)
            .bg(colors.editor_background)
            .child(self.toolbar(width))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(
                div()
                    .row()
                    .h_px(middle_h / scale)
                    .child(
                        div()
                            .w_px(tree_w / scale)
                            .h_px(middle_h / scale)
                            .bg(colors.panel_background),
                    )
                    .child(
                        div()
                            .w_px(1.0)
                            .h_px(middle_h / scale)
                            .bg(colors.border_variant),
                    ),
            )
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(self.status_bar())
            .into();
        let mut painted = ui::render(&frame, body);
        let tree_area = Rect::new(body.x, top, tree_w, middle_h, Rgba::TRANSPARENT);
        let tree = self.tree_view();
        let mut tree = self.tree.paint(&tree, tree_area);
        details::clip_right(&mut tree, body.x + tree_w);
        details::merge(&mut painted, tree);
        let detail_area = Rect::new(
            body.x + tree_w + scale,
            top,
            (body.w - tree_w - scale).max(0.0),
            middle_h,
            Rgba::TRANSPARENT,
        );
        let detail = self.detail_view(detail_area.w / scale, middle_h / scale);
        let mut detail = self.detail.paint(&detail, detail_area);
        details::clip_right(&mut detail, body.x + body.w - 2.0);
        details::merge(&mut painted, detail);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let modifiers = keystroke.modifiers;
        if modifiers.cmd && keystroke.key == "r" {
            self.load();
            return TerminalKeyOutcome::Handled;
        }
        if self.focus != Focus::List {
            match keystroke.key.as_str() {
                "enter" => {
                    if self.focus == Focus::Ttl {
                        self.apply_ttl();
                    } else {
                        self.focus = Focus::List;
                        self.selected = None;
                        self.load();
                    }
                }
                "escape" => self.focus = Focus::List,
                "c" if modifiers.cmd => {
                    return self
                        .field_mut()
                        .and_then(|field| field.selected_text())
                        .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
                }
                "v" if modifiers.cmd => return TerminalKeyOutcome::Paste,
                key => {
                    let edit = match key {
                        "left" if modifiers.cmd => EditKey::Home,
                        "right" if modifiers.cmd => EditKey::End,
                        "left" => EditKey::Left,
                        "right" => EditKey::Right,
                        "home" => EditKey::Home,
                        "end" => EditKey::End,
                        "backspace" if modifiers.alt => EditKey::DeleteWordLeft,
                        "backspace" => EditKey::Backspace,
                        "delete" => EditKey::Delete,
                        "a" if modifiers.cmd => EditKey::SelectAll,
                        _ => return TerminalKeyOutcome::Ignored,
                    };
                    if let Some(field) = self.field_mut() {
                        field.key(edit, modifiers.shift);
                    }
                }
            }
            return TerminalKeyOutcome::Handled;
        }
        let order: Vec<usize> = self
            .lines()
            .into_iter()
            .filter_map(|line| match line {
                Line::Key(index) => Some(index),
                Line::Group { .. } => None,
            })
            .collect();
        let at = self
            .selected_index()
            .and_then(|index| order.iter().position(|shown| *shown == index));
        match keystroke.key.as_str() {
            "down" => {
                let next = at.map_or(0, |at| (at + 1).min(order.len().saturating_sub(1)));
                if let Some(index) = order.get(next).copied() {
                    self.select(index);
                }
            }
            "up" => {
                let next = at.map_or(0, |at| at.saturating_sub(1));
                if let Some(index) = order.get(next).copied() {
                    self.select(index);
                }
            }
            "c" if modifiers.cmd => {
                self.click(COPY_VALUE);
                return TerminalKeyOutcome::Handled;
            }
            _ => return TerminalKeyOutcome::Ignored,
        }
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = self.field_mut() {
            field.insert(&typed);
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(text);
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        match self.hit(x, y) {
            Some(id) => self.click(id),
            None => self.focus = Focus::List,
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    fn pointer_scroll(&mut self, x: f32, y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        if self.tree.contains(x, y) {
            self.tree.scroll_by(delta_y)
        } else {
            self.detail.scroll_by(delta_y)
        }
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = false;
        if let Some(loaded) = self.loading.poll() {
            match loaded {
                Ok((keys, truncated)) => {
                    self.keys = keys;
                    self.truncated = truncated;
                    if self.selected_index().is_none() {
                        self.selected = None;
                        self.select(0);
                    } else if let Some(key) =
                        self.selected_index().map(|index| self.keys[index].clone())
                    {
                        if self.focus != Focus::Ttl {
                            self.ttl =
                                field(&key.ttl.map(|ttl| ttl.to_string()).unwrap_or_default());
                        }
                    }
                }
                Err(error) => self.status = Some((error, true)),
            }
            changed = true;
        }
        if let Some(value) = self.loading_value.poll() {
            match value {
                Ok((key, value)) => self.set_value(key, value),
                Err(error) => self.status = Some((error, true)),
            }
            changed = true;
        }
        if let Some(done) = self.acting.poll() {
            match done {
                Ok(message) => self.status = Some((message, false)),
                Err(error) => self.status = Some((error, true)),
            }
            self.load();
            self.load_value();
            changed = true;
        }
        ItemTick {
            changed,
            clipboard_store: self.clipboard.take(),
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.loading.busy() || self.loading_value.busy() || self.acting.busy()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, kind: &str, ttl: Option<i64>) -> RedisKey {
        RedisKey {
            key: key.into(),
            kind: kind.into(),
            ttl,
        }
    }

    fn item() -> (crate::tests::TestContext, KeyspaceItem) {
        let context = crate::tests::context();
        let database = context
            .context
            .databases()
            .into_iter()
            .find(|database| database.engine == pom_db::Engine::Redis)
            .expect("redis");
        let table = Table {
            schema: String::new(),
            name: "bull".into(),
            kind: pom_db::TableKind::Keyspace,
            count: Some(3),
        };
        let item = KeyspaceItem::new(context.context.clone(), database, table);
        (context, item)
    }

    #[test]
    fn keys_group_by_their_next_part_and_folds_hide_them() {
        let (_context, mut item) = item();
        item.show(
            vec![
                key("bull:mail:1", "hash", None),
                key("bull:mail:2", "hash", Some(90)),
                key("bull:sms:1", "list", None),
            ],
            0,
            RedisValue::Hash(vec![("to".into(), "ann@example.com".into())]),
        );
        let groups: Vec<String> = item
            .lines()
            .into_iter()
            .filter_map(|line| match line {
                Line::Group { name, .. } => Some(name),
                Line::Key(_) => None,
            })
            .collect();
        assert_eq!(groups, ["bull:mail:", "bull:sms:"]);
        item.click(GROUP_BASE);
        assert_eq!(item.lines().len(), 3, "mail folded: two groups and sms:1");
        assert_eq!(short_ttl(3723), "1h 2m");
    }

    #[test]
    fn a_json_string_reads_as_a_tree_and_ttl_takes_seconds() {
        let (_context, mut item) = item();
        item.show(
            vec![key("bull:job:1", "string", Some(60))],
            0,
            RedisValue::Text(r#"{"id":1,"name":"send"}"#.into()),
        );
        assert!(item.parsed.is_some());
        assert_eq!(item.ttl.text(), "60");
        item.ttl.set_text("soon");
        item.apply_ttl();
        assert!(item.status.as_ref().is_some_and(|(_, error)| *error));
        let saved = item.serialize().expect("saved");
        assert_eq!(saved.data["pattern"], "bull:*");
    }
}
