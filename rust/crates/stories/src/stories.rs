//! Named ways to build real UI from fixture data. The app's own code draws each one, so the gallery, snapshot
//! tests and the design canvas show exactly what a release shows.

pub mod agents;
pub mod atoms;
pub mod chrome;
pub mod database;
pub mod dev_services;
pub mod editor;
pub mod environment;
pub mod fixtures;
pub mod foundations;
pub mod git;
pub mod module_store;
pub mod molecules;
pub mod onboarding;
pub mod page;
pub mod pull_request;
pub mod remote;
pub mod services;
pub mod settings;
pub mod terminal;
pub mod usage;
pub mod windows;
pub mod workspace_forms;

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
    /// What floats above the story, such as an open menu, painted as a layer of its own so the story's text
    /// does not show through it.
    fn overlay(&mut self, _area: ui::Rect) -> Option<ui::Painted> {
        None
    }
    /// Every layer that floats above the story, back to front, each with the region it is clipped to.
    fn overlays(&mut self, area: ui::Rect) -> Vec<ui::Overlay> {
        self.overlay(area)
            .map(|painted| ui::Overlay {
                painted,
                clip: None,
            })
            .into_iter()
            .collect()
    }
    /// The context menu a right click on `id` opens; empty when it opens none.
    fn context_menu(&mut self, _id: u64) -> Vec<components::MenuItem> {
        Vec::new()
    }
    fn submenu(&self, _id: u64) -> Vec<components::MenuItem> {
        Vec::new()
    }
    fn menu_action(&mut self, _item: u64) {}
    /// True while the story still fills in off the main thread; the gallery keeps redrawing it until then.
    fn pending(&mut self) -> bool {
        false
    }
    /// True for a story that moves on its own, such as a spinner; the gallery redraws it every frame.
    fn animating(&self) -> bool {
        false
    }
}

