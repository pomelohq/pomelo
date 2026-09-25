//! The auto-update view: turns `auto_update::Status` into a small banner (rects + text). Pure — no state,
//! no side effects; the binary polls `auto_update::status()` and renders whatever this returns.

use auto_update::Status;
use ui::{theme, Rect, Rgba, Text};

pub const BANNER_H: f32 = 28.0;

fn bg_c() -> Rgba {
    let a = theme().text_accent;
    Rgba::new(a.r, a.g, a.b, 0.18)
}
fn fg_c() -> Rgba {
    theme().text
}

/// Build the banner for the given status, spanning `width`. Empty when there is nothing to show.
pub fn banner(status: &Status, width: f32) -> (Vec<Rect>, Vec<Text>) {
    match status {
        Status::Idle => (Vec::new(), Vec::new()),
        Status::UpdateAvailable(version) => {
            let rects = vec![Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: BANNER_H,
                color: bg_c(),
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            }];
            let texts = vec![Text {
                x: 12.0,
                y: 7.0,
                size: 12.0,
                color: fg_c(),
                text: format!("Update {version} available - restarting to apply"),
                mono: false,
                weight: 400,
                italic: false,
                wrap: 0.0,
            }];
            (rects, texts)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_empty() {
        let (r, t) = banner(&Status::Idle, 800.0);
        assert!(r.is_empty() && t.is_empty());
    }

    #[test]
    fn available_shows_version() {
        let (r, t) = banner(&Status::UpdateAvailable("1.2.3".into()), 800.0);
        assert_eq!(r.len(), 1);
        assert!(t[0].text.contains("1.2.3"));
    }
}
