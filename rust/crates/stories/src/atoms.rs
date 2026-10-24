//! The shared controls screens are built from, one state per board.

use ui::{div, theme, ButtonSize, IconKind, Node};
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

/// One kind at both sizes, so the two read side by side.
fn sized(kind: ButtonKind) -> Node {
    alone(
        div()
            .items_center()
            .gap(12.0)
            .child(workspace::button(ID, "Small").kind(kind))
            .child(
                workspace::button(ID + 1, "Medium")
                    .kind(kind)
                    .size(ButtonSize::Medium),
            ),
    )
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
                    workspace::button(ID, "Start all")
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
                        workspace::button(ID, "Create")
                            .kind(ButtonKind::Primary)
                            .size(ButtonSize::Medium)
                            .keystroke(Some("enter")),
                    )
                })
            },
        ),
        story("button-hovered", "Button", "Hovered", BUTTON, || {
            still(|_, _| alone(workspace::button(ID, "Restart").hovered(Some(ID))))
        }),
        story("button-disabled", "Button", "Disabled", BUTTON, || {
            still(|_, _| {
                alone(
                    div()
                        .items_center()
                        .gap(12.0)
                        .child(workspace::button(ID, "Restart").enabled(false))
                        .child(
                            workspace::button(ID + 1, "Create")
                                .kind(ButtonKind::Primary)
                                .enabled(false),
                        ),
                )
            })
        }),
        story("link", "Link", "Enabled", BUTTON, || {
            still(|_, _| {
                alone(workspace::accent_action(
                    ID,
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                    true,
                ))
            })
        }),
        story("link-disabled", "Link", "Disabled", BUTTON, || {
            still(|_, _| {
                alone(workspace::accent_action(
                    ID,
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                    false,
                ))
            })
        }),
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
