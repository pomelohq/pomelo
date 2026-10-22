//! The strip over a remote terminal whose connection dropped: reconnecting, or disconnected with Retry. The
//! screen under it stays as it was, and typing is refused until the connection is back.

use std::time::Duration;

use terminal::RemoteLink;
use ui::{div, icon, label, theme, IconKind, Node};

const RETRY: u64 = 0;
const IDS: u64 = 1;

pub struct LinkBar {
    base: u64,
}

impl Default for LinkBar {
    fn default() -> Self {
        LinkBar {
            base: workspace::toolbar_ids(IDS),
        }
    }
}

impl LinkBar {
    pub fn render(&self, link: &RemoteLink, host: &str, width: f32) -> Option<Node> {
        strip(self.base, link, host, width)
    }

    /// Whether `id` is the Retry button.
    pub fn is_retry(&self, id: u64) -> bool {
        id == self.base + RETRY
    }
}

fn place(host: &str) -> String {
    if host.is_empty() {
        "the host".to_string()
    } else {
        host.to_string()
    }
}

/// What the strip says for `link`; `None` while connected.
pub fn link_text(link: &RemoteLink, host: &str) -> Option<String> {
    let host = place(host);
    match link {
        RemoteLink::Connected => None,
        RemoteLink::Reconnecting { attempt, next_in } if *attempt > 0 && !next_in.is_zero() => {
            Some(format!(
                "Lost connection to {host}. Reconnecting... (attempt {attempt} in {}s)",
                seconds(*next_in)
            ))
        }
        RemoteLink::Reconnecting { .. } => {
            Some(format!("Lost connection to {host}. Reconnecting..."))
        }
        RemoteLink::Disconnected { .. } => Some(format!("Disconnected from {host}")),
    }
}

fn seconds(wait: Duration) -> u64 {
    wait.as_secs_f64().ceil() as u64
}

/// The strip for `link`, its Retry button's click id counted from `base`.
pub fn strip(base: u64, link: &RemoteLink, host: &str, width: f32) -> Option<Node> {
    let text = link_text(link, host)?;
    let colors = theme();
    let (glyph, ink) = match link {
        RemoteLink::Disconnected { .. } => (IconKind::XCircle, colors.error),
        _ => (IconKind::Warning, colors.warning),
    };
    let mut card = div()
        .row()
        .h_px(36.0)
        .pl(12.0)
        .pr(6.0)
        .gap(8.0)
        .items_center()
        .child(icon(glyph).size(14.0).color(ink))
        .child(
            div()
                .row()
                .flex(1.0)
                .child(label(text).size(12.0).color(colors.text).truncate()),
        );
    if matches!(link, RemoteLink::Disconnected { .. }) {
        card = card.child(crate::lease_bar::button(
            base + RETRY,
            None,
            "Retry",
            None,
            true,
        ));
    }
    Some(crate::lease_bar::tray(vec![card], width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_says_reconnecting_then_disconnected_and_nothing_once_back() {
        assert!(link_text(&RemoteLink::Connected, "dev").is_none());
        let reconnecting = RemoteLink::Reconnecting {
            attempt: 2,
            next_in: Duration::from_millis(900),
        };
        assert_eq!(
            link_text(&reconnecting, "dev").as_deref(),
            Some("Lost connection to dev. Reconnecting... (attempt 2 in 1s)")
        );
        let disconnected = RemoteLink::Disconnected {
            reason: "Connection refused".into(),
        };
        assert_eq!(
            link_text(&disconnected, "dev").as_deref(),
            Some("Disconnected from dev")
        );
        let bar = LinkBar::default();
        assert!(bar.render(&reconnecting, "dev", 600.0).is_some());
        assert!(bar.is_retry(bar.base + RETRY));
        assert!(!bar.is_retry(bar.base + IDS));
    }
}
