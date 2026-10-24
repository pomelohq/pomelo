//! The shared controls screens are built from, one state per board.

use ui::{div, theme, ButtonSize, ButtonStyle, IconKind, Node};
use workspace::{ButtonKind, InputField};

use crate::{still, Story, StoryDef};

const PAGE: &str = "Atoms";
const BUTTON: (f32, f32) = (220.0, 72.0);
const CONTROL: (f32, f32) = (260.0, 72.0);
const INPUT: (f32, f32) = (320.0, 108.0);
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

fn styled(style: ButtonStyle) -> Node {
    alone(
        div()
            .items_center()
            .gap(12.0)
            .child(ui::button(ID, "Default", style))
            .child(ui::button_sized(
                ID + 1,
                "Medium",
                style,
                ButtonSize::Medium,
            )),
    )
}

fn small(kind: ButtonKind) -> Node {
    alone(workspace::small_button(
        ID, None, "Restart", None, kind, true, None,
    ))
}

fn modal(kind: ButtonKind, keystroke: Option<&str>, enabled: bool) -> Node {
    alone(workspace::modal_button(
        ID, "Create", keystroke, kind, enabled,
    ))
}

fn input(
    field: InputField,
    text: &str,
    focused: bool,
    hint: Option<&str>,
    error: Option<&str>,
) -> Node {
    let mut field = field;
    field.field.set_text(text);
    field.field.move_to_end();
    div()
        .col()
        .w_px(INPUT.0)
        .p(16.0)
        .child(field.render(ID, focused, hint, error))
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
        story("button-filled", "Button", "Filled", BUTTON, || {
            still(|_, _| styled(ButtonStyle::Filled))
        }),
        story("button-outlined", "Button", "Outlined", BUTTON, || {
            still(|_, _| styled(ButtonStyle::Outlined))
        }),
        story(
            "button-outlined-ghost",
            "Button",
            "Outlined ghost",
            BUTTON,
            || still(|_, _| styled(ButtonStyle::OutlinedGhost)),
        ),
        story("button-subtle", "Button", "Subtle", BUTTON, || {
            still(|_, _| styled(ButtonStyle::Subtle))
        }),
        story("button-tinted", "Button", "Tinted accent", BUTTON, || {
            still(|_, _| styled(ButtonStyle::TintedAccent))
        }),
        story(
            "small-button-primary",
            "Small button",
            "Primary",
            BUTTON,
            || still(|_, _| small(ButtonKind::Primary)),
        ),
        story(
            "small-button-secondary",
            "Small button",
            "Secondary",
            BUTTON,
            || still(|_, _| small(ButtonKind::Secondary)),
        ),
        story(
            "small-button-ghost",
            "Small button",
            "Ghost",
            BUTTON,
            || still(|_, _| small(ButtonKind::Ghost)),
        ),
        story(
            "small-button-danger",
            "Small button",
            "Danger",
            BUTTON,
            || still(|_, _| small(ButtonKind::Danger)),
        ),
        story(
            "small-button-icon-key",
            "Small button",
            "Icon and key",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::small_button(
                        ID,
                        Some(IconKind::Play),
                        "Start all",
                        Some("cmd-shift-s"),
                        ButtonKind::Secondary,
                        true,
                        None,
                    ))
                })
            },
        ),
        story(
            "small-button-hovered",
            "Small button",
            "Hovered",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::small_button(
                        ID,
                        None,
                        "Restart",
                        None,
                        ButtonKind::Secondary,
                        true,
                        Some(ID),
                    ))
                })
            },
        ),
        story(
            "small-button-disabled",
            "Small button",
            "Disabled",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::small_button(
                        ID,
                        None,
                        "Restart",
                        None,
                        ButtonKind::Secondary,
                        false,
                        None,
                    ))
                })
            },
        ),
        story(
            "modal-button-primary",
            "Modal button",
            "Primary",
            BUTTON,
            || still(|_, _| modal(ButtonKind::Primary, Some("cmd-enter"), true)),
        ),
        story(
            "modal-button-secondary",
            "Modal button",
            "Secondary",
            BUTTON,
            || still(|_, _| modal(ButtonKind::Secondary, Some("escape"), true)),
        ),
        story(
            "modal-button-danger",
            "Modal button",
            "Danger",
            BUTTON,
            || still(|_, _| modal(ButtonKind::Danger, None, true)),
        ),
        story(
            "modal-button-disabled",
            "Modal button",
            "Disabled",
            BUTTON,
            || still(|_, _| modal(ButtonKind::Primary, Some("cmd-enter"), false)),
        ),
        story(
            "outlined-button",
            "Outlined button",
            "Enabled",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::outlined_button(
                        ID,
                        Some(IconKind::Plus),
                        "Add repo",
                        true,
                    ))
                })
            },
        ),
        story(
            "outlined-button-disabled",
            "Outlined button",
            "Disabled",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::outlined_button(
                        ID,
                        Some(IconKind::Plus),
                        "Add repo",
                        false,
                    ))
                })
            },
        ),
        story("accent-action", "Accent action", "Enabled", BUTTON, || {
            still(|_, _| {
                alone(workspace::accent_action(
                    ID,
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                    true,
                ))
            })
        }),
        story(
            "accent-action-disabled",
            "Accent action",
            "Disabled",
            BUTTON,
            || {
                still(|_, _| {
                    alone(workspace::accent_action(
                        ID,
                        Some(IconKind::ArrowUpRight),
                        "Open in browser",
                        false,
                    ))
                })
            },
        ),
        story("checkbox-checked", "Checkbox", "Checked", CONTROL, || {
            still(|_, _| alone(workspace::checkbox(ID, true, "Start with the workspace")))
        }),
        story(
            "checkbox-unchecked",
            "Checkbox",
            "Unchecked",
            CONTROL,
            || still(|_, _| alone(workspace::checkbox(ID, false, "Start with the workspace"))),
        ),
        story("segmented", "Segmented", "First chosen", CONTROL, || {
            still(|_, _| {
                alone(workspace::segmented(
                    &[(ID, "All"), (ID + 1, "Running"), (ID + 2, "Failed")],
                    0,
                    None,
                ))
            })
        }),
        story("segmented-hovered", "Segmented", "Hovered", CONTROL, || {
            still(|_, _| {
                alone(workspace::segmented(
                    &[(ID, "All"), (ID + 1, "Running"), (ID + 2, "Failed")],
                    0,
                    Some(ID + 2),
                ))
            })
        }),
        story(
            "toggle-group",
            "Toggle group",
            "With icons",
            CONTROL,
            || {
                still(|_, _| {
                    alone(workspace::toggle_button_group(
                        &[
                            (ID, Some(IconKind::Monitor), "This Mac"),
                            (ID + 1, Some(IconKind::Server), "Host"),
                        ],
                        0,
                    ))
                })
            },
        ),
        story("progress-start", "Progress", "Starting", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(workspace::progress_bar(0.1))))
        }),
        story("progress-half", "Progress", "Halfway", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(workspace::progress_bar(0.6))))
        }),
        story("progress-done", "Progress", "Done", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(workspace::progress_bar(1.0))))
        }),
        story(
            "status-pill-running",
            "Status pill",
            "Running",
            BUTTON,
            || still(|_, _| alone(workspace::status_pill("Running", theme().success))),
        ),
        story(
            "status-pill-starting",
            "Status pill",
            "Starting",
            BUTTON,
            || still(|_, _| alone(workspace::status_pill("Starting", theme().warning))),
        ),
        story(
            "status-pill-crashed",
            "Status pill",
            "Crashed",
            BUTTON,
            || still(|_, _| alone(workspace::status_pill("Crashed", theme().error))),
        ),
        story(
            "status-pill-stopped",
            "Status pill",
            "Stopped",
            BUTTON,
            || still(|_, _| alone(workspace::status_pill("Stopped", theme().text_muted))),
        ),
        story("status-line-ok", "Status line", "Done", CONTROL, || {
            still(|_, _| {
                alone(workspace::status_line(
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
                    alone(workspace::status_line(
                        IconKind::Warning,
                        theme().error,
                        "Could not reach dev",
                    ))
                })
            },
        ),
        story("key-chip", "Key chip", "On a panel", BUTTON, || {
            still(|_, _| alone(workspace::key_chip("cmd-shift-p", false)))
        }),
        story("key-chip-accent", "Key chip", "On accent", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .p(8.0)
                        .rounded(6.0)
                        .bg(theme().accent_fill)
                        .child(workspace::key_chip("cmd-enter", true)),
                )
            })
        }),
        story("divider", "Divider", "Default", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(ui::divider())))
        }),
        story("divider-faded", "Divider", "Faded", CONTROL, || {
            still(|_, _| alone(div().col().w_px(200.0).child(ui::divider_faded())))
        }),
        story("input-empty", "Input", "Empty", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Branch", "feat-login"),
                    "",
                    false,
                    Some("A new worktree for every repo"),
                    None,
                )
            })
        }),
        story("input-focused", "Input", "Focused", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Branch", "feat-login"),
                    "feat-signup",
                    true,
                    None,
                    None,
                )
            })
        }),
        story("input-error", "Input", "Error", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Branch", "feat-login"),
                    "feat login",
                    false,
                    None,
                    Some("A branch name has no spaces"),
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
                    None,
                    None,
                )
            })
        }),
        story("input-masked", "Input", "Masked", INPUT, || {
            still(|_, _| {
                input(
                    InputField::new("Token", "Paste a token").masked(),
                    "abcdefgh",
                    false,
                    None,
                    None,
                )
            })
        }),
    ]
}
