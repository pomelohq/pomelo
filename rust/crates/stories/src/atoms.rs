//! The shared controls screens are built from, one state per board.

use components::{ButtonKind, ButtonSize};
use ui::{div, theme, Corner, IconKind, Node, Rgba};
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
        .p(16.0)
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
            .gap(12.0)
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
        .p(16.0)
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
                        .gap(8.0)
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
                            .gap(8.0)
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
                        .gap(12.0)
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
                        .p(8.0)
                        .rounded(6.0)
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
                        .gap(12.0)
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
                        .gap(12.0)
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
                        .gap(16.0)
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
    ]
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
        .rounded(4.0)
        .bg(colors.ghost_element_hover)
        .child(ui::icon(kind).size(14.0).color(colors.icon))
        .pin(Corner::TopLeft, 11.0, 9.0, POINTER, POINTER, pointer);
    div()
        .col()
        .p(16.0)
        .gap(4.0)
        .child(target)
        .child(div().row().child(components::tooltip_bubble(text, keys)))
        .into()
}

const POINTER: f32 = 18.0;
