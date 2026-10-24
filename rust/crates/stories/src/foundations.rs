//! What every screen is built from: the active theme's color tokens, the text styles and the icons.

use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rgba, TextColor};

use crate::{still, Story, StoryDef};

const PAGE: &str = "Foundations";

/// Color tokens by role, named as theme files write them.
const COLOR_GROUPS: &[(&str, &[&str])] = &[
    (
        "Surfaces",
        &[
            "background",
            "surface.background",
            "elevated_surface.background",
            "panel.background",
            "title_bar.background",
            "toolbar.background",
            "tab_bar.background",
            "tab.active_background",
            "tab.inactive_background",
        ],
    ),
    (
        "Borders",
        &[
            "border",
            "border.variant",
            "border.focused",
            "border.selected",
        ],
    ),
    (
        "Elements",
        &[
            "element.background",
            "element.hover",
            "element.active",
            "element.selected",
            "ghost_element.hover",
            "ghost_element.active",
            "accent.background",
            "accent.foreground",
        ],
    ),
    (
        "Text and icons",
        &[
            "text",
            "text.muted",
            "text.placeholder",
            "text.accent",
            "text.disabled",
            "icon",
            "icon.muted",
            "icon.accent",
        ],
    ),
    (
        "Status",
        &[
            "success",
            "warning",
            "warning.background",
            "warning.border",
            "error",
            "error.background",
            "error.border",
            "info",
            "info.background",
            "info.border",
            "hint",
            "hint.background",
            "hint.border",
            "attention",
            "attention.foreground",
        ],
    ),
    (
        "Editor",
        &[
            "editor.background",
            "editor.foreground",
            "editor.active_line.background",
            "editor.gutter.background",
            "editor.line_number",
            "editor.active_line_number",
            "editor.indent_guide",
            "editor.indent_guide_active",
            "search.match_background",
            "search.active_match_background",
        ],
    ),
];

const SWATCH_ROW: f32 = 36.0;
const COLORS_WIDTH: f32 = 420.0;
const ICON_COLUMNS: usize = 8;
const ICON_CELL: (f32, f32) = (88.0, 64.0);

fn hex(color: Rgba) -> String {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgb = format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b)
    );
    if color.a < 1.0 {
        format!("{rgb}{:02x}", byte(color.a))
    } else {
        rgb
    }
}

fn swatches(tokens: &[&str]) -> Node {
    let colors = theme();
    let mut column = div().col().p(16.0);
    for name in tokens {
        let Some(color) = ui::theme_file::token(&colors, name) else {
            continue;
        };
        column = column.child(
            div()
                .h_px(SWATCH_ROW)
                .items_center()
                .gap(12.0)
                .child(
                    div()
                        .w_px(40.0)
                        .h_px(24.0)
                        .rounded(4.0)
                        .bg(color)
                        .border(1.0, colors.border),
                )
                .child(div().flex(1.0).child(label(*name).size(12.0).truncate()))
                .child(label(hex(color)).size(11.0).mono().color(colors.text_muted)),
        );
    }
    column.into()
}

fn colors(group: usize) -> Node {
    COLOR_GROUPS
        .get(group)
        .map_or_else(|| div().into(), |(_, tokens)| swatches(tokens))
}

fn colors_height(group: usize) -> f32 {
    COLOR_GROUPS
        .get(group)
        .map_or(0.0, |(_, tokens)| tokens.len() as f32 * SWATCH_ROW + 32.0)
}

fn sizes() -> Node {
    let mut column = div().col().gap(14.0).p(16.0);
    for (name, size) in [
        ("Large 16", LabelSize::Large),
        ("Default 14", LabelSize::Default),
        ("Small 12", LabelSize::Small),
        ("XSmall 10", LabelSize::XSmall),
    ] {
        column = column.child(
            div()
                .items_center()
                .gap(16.0)
                .child(
                    div().w_px(90.0).child(
                        label(name)
                            .label_size(LabelSize::Small)
                            .color(theme().text_muted),
                    ),
                )
                .child(label("Start every service of feat-login").label_size(size)),
        );
    }
    column.into()
}