/// Waits for a story to fill in, for a still that must show it whole.
pub fn settle(story: &mut dyn Story) {
    while story.pending() {
        std::thread::sleep(std::time::Duration::from_millis(10));
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
    all.extend(git::stories());
    all.extend(settings::stories());
    all.extend(agents::stories());
    all.extend(chrome::stories());
    all.extend(database::stories());
    all.extend(pull_request::stories());
    all.extend(remote::stories());
    all.extend(onboarding::stories());
    all.extend(workspace_forms::stories());
    all.extend(module_store::stories());
    all.extend(usage::stories());
    all.extend(environment::stories());
    all.extend(dev_services::stories());
    all.extend(editor::stories());
    all.extend(terminal::stories());
    all.extend(windows::stories());
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

/// Each page's groups by what they are for, in the order the gallery lays them out: a section per job, so
/// controls that answer the same need sit together.
pub const SECTIONS: &[(&str, &str, &[&str])] = &[
    (
        "Foundations",
        "Tokens",
        &[
            "Color",
            "Type",
            "Spacing",
            "Radius",
            "Elevation",
            "Motion",
            "Icons",
        ],
    ),
    (
        "Atoms",
        "Actions",
        &["Button", "Split button", "Icon button", "Link"],
    ),
    (
        "Atoms",
        "Inputs",
        &["Input", "Checkbox", "Radio", "Switch", "Stepper", "Select"],
    ),
    ("Atoms", "Navigation", &["Tab", "Segmented"]),
    (
        "Atoms",
        "Status",
        &[
            "Dot",
            "Pill",
            "Chip",
            "Progress",
            "Skeleton",
            "Spinner",
            "Badge",
            "Status line",
            "Status item",
        ],
    ),
    (
        "Atoms",
        "Content",
        &[
            "Paragraph",
            "Key value",
            "Avatar",
            "Key chip",
            "Key hints",
            "Tooltip",
            "Divider",
        ],
    ),
    (
        "Molecules",
        "Forms",
        &["Field", "Text area", "Search field"],
    ),
    ("Molecules", "Menus", &["Menu", "Select menu", "Picker"]),
    (
        "Molecules",
        "Lists",
        &["Section header", "List row", "Tree row", "Empty state"],
    ),
    (
        "Molecules",
        "Headers and bars",
        &[
            "Page header",
            "Dock header",
            "Tab bar",
            "Toolbar",
            "Breadcrumb",
        ],
    ),
    ("Molecules", "Surfaces", &["Card", "Popover", "Modal"]),
    ("Molecules", "Messages", &["Banner", "Toast"]),
    ("Organisms", "Services", &["Services panel", "Service tab"]),
    ("Organisms", "Git", &["Git panel"]),
    ("Organisms", "Settings", &["Settings window"]),
    ("Organisms", "Agents", &["Agent dock"]),
    (
        "Organisms",
        "Workspace",
        &[
            "Title bar",
            "Session menu",
            "Status bar",
            "Workspace rail",
            "Pane tab bar",
            "Toasts",
            "Welcome",
            "Prompt",
            "Tab switcher",
            "Workspace switcher",
        ],
    ),
    (
        "Organisms",
        "Data",
        &["Database panel", "Environment and secrets"],
    ),
    ("Organisms", "Pull requests", &["Pull request"]),
    ("Organisms", "Remote", &["Remote"]),
    ("Organisms", "Onboarding", &["Onboarding"]),
    ("Organisms", "Workspace forms", &["Workspace forms"]),
    ("Organisms", "Module store", &["Module store"]),
    ("Organisms", "Usage", &["Usage page"]),
    ("Organisms", "Dev services", &["Dev services log"]),
    (
        "Organisms",
        "Editor",
        &[
            "File tree",
            "Editor",
            "Editor popovers",
            "Project search",
            "Diagnostics",
        ],
    ),
    (
        "Organisms",
        "Terminal",
        &["Terminal panel", "Side agent bar"],
    ),
    ("Pages", "Windows", &["Windows"]),
];

/// What a group is for, so someone picking a control knows which one fits.
pub fn group_note(group: &str) -> Option<&'static str> {
    Some(match group {
        "Color" => "Every color token of the active theme, by role. Screens use these names, never raw colors. Hover: ghost element hover on rows and bare controls, element hover on filled controls, the theme tint on raised cards and menus.",
        "Type" => "Text sizes, weights and colors. Default 14 for body text, Small 12 for secondary lines.",
        "Spacing" => "Gap and padding steps. 4 to 8 between items in a row, 10 to 16 around a card or a section, 20 and 24 between groups. Px only for space that follows something else, such as a hairline.",
        "Radius" => "Corner radius steps. Small 4 for controls and rows, Medium 6 for inputs and chips, Large 8 for menus and popovers, XLarge 10 for cards and modals.",
        "Elevation" => "How far a surface floats, which sets its shadow: Elevated for menus, popovers and tooltips, Modal for dialogs and pickers, Overlay for toasts over everything.",
        "Motion" => "How long UI motion takes: Fast for hover and press, Normal for what enters or leaves, Slow for fades that linger.",
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
        "Spinner" => "Work under way with no known end, such as a take over or an install; it stays still with reduced motion.",
        "Badge" => "How many things wait to be seen, past 99 as 99+; zero shows nothing. The dot form marks a status bar icon.",
        "Key value" => "One fact: a muted key in a fixed column, its value, and a quieter note after it.",
        "Key hints" => "The keys a picker answers to, along its footer; a hint with an id is also a button.",
        "Paragraph" => "Body copy broken at word boundaries to fit a width; centered under an empty state's title.",
        "Chip" => "A short read-only tag: a branch, a key kind, a plan, a detected stack. One shape; only the ink, fill and edge change.",
        "Tooltip" => "Names what an icon does, with the keys that run it when there are any.",
        "Dot" => "Running, needs a look, failed, busy; the ring is a state that has not started. 6 in dense rows, 7 beside a label, 8 on its own.",
        "Input" => "The box a line of text is typed in: ringed while focused, red on an error, mono for code, masked for secrets. A Field puts its label and hint around it.",
        "Field" => "A labeled input: the label above, the hint or the error below. Every form field is one.",
        "Text area" => "Several lines of text: a commit message, a review, a value being edited. The same box as an input, sized by its owner; the text wraps and scrolls inside.",
        "Search field" => "The box a panel filters its list with: a search icon, the query, and a clear button once something is typed.",
        "Section header" => "The title over a group of rows or fields: uppercase and muted, with a count or one quiet action at the right; a chevron when the group folds away.",
        "List row" => "One row of a picker or list: icon, text and anything at the end; tinted while selected, lit while hovered.",
        "Banner" => "A card across a page for one thing that needs doing or knowing: what is wrong, the fix as a button, and the commands it runs. A second line says what happens meanwhile; flush runs edge to edge over a panel; a pill can stand for the icon when a decision waits.",
        "Picker" => "The one picker: the query on top, the matches under it or why there are none, and the keys at the foot. File finder, command palette, language and outline use it.",
        "Popover" => "A raised card under what opened it. Rows sit edge to edge with a 4px inset; text and buttons get the content padding. The title lines up with what is below it.",
        "Card" => "The one card: a rounded box on the element fill. Hovered when it opens something, chosen when picked, tinted for a problem, raised for a popover over the screen.",
        "Page header" => "The head of a page in a tab: the key and state with the actions on top, then the title and a meta line. Compact puts all of it on one line.",
        "Dock header" => "A docked panel's title strip: the name in small caps and one icon action.",
        "Tree row" => "One row of a tree: files, databases, changes, services. A guide for each level, a chevron when it opens; selected, hovered, ringed where the keyboard is, or marked by what is picked elsewhere.",
        "Breadcrumb" => "Where the thing on screen sits: its parents, each a step back, then itself, and the branch it is on.",
        "Tab bar" => "The strip at the top of a pane or a page: back and forward at the start, the open tabs, the pane's own buttons at the end. Tabs that do not fit scroll sideways under a wheel.",
        "Toolbar" => "The strip over a tab with what acts on its content: where it is, filters and buttons. Under a tab it counts what is shown and says what the last action did.",
        "Empty state" => "What a pane says when it has nothing to show: the reason, how it fills, and the action that fills it. Compact is the one line a list shows instead of rows.",
        "Modal" => "A form over the whole window: the title with a close button, the fields, and the footer with a note at the left and the buttons at the right.",
        "Toast" => "A short message over the bottom of the window that goes away by itself, with at most one action.",
        "Menu" => "Every list of choices on a raised card: context menus, a select's choices, settings submenus. Groups under a muted header, icons at the left, keys at the right, Danger last; a long list gets a search line; a check marks the current choice.",
        "Select menu" => "A select with its menu open, as wide as the select. It opens below; near the right edge it lines up with the select's right side; near the bottom it opens above with its search line at the bottom.",
        "Services panel" => "The Services panel in the side dock: every service of the workspace, what needs attention, and the shared services.",
        "Service tab" => "One service opened in the center: status, URL, env, command and live logs.",
        "Git panel" => "The Git panel in the side dock over every repo of the workspace: what changed and what is staged, the branch against origin with its pull request, the history, and the same panel in the window of a project on a host.",
        "Settings window" => "The Settings window: a page per category, a search across them, and the host tab a remote window adds beside This Mac, with the host's settings read, being read or out of reach.",
        "Database panel" => "The Database panel in the side dock: every database, keyspace and bucket of the workspace, opened to its tables and columns, still connecting, failed with the fix to run, or none in pom.yml.",
        "Environment and secrets" => "The Environment tab with a service's env and where each value comes from, and the Secrets tab with the session's secrets, one being added and one armed for delete.",
        "Pull request" => "The pull request tab of a branch: its overview with checks, reviews and the conversation, the Checks list, and the card it shows while looking one up, when there is none, or when the lookup failed.",
        "Remote" => "A project on a host: Connect to Host from picking the host to picking the project, and the page in place of the editor for each state of the connection and of pom on the host.",
        "Onboarding" => "The new project page: the repos, setup and review steps of its form, the run that sets the project up, a failed check and its fix, and the summary once it works.",
        "Workspace forms" => "The forms that make and change workspaces: Create with its ticket and branch pickers, a taken branch, Rename, Add repo, Use branch and Pick repos.",
        "Module store" => "The node_modules store: each repo's lockfile versions with the workspaces that share them, sizes being measured, nothing read yet, a store that could not be read, and the store of a host.",
        "Usage page" => "What the project's agents spent, by workspace, agent or model, with the account's limits; costs can be hidden, and an empty page says no agent ran yet.",
        "Dev services log" => "The requests through the dev proxy and the webhooks fanned out to workspaces, one opened with its bodies, filtered to errors, none yet, and the servers stopped or held by pom proxy.",
        "Agent dock" => "The agent dock: the workspace's Main agent pinned first in its tab bar, and the strip over an agent someone else drives, with Take over and the tool call waiting for Allow or Deny.",
        "Title bar" => "The top of the main window: the host for a project on another machine, the session, its branch, a pill when another workspace waits for you, the update button, the agents' usage and the app menu.",
        "Session menu" => "The session switcher under the title bar: the project in this window, recent ones with the host each lives on, and starting, opening or connecting to another.",
        "Status bar" => "The bottom strip: the workspace's functions and terminal on the left, today's agent spend, the terminal and agent toggles on the right.",
        "Workspace rail" => "The workspaces of the project as the side dock shows them: a tile per workspace when the dock is collapsed, a row with its status, agent and pull request when it is open.",
        "Pane tab bar" => "A center pane's tabs over real items: a pinned tab, the active one, a preview in italics, and the pane's split and zoom buttons.",
        "Toasts" => "What the window says above its status bar: a toast that goes by itself, and the cards that stay until handled, such as an update to restart into or a config that stopped loading.",
        "Welcome" => "The window without a project: what to start with, what this Mac lacks to run services, and the recent projects.",
        "Prompt" => "A confirmation over the dimmed window before something that cannot be undone or that drops work.",
        "Tab switcher" => "The open tabs by recency while ctrl-tab is held; the highlighted one opens on release.",
        "Workspace switcher" => "Every workspace by what needs you first, then main, then by ticket status, with the new-workspace command at the foot.",
        "File tree" => "The Files tree beside the editor: opened down to the file being edited, with git marks on what changed, and its context menu on a file.",
        "Editor" => "The center pane over real files: tabs over a file with its diff marks, two panes split side by side, and a file opened read only.",
        "Editor popovers" => "What opens over the text: find and replace, go to line in each of its states, the hover with a diagnostic and the docs, and completions with the chosen one's doc.",
        "Project search" => "cmd-shift-f in a tab: the matches by file with replace and the include filter open, and the tab before the first search.",
        "Diagnostics" => "Every problem the language servers report across the project, by file, each opened to its line.",
        "Terminal panel" => "The terminal dock of a project on a host: a shell with its output, and the same shell kept open under the notice while the host is down.",
        "Side agent bar" => "The strip over a side agent's console: its role, whether it may edit, what it started with, and Send to main. A narrow dock drops the send label, then the context, then the tag.",
        "Windows" => "Whole windows as the app lays them out and paints them: the main window over a project on this Mac, and a project on a host with its editor, services, terminal and agents.",
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

/// The story and what floats above it, one layer each; a clipped region of the story keeps its own layer.
pub fn paint_layers(story: &mut dyn Story, width: f32, height: f32) -> Vec<ui::Overlay> {
    let area = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
    let frame = ui::paint_frame(
        &ui::div()
            .bg(ui::theme().panel_background)
            .child(story.render(width, height))
            .into(),
        area,
    );
    let mut base = frame.base;
    base.hits = frame.hits;
    let mut layers = vec![ui::Overlay {
        painted: base,
        clip: None,
    }];
    layers.extend(frame.overlays);
    layers.extend(story.overlays(area));
    layers
}

pub fn layers(painted: &[ui::Overlay]) -> Vec<ui::Layer<'_>> {
    painted
        .iter()
        .map(|layer| {
            (
                layer.painted.rects.as_slice(),
                layer.painted.tris.as_slice(),
                layer.painted.texts.as_slice(),
                layer.painted.icons.as_slice(),
                layer.clip.map(|clip| (clip.x, clip.y, clip.w, clip.h)),
            )
        })
        .collect()
}

/// Renders a story with the GPU, off screen, to a PNG at `scale` device pixels per point.
pub fn write_png(def: &StoryDef, scale: f32, out: &Path) -> anyhow::Result<(u32, u32)> {
    let (width, height) = def.size;
    let mut story = (def.build)()?;
    settle(story.as_mut());
    let painted = paint_layers(story.as_mut(), width, height);
    let mut renderer =
        ui::UiRenderer::new_headless((width * scale) as u32, (height * scale) as u32, scale)?;
    renderer.render_frame(ui::theme().panel_background, &layers(&painted))?;
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
    fn every_group_sits_in_one_section_of_its_page() {
        let all = super::all();
        for def in &all {
            let homes = super::SECTIONS
                .iter()
                .filter(|(page, _, groups)| *page == def.page && groups.contains(&def.group))
                .count();
            assert_eq!(homes, 1, "{} > {} needs one section", def.page, def.group);
        }
        for (page, section, groups) in super::SECTIONS {
            for group in *groups {
                assert!(
                    all.iter()
                        .any(|def| def.page == *page && def.group == *group),
                    "{section} lists {group}, which {page} does not show"
                );
            }
        }
    }

    #[test]
    fn every_story_paints() {
        for def in super::all() {
            let mut story = (def.build)().unwrap_or_else(|error| panic!("{}: {error}", def.id));
            super::settle(story.as_mut());
            let painted = super::paint(story.as_mut(), def.size.0, def.size.1);
            // The first rect is the background every story sits on.
            let drew =
                painted.rects.len() > 1 || !painted.texts.is_empty() || !painted.icons.is_empty();
            assert!(drew, "{} drew nothing", def.id);
        }
    }
}
