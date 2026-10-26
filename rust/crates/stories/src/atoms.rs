//! The shared controls screens are built from, one state per board.

use components::{ButtonKind, ButtonSize};
use ui::{div, theme, Corner, IconKind, LabelSize, Node, Radius, Rgba, Space};
use workspace::InputField;

use crate::{still, Story, StoryDef};

const PAGE: &str = "Atoms";
const BUTTON: (f32, f32) = (220.0, 72.0);
const CONTROL: (f32, f32) = (260.0, 72.0);
const INPUT: (f32, f32) = (320.0, 108.0);
const TOOLTIP: (f32, f32) = (260.0, 96.0);
const ID: u64 = 1;

/// A control on its own, centered with room around it.
fn alone(control: impl Into<Node>) -> Node {
    div()
        .p(Space::S16)
        .items_center()
        .justify_center()
        .child(control.into())
        .into()
}

/// One kind at both sizes, so the two read side by side.
fn sized(kind: ButtonKind) -> Node {
    alone(
        div()
            .items_center()
            .gap(Space::S12)
            .child(components::button(ID, "Small").kind(kind))
            .child(
                components::button(ID + 1, "Medium")
                    .kind(kind)
                    .size(ButtonSize::Medium),
            ),
    )
}

fn input(field: InputField, text: &str, focused: bool, error: bool) -> Node {
    let mut field = field;
    field.field.set_text(text);
    field.field.move_to_end();
    div()
        .col()
        .w_px(INPUT.0)
        .p(Space::S16)
        .child(field.render_input(ID, focused, error))
        .into()
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, size, build: fn() -> anyhow::Result<Box<dyn Story>>| StoryDef {
        id,
        page: PAGE,
        group,
        title,
        size,
        build,
    };
    vec![
        story("button-primary", "Button", "Primary", BUTTON, || {
            still(|_, _| sized(ButtonKind::Primary))
        }),
        story("button-secondary", "Button", "Secondary", BUTTON, || {
            still(|_, _| sized(ButtonKind::Secondary))
        }),
        story("button-ghost", "Button", "Ghost", BUTTON, || {
            still(|_, _| sized(ButtonKind::Ghost))
        }),
        story("button-danger", "Button", "Danger", BUTTON, || {
            still(|_, _| sized(ButtonKind::Danger))
        }),
        story("button-icon-key", "Button", "Icon and key", BUTTON, || {
            still(|_, _| {
                alone(
                    components::button(ID, "Start all")
                        .icon(Some(IconKind::Play))
                        .keystroke(Some("cmd-shift-s")),
                )
            })
        }),
        story(
            "button-primary-key",
            "Button",
            "Primary with key",
            BUTTON,
            || {
                still(|_, _| {
                    alone(
                        components::button(ID, "Create")
                            .kind(ButtonKind::Primary)
                            .size(ButtonSize::Medium)
                            .keystroke(Some("enter")),
                    )
                })
            },
        ),
        story("button-agent", "Button", "Agent", BUTTON, || {
            still(|_, _| {
                alone(
                    components::button(ID, "Fix with Claude")
                        .kind(ButtonKind::Agent)
                        .icon(Some(IconKind::Sparkle)),
                )
            })
        }),
        story("icon-button", "Icon button", "Sizes", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .row()
                        .items_center()
                        .gap(Space::S8)
                        .child(components::icon_button(ID, IconKind::Copy).size(ButtonSize::XSmall))
                        .child(components::icon_button(ID + 1, IconKind::RotateCw))
                        .child(
                            components::icon_button(ID + 2, IconKind::Sliders)
                                .size(ButtonSize::Medium),
                        ),
                )
            })
        }),
        story(
            "icon-button-states",
            "Icon button",
            "Hovered, on and off",
            BUTTON,
            || {
                still(|_, _| {
                    alone(
                        div()
                            .row()
                            .items_center()
                            .gap(Space::S8)
                            .child(
                                components::icon_button(ID, IconKind::RotateCw).hovered(Some(ID)),
                            )
                            .child(
                                components::icon_button(ID + 1, IconKind::CaseSensitive)
                                    .toggled(true),
                            )
                            .child(components::icon_button(ID + 2, IconKind::Trash).enabled(false)),
                    )
                })
            },
        ),
        story("button-hovered", "Button", "Hovered", BUTTON, || {
            still(|_, _| alone(components::button(ID, "Restart").hovered(Some(ID))))
        }),
        story("button-disabled", "Button", "Disabled", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .items_center()
                        .gap(Space::S12)
                        .child(components::button(ID, "Restart").enabled(false))
                        .child(
                            components::button(ID + 1, "Create")
                                .kind(ButtonKind::Primary)
                                .enabled(false),
                        ),
                )
            })
        }),
        story("link", "Link", "Enabled", BUTTON, || {
            still(|_, _| {
                alone(components::link(
                    ID,
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                    true,
                ))
            })
        }),
        story("link-disabled", "Link", "Disabled", BUTTON, || {
            still(|_, _| {
                alone(components::link(
                    ID,
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                    false,
                ))
            })
        }),
        story("select-chosen", "Select", "Chosen", CONTROL, || {
            still(|_, _| alone(components::select(ID, "Pomelo Dark").min_width(188.0)))
        }),
        story("select-empty", "Select", "Nothing chosen", CONTROL, || {
            still(|_, _| {
                alone(
                    components::select(ID, "")
                        .placeholder("Select database")
                        .min_width(188.0),
                )
            })
        }),
        story(
            "select-icon-small",
            "Select",
            "Small with icon",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        components::select(ID, "main")
                            .icon(Some(IconKind::Cylinder))
                            .size(ButtonSize::Small),
                    )
                })
            },
        ),
        story("select-hovered", "Select", "Hovered", CONTROL, || {
            still(|_, _| {
                alone(
                    components::select(ID, "Pomelo Dark")
                        .min_width(188.0)
                        .hovered(Some(ID)),
                )
            })
        }),
        story(
            "select-cut",
            "Select",
            "Long value, fixed width",
            CONTROL,
            || {
                still(|_, _| {
                    alone(components::select(ID, "api/.env.development.local").width(160.0))
                })
            },
        ),
        story("select-disabled", "Select", "Disabled", CONTROL, || {
            still(|_, _| {
                alone(
                    components::select(ID, "Pomelo Dark")
                        .min_width(188.0)
                        .enabled(false),
                )
            })
        }),
        story("checkbox-checked", "Checkbox", "Checked", CONTROL, || {
            still(|_, _| alone(components::checkbox(ID, true, "Start with the workspace")))
        }),
        story(
            "checkbox-unchecked",
            "Checkbox",
            "Unchecked",
            CONTROL,
            || still(|_, _| alone(components::checkbox(ID, false, "Start with the workspace"))),
        ),
        story("segmented", "Segmented", "On a toolbar", CONTROL, || {
            still(|_, _| alone(filters(None)))
        }),
        story("segmented-hovered", "Segmented", "Hovered", CONTROL, || {
            still(|_, _| alone(filters(Some(ID + 2))))
        }),
        story(
            "segmented-form",
            "Segmented",
            "In a form, with icons",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        components::segmented(
                            [
                                components::segment(ID, "This Mac").icon(Some(IconKind::Monitor)),
                                components::segment(ID + 1, "Host").icon(Some(IconKind::Server)),
                            ],
                            0,
                        )
                        .size(ButtonSize::Medium),
                    )
                })
            },
        ),
        story(
            "segmented-disabled",
            "Segmented",
            "One unavailable",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        components::segmented(
                            [
                                components::segment(ID, "Claude Code"),
                                components::segment(ID + 1, "Codex").enabled(false),
                                components::segment(ID + 2, "Gemini CLI"),
                            ],
                            0,
                        )
                        .size(ButtonSize::Medium),
                    )
                })
            },
        ),
        story("progress-start", "Progress", "Starting", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(components::progress_bar(0.1))))
        }),
        story("progress-half", "Progress", "Halfway", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(components::progress_bar(0.6))))
        }),
        story("progress-done", "Progress", "Done", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(components::progress_bar(1.0))))
        }),
        story("status-pill-running", "Pill", "Running", BUTTON, || {
            still(|_, _| alone(components::pill("Running").color(theme().success)))
        }),
        story("status-pill-starting", "Pill", "Starting", BUTTON, || {
            still(|_, _| alone(components::pill("Starting").color(theme().warning)))
        }),
        story("status-pill-crashed", "Pill", "Crashed", BUTTON, || {
            still(|_, _| alone(components::pill("Crashed").color(theme().error)))
        }),
        story("status-pill-stopped", "Pill", "Stopped", BUTTON, || {
            still(|_, _| alone(components::pill("Stopped").color(theme().text_muted)))
        }),
        story(
            "status-pill-waiting",
            "Pill",
            "Waiting on you",
            BUTTON,
            || still(|_, _| alone(components::pill("Waiting for you").attention())),
        ),
        story("status-line-ok", "Status line", "Done", CONTROL, || {
            still(|_, _| {
                alone(components::status_line(
                    IconKind::Check,
                    theme().success,
                    "Connected to dev",
                ))
            })
        }),
        story(
            "status-line-error",
            "Status line",
            "Failed",
            CONTROL,
            || {
                still(|_, _| {
                    alone(components::status_line(
                        IconKind::Warning,
                        theme().error,
                        "Could not reach dev",
                    ))
                })
            },
        ),
        story("key-chip", "Key chip", "On a panel", BUTTON, || {
            still(|_, _| alone(components::key_chip("cmd-shift-p", false)))
        }),
        story("key-chip-accent", "Key chip", "On accent", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .p(Space::S8)
                        .rounded(Radius::Medium)
                        .bg(theme().accent_fill)
                        .child(components::key_chip("cmd-enter", true)),
                )
            })
        }),
        story("divider", "Divider", "Between sections", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(components::divider())))
        }),
        story("chip", "Chip", "Default", BUTTON, || {
            still(|_, _| alone(components::chip("feat-login")))
        }),
        story("chip-muted", "Chip", "Muted", BUTTON, || {
            still(|_, _| alone(components::chip("docker compose").color(theme().text_muted)))
        }),
        story("chip-icon", "Chip", "With icon", BUTTON, || {
            still(|_, _| {
                alone(
                    components::chip("main")
                        .icon(IconKind::Branch)
                        .color(theme().text_muted),
                )
            })
        }),
        story("chip-tinted", "Chip", "Tinted", BUTTON, || {
            still(|_, _| {
                let accent = theme().text_accent;
                alone(
                    components::chip("Pro")
                        .color(accent)
                        .fill(accent.alpha(0.14))
                        .border(accent.alpha(0.3)),
                )
            })
        }),
        story("tooltip", "Tooltip", "Text", TOOLTIP, || {
            still(|_, _| hovered_with_tooltip(IconKind::ArrowUpRight, "Open in Browser", None))
        }),
        story("tooltip-keys", "Tooltip", "With keys", TOOLTIP, || {
            still(|_, _| hovered_with_tooltip(IconKind::Sidebar, "Toggle Left Dock", Some("cmd-b")))
        }),
        story("dot-states", "Dot", "States", CONTROL, || {
            still(|_, _| {
                let colors = theme();
                alone(
                    div()
                        .items_center()
                        .gap(Space::S12)
                        .child(components::dot(colors.success, 7.0))
                        .child(components::dot(colors.warning, 7.0))
                        .child(components::dot(colors.error, 7.0))
                        .child(components::dot(colors.text_accent, 7.0))
                        .child(components::ring(colors.icon_muted, 8.0)),
                )
            })
        }),
        story("dot-sizes", "Dot", "Sizes", CONTROL, || {
            still(|_, _| {
                let color = theme().success;
                alone(
                    div()
                        .items_center()
                        .gap(Space::S12)
                        .child(components::dot(color, 6.0))
                        .child(components::dot(color, 7.0))
                        .child(components::dot(color, 8.0)),
                )
            })
        }),
        story("input-empty", "Input", "Empty", INPUT, || {
            still(|_, _| input(InputField::new("Branch", "feat-login"), "", false, false))
        }),
        story("input-focused", "Input", "Focused", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Branch", "feat-login"),
                    "feat-signup",
                    true,
                    false,
                )
            })
        }),
        story("input-error", "Input", "Error", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Branch", "feat-login"),
                    "feat login",
                    false,
                    true,
                )
            })
        }),
        story("input-mono-icon", "Input", "Mono with icon", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Command", "npm run dev")
                        .mono()
                        .with_icon(IconKind::Terminal),
                    "bin/rails server -p $PORT",
                    false,
                    false,
                )
            })
        }),
        story("input-masked", "Input", "Masked", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Token", "Paste a token").masked(),
                    "abcdefgh",
                    false,
                    false,
                )
            })
        }),
        story("switch-on", "Switch", "On", BUTTON, || {
            still(|_, _| alone(components::switch(ID, true)))
        }),
        story("switch-off", "Switch", "Off", BUTTON, || {
            still(|_, _| alone(components::switch(ID, false)))
        }),
        story("stepper-value", "Stepper", "A value", CONTROL, || {
            still(|_, _| alone(components::stepper("15", ID, ID + 1, ID + 2)))
        }),
        story(
            "stepper-editing",
            "Stepper",
            "Typing a value",
            CONTROL,
            || {
                still(|_, _| {
                    alone(components::stepper("15", ID, ID + 1, ID + 2).editing(Some("30")))
                })
            },
        ),
        story("radio-choices", "Radio", "Chosen and not", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .row()
                        .gap(Space::S16)
                        .items_center()
                        .child(components::radio(true))
                        .child(components::radio(false)),
                )
            })
        }),
        story(
            "progress-thin",
            "Progress",
            "Thin, inside a button",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        div()
                            .row()
                            .w_px(46.0)
                            .child(components::progress_bar(0.4).thin()),
                    )
                })
            },
        ),
        story(
            "status-line-tinted",
            "Status line",
            "Tinted, the line is the news",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        div().row().w_px(220.0).child(
                            components::status_line(
                                IconKind::Warning,
                                theme().error,
                                "Install failed",
                            )
                            .tinted(),
                        ),
                    )
                })
            },
        ),
        story("pill-icon", "Pill", "With its own mark", BUTTON, || {
            still(|_, _| {
                alone(
                    components::pill("Merged")
                        .color(theme().text_accent)
                        .icon(IconKind::Merged),
                )
            })
        }),
        story(
            "select-quiet",
            "Select",
            "Quiet, inside a line",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        components::select(ID, "Current sprint")
                            .size(ButtonSize::XSmall)
                            .quiet(),
                    )
                })
            },
        ),
        story(
            "checkbox-partial",
            "Checkbox",
            "Some of it on",
            CONTROL,
            || still(|_, _| alone(components::checkbox(ID, false, "Stage api").partial(true))),
        ),
        story(
            "progress-meter",
            "Progress",
            "A usage meter",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        components::progress_bar(0.82)
                            .height(6.0)
                            .width(120.0)
                            .color(theme().warning),
                    )
                })
            },
        ),
        story(
            "divider-vertical",
            "Divider",
            "Between groups in a row",
            CONTROL,
            || {
                still(|_, _| {
                    alone(
                        div()
                            .row()
                            .gap(Space::S8)
                            .items_center()
                            .child(
                                ui::label("Ln 12, Col 4")
                                    .size(LabelSize::Small)
                                    .color(theme().text),
                            )
                            .child(components::vertical_divider(14.0))
                            .child(
                                ui::label("Rust")
                                    .size(LabelSize::Small)
                                    .color(theme().text_muted),
                            ),
                    )
                })
            },
        ),
        story(
            "status-item",
            "Status item",
            "On the status bar",
            (320.0, 60.0),
            || {
                still(|_, _| {
                    let colors = theme();
                    alone(
                        div()
                            .row()
                            .gap(Space::S6)
                            .items_center()
                            .child(
                                components::status_item(Some(ID))
                                    .badged_icon(
                                        IconKind::Server,
                                        colors.icon_accent,
                                        Some(colors.success),
                                    )
                                    .width(26.0),
                            )
                            .child(
                                components::status_item(Some(ID + 1))
                                    .icon(IconKind::Terminal, colors.icon_muted)
                                    .width(26.0)
                                    .hovered(true),
                            )
                            .child(components::vertical_divider(14.0))
                            .child(
                                components::status_item(Some(ID + 2))
                                    .text("Ln 12, Col 4", colors.text),
                            )
                            .child(components::status_item(None).mono("NORMAL", colors.text_muted)),
                    )
                })
            },
        ),
        story(
            "split-button",
            "Split button",
            "Secondary",
            (260.0, 72.0),
            || still(|_, _| alone(components::split_button(ID, ID + 1, "Stage All"))),
        ),
        story(
            "split-button-primary",
            "Split button",
            "Primary, with a note",
            (300.0, 72.0),
            || {
                still(|_, _| {
                    alone(
                        components::split_button(ID, ID + 1, "Commit")
                            .note(Some("+amend"))
                            .primary()
                            .size(ButtonSize::Medium),
                    )
                })
            },
        ),
        story(
            "split-button-disabled",
            "Split button",
            "Disabled",
            (260.0, 72.0),
            || {
                still(|_, _| {
                    alone(
                        components::split_button(ID, ID + 1, "Sync")
                            .primary()
                            .enabled(false),
                    )
                })
            },
        ),
        story("tab-active", "Tab", "Active", (260.0, 64.0), || {
            still(|_, _| alone(tab("main.rs").active(true)))
        }),
        story("tab-idle", "Tab", "Not active", (260.0, 64.0), || {
            still(|_, _| alone(tab("main.rs")))
        }),
        story("tab-hovered", "Tab", "Hovered", (260.0, 64.0), || {
            still(|_, _| alone(tab("main.rs").hovered(true)))
        }),
        story("tab-unsaved", "Tab", "Unsaved", (260.0, 64.0), || {
            still(|_, _| alone(tab("main.rs").unsaved(Some(theme().text_muted))))
        }),
        story(
            "tab-pinned-preview",
            "Tab",
            "Pinned, and a preview",
            (360.0, 64.0),
            || {
                still(|_, _| {
                    alone(
                        div()
                            .row()
                            .gap(Space::S4)
                            .child(tab("pom.yml").pinned(true))
                            .child(tab("README.md").preview(true)),
                    )
                })
            },
        ),
        story(
            "tab-floating",
            "Tab",
            "Carried while dragged",
            (260.0, 64.0),
            || still(|_, _| alone(tab("main.rs").floating())),
        ),
        story("paragraph", "Paragraph", "Wrapped", (320.0, 100.0), || {
            still(|_, _| {
                alone(components::paragraph(
                    "Shared services of feat-login are forwarded once they run. Forward any other port of dev above.",
                    260.0,
                ))
            })
        }),
        story(
            "paragraph-centered",
            "Paragraph",
            "Centered",
            (320.0, 100.0),
            || {
                still(|_, _| {
                    alone(
                        components::paragraph(
                            "Requests the frontend sends through the proxy show up here.",
                            220.0,
                        )
                        .centered(),
                    )
                })
            },
        ),
        story("avatar", "Avatar", "A person", (200.0, 60.0), || {
            still(|_, _| {
                alone(
                    div()
                        .row()
                        .gap(Space::S8)
                        .items_center()
                        .child(components::avatar("Ada Lovelace"))
                        .child(components::avatar("grace"))
                        .child(components::avatar("Linus")),
                )
            })
        }),
        story(
            "skeleton",
            "Skeleton",
            "Loading rows",
            (320.0, 100.0),
            || {
                still(|_, _| {
                    alone(
                        div()
                            .col()
                            .gap(Space::S8)
                            .child(components::skeleton(240.0, 1.4, 0.0))
                            .child(components::skeleton(180.0, 1.4, 0.15))
                            .child(components::skeleton(210.0, 1.4, 0.3)),
                    )
                })
            },
        ),
        story("key-value", "Key value", "Facts", (360.0, 110.0), || {
            still(|_, _| {
                alone(
                    div()
                        .col()
                        .gap(Space::S4)
                        .w_px(320.0)
                        .child(components::key_value(
                            components::key_label("Branch"),
                            ui::label("feat-login")
                                .size(LabelSize::Small)
                                .mono()
                                .color(theme().text),
                            None,
                        ))
                        .child(components::key_value(
                            components::key_label("Port"),
                            ui::label("3000")
                                .size(LabelSize::Small)
                                .mono()
                                .color(theme().text),
                            Some("forwarded"),
                        )),
                )
            })
        }),
        story(
            "key-hints",
            "Key hints",
            "Picker footer",
            (420.0, 60.0),
            || {
                still(|_, _| {
                    alone(
                        components::key_hints()
                            .hint(None, &["enter"], "Open")
                            .hint(Some(ID), &["cmd", "enter"], "Open in split")
                            .hint(None, &["esc"], "Close"),
                    )
                })
            },
        ),
    ]
}

