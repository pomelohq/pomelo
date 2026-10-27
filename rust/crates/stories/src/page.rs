//! Hosts for what the window shows on its own: a tab's body, and a form over the window.

use workspace::WindowModal;

use crate::Story;

/// A tab's body as the center pane paints it, with what floats over it.
pub struct TabStory {
    item: Box<dyn workspace::Item>,
    /// Kept for as long as the tab reads from it.
    _keep: Option<Box<dyn std::any::Any>>,
}

impl TabStory {
    pub fn keeping(item: impl workspace::Item + 'static, keep: impl std::any::Any) -> TabStory {
        TabStory {
            item: Box::new(item),
            _keep: Some(Box::new(keep)),
        }
    }

    pub fn boxed(item: Box<dyn workspace::Item>, keep: impl std::any::Any) -> TabStory {
        TabStory {
            item,
            _keep: Some(Box::new(keep)),
        }
    }
}

impl Story for TabStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = self.item.paint_body(body, true).unwrap_or_default();
        ui::div()
            .w_px(width)
            .h_px(height)
            .bg(self.item.body_background())
            .painted(painted)
            .into()
    }

    fn overlay(&mut self, area: ui::Rect) -> Option<ui::Painted> {
        self.item.paint_popover(area)
    }
}

pub fn tab(item: impl workspace::Item + 'static) -> anyhow::Result<Box<dyn Story>> {
    Ok(Box::new(TabStory {
        item: Box::new(item),
        _keep: None,
    }))
}

/// A form over the window, on the editor's background, answering clicks and keys as the window would pass them.
/// What drops down from it, such as a picker's list, is a layer of its own so the form's text does not show
/// through.
pub struct ModalStory {
    modal: Box<dyn WindowModal>,
}

impl ModalStory {
    fn frame(&mut self, area: ui::Rect) -> ui::Frame {
        let node = ui::div()
            .col()
            .w_px(area.w)
            .h_px(area.h)
            .p(ui::Space::S20)
            .bg(ui::theme().editor_background)
            .child(self.modal.render())
            .into();
        ui::paint_frame(&node, area)
    }
}

impl Story for ModalStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.modal.tick();
        let frame = self.frame(ui::Rect::new(
            0.0,
            0.0,
            width,
            height,
            ui::Rgba::TRANSPARENT,
        ));
        let mut base = frame.base;
        base.hits = frame.hits;
        ui::div().w_px(width).h_px(height).painted(base).into()
    }

    fn overlays(&mut self, area: ui::Rect) -> Vec<ui::Overlay> {
        self.frame(area).overlays
    }

    fn click(&mut self, id: u64) {
        self.modal.click(id);
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        self.modal.hover(id)
    }

    fn text(&mut self, text: &str) -> bool {
        self.modal.text(text)
    }

    fn key(&mut self, key: workspace::EditKey, shift: bool) -> bool {
        self.modal.key(key, shift)
    }
}

pub fn modal(modal: Option<Box<dyn WindowModal>>) -> anyhow::Result<Box<dyn Story>> {
    let modal = modal.ok_or_else(|| anyhow::anyhow!("no such form"))?;
    Ok(Box::new(ModalStory { modal }))
}
