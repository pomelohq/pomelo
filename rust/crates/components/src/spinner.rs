use ui::{div, theme, Corner, Node, Rgba};

use crate::dot;

const DOTS: usize = 8;
const PERIOD: f32 = 0.9;

/// Work under way with no known end: eight dots round a circle, the bright one going round. `seconds` is
/// the screen's clock. With reduced motion it stands still, every dot half lit.
pub struct Spinner {
    seconds: f32,
    size: f32,
    color: Option<Rgba>,
}

pub fn spinner(seconds: f32) -> Spinner {
    Spinner {
        seconds,
        size: 14.0,
        color: None,
    }
}

impl Spinner {
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// For a spinner on an accent fill or beside a colored state; muted text color otherwise.
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }
}

impl From<Spinner> for Node {
    fn from(spinner: Spinner) -> Node {
        let color = spinner.color.unwrap_or(theme().text_muted);
        let size = spinner.size;
        let mark = (size / 5.0).max(2.0);
        let radius = (size - mark) / 2.0;
        let head = if ui::reduce_motion() {
            None
        } else {
            Some((spinner.seconds / PERIOD).fract() * DOTS as f32)
        };
        let mut ring = div().w_px(size).h_px(size);
        for index in 0..DOTS {
            let angle = index as f32 / DOTS as f32 * std::f32::consts::TAU;
            let alpha = match head {
                // Each dot fades over the turn after the head passes it.
                Some(head) => {
                    1.0 - ((head - index as f32).rem_euclid(DOTS as f32) / DOTS as f32) * 0.8
                }
                None => 0.5,
            };
            let mut fill = color;
            fill.a *= alpha;
            ring = ring.pin(
                Corner::TopLeft,
                radius + radius * angle.sin(),
                radius - radius * angle.cos(),
                mark,
                mark,
                dot(fill, mark),
            );
        }
        ring.into()
    }
}
