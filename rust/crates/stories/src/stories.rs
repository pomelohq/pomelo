//! Named ways to build real UI from fixture data. The app's own code draws each one, so the gallery, snapshot
//! tests and the design canvas show exactly what a release shows.

pub mod atoms;
pub mod fixtures;
pub mod foundations;
pub mod molecules;
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
    all.extend(molecules::stories());
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
        "Button" => "Every button. Primary for the one action a dialog or card is for, Secondary for the rest, Ghost where a border would be noise, Danger for destructive actions, Agent where the work goes to an agent. Small in panels and toolbars, Medium in dialogs and forms.",
        "Icon button" => "A square button showing only an icon: XSmall beside a value, Small in toolbars, Medium in headers. Toggled when it switches something on; name it with a tooltip.",
        "Link" => "A text-only action that reads like a link, beside the value it acts on: Open in browser, Change.",
        "Select" => "Pick one of a few known values: a theme, a profile, a database. Opens a Menu of the choices; a placeholder shows while nothing is chosen.",
        "Checkbox" => "An on or off choice in a form; partial when only some of what it stands for is on.",
        "Switch" => "Turns a setting on or off right away, with nothing to save.",
        "Stepper" => "A number changed a step at a time, or typed after a click on it: minutes, ports, limits.",
        "Radio" => "One of a few choices shown all at once, each with a line about it.",
        "Segmented" => "One of a few mutually exclusive choices: Small for toolbar filters (All, Running, Failed), Medium in forms; a choice can carry an icon or be unavailable.",
        "Progress" => "How far a job of known length has got; as a meter, how close something is to its limit, in the color of that closeness.",
        "Pill" => "The one pill: a state in a word or two with its dot, tinted by its color. Warning when something waits on the person.",
        "Status line" => "The result of a check, under the control that ran it.",
        "Key chip" => "A shortcut next to what it runs; the accent one sits on primary buttons.",
        "Divider" => "A 1px line between sections or groups of rows; upright between groups in a row.",
        "Status item" => "One thing on the window's status bar: a dock toggle with its news dot, the cursor position, the language. Lit on hover, clicked to act on it.",
        "Split button" => "One action and a chevron for its other ways: Stage All and what to stage, Commit and how, Merge and the method.",
        "Tab" => "One open thing in a pane: its icon and title, a close button where a pin or an unsaved dot can stand, italic while it is only a preview. The dragged copy floats.",
        "Avatar" => "A person as the first letter of their name in a circle: a commit's author, a reviewer.",
        "Skeleton" => "A gray bar that pulses where text will be once it loads; it stays still with reduced motion.",
        "Key value" => "One fact: a muted key in a fixed column, its value, and a quieter note after it.",
        "Key hints" => "The keys a picker answers to, along its footer; a hint with an id is also a button.",
        "Paragraph" => "Body copy broken at word boundaries to fit a width; centered under an empty state's title.",
        "Chip" => "A short read-only tag: a branch, a key kind, a plan, a detected stack. One shape; only the ink, fill and edge change.",
        "Tooltip" => "Names what an icon does, with the keys that run it when there are any.",
        "Dot" => "Running, needs a look, failed, busy; the ring is a state that has not started. 6 in dense rows, 7 beside a label, 8 on its own.",
        "Input" => "The box a line of text is typed in: ringed while focused, red on an error, mono for code, masked for secrets. A Field puts its label and hint around it.",
        "Field" => "A labeled input: the label above, the hint or the error below. Every form field is one.",
        "Search field" => "The box a panel filters its list with: a search icon, the query, and a clear button once something is typed.",
        "Section header" => "The title over a group of rows or fields: uppercase and muted, with a count or one quiet action at the right; a chevron when the group folds away.",
        "List row" => "One row of a picker or list: icon, text and anything at the end; tinted while selected, lit while hovered.",
        "Banner" => "A card across a page for one thing that needs doing or knowing: what is wrong, the fix as a button, and the commands it runs. A second line says what happens meanwhile; flush runs edge to edge over a panel; a pill can stand for the icon when a decision waits.",
        "Card" => "The one card: a rounded box on the element fill. Hovered when it opens something, chosen when picked, tinted for a problem, raised for a popover over the screen.",
        "Page header" => "The head of a page in a tab: the key and state with the actions on top, then the title and a meta line. Compact puts all of it on one line.",
        "Dock header" => "A docked panel's title strip: the name in small caps and one icon action.",
        "Tree row" => "One row of a tree: files, databases, changes, services. A guide for each level, a chevron when it opens; selected, hovered, ringed where the keyboard is, or marked by what is picked elsewhere.",
        "Breadcrumb" => "Where the thing on screen sits: its parents, each a step back, then itself, and the branch it is on.",
        "Toolbar" => "The strip over a tab with what acts on its content: where it is, filters and buttons. Under a tab it counts what is shown and says what the last action did.",
        "Empty state" => "What a pane says when it has nothing to show: the reason, how it fills, and the action that fills it. Compact is the one line a list shows instead of rows.",
        "Modal" => "A form over the whole window: the title with a close button, the fields, and the footer with a note at the left and the buttons at the right.",
        "Toast" => "A short message over the bottom of the window that goes away by itself, with at most one action.",
        "Menu" => "Every list of choices on a raised card: context menus, a select's choices, settings submenus. Groups under a muted header, icons at the left, keys at the right, Danger last; a long list gets a search line; a check marks the current choice.",
        "Select menu" => "A select with its menu open, as wide as the select. It opens below; near the right edge it lines up with the select's right side; near the bottom it opens above with its search line at the bottom.",
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

    /// Each file of the components crate is one component, so each has its group in the gallery, and every
    /// Atoms group is a component file: the gallery and the crate list the same things.
    #[test]
    fn the_gallery_shows_every_component_once() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../components/src");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .expect("components sources")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.path().file_stem()?.to_str().map(str::to_string))
            .filter(|stem| stem != "components")
            .collect();
        files.sort();
        let file_of = |group: &str| group.to_lowercase().replace(' ', "_");
        let groups: Vec<String> = super::all().iter().map(|def| file_of(def.group)).collect();
        for file in &files {
            assert!(
                groups.contains(file),
                "components/src/{file}.rs has no group in the gallery"
            );
        }
        for def in super::all().iter().filter(|def| def.page == "Atoms") {
            assert!(
                files.contains(&file_of(def.group)),
                "the {} group shows no component file",
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
