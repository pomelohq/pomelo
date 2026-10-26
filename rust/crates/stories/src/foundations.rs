//! What every screen is built from: the active theme's color tokens, the text styles and the icons.

use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Radius, Rgba, Space, TextColor};

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
    let mut column = div().col().p(Space::S16);
    for name in tokens {
        let Some(color) = ui::theme_file::token(&colors, name) else {
            continue;
        };
        column = column.child(
            div()
                .h_px(SWATCH_ROW)
                .items_center()
                .gap(Space::S12)
                .child(
                    div()
                        .w_px(40.0)
                        .h_px(24.0)
                        .rounded(Radius::Small)
                        .bg(color)
                        .border(1.0, colors.border),
                )
                .child(
                    div()
                        .flex(1.0)
                        .child(label(*name).size(LabelSize::Small).truncate()),
                )
                .child(
                    label(hex(color))
                        .size(LabelSize::Mini)
                        .mono()
                        .color(colors.text_muted),
                ),
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
    let mut column = div().col().gap(Space::S14).p(Space::S16);
    for (name, size) in [
        ("Large 16", LabelSize::Large),
        ("Default 14", LabelSize::Default),
        ("Medium 13", LabelSize::Medium),
        ("Small 12", LabelSize::Small),
        ("Mini 11", LabelSize::Mini),
        ("XSmall 10", LabelSize::XSmall),
    ] {
        column = column.child(
            div()
                .items_center()
                .gap(Space::S16)
                .child(
                    div()
                        .w_px(90.0)
                        .child(label(name).size(LabelSize::Small).color(theme().text_muted)),
                )
                .child(label("Start every service of feat-login").size(size)),
        );
    }
    column.into()
}

fn weights() -> Node {
    let mut column = div().col().gap(Space::S14).p(Space::S16);
    for (name, weight) in [
        ("Regular 400", 400),
        ("Medium 500", 500),
        ("Semibold 600", 600),
    ] {
        column = column.child(
            div()
                .items_center()
                .gap(Space::S16)
                .child(
                    div()
                        .w_px(90.0)
                        .child(label(name).size(LabelSize::Small).color(theme().text_muted)),
                )
                .child(
                    label("Services feat-login")
                        .size(LabelSize::Default)
                        .weight(weight),
                ),
        );
    }
    column
        .child(
            div()
                .items_center()
                .gap(Space::S16)
                .child(
                    div().w_px(90.0).child(
                        label("Mono")
                            .size(LabelSize::Small)
                            .color(theme().text_muted),
                    ),
                )
                .child(
                    label("bin/rails server -p $PORT")
                        .size(LabelSize::Medium)
                        .mono(),
                ),
        )
        .into()
}

fn text_colors() -> Node {
    let mut column = div().col().gap(Space::S12).p(Space::S16);
    for (name, color) in [
        ("Default", TextColor::Default),
        ("Muted", TextColor::Muted),
        ("Accent", TextColor::Accent),
        ("Placeholder", TextColor::Placeholder),
        ("Disabled", TextColor::Disabled),
    ] {
        column = column.child(
            label(format!("{name} text"))
                .size(LabelSize::Default)
                .color(color.rgba()),
        );
    }
    column.into()
}

fn icons() -> Node {
    let colors = theme();
    let mut grid = div().col().p(Space::S12);
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
                    .gap(Space::S8)
                    .child(icon(*kind).size(18.0).color(colors.icon))
                    .child(
                        label(format!("{kind:?}"))
                            .size(LabelSize::XSmall)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
            );
        }
        grid = grid.child(row);
    }
    grid.into()
}

fn spacings() -> Node {
    let colors = theme();
    let mut column = div().col().gap(Space::S8).p(Space::S16);
    for space in Space::STEPS {
        column = column.child(
            div()
                .row()
                .gap(Space::S12)
                .items_center()
                .child(
                    div()
                        .w_px(44.0)
                        .child(label(format!("{}", space.px())).size(LabelSize::Small)),
                )
                .child(
                    div()
                        .w_px(space.px())
                        .h_px(12.0)
                        .bg(colors.text_accent.alpha(0.6)),
                ),
        );
    }
    column.into()
}

fn radii() -> Node {
    let colors = theme();
    let mut row = div().row().gap(Space::S20).p(Space::S16);
    for radius in Radius::ALL {
        row = row.child(
            div()
                .col()
                .gap(Space::S8)
                .items_center()
                .child(
                    div()
                        .w_px(64.0)
                        .h_px(48.0)
                        .rounded(radius)
                        .bg(colors.element_background)
                        .border(1.0, colors.border),
                )
                .child(label(format!("{radius:?} {}", radius.px())).size(LabelSize::Small)),
        );
    }
    row.into()
}

/// Cards on the editor background, each floating at its elevation.
struct Elevations;

const ELEVATION_CARD: (f32, f32) = (150.0, 84.0);

impl Story for Elevations {
    fn render(&mut self, width: f32, height: f32) -> Node {
        div()
            .w_px(width)
            .h_px(height)
            .bg(theme().editor_background)
            .into()
    }

    fn overlay(&mut self, area: ui::Rect) -> Option<ui::Painted> {
        let colors = theme();
        let mut painted = ui::Painted::default();
        for (index, elevation) in ui::Elevation::ALL.into_iter().enumerate() {
            let rect = ui::Rect::new(
                area.x + 24.0 + index as f32 * (ELEVATION_CARD.0 + 32.0),
                area.y + 28.0,
                ELEVATION_CARD.0,
                ELEVATION_CARD.1,
                Rgba::TRANSPARENT,
            );
            painted
                .rects
                .extend(ui::elevation_shadow(rect, elevation, 1.0));
            let card: Node = div()
                .w_px(rect.w)
                .h_px(rect.h)
                .p(Space::S12)
                .rounded(Radius::Large)
                .bg(colors.elevated_surface_background)
                .border(1.0, colors.border)
                .child(label(format!("{elevation:?}")).size(LabelSize::Small))
                .into();
            let card = ui::render(&card, rect);
            painted.rects.extend(card.rects);
            painted.texts.extend(card.texts);
        }
        Some(painted)
    }
}

fn motions() -> Node {
    let colors = theme();
    let mut column = div().col().gap(Space::S12).p(Space::S16);
    for motion in ui::Motion::ALL {
        let millis = motion.duration().as_millis() as f32;
        column = column.child(
            div()
                .row()
                .gap(Space::S12)
                .items_center()
                .child(
                    div()
                        .w_px(110.0)
                        .child(label(format!("{motion:?} {millis} ms")).size(LabelSize::Small)),
                )
                .child(
                    div()
                        .w_px(millis * 0.6)
                        .h_px(8.0)
                        .rounded(Radius::XSmall)
                        .bg(colors.text_accent),
                ),
        );
    }
    column.into()
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
        story("type-sizes", "Type", "Sizes", (460.0, 248.0), || {
            still(|_, _| sizes())
        }),
        story("type-weights", "Type", "Weights", (460.0, 176.0), || {
            still(|_, _| weights())
        }),
        story("type-colors", "Type", "Colors", (240.0, 176.0), || {
            still(|_, _| text_colors())
        }),
        story("spacing", "Spacing", "Steps", (220.0, 262.0), || {
            still(|_, _| spacings())
        }),
        story("radius", "Radius", "Steps", (480.0, 112.0), || {
            still(|_, _| radii())
        }),
        story("elevation", "Elevation", "Levels", (594.0, 160.0), || {
            Ok(Box::new(Elevations))
        }),
        story("motion", "Motion", "Durations", (420.0, 120.0), || {
            still(|_, _| motions())
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
