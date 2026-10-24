//! Named ways to build real UI from fixture data. The app's own code draws each one, so the gallery, snapshot
//! tests and the design canvas show exactly what a release shows.

pub mod fixtures;
pub mod services;

use std::io::BufWriter;
use std::path::Path;

pub trait Story {
    fn render(&mut self, width: f32, height: f32) -> ui::Node;
}

#[derive(Clone, Copy)]
pub struct StoryDef {
    pub id: &'static str,
    pub page: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    pub size: (f32, f32),
    pub build: fn() -> anyhow::Result<Box<dyn Story>>,
}

pub fn all() -> Vec<StoryDef> {
    services::stories()
}

pub fn find(id: &str) -> Option<StoryDef> {
    all().into_iter().find(|def| def.id == id)
}

/// Lays out and paints a story on the panel background, the way a dock hosts it.
pub fn paint(story: &mut dyn Story, width: f32, height: f32) -> ui::Painted {
    ui::render(
        &ui::div()
            .bg(ui::theme().panel_background)
            .child(story.render(width, height))
            .into(),
        ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
    )
}

/// Renders a story with the GPU, off screen, to a PNG at `scale` device pixels per point.
pub fn write_png(def: &StoryDef, scale: f32, out: &Path) -> anyhow::Result<(u32, u32)> {
    let (width, height) = def.size;
    let mut story = (def.build)()?;
    let painted = paint(story.as_mut(), width, height);
    let mut renderer =
        ui::UiRenderer::new_headless((width * scale) as u32, (height * scale) as u32, scale)?;
    renderer.render_frame(
        ui::theme().panel_background,
        &[(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )],
    )?;
    let (w, h, rgba) = renderer.read_rgba()?;
    let mut encoder = png::Encoder::new(BufWriter::new(std::fs::File::create(out)?), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = super::all().iter().map(|def| def.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    // Layout and paint only: CI has no GPU, but a story that panics or draws nothing fails here.
    #[test]
    fn every_story_paints() {
        for def in super::all() {
            let mut story = (def.build)().unwrap_or_else(|error| panic!("{}: {error}", def.id));
            let painted = super::paint(story.as_mut(), def.size.0, def.size.1);
            assert!(!painted.texts.is_empty(), "{} drew no text", def.id);
        }
    }
}
