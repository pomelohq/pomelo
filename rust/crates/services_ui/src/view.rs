//! Drawing pieces of the Services panel that carry no state of their own.

use ui::{div, icon, label, measure, theme, IconKind, Node, Rgba};

/// A row's standing, as its icon and trailing text show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Running,
    Busy(&'static str),
    Crashed,
    /// A start that failed; `port` when it could not bind that port.
    Failed {
        port: Option<u16>,
    },
    Stopped,
}

impl State {
    pub(crate) fn needs_attention(&self) -> bool {
        matches!(self, State::Crashed | State::Failed { .. })
    }
}

pub(crate) fn ai_tint() -> Rgba {
    theme().terminal_ansi[5]
}

pub(crate) fn status_icon(state: &State) -> Node {
    let colors = theme();
    let inner: Node = match state {
        State::Running => div()
            .w_px(8.0)
            .h_px(8.0)
            .rounded(4.0)
            .bg(colors.success)
            .into(),
        State::Busy(_) => icon(IconKind::RotateCw)
            .size(11.0)
            .color(colors.text_accent)
            .into(),
        State::Crashed | State::Failed { port: None } => icon(IconKind::XCircle)
            .size(12.0)
            .color(colors.error)
            .into(),
        State::Failed { port: Some(_) } => icon(IconKind::Warning)
            .size(12.0)
            .color(colors.warning)
            .into(),
        State::Stopped => div()
            .w_px(8.0)
            .h_px(8.0)
            .rounded(4.0)
            .border(1.5, colors.icon_muted)
            .into(),
    };
    div()
        .row()
        .w_px(14.0)
        .h_px(14.0)
        .items_center()
        .justify_center()
        .child(inner)
        .into()
}

/// A small dot in `color`, for legends and repo pips.
pub(crate) fn dot(color: Rgba, size: f32) -> Node {
    div()
        .w_px(size)
        .h_px(size)
        .rounded(size / 2.0)
        .bg(color)
        .into()
}

pub(crate) fn state_color(state: &State) -> Rgba {
    let colors = theme();
    match state {
        State::Running => colors.success,
        State::Busy(_) => colors.text_accent,
        State::Crashed | State::Failed { port: None } => colors.error,
        State::Failed { port: Some(_) } => colors.warning,
        State::Stopped => colors.border,
    }
}

/// A mono tag with a thin border (a service's mode).
pub(crate) fn tag(text: &str) -> Node {
    div()
        .row()
        .h_px(16.0)
        .px(4.0)
        .items_center()
        .rounded(3.0)
        .border(1.0, theme().border_variant)
        .child(
            label(text.to_string())
                .size(10.5)
                .mono()
                .color(theme().text_placeholder),
        )
        .into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Plain,
    Primary,
    Agent,
}

/// A small labeled button, optionally with an icon before the label.
pub(crate) fn action_button(
    id: u64,
    kind: Option<IconKind>,
    text: &str,
    tone: Tone,
    hot: bool,
) -> Node {
    let colors = theme();
    let (bg, border, icon_color) = match tone {
        Tone::Plain => (
            colors.element_background,
            colors.border_variant,
            colors.icon_muted,
        ),
        Tone::Primary => (colors.info_background, colors.info_border, colors.icon),
        Tone::Agent => (ai_tint().alpha(0.12), ai_tint().alpha(0.55), ai_tint()),
    };
    let mut button = div()
        .row()
        .h_px(22.0)
        .px(if text.is_empty() { 5.0 } else { 7.0 })
        .gap(4.0)
        .items_center()
        .rounded(4.0)
        .bg(if hot { colors.element_hover } else { bg })
        .border(1.0, border)
        .on_click(id);
    if let Some(kind) = kind {
        button = button.child(icon(kind).size(11.0).color(icon_color));
    }
    if !text.is_empty() {
        button = button.child(label(text.to_string()).size(11.5).color(colors.text));
    }
    button.into()
}

/// Lays `nodes` out left to right, starting a new line when the next would pass `width`.
pub(crate) fn pack(nodes: Vec<Node>, width: f32, gap: f32) -> Node {
    let mut column = div().col().gap(gap);
    let mut line = div().row().gap(gap).items_center();
    let mut used = 0.0;
    for node in nodes {
        let node_width = measure(&node).0;
        if used > 0.0 && used + gap + node_width > width {
            column = column.child(line);
            line = div().row().gap(gap).items_center();
            used = 0.0;
        }
        used += if used > 0.0 {
            gap + node_width
        } else {
            node_width
        };
        line = line.child(node);
    }
    if used > 0.0 {
        column = column.child(line);
    }
    column.into()
}

/// An uppercase section title with an optional count at the right.
pub(crate) fn section(title: &str, count: Option<usize>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .h_px(26.0)
        .pl(10.0)
        .pr(10.0)
        .items_center()
        .child(
            div().row().flex(1.0).items_center().child(
                label(title.to_uppercase())
                    .size(11.0)
                    .weight(600)
                    .color(colors.text_placeholder),
            ),
        );
    if let Some(count) = count {
        row = row.child(
            label(count.to_string())
                .size(11.0)
                .color(colors.text_placeholder),
        );
    }
    row.into()
}

/// How long the holder whose pidfile is `pidfile` has been running: "41m 58s", "2h 5m".
pub(crate) fn uptime(pidfile: &std::path::Path) -> Option<String> {
    let started = std::fs::metadata(pidfile)
        .and_then(|meta| meta.modified())
        .ok()?;
    let seconds = started.elapsed().ok()?.as_secs();
    Some(match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
        _ => format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600),
    })
}

/// `node` with the panel's side margin and a gap below it.
pub(crate) fn inset(node: Node) -> Node {
    div().row().px(8.0).pb(6.0).child(node).into()
}

/// "just now", "2m ago", "3h ago", "2d ago".
pub(crate) fn ago(at: std::time::SystemTime) -> String {
    let seconds = at.elapsed().map_or(0, |elapsed| elapsed.as_secs());
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        let now = std::time::SystemTime::now();
        let before = |seconds| now - std::time::Duration::from_secs(seconds);
        assert_eq!(ago(before(5)), "just now");
        assert_eq!(ago(before(130)), "2m ago");
        assert_eq!(ago(before(7200)), "2h ago");
        assert_eq!(ago(before(200_000)), "2d ago");
    }
}
