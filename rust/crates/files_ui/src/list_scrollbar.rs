//! The thin scrollbar pickers draw over their list: it shows when the list scrolls, stays a second, then fades.
//! Drawn as an overlay so it takes no width from the rows.

use std::time::{Duration, Instant};

use ui::{deferred, div, theme, Node};

const HIDE_DELAY: Duration = Duration::from_secs(1);
const FADE: Duration = Duration::from_millis(400);
const THUMB_WIDTH: f32 = 6.0;
const PADDING: f32 = 4.0;
const MIN_THUMB: f32 = 25.0;

#[derive(Default)]
pub struct ScrollbarReveal {
    shown_at: Option<Instant>,
}

impl ScrollbarReveal {
    pub fn reveal(&mut self) {
        self.shown_at = Some(Instant::now());
    }

    /// 1 while shown, easing to 0 over the fade after the hide delay.
    pub fn opacity(&self) -> f32 {
        let Some(shown_at) = self.shown_at else {
            return 0.0;
        };
        let elapsed = shown_at.elapsed();
        if elapsed <= HIDE_DELAY {
            1.0
        } else {
            1.0 - ((elapsed - HIDE_DELAY).as_secs_f32() / FADE.as_secs_f32()).min(1.0)
        }
    }

    pub fn is_animating(&self) -> bool {
        self.shown_at
            .is_some_and(|shown_at| shown_at.elapsed() < HIDE_DELAY + FADE)
    }
}

/// A zero-size overlay at the top-left of a list `width` by `height` whose first visible row is `top` of
/// `total`, `visible` at a time. Nothing when everything fits or it has faded.
pub fn render(
    width: f32,
    height: f32,
    top: usize,
    visible: usize,
    total: usize,
    opacity: f32,
) -> Option<Node> {
    if total <= visible || opacity <= 0.0 {
        return None;
    }
    let track = height - PADDING * 2.0;
    let thumb = (track * visible as f32 / total as f32)
        .max(MIN_THUMB)
        .min(track);
    let max_top = (total - visible) as f32;
    let offset = (track - thumb) * (top as f32 / max_top).min(1.0);
    let color = theme().scrollbar_thumb_background;
    let bar = div()
        .col()
        .w_px(width)
        .h_px(height)
        .child(div().h_px(PADDING + offset))
        .child(
            div()
                .row()
                .child(div().flex(1.0))
                .child(
                    div()
                        .w_px(THUMB_WIDTH)
                        .h_px(thumb)
                        .rounded(THUMB_WIDTH / 2.0)
                        .bg(color.alpha(color.a * opacity)),
                )
                .child(div().w_px(PADDING)),
        );
    Some(deferred(bar).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_when_everything_fits() {
        assert!(render(200.0, 100.0, 0, 5, 5, 1.0).is_none());
        assert!(render(200.0, 100.0, 0, 5, 50, 0.0).is_none());
        assert!(render(200.0, 100.0, 0, 5, 50, 1.0).is_some());
    }

    #[test]
    fn fades_after_the_delay() {
        let mut reveal = ScrollbarReveal::default();
        assert_eq!(reveal.opacity(), 0.0);
        reveal.reveal();
        assert_eq!(reveal.opacity(), 1.0);
        assert!(reveal.is_animating());
        reveal.shown_at = Instant::now().checked_sub(HIDE_DELAY + FADE / 2);
        let halfway = reveal.opacity();
        assert!(halfway > 0.4 && halfway < 0.6, "{halfway}");
    }
}
