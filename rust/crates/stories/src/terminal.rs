//! The terminal dock of a project's window over a stand-in shell, and the bar over a side agent's console.

use std::time::Duration;

use terminal_ui::ConsoleToolbar;

use crate::fixtures::host::HostWindow;
use crate::windows::{cropped, Source, WINDOW};
use crate::{still, Story, StoryDef};

/// Long enough for the stand-in shell's output to arrive over the wire.
const SETTLING: Duration = Duration::from_millis(800);
const DOCK: (f32, f32, f32, f32) = (256.0, 512.0, 1178.0, 360.0);
const CENTER: (f32, f32, f32, f32) = (256.0, 40.0, 1178.0, 832.0);

fn panel(mode: &str, crop: (f32, f32, f32, f32)) -> anyhow::Result<Box<dyn Story>> {
    let window = HostWindow::open(mode, WINDOW, 1.0)?;
    cropped(Source::Host(window), crop, SETTLING)
}

fn side_bar(
    icon: ui::IconKind,
    role: &str,
    read_only: bool,
    context: &str,
) -> terminal_ui::SideAgentBar {
    terminal_ui::SideAgentBar::new(
        icon,
        role,
        read_only,
        context.into(),
        "agent:main".into(),
        Box::new(|| None),
    )
}

fn side_bars(width: f32, height: f32) -> ui::Node {
    let colors = ui::theme();
    let caption = |text: &str| {
        ui::label(text.to_string())
            .size(ui::LabelSize::Small)
            .color(colors.text_muted)
    };
    let rows = [
        (
            "Fix: can edit",
            side_bar(
                ui::IconKind::Wrench,
                "Fix",
                false,
                "Auto: fork + compact - 59k",
            ),
            width - 32.0,
        ),
        (
            "Review: read-only",
            side_bar(ui::IconKind::Search, "Review", true, "Fresh - packet"),
            width - 32.0,
        ),
        (
            "Ask in a narrow dock",
            side_bar(
                ui::IconKind::HelpCircle,
                "Ask",
                true,
                "Auto: fork + compact - 9.3k",
            ),
            420.0,
        ),
        (
            "Fix at its narrowest",
            side_bar(
                ui::IconKind::Wrench,
                "Fix",
                false,
                "Auto: fork + compact - 59k",
            ),
            300.0,
        ),
    ];
    ui::div()
        .col()
        .w_px(width)
        .h_px(height)
        .p(ui::Space::S16)
        .gap(ui::Space::S8)
        .children(rows.into_iter().map(|(title, bar, bar_width)| {
            ui::div()
                .col()
                .gap(ui::Space::S4)
                .child(caption(title))
                .child(
                    ui::div()
                        .w_px(bar_width)
                        .bg(colors.editor_background)
                        .child(bar.render(bar_width, 0)),
                )
                .into()
        }))
        .into()
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group,
        title,
        size,
        build,
    };
    let (_, _, dock_w, dock_h) = DOCK;
    let (_, _, center_w, center_h) = CENTER;
    vec![
        story(
            "terminal-panel",
            "Terminal panel",
            "A shell on a host",
            (dock_w, dock_h),
            || panel("terminal", DOCK),
        ),
        story(
            "terminal-panel-disconnected",
            "Terminal panel",
            "Kept while the host is down",
            (center_w, center_h),
            || panel("terminal-disconnected", CENTER),
        ),
        story(
            "terminal-side-agent-bar",
            "Side agent bar",
            "Roles and widths",
            (900.0, 300.0),
            || still(side_bars),
        ),
    ]
}
