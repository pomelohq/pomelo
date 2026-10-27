//! The main window of a project on this Mac over the editor fixture: the rail folded, the Files tree opened down
//! to the file being edited, its tabs and the agent dock. An editor board opens its own files and takes its steps.

use std::path::{Path, PathBuf};

use workspace::pane::PaneCommand;
use workspace::pane_group::SplitDirection;

use super::Scratch;

pub type BoardStep = Box<dyn Fn(&mut workspace::WorkspaceView)>;

/// The caret a board starts at: line, column, and how many lines below it are selected.
pub type Select = (usize, usize, usize);

/// The files an editor board opens, the last one active, and where its caret starts.
pub fn board_files(board: &str, root: &Path) -> (Vec<PathBuf>, Select) {
    let open = |paths: &[&str]| paths.iter().map(|path| root.join(path)).collect();
    let controller = "api/app/controllers/users_controller.rb";
    match board {
        "language" => (open(&["web/src/api/users"]), (6, 0, 0)),
        "markdown" => (open(&["api/README.md"]), (6, 8, 0)),
        "image" => (open(&["web/public/logo.png"]), (0, 0, 0)),
        "readonly" => (open(&[controller]), (5, 4, 0)),
        "tabs" => (
            open(&[
                controller,
                "api/config/routes.rb",
                "api/README.md",
                "web/src/api/users.ts",
                "pom.yml",
            ]),
            (5, 4, 0),
        ),
        _ => (
            open(&[
                controller,
                "api/config/routes.rb",
                "api/README.md",
                controller,
            ]),
            (5, 4, 0),
        ),
    }
}

pub fn move_caret(view: &mut workspace::WorkspaceView, line: usize, column: usize) {
    view.editor_key(workspace::EditKey::DocumentStart, false);
    for _ in 0..line {
        view.editor_key(workspace::EditKey::Down, false);
    }
    for _ in 0..column {
        view.editor_key(workspace::EditKey::Right, false);
    }
}

/// What an editor board does once its files are open, a frame drawn before each step.
pub fn board_steps(board: &str, root: &Path) -> Vec<BoardStep> {
    let open = |path: &'static str| -> BoardStep {
        let path = root.join(path);
        Box::new(move |view| view.open_file(&path))
    };
    match board {
        "split" => {
            let (spec, other) = (
                root.join("api/spec/requests/users_spec.rb"),
                root.join("api/spec/models/user_spec.rb"),
            );
            vec![Box::new(move |view| {
                view.pane_command(PaneCommand::Split(SplitDirection::Right));
                view.open_file(&spec);
                view.open_file(&other);
                view.pane_command(PaneCommand::ActivateItem(0));
                view.pane_command(PaneCommand::CloseActiveItem);
                view.pane_command(PaneCommand::ActivateItem(0));
                move_caret(view, 8, 16);
            })]
        }
        "tabs" => vec![
            Box::new(|view| view.deploy_project_diagnostics(false)),
            open("pom.yml"),
            open("web/src/api/users.ts"),
            open("api/README.md"),
            open("api/config/routes.rb"),
            open("api/app/controllers/users_controller.rb"),
            Box::new(|view| {
                view.run_action(workspace::keymap::Action::ToggleTabSwitcher);
            }),
        ],
        "find" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::DeploySearch, false);
            }),
            Box::new(|view| {
                view.editor_text("user");
                view.editor_key(workspace::EditKey::ToggleSearchWholeWord, false);
                view.editor_key(workspace::EditKey::ToggleSearchReplace, false);
            }),
            Box::new(|view| {
                view.editor_key(workspace::EditKey::Tab, false);
                view.editor_text("account");
                view.editor_key(workspace::EditKey::Tab, false);
            }),
        ],
        "language" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::ToggleLanguageSelector, false);
            }),
            Box::new(|view| {
                view.editor_text("t");
            }),
        ],
        "goto" => vec![
            Box::new(|view| {
                view.editor_key(workspace::EditKey::ToggleGoToLine, false);
            }),
            Box::new(|view| {
                view.editor_text("26:5");
            }),
        ],
        "image" => {
            let broken = root.join("web/public/broken.png");
            vec![
                Box::new(move |view| {
                    view.pane_command(PaneCommand::Split(SplitDirection::Right));
                    view.open_file(&broken);
                    view.pane_command(PaneCommand::ActivateItem(0));
                    view.pane_command(PaneCommand::CloseActiveItem);
                }),
                Box::new(|view| {
                    view.pane_command(PaneCommand::ActivatePane(SplitDirection::Left));
                }),
            ]
        }
        "markdown" => vec![Box::new(|view| {
            view.editor_key(workspace::EditKey::OpenMarkdownPreviewToTheSide, false);
        })],
        "vim-normal" => vec![Box::new(|view| move_caret(view, 5, 13))],
        "vim-visual" => vec![Box::new(|view| {
            move_caret(view, 5, 2);
            view.editor_text("Vjj");
        })],
        "tree-menu" => vec![Box::new(|view| {
            view.right_click(200.0, 205.0);
        })],
        _ => Vec::new(),
    }
}

