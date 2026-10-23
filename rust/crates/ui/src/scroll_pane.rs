use crate::{Node, Rect, Rgba};

/// A scrolled region whose content taller than the view is cut to it.
#[derive(Default)]
pub struct ScrollPane {
    pub scroll: f32,
    content_h: f32,
    view_h: f32,
    rect: Option<Rect>,
}

impl ScrollPane {
    pub fn paint(&mut self, tree: &Node, area: Rect) -> crate::Painted {
        let scale = crate::ui_text_scale();
        self.rect = Some(area);
        self.content_h = crate::measure(tree).1 * scale;
        self.view_h = area.h;
        self.scroll = self
            .scroll
            .clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let shifted = Rect::new(
            area.x,
            area.y - self.scroll,
            area.w,
            area.h.max(self.content_h),
            Rgba::TRANSPARENT,
        );
        let mut painted = crate::render(tree, shifted);
        let (top, bottom) = (area.y, area.y + area.h);
        painted.rects.retain_mut(|rect| {
            let (start, end) = (rect.y.max(top), (rect.y + rect.h).min(bottom));
            if end <= start {
                return false;
            }
            rect.h = end - start;
            rect.y = start;
            true
        });
        painted
            .texts
            .retain(|text| text.y >= top - 1.0 && text.y + text.size <= bottom + 1.0);
        painted
            .icons
            .retain(|quad| quad.y >= top - 1.0 && quad.y + quad.h <= bottom + 1.0);
        painted.tris.clear();
        painted
            .hits
            .retain(|(rect, _)| rect.y + rect.h > top && rect.y < bottom);
        painted
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.is_some_and(|rect| {
            x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
        })
    }

    pub fn scroll_by(&mut self, delta: f32) -> bool {
        let next = (self.scroll - delta).clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    /// The scrolled-off part in design px, and the view's height.
    pub fn window(&self) -> (f32, f32) {
        let scale = crate::ui_text_scale();
        (self.scroll / scale, self.view_h / scale)
    }
}
