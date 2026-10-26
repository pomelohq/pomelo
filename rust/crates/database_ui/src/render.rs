//! How the Database panel draws: its header, the filter box and each kind of tree row.

use std::ops::Range;

use pom_db::object_storage::format_size;
use pom_db::{Engine, TableKind};
use ui::{
    div, icon, label, material_icon, theme, Div, IconKind, LabelSize, MaterialIcon, Node, Rgba,
};
use workspace::text_field::FieldFont;

use crate::panel::{
    DatabasePanel, CHEVRON, COLLAPSE_ALL, FAILURE_BLOCK_BOTTOM, FAILURE_BLOCK_LEFT,
    FAILURE_BLOCK_RIGHT, FAILURE_BLOCK_TOP, FILTER, FILTER_H, HEADER_H, NEW_CONSOLE, REFRESH,
    RENAME_FIELD, ROW_H,
};
use crate::tree::{format_count, prefix_name, Row, Section, Status, OTHER_SERVICES};

pub(crate) const INDENT: f32 = 16.0;

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
/// One vertical guide per tree level above `depth`, 16px apart like the Files and Git panels.
/// A name with the filter's hit marked; the part after the hit gives way when the row is narrow.
fn name(text: &str, hit: Option<Range<usize>>, color: Rgba, medium: bool) -> Node {
    let styled = |part: &str| {
        let part = label(part.to_string()).size(LabelSize::Medium).color(color);
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
        .size(LabelSize::Small)
        .color(theme().text_placeholder)
        .into()
}

fn dot(status: Status) -> Node {
    let colors = theme();
    let mark = match status {
        Status::Connected => components::dot(colors.success, 7.0),
        Status::Failed { warning: true } => components::dot(colors.warning, 7.0),
        Status::Failed { warning: false } => components::dot(colors.error, 7.0),
        Status::NotConnected | Status::Loading => components::ring(colors.text_placeholder, 8.0),
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

fn row_base(
    panel: &DatabasePanel,
    index: usize,
    row: &Row,
    open: Option<bool>,
    own_chevron: bool,
) -> Div {
    let id = DatabasePanel::id(index);
    let selected = panel.selected.as_deref() == Some(row.key().as_str());
    let mut line = components::tree_row(id, row.depth())
        .height(ROW_H)
        .expander(open)
        .selected(selected)
        .focused(selected && panel.keyboard)
        .hovered(
            panel
                .hover
                .is_some_and(|hover| hover == id || hover == id + CHEVRON),
        );
    if own_chevron {
        line = line.expander_id(id + CHEVRON);
    }
    line.render()
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
                Section::Consoles => "Consoles",
                Section::Databases => "Databases",
            };
            let mut header = components::section_header(title);
            if let Some(count) = count {
                header = header.count(*count);
            }
            if *section == Section::Consoles {
                header = header.expander(Some(*open));
            }
            let mut line = div()
                .row()
                .h_px(ROW_H + 4.0)
                .pt(4.0)
                .px(8.0)
                .items_center()
                .child(header.render().flex(1.0));
            if *section == Section::Consoles {
                line = line.on_click(id);
            }
            line.into()
        }
        Row::Console { console, place } => {
            let line = row_base(panel, index, row, None, false)
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
            let mut line = row_base(panel, index, row, Some(*open), false).child(name(
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
            let line = row_base(panel, index, row, Some(*open), false)
                .child(icon(kind).size(14.0).color(color))
                .child(
                    label(found.label.clone())
                        .size(LabelSize::Medium)
                        .color(colors.text),
                )
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(subtitle)
                            .size(LabelSize::Small)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ),
                )
                .child(dot(*status));
            if panel.hover == Some(id) && !detail.tooltip.is_empty() {
                components::with_tooltip(line, &detail.tooltip, false).into()
            } else {
                line.into()
            }
        }
        Row::Group {
            kind, count, open, ..
        } => row_base(panel, index, row, Some(*open), false)
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
            let (glyph, tint) = if table.kind == TableKind::View {
                (IconKind::Eye, colors.icon_muted)
            } else {
                (IconKind::Table, colors.icon_muted)
            };
            let mut line = row_base(
                panel,
                index,
                row,
                has_columns.then_some(*open),
                *has_columns,
            )
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
            row_base(panel, index, row, None, false)
                .child(glyph)
                .child(name(&column.name, hit.clone(), colors.text_muted, false))
                .child(
                    label(kind)
                        .size(LabelSize::Mini)
                        .mono()
                        .color(colors.text_placeholder),
                )
                .into()
        }
        Row::Keyspace { table, hit, .. } => {
            let pattern = format!("{}:*", table.name);
            let mut line = row_base(panel, index, row, None, false)
                .child(icon(IconKind::Key).size(13.0).color(colors.icon_muted))
                .child(name(&pattern, hit.clone(), colors.text, false));
            if let Some(count) = table.count {
                line = line.child(trailing(format!("{} keys", format_count(count))));
            }
            line.into()
        }
        Row::Bucket { bucket, open, .. } => row_base(panel, index, row, Some(*open), true)
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
            let mut line = row_base(panel, index, row, Some(*open), true)
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
        Row::Object { object, hit, .. } => row_base(panel, index, row, None, false)
            .child(material_icon(object_icon(object.name())).size(14.0))
            .child(name(object.name(), hit.clone(), colors.text, false))
            .child(trailing(format_size(object.size)))
            .into(),
        Row::More { loading, .. } => row_base(panel, index, row, None, false)
            .child(
                label(if *loading {
                    "Loading..."
                } else {
                    "Show 50 more"
                })
                .size(LabelSize::Small)
                .color(colors.text_accent),
            )
            .into(),
        Row::Note { depth, text, error } => components::indent_guides(
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
                .size(LabelSize::Small)
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
            components::indent_guides(
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
    let id = DatabasePanel::base() + offset;
    let button = components::icon_button(id, kind)
        .hovered(panel.hover)
        .render();
    if panel.hover == Some(id) {
        components::with_tooltip(button, tip, false).into()
    } else {
        button.into()
    }
}

fn filter_box(panel: &DatabasePanel) -> Node {
    let colors = theme();
    let mut field = components::search_box(DatabasePanel::base() + FILTER, panel.filter_focused)
        .flex(1.0)
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
        .child(
            label("Database")
                .size(LabelSize::Medium)
                .medium()
                .color(colors.text),
        )
        .child(
            div().row().flex(1.0).pl(4.0).items_center().child(
                label(panel.context.branch.clone())
                    .size(LabelSize::Small)
                    .color(colors.text_placeholder)
                    .truncate(),
            ),
        )
        .child(header_button(
            panel,
            NEW_CONSOLE,
            IconKind::Plus,
            &workspace::keymap::with_key_hint(
                "New Console",
                workspace::keymap::Action::DatabaseNewConsole,
            ),
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
        list = list.child(components::empty_state("No databases in pom.yml").compact());
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
