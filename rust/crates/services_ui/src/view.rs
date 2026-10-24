//! Drawing pieces of the Services panel that carry no state of their own.

use components::{ButtonKind, ButtonSize};
use ui::{div, icon, measure, theme, IconKind, Node, Rgba};

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

pub(crate) fn status_icon(state: &State) -> Node {
    let colors = theme();
    let inner: Node = match state {
        State::Running => components::dot(colors.success, 8.0).into(),
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
        State::Stopped => components::ring(colors.icon_muted, 8.0).into(),
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
    components::button(id, text)
        .kind(match tone {
            Tone::Plain => ButtonKind::Secondary,
            Tone::Primary => ButtonKind::Primary,
            Tone::Agent => ButtonKind::Agent,
        })
        .icon(kind)
        .hovered(hot.then_some(id))
        .into()
}

/// A borderless clickable `child` that lights up under the pointer and while pressed.
pub(crate) fn ghost(id: u64) -> ui::Div {
    let colors = theme();
    div()
        .row()
        .items_center()
        .justify_center()
        .rounded(3.0)
        .hover_bg(colors.ghost_element_hover)
        .active_bg(colors.ghost_element_active)
        .on_click(id)
}

/// A small icon-only ghost button, such as a copy icon beside a value.
pub(crate) fn icon_button(id: u64, kind: IconKind) -> Node {
    components::icon_button(id, kind)
        .size(ButtonSize::XSmall)
        .into()
}

/// A small button that cannot be pressed right now.
pub(crate) fn disabled_button(kind: Option<IconKind>, text: &str) -> Node {
    components::button(0, text).icon(kind).enabled(false).into()
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
    let mut header = components::section_header(title);
    if let Some(count) = count {
        header = header.count(count);
    }
    div()
        .row()
        .h_px(26.0)
        .px(10.0)
        .items_center()
        .child(header.render().flex(1.0))
        .into()
}

/// How long the holder whose pidfile is `pidfile` has been running: "41m 58s", "2h 5m".
pub(crate) fn uptime(pidfile: &std::path::Path) -> Option<String> {
    let started = std::fs::metadata(pidfile)
        .and_then(|meta| meta.modified())
        .ok()?;
    uptime_since(started)
}

/// How long something started at `started` has been running: "41m 58s", "2h 5m".
pub(crate) fn uptime_since(started: std::time::SystemTime) -> Option<String> {
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
pub fn ago(at: std::time::SystemTime) -> String {
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