/// How the window opens: its files, the caret, and whether it is an editor board.
struct Opening {
    open: Vec<PathBuf>,
    select: Option<Select>,
    read_only: bool,
    board: bool,
}

pub struct MainWindow {
    app: ui::Application,
    handle: ui::WindowHandle,
    entity: ui::Entity<workspace::WorkspaceView>,
    // Dropped after the app, whose files view reads these files.
    _scratch: Scratch,
}

impl MainWindow {
    pub fn open(size: (f32, f32), scale: f32) -> anyhow::Result<Self> {
        let scratch = Scratch::new("window")?;
        let root = scratch.path().join("myproject");
        super::editor::create(&root)?;
        let controller = root.join("api/app/controllers/users_controller.rb");
        let opening = Opening {
            open: vec![
                controller.clone(),
                root.join("api/config/routes.rb"),
                root.join("api/README.md"),
                controller,
            ],
            select: None,
            read_only: false,
            board: false,
        };
        Self::with(scratch, root, opening, size, scale)
    }

    /// The window as the editor board `board` shows it: the agent dock closed, its files open and its steps
    /// taken.
    pub fn editor_board(board: &str, size: (f32, f32), scale: f32) -> anyhow::Result<Self> {
        let scratch = Scratch::new("editor")?;
        let root = scratch.path().join("myproject");
        super::editor::create(&root)?;
        let (open, select) = board_files(board, &root);
        let opening = Opening {
            open,
            select: Some(select),
            read_only: board == "readonly",
            board: true,
        };
        let mut window = Self::with(scratch, root.clone(), opening, size, scale)?;
        for step in board_steps(board, &root) {
            window.app.draw(window.handle);
            window
                .entity
                .update(window.app.app_mut(), |view, _| step(view));
        }
        Ok(window)
    }

    fn with(
        scratch: Scratch,
        root: PathBuf,
        opening: Opening,
        (width, height): (f32, f32),
        scale: f32,
    ) -> anyhow::Result<Self> {
        let project = super::window_project(&root);
        let mut app = ui::Application::new();
        let (handle, entity) = app.open_raw_window(
            ui::WindowOptions {
                width,
                height,
                scale,
                ..Default::default()
            },
            move |_| {
                let mut files =
                    files_ui::FilesView::local(root.clone()).read_only(opening.read_only);
                for path in &opening.open {
                    workspace::FunctionView::open_file_at(&mut files, path, None, None);
                }
                if let Some(last) = opening.open.last() {
                    workspace::FunctionView::reveal_in_tree(&mut files, last);
                }
                if let Some((line, column, lines)) = opening.select {
                    use workspace::ItemInput;
                    for _ in 0..line {
                        files.editor_key(workspace::EditKey::Down, false);
                    }
                    for _ in 0..column {
                        files.editor_key(workspace::EditKey::Right, false);
                    }
                    for _ in 0..lines {
                        files.editor_key(workspace::EditKey::Down, true);
                    }
                }
                let mut layout = workspace::Layout {
                    project: Some(project),
                    files_view: Some(Box::new(files)),
                    usage: super::window_usage(),
                    ..Default::default()
                };
                layout.left.width = 256.0;
                layout.right.width = 380.0;
                layout.right.collapsed = opening.board;
                layout.bottom.collapsed = true;
                workspace::WorkspaceView::new(layout)
            },
        );
        entity.update(app.app_mut(), |view, _| {
            let sessions = ["myproject", "api", "web"]
                .iter()
                .map(|name| workspace::Session {
                    name: name.to_string(),
                    path: format!("/projects/{name}").into(),
                    ..workspace::Session::default()
                })
                .collect();
            view.set_sessions(sessions, Some(0));
            view.set_bindings(vec![(
                workspace::keymap::Action::NextWorkspaceNeedingAttention,
                "cmd-alt-a".into(),
            )]);
            view.set_agent_states(
                [
                    ("proj-101-login-google", workspace::AgentDot::ToolUse),
                    ("api-102-rate-limit", workspace::AgentDot::AwaitingInput),
                    ("investigate-0917-5fwg", workspace::AgentDot::Thinking),
                ]
                .into_iter()
                .map(|(branch, dot)| (branch.to_string(), dot))
                .collect(),
            );
            view.run_action(workspace::keymap::Action::ToggleLeftDock);
        });
        Ok(MainWindow {
            app,
            handle,
            entity,
            _scratch: scratch,
        })
    }

    pub fn frame(&mut self) -> Option<ui::Frame> {
        self.app.draw(self.handle)
    }
}
