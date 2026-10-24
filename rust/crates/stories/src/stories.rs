//! Named ways to build real UI from fixture data. The app's own code draws each one, so the gallery, snapshot
//! tests and the design canvas show exactly what a release shows.

pub mod atoms;
pub mod fixtures;
pub mod foundations;
pub mod services;

use std::io::BufWriter;
use std::path::Path;

/// A story draws at a size; one that is stepped into takes pointer and keys as the app's dock would give them.
pub trait Story {
    fn render(&mut self, width: f32, height: f32) -> ui::Node;
    fn click(&mut self, _id: u64) {}
    fn hover(&mut self, _id: Option<u64>) -> bool {
        false
    }
    fn scroll(&mut self, _dy: f32) -> bool {
        false
    }
    fn text(&mut self, _text: &str) -> bool {
        false
    }
    fn key(&mut self, _key: workspace::EditKey, _shift: bool) -> bool {
        false
    }
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

/// Pages run from the smallest parts up, as the atomic levels do.
pub fn all() -> Vec<StoryDef> {
    let mut all = foundations::stories();
    all.extend(atoms::stories());
    all.extend(services::stories());
    all
}

/// A story that only draws: a foundation or a control in one state.
struct Static(fn(f32, f32) -> ui::Node);

impl Story for Static {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        (self.0)(width, height)
    }
}

pub fn still(render: fn(f32, f32) -> ui::Node) -> anyhow::Result<Box<dyn Story>> {
    Ok(Box::new(Static(render)))
}

/// What a group is for, so someone picking a control knows which one fits.
pub fn group_note(group: &str) -> Option<&'static str> {
    Some(match group {
        "Color" => "Every color token of the active theme, by role. Screens use these names, never raw colors.",
        "Type" => "Text sizes, weights and colors. Default 14 for body text, Small 12 for secondary lines.",
        "Icons" => "Every icon the app ships. Pick one here before drawing a new one.",
        "Button" => "Every button. Primary for the one action a dialog or card is for, Secondary for the rest, Ghost where a border would be noise, Danger for destructive actions. Small in panels and toolbars, Medium in dialogs and forms.",
        "Link" => "A text-only action that reads like a link, beside the value it acts on: Open in browser, Change.",
        "Select" => "Pick one of a few known values: a theme, a profile, a database. Opens the screen's menu of choices; a placeholder shows while nothing is chosen.",
        "Checkbox" => "An on or off choice in a form.",
        "Segmented" => "Compact filters on a toolbar, such as All, Running, Failed.",
        "Toggle group" => "Mutually exclusive choices inside a form, with icons.",
        "Progress" => "How far a job of known length has got.",
        "Status pill" => "The state of a service or job in one word.",
        "Status line" => "The result of a check, under the control that ran it.",
        "Key chip" => "A shortcut next to what it runs; the accent one sits on primary buttons.",
        "Divider" => "Separates sections; Faded inside a list.",
        "Input" => "A labeled text field with its hint or its error below.",
        "Services panel" => "The Services panel in the side dock: every service of the workspace, what needs attention, and the shared services.",
        "Service tab" => "One service opened in the center: status, URL, env, command and live logs.",
        _ => return None,
    })
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
    fn every_group_says_what_it_is_for() {
        for def in super::all() {
            assert!(
                super::group_note(def.group).is_some(),
                "{} has no note",
                def.group
            );
        }
    }

    #[test]
    fn every_story_paints() {
        for def in super::all() {
            let mut story = (def.build)().unwrap_or_else(|error| panic!("{}: {error}", def.id));
            let painted = super::paint(story.as_mut(), def.size.0, def.size.1);
            // The first rect is the background every story sits on.
            let drew =
                painted.rects.len() > 1 || !painted.texts.is_empty() || !painted.icons.is_empty();
            assert!(drew, "{} drew nothing", def.id);
        }
    }
}
