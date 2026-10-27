//! The main window of a project on this Mac over the editor fixture: the rail folded, the Files tree opened down
//! to the file being edited, its tabs and the agent dock.

use super::Scratch;

pub struct MainWindow {
    app: ui::Application,
    handle: ui::WindowHandle,
    // Dropped after the app, whose files view reads these files.
    _scratch: Scratch,
}

impl MainWindow {
    pub fn open((width, height): (f32, f32), scale: f32) -> anyhow::Result<Self> {
        let scratch = Scratch::new("window")?;
        let root = scratch.path().join("myproject");
        super::editor::create(&root)?;
        let controller = root.join("api/app/controllers/users_controller.rb");
        let open = [
            controller.clone(),
            root.join("api/config/routes.rb"),
            root.join("api/README.md"),
            controller.clone(),
        ];
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
                let mut files = files_ui::FilesView::local(root.clone());
                for path in &open {
                    workspace::FunctionView::open_file_at(&mut files, path, None, None);
                }
                workspace::FunctionView::reveal_in_tree(&mut files, &controller);
                let mut layout = workspace::Layout {
                    project: Some(project),
                    files_view: Some(Box::new(files)),
                    usage: super::window_usage(),
                    ..Default::default()
                };
                layout.left.width = 256.0;
                layout.right.width = 380.0;
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
            _scratch: scratch,
        })
    }

    pub fn frame(&mut self) -> Option<ui::Frame> {
        self.app.draw(self.handle)
    }
}
