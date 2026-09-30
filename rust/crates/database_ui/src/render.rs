//! How the Database panel draws: its header, the filter box and each kind of tree row.

use std::ops::Range;

use pom_db::object_storage::format_size;
use pom_db::{Engine, TableKind};
use ui::{
    deferred, div, icon, label, material_icon, theme, Corner, Div, IconKind, MaterialIcon, Node,
    Rgba,
};
use workspace::text_field::FieldFont;

use crate::panel::{
    DatabasePanel, CHEVRON, COLLAPSE_ALL, FAILURE_BLOCK_BOTTOM, FAILURE_BLOCK_LEFT,
    FAILURE_BLOCK_RIGHT, FAILURE_BLOCK_TOP, FILTER, FILTER_H, HEADER_H, NEW_CONSOLE, REFRESH,
    RENAME_FIELD, ROW_H,
};
use crate::tree::{format_count, prefix_name, Row, Section, Status, OTHER_SERVICES};

pub(crate) const INDENT: f32 = 16.0;
const TOOLTIP_H: f32 = 24.0;
const BUTTON: f32 = 24.0;

/// The engine's logo and a brand color that reads on a dark panel.
pub fn engine_logo(engine: Engine) -> (IconKind, Rgba) {
    let (kind, color) = match engine {
        Engine::Postgres => (IconKind::EnginePostgresql, "#4169E1"),
        Engine::Mysql => (IconKind::EngineMysql, "#4479A1"),
        Engine::Mariadb => (IconKind::EngineMariadb, "#C0765A"),
        Engine::Redis => (IconKind::EngineRedis, "#FF4438"),
        Engine::Mongodb => (IconKind::EngineMongodb, "#47A248"),
        Engine::Sqlite => (IconKind::EngineSqlite, "#44A8D8"),
        Engine::Elasticsearch => (IconKind::EngineElasticsearch, "#00BFB3"),
        Engine::Opensearch => (IconKind::EngineOpensearch, "#3D8FE0"),
        Engine::Minio => (IconKind::EngineMinio, "#C72E49"),
        Engine::Rabbitmq => (IconKind::EngineRabbitmq, "#FF6600"),
        Engine::Kafka => (IconKind::EngineKafka, "#DCE0E5"),
        Engine::Other => return (IconKind::Cylinder, theme().icon_muted),
    };
    (kind, Rgba::hex(color))
}

fn object_icon(name: &str) -> MaterialIcon {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => MaterialIcon::Image,
        "json" => MaterialIcon::Json,
        "yml" | "yaml" => MaterialIcon::Yaml,
        "md" => MaterialIcon::Markdown,
        "html" | "htm" => MaterialIcon::Html,
        "sh" | "log" => MaterialIcon::Console,
        _ => MaterialIcon::Document,
    }
}