fn weights() -> Node {
    let mut column = div().col().gap(14.0).p(16.0);
    for (name, weight) in [
        ("Regular 400", 400),
        ("Medium 500", 500),
        ("Semibold 600", 600),
    ] {
        column = column.child(
            div()
                .items_center()
                .gap(16.0)
                .child(
                    div().w_px(90.0).child(
                        label(name)
                            .label_size(LabelSize::Small)
                            .color(theme().text_muted),
                    ),
                )
                .child(label("Services feat-login").size(14.0).weight(weight)),
        );
    }
    column
        .child(
            div()
                .items_center()
                .gap(16.0)
                .child(
                    div().w_px(90.0).child(
                        label("Mono")
                            .label_size(LabelSize::Small)
                            .color(theme().text_muted),
                    ),
                )
                .child(label("bin/rails server -p $PORT").size(13.0).mono()),
        )
        .into()
}

fn text_colors() -> Node {
    let mut column = div().col().gap(12.0).p(16.0);
    for (name, color) in [
        ("Default", TextColor::Default),
        ("Muted", TextColor::Muted),
        ("Accent", TextColor::Accent),
        ("Placeholder", TextColor::Placeholder),
        ("Disabled", TextColor::Disabled),
    ] {
        column = column.child(label(format!("{name} text")).size(14.0).color(color.rgba()));
    }
    column.into()
}

fn icons() -> Node {
    let colors = theme();
    let mut grid = div().col().p(12.0);
    for chunk in IconKind::ALL.chunks(ICON_COLUMNS) {
        let mut row = div();
        for kind in chunk {
            row = row.child(
                div()
                    .col()
                    .w_px(ICON_CELL.0)
                    .h_px(ICON_CELL.1)
                    .items_center()
                    .justify_center()
                    .gap(8.0)
                    .child(icon(*kind).size(18.0).color(colors.icon))
                    .child(
                        label(format!("{kind:?}"))
                            .size(10.0)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
            );
        }
        grid = grid.child(row);
    }
    grid.into()
}

fn icons_size() -> (f32, f32) {
    let rows = IconKind::ALL.len().div_ceil(ICON_COLUMNS);
    (
        ICON_CELL.0 * ICON_COLUMNS as f32 + 24.0,
        ICON_CELL.1 * rows as f32 + 24.0,
    )
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, size, build| StoryDef {
        id,
        page: PAGE,
        group,
        title,
        size,
        build,
    };
    let color = |id, index: usize, build: fn() -> anyhow::Result<Box<dyn Story>>| {
        story(
            id,
            "Color",
            COLOR_GROUPS.get(index).map_or("", |(title, _)| *title),
            (COLORS_WIDTH, colors_height(index)),
            build,
        )
    };
    vec![
        color("color-surfaces", 0, || still(|_, _| colors(0))),
        color("color-borders", 1, || still(|_, _| colors(1))),
        color("color-elements", 2, || still(|_, _| colors(2))),
        color("color-text", 3, || still(|_, _| colors(3))),
        color("color-status", 4, || still(|_, _| colors(4))),
        color("color-editor", 5, || still(|_, _| colors(5))),
        story("type-sizes", "Type", "Sizes", (460.0, 176.0), || {
            still(|_, _| sizes())
        }),
        story("type-weights", "Type", "Weights", (460.0, 176.0), || {
            still(|_, _| weights())
        }),
        story("type-colors", "Type", "Colors", (240.0, 176.0), || {
            still(|_, _| text_colors())
        }),
        story("icons", "Icons", "Every icon", icons_size(), || {
            still(|_, _| icons())
        }),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_color_token_resolves() {
        let colors = ui::theme();
        for (_, tokens) in super::COLOR_GROUPS {
            for name in *tokens {
                assert!(
                    ui::theme_file::token(&colors, name).is_some(),
                    "{name} is not a theme token"
                );
            }
        }
    }
}
