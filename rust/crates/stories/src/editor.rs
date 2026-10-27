//! The editor as the main window paints it over the editor fixture, cropped to the part each board is about:
//! the file tree, the editor and its popovers, project search and the project diagnostics tab.

use std::time::Duration;

use crate::fixtures::editor as fixture;
use crate::fixtures::window::MainWindow;
use crate::fixtures::Scratch;
use crate::page::TabStory;
use crate::windows::{cropped, Source, WINDOW};
use crate::{still, Story, StoryDef};

/// Long enough for highlighting and the diff marks against git to arrive.
const SETTLING: Duration = Duration::from_millis(1000);
/// The editor area right of the folded rail and the tree, down to the status bar.
const EDITOR: (f32, f32, f32, f32) = (342.0, 40.0, 1092.0, 832.0);
const TREE: (f32, f32, f32, f32) = (56.0, 40.0, 286.0, 832.0);
/// The tree with the menu, which opens past the tree's right edge.
const TREE_MENU: (f32, f32, f32, f32) = (56.0, 40.0, 360.0, 680.0);

fn board(board: &str, crop: (f32, f32, f32, f32)) -> anyhow::Result<Box<dyn Story>> {
    let window = MainWindow::editor_board(board, WINDOW, 1.0)?;
    cropped(Source::Main(window), crop, SETTLING)
}

fn popovers(width: f32, height: f32, popovers: Vec<(ui::Node, ui::Rect)>) -> ui::Node {
    ui::div()
        .w_px(width)
        .h_px(height)
        .bg(ui::theme().editor_background)
        .painted(fixture::paint_popovers(popovers))
        .into()
}

fn hover(width: f32, height: f32) -> ui::Node {
    popovers(width, height, fixture::hover())
}

fn completion(width: f32, height: f32) -> ui::Node {
    popovers(width, height, fixture::completion("fi"))
}

fn go_to_line_states(width: f32, height: f32) -> ui::Node {
    ui::div()
        .col()
        .w_px(width)
        .h_px(height)
        .p(ui::Space::S16)
        .gap(ui::Space::S16)
        .bg(ui::theme().editor_background)
        .children(["", "26:5", "-2"].map(|query| files_ui::go_to_line_preview(5, 4, 31, query)))
        .into()
}

fn project_search(query: &str) -> anyhow::Result<Box<dyn Story>> {
    let scratch = Scratch::new("search")?;
    let root = scratch.path().join("myproject");
    fixture::create_search(&root)?;
    let mut item = if query.is_empty() {
        files_ui::project_search_preview(root, "", false)
    } else {
        files_ui::project_search_scoped(root, query, "User.active", "api/**/*.rb, web/src/**")
    };
    item.set_focused(false);
    Ok(Box::new(TabStory::boxed(item, scratch)))
}

fn project_diagnostics() -> anyhow::Result<Box<dyn Story>> {
    let mut item = fixture::project_diagnostics();
    item.set_focused(true);
    Ok(Box::new(TabStory::boxed(item, ())))
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, (_, _, w, h): (f32, f32, f32, f32), build| StoryDef {
        id,
        page: "Organisms",
        group,
        title,
        size: (w, h),
        build,
    };
    let sized = |w, h| (0.0, 0.0, w, h);
    vec![
        story(
            "editor-file-tree",
            "File tree",
            "Opened to the file",
            TREE,
            || board("open", TREE),
        ),
        story(
            "editor-file-tree-menu",
            "File tree",
            "Context menu",
            TREE_MENU,
            || board("tree-menu", TREE_MENU),
        ),
        story("editor-open", "Editor", "Tabs over a file", EDITOR, || {
            board("open", EDITOR)
        }),
        story("editor-split", "Editor", "Split right", EDITOR, || {
            board("split", EDITOR)
        }),
        story("editor-read-only", "Editor", "Read only", EDITOR, || {
            board("readonly", EDITOR)
        }),
        story(
            "editor-find",
            "Editor popovers",
            "Find and replace",
            sized(1092.0, 300.0),
            || board("find", (342.0, 40.0, 1092.0, 300.0)),
        ),
        story(
            "editor-go-to-line",
            "Editor popovers",
            "Go to line",
            sized(1092.0, 300.0),
            || board("goto", (342.0, 40.0, 1092.0, 300.0)),
        ),
        story(
            "editor-go-to-line-states",
            "Editor popovers",
            "Go to line: empty, typed, out of range",
            sized(420.0, 240.0),
            || still(go_to_line_states),
        ),
        story(
            "editor-hover",
            "Editor popovers",
            "Hover with a diagnostic",
            sized(520.0, 170.0),
            || still(hover),
        ),
        story(
            "editor-completion",
            "Editor popovers",
            "Completion with its doc",
            sized(1080.0, 190.0),
            || still(completion),
        ),
        story(
            "editor-project-search",
            "Project search",
            "Results with replace and filters",
            sized(1092.0, 720.0),
            || project_search("User.where(active: true)"),
        ),
        story(
            "editor-project-search-empty",
            "Project search",
            "Before the first search",
            sized(1092.0, 360.0),
            || project_search(""),
        ),
        story(
            "editor-project-diagnostics",
            "Diagnostics",
            "Project diagnostics",
            sized(1092.0, 460.0),
            project_diagnostics,
        ),
    ]
}