/// A bubble under the hovered control, kept inside the window.
fn tooltip(target: Div, text: &str) -> Div {
    let width = ui::measure_text_width(text, 12.0, false, ui::ui_font_weight())
        / ui::ui_text_scale()
        + 16.0;
    let bubble = div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(TOOLTIP_H)
        .rounded(6.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(label(text.to_string()).size(12.0).color(theme().text));
    target.pin(
        Corner::BottomLeft,
        0.0,
        TOOLTIP_H + 4.0,
        width,
        TOOLTIP_H,
        deferred(bubble).priority(5).snap_to_window(),
    )
}

/// One vertical guide per tree level above `depth`, 16px apart like the Files and Git panels.
fn with_guides(mut row: Div, depth: usize) -> Div {
    for level in 0..depth {
        row = row.pin_left_edge(
            15.0 + level as f32 * INDENT,
            0.0,
            1.0,
            div().bg(theme().panel_indent_guide),
        );
    }
    row
}

fn chevron(id: Option<u64>, open: bool) -> Node {
    let glyph = icon(if open {
        IconKind::ChevronDown
    } else {
        IconKind::ChevronRight
    })
    .size(12.0)
    .color(theme().icon_muted);
    let mut slot = div().w_px(12.0).h_px(ROW_H).items_center().justify_center();
    if let Some(id) = id {
        slot = slot.on_click(id);
    }
    slot.child(glyph).into()
}

fn spacer() -> Node {
    div().w_px(12.0).into()
}

/// A name with the filter's hit marked; the part after the hit gives way when the row is narrow.
fn name(text: &str, hit: Option<Range<usize>>, color: Rgba, medium: bool) -> Node {
    let styled = |part: &str| {
        let part = label(part.to_string()).size(13.0).color(color);
        if medium {
            part.medium()
        } else {
            part
        }
    };
    let Some(hit) = hit.filter(|hit| text.get(hit.clone()).is_some() && hit.end > hit.start) else {
        return div()
            .row()
            .flex(1.0)
            .items_center()
            .child(styled(text).truncate())
            .into();
    };
    let (before, found, after) = (&text[..hit.start], &text[hit.clone()], &text[hit.end..]);
    let mut row = div().row().flex(1.0).items_center();
    if !before.is_empty() {
        row = row.child(styled(before));
    }
    row.child(
        div()
            .row()
            .rounded(2.0)
            .bg(theme().search_match_background)
            .child(styled(found)),
    )
    .child(styled(after).truncate())
    .into()
}

fn trailing(text: String) -> Node {
    label(text)
        .size(11.5)
        .color(theme().text_placeholder)
        .into()
}

fn dot(status: Status) -> Node {
    let colors = theme();
    let mark = match status {
        Status::Connected => div().w_px(7.0).h_px(7.0).rounded(3.5).bg(colors.success),
        Status::Failed { warning: true } => {
            div().w_px(7.0).h_px(7.0).rounded(3.5).bg(colors.warning)
        }
        Status::Failed { warning: false } => {
            div().w_px(7.0).h_px(7.0).rounded(3.5).bg(colors.error)
        }
        Status::NotConnected | Status::Loading => div()
            .w_px(8.0)
            .h_px(8.0)
            .rounded(4.0)
            .border(1.5, colors.text_placeholder),
    };
    div()
        .row()
        .w_px(10.0)
        .h_px(ROW_H)
        .items_center()
        .justify_center()
        .child(mark)
        .into()
}

/// The text a row is found by (previews and tests).
pub(crate) fn row_name(panel: &DatabasePanel, row: &Row) -> Option<String> {
    Some(match row {
        Row::Console { console, .. } => console.title.clone(),
        Row::Repo { repo, .. } => repo.clone(),
        Row::Database { index, repo, .. } => {
            let database = panel.model.databases.get(*index)?;
            format!("{repo}/{}", database.label)
        }
        Row::Table { table, .. } | Row::Keyspace { table, .. } => table.qualified(),
        Row::Column { table, column, .. } => format!("{}.{}", table.qualified(), column.name),
        Row::Bucket { bucket, .. } => bucket.clone(),
        Row::Prefix { prefix, .. } => prefix.clone(),
        Row::Object { object, .. } => object.key.clone(),
        _ => return None,
    })
}

fn row_base(panel: &DatabasePanel, index: usize, row: &Row) -> Div {
    let colors = theme();
    let id = DatabasePanel::id(index);
    let depth = row.depth();
    let mut line = div()
        .row()
        .h_px(ROW_H)
        .pl(8.0 + depth as f32 * INDENT)
        .pr(8.0)
        .gap(6.0)
        .items_center()
        .on_click(id);
    if panel.selected.as_deref() == Some(row.key().as_str()) {
        line = line.bg(colors.element_selected);
    } else if panel
        .hover
        .is_some_and(|hover| hover == id || hover == id + CHEVRON)
    {
        line = line.bg(colors.ghost_element_hover);
    }
    with_guides(line, depth)
}

pub(crate) fn render_row(panel: &DatabasePanel, index: usize, row: &Row) -> Node {
    let colors = theme();
    let id = DatabasePanel::id(index);
    match row {
        Row::Section {
            section,
            count,
            open,
        } => {
            let title = match section {
                Section::Consoles => "CONSOLES",
                Section::Databases => "DATABASES",
            };
            let mut line = div()
                .row()
                .h_px(ROW_H + 4.0)
                .pt(4.0)
                .pl(8.0)
                .pr(8.0)
                .gap(6.0)
                .items_center();
            if *section == Section::Consoles {
                line = line.on_click(id).child(chevron(None, *open));
            }
            line = line.child(
                div().row().flex(1.0).items_center().child(
                    label(title)
                        .size(11.0)
                        .medium()
                        .color(colors.text_placeholder),
                ),
            );
            if let Some(count) = count {
                line = line.child(trailing(count.to_string()));
            }
            line.into()
        }
        Row::Console { console, place } => {
            let line = row_base(panel, index, row)
                .child(spacer())
                .child(icon(IconKind::File).size(14.0).color(colors.icon_muted));
            let renaming = panel
                .rename
                .as_ref()
                .filter(|(renamed, _)| *renamed == console.id);
            match renaming {
                Some((_, field)) => line
                    .border(1.0, colors.panel_focused_border)
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .items_center()
                            .on_click(DatabasePanel::base() + RENAME_FIELD)
                            .child(field.render("", true, colors.text, ROW_H - 4.0, FieldFont::Ui)),
                    )
                    .into(),
                None => line
                    .child(name(&console.title, None, colors.text, false))
                    .child(trailing(place.clone()))
                    .into(),
            }
        }
        Row::Repo {
            repo,
            other,
            open,
            engines,
            failed,
        } => {
            let mut line = row_base(panel, index, row)
                .child(chevron(None, *open))
                .child(name(
                    repo,
                    None,
                    if *other {
                        colors.text_muted
                    } else {
                        colors.text
                    },
                    !*other,
                ));
            if *failed {
                line = line.child(dot(Status::Failed { warning: false }));
            }
            let mut logos = div().row().gap(4.0).items_center();
            for engine in engines {
                let (kind, color) = engine_logo(*engine);
                logos = logos.child(icon(kind).size(12.0).color(color));
            }
            line.child(logos).into()
        }
        Row::Database {
            index: database,
            open,
            shared_with,
            status,
            repo,
        } => {
            let Some(found) = panel.model.databases.get(*database) else {
                return div().into();
            };
            let detail = panel.details.get(&found.name).cloned().unwrap_or_default();
            let mut subtitle = detail.subtitle.clone();
            if !shared_with.is_empty() {
                let note = format!("shared with {}", shared_with.join(", "));
                subtitle = if subtitle.is_empty() {
                    note
                } else {
                    format!("{subtitle} - {note}")
                };
            } else if found.is_shared() && repo == OTHER_SERVICES {
                subtitle = "no repo refers to it".into();
            }
            let (kind, color) = engine_logo(found.engine);
            let line = row_base(panel, index, row)
                .child(chevron(None, *open))
                .child(icon(kind).size(14.0).color(color))
                .child(label(found.label.clone()).size(13.0).color(colors.text))
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(subtitle)
                            .size(11.5)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ),
                )
                .child(dot(*status));
            if panel.hover == Some(id) && !detail.tooltip.is_empty() {
                tooltip(line, &detail.tooltip).into()
            } else {
                line.into()
            }
        }
        Row::Group {
            kind, count, open, ..
        } => row_base(panel, index, row)
            .child(chevron(None, *open))
            .child(name(
                if *kind == TableKind::View {
                    "views"
                } else {
                    "tables"
                },
                None,
                colors.text_muted,
                false,
            ))
            .child(trailing(count.to_string()))
            .into(),
        Row::Table {
            table,
            open,
            has_columns,
            hit,
            ..
        } => {
            let lead = if *has_columns {
                chevron(Some(id + CHEVRON), *open)
            } else {
                spacer()
            };
            let (glyph, tint) = if table.kind == TableKind::View {
                (IconKind::Eye, colors.icon_muted)
            } else {
                (IconKind::Table, colors.icon_muted)
            };
            let mut line = row_base(panel, index, row)
                .child(lead)
                .child(icon(glyph).size(14.0).color(tint))
                .child(name(&table.qualified(), hit.clone(), colors.text, false));
            if let Some(count) = table.count {
                line = line.child(trailing(format_count(count)));
            }
            line.into()
        }
        Row::Column { column, hit, .. } => {
            let glyph = if column.primary_key {
                icon(IconKind::Key).size(13.0).color(colors.warning)
            } else {
                icon(IconKind::Column)
                    .size(13.0)
                    .color(colors.text_placeholder)
            };
            let mut kind = column.data_type.clone();
            if let Some(target) = &column.references {
                kind.push_str(&format!(" -> {target}"));
            }
            row_base(panel, index, row)
                .child(spacer())
                .child(glyph)
                .child(name(&column.name, hit.clone(), colors.text_muted, false))
                .child(label(kind).size(11.0).mono().color(colors.text_placeholder))
                .into()
        }
        Row::Keyspace { table, hit, .. } => {
            let pattern = format!("{}:*", table.name);
            let mut line = row_base(panel, index, row)
                .child(spacer())
                .child(icon(IconKind::Key).size(13.0).color(colors.icon_muted))
                .child(name(&pattern, hit.clone(), colors.text, false));
            if let Some(count) = table.count {
                line = line.child(trailing(format!("{} keys", format_count(count))));
            }
            line.into()
        }
        Row::Bucket { bucket, open, .. } => row_base(panel, index, row)
            .child(chevron(Some(id + CHEVRON), *open))
            .child(
                icon(if *open {
                    IconKind::FolderOpen
                } else {
                    IconKind::Folder
                })
                .size(14.0)
                .color(colors.icon_muted),
            )
            .child(name(bucket, None, colors.text, false))
            .into(),
        Row::Prefix {
            prefix,
            open,
            stats,
            hit,
            ..
        } => {
            let mut line = row_base(panel, index, row)
                .child(chevron(Some(id + CHEVRON), *open))
                .child(
                    icon(if *open {
                        IconKind::FolderOpen
                    } else {
                        IconKind::Folder
                    })
                    .size(14.0)
                    .color(colors.icon_muted),
                )
                .child(name(prefix_name(prefix), hit.clone(), colors.text, false));
            if let Some(stats) = stats {
                let plus = if stats.capped { "+" } else { "" };
                line = line.child(trailing(format!(
                    "{}{plus} - {}",
                    format_count(stats.objects as usize),
                    format_size(stats.bytes)
                )));
            }
            line.into()
        }
        Row::Object { object, hit, .. } => row_base(panel, index, row)
            .child(spacer())
            .child(material_icon(object_icon(object.name())).size(14.0))
            .child(name(object.name(), hit.clone(), colors.text, false))
            .child(trailing(format_size(object.size)))
            .into(),
        Row::More { loading, .. } => row_base(panel, index, row)
            .child(spacer())
            .child(
                label(if *loading {
                    "Loading..."
                } else {
                    "Show 50 more"
                })
                .size(12.0)
                .color(colors.text_accent),
            )
            .into(),
        Row::Note { depth, text, error } => with_guides(
            div()
                .row()
                .h_px(ROW_H)
                .pl(8.0 + *depth as f32 * INDENT + 18.0)
                .pr(8.0)
                .items_center(),
            *depth,
        )
        .child(
            label(text.clone())
                .size(12.0)
                .color(if *error {
                    colors.error
                } else {
                    colors.text_placeholder
                })
                .truncate(),
        )
        .into(),
        Row::Failure { index: database } => {
            let Some(failure) = panel
                .model
                .databases
                .get(*database)
                .and_then(|found| panel.failure(&found.name))
            else {
                return div().into();
            };
            let block_width = panel.width - FAILURE_BLOCK_LEFT - FAILURE_BLOCK_RIGHT;
            let owner = *database;
            with_guides(
                div()
                    .row()
                    .pl(FAILURE_BLOCK_LEFT)
                    .pr(FAILURE_BLOCK_RIGHT)
                    .pt(FAILURE_BLOCK_TOP)
                    .pb(FAILURE_BLOCK_BOTTOM),
                2,
            )
            .child(failure.render(
                block_width,
                |action| DatabasePanel::failure_id(owner, action),
                panel.hover,
            ))
            .into()
        }
    }
}