fn tab(title: &str) -> components::Tab {
    components::tab(ID, title)
        .glyph(ui::material_icon(ui::MaterialIcon::Document).size(14.0))
        .close(ID + 1)
}

fn filters(hovered: Option<u64>) -> components::Segmented {
    components::segmented(
        [
            components::segment(ID, "All"),
            components::segment(ID + 1, "Running"),
            components::segment(ID + 2, "Failed"),
        ],
        0,
    )
    .hovered(hovered)
}

/// A tooltip only shows while the pointer rests on an icon, so the board draws all three.
fn hovered_with_tooltip(kind: IconKind, text: &str, keys: Option<&str>) -> Node {
    let colors = theme();
    let pointer = div()
        .w_px(POINTER)
        .h_px(POINTER)
        .pin(
            Corner::TopLeft,
            0.0,
            0.0,
            POINTER,
            POINTER,
            ui::icon(IconKind::PointerEdge)
                .size(POINTER)
                .color(Rgba::new(1.0, 1.0, 1.0, 1.0)),
        )
        .pin(
            Corner::TopLeft,
            0.0,
            0.0,
            POINTER,
            POINTER,
            ui::icon(IconKind::Pointer)
                .size(POINTER)
                .color(Rgba::new(0.0, 0.0, 0.0, 1.0)),
        );
    let target = div()
        .row()
        .w_px(24.0)
        .h_px(24.0)
        .items_center()
        .justify_center()
        .rounded(Radius::Small)
        .bg(colors.ghost_element_hover)
        .child(ui::icon(kind).size(14.0).color(colors.icon))
        .pin(Corner::TopLeft, 11.0, 9.0, POINTER, POINTER, pointer);
    div()
        .col()
        .p(Space::S16)
        .gap(Space::S4)
        .child(target)
        .child(div().row().child(components::tooltip_bubble(text, keys)))
        .into()
}

const POINTER: f32 = 18.0;