fn header_button(panel: &DatabasePanel, offset: u64, kind: IconKind, tip: &str) -> Node {
    let colors = theme();
    let id = DatabasePanel::base() + offset;
    let hot = panel.hover == Some(id);
    let button = div()
        .w_px(BUTTON)
        .h_px(BUTTON)
        .rounded(4.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .bg(if hot {
            colors.ghost_element_hover
        } else {
            Rgba::TRANSPARENT
        })
        .child(
            icon(kind)
                .size(14.0)
                .color(if hot { colors.icon } else { colors.icon_muted }),
        );
    if hot {
        tooltip(button, tip).into()
    } else {
        button.into()
    }
}

fn filter_box(panel: &DatabasePanel) -> Node {
    let colors = theme();
    let mut field = div()
        .row()
        .flex(1.0)
        .h_px(28.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .rounded(5.0)
        .bg(colors.editor_background)
        .border(
            1.0,
            if panel.filter_focused {
                colors.border_focused
            } else {
                colors.border_variant
            },
        )
        .on_click(DatabasePanel::base() + FILTER)
        .child(
            icon(IconKind::Search)
                .size(13.0)
                .color(colors.text_placeholder),
        )
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .child(panel.filter.render(
                    "Filter tables and columns",
                    panel.filter_focused,
                    colors.text,
                    20.0,
                    FieldFont::Ui,
                )),
        );
    if !panel.filter.text().trim().is_empty() {
        field = field.child(trailing(format!("{} found", panel.found)));
    }
    div()
        .row()
        .h_px(FILTER_H)
        .px(8.0)
        .pb(6.0)
        .items_center()
        .child(field)
        .into()
}

pub(crate) fn render_panel(
    panel: &DatabasePanel,
    width: f32,
    height: f32,
    visible: &[usize],
) -> Node {
    let colors = theme();
    let header = div()
        .row()
        .h_px(HEADER_H)
        .pl(12.0)
        .pr(6.0)
        .gap(4.0)
        .items_center()
        .child(label("Database").size(13.0).medium().color(colors.text))
        .child(
            div().row().flex(1.0).pl(4.0).items_center().child(
                label(panel.context.branch.clone())
                    .size(12.0)
                    .color(colors.text_placeholder)
                    .truncate(),
            ),
        )
        .child(header_button(
            panel,
            NEW_CONSOLE,
            IconKind::Plus,
            "New Console",
        ))
        .child(header_button(panel, REFRESH, IconKind::RotateCw, "Refresh"))
        .child(header_button(
            panel,
            COLLAPSE_ALL,
            IconKind::ChevronUp,
            "Collapse All",
        ));
    let mut list = div().col().pb(8.0);
    if panel.model.databases.is_empty() {
        list = list.child(
            div().row().h_px(ROW_H).px(12.0).items_center().child(
                label("No databases in pom.yml")
                    .size(12.0)
                    .color(colors.text_muted),
            ),
        );
    }
    for index in visible {
        if let Some(row) = panel.rows.get(*index) {
            list = list.child(render_row(panel, *index, row));
        }
    }
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .child(header)
        .child(filter_box(panel))
        .child(list)
        .into()
}
