//! A project on another machine: Connect to Host through its steps, and the page in place of the editor in each
//! state of the connection and of the host's pom. The host's answers arrive as the JSON its pom sends.

use std::time::{Duration, SystemTime};

use crate::fixtures::forms;
use crate::page::modal;
use crate::{Story, StoryDef};

/// `pom ws ls --json` on the host.
const WORKSPACES_JSON: &str = r#"{
  "schema": "pom.workspaces/1",
  "project": "myproject",
  "root": "/home/dev/pom/myproject",
  "workspaces": [
    {"branch": "main", "is_main": true, "path": "/home/dev/pom/myproject/main", "name": "main", "repos": ["api", "web"], "running": 2, "state": "running"},
    {"branch": "feat-login", "is_main": false, "path": "/home/dev/pom/myproject/workspace--feat-login", "name": "PROJ-101 Fix login", "repos": ["api", "web"], "running": 3, "state": "running"}
  ]
}"#;

#[derive(Clone, Copy)]
enum Link {
    Connected,
    OverSsh,
    Reconnecting,
    Disconnected,
    OlderPom,
    OlderPomManual,
    Updating,
    NewerPom,
}

fn snapshot(link: Link) -> anyhow::Result<pom_remote::Snapshot> {
    let workspaces: pom_remote::WorkspaceList = serde_json::from_str(WORKSPACES_JSON)?;
    let mut transport = pom_remote::TransportInfo {
        via: pom_remote::Via::Server,
        remote_version: Some(pom_remote::VERSION.to_string()),
        terminals: true,
        files: true,
        ..pom_remote::TransportInfo::default()
    };
    let connection = match link {
        Link::Reconnecting => pom_remote::Connection::Reconnecting {
            attempt: 3,
            next_in: Duration::from_secs(2),
        },
        Link::Disconnected => pom_remote::Connection::Disconnected {
            reason: "ssh: connect to host dev.example.net port 22: Operation timed out".into(),
        },
        _ => pom_remote::Connection::Connected,
    };
    match link {
        Link::OverSsh => {
            transport.via = pom_remote::Via::Ssh;
            transport.terminals = false;
            transport.fallback_reason = Some("pom server could not start on dev".into());
        }
        Link::OlderPom | Link::OlderPomManual | Link::Updating => {
            transport.version_mismatch = true;
            transport.remote_version = Some("0.1.0".into());
        }
        Link::NewerPom => {
            transport.version_mismatch = true;
            transport.host_newer = true;
            transport.remote_version = Some("99.0.0".into());
        }
        _ => {}
    }
    Ok(pom_remote::Snapshot {
        connection,
        answered_at: Some(SystemTime::now() - Duration::from_secs(4)),
        transport: Some(transport),
        branch: "feat-login".into(),
        workspaces: Some(workspaces),
        ..pom_remote::Snapshot::default()
    })
}

struct NoticeStory {
    notice: workspace::CenterNotice,
    hovered: Option<u64>,
}

impl Story for NoticeStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        ui::div()
            .col()
            .items_center()
            .w_px(width)
            .h_px(height)
            .py(ui::Space::S24)
            .bg(ui::theme().editor_background)
            .child(workspace::notice_page(
                &self.notice,
                (width - 80.0).min(760.0),
                self.hovered,
            ))
            .into()
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        let changed = self.hovered != id;
        self.hovered = id;
        changed
    }
}

fn notice(link: Link) -> anyhow::Result<Box<dyn Story>> {
    let update = remote_ui::PomUpdate {
        can_install: !matches!(link, Link::OlderPomManual),
        running: matches!(link, Link::Updating).then_some(pom_remote::update::UpdateStep::Download),
        error: None,
    };
    let (notice, _) = remote_ui::remote_notice(
        &snapshot(link)?,
        "dev (dev@dev.example.net)",
        "dev",
        "myproject",
        &update,
    );
    Ok(Box::new(NoticeStory {
        notice,
        hovered: None,
    }))
}

fn restarted() -> anyhow::Result<Box<dyn Story>> {
    crate::still(|width, height| {
        let (toast, _, _) = workspace::toast_preview(&remote_ui::restart_notice("dev"), None);
        ui::div()
            .col()
            .w_px(width)
            .h_px(height)
            .p(ui::Space::S20)
            .bg(ui::theme().editor_background)
            .child(toast)
            .into()
    })
}

const CONNECT: (f32, f32) = (500.0, 300.0);
const PAGE: (f32, f32) = (860.0, 380.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group: "Remote",
        title,
        size,
        build,
    };
    vec![
        story(
            "remote-connect-hosts",
            "Connect: pick a host",
            CONNECT,
            || modal(forms::modal("connect-hosts")),
        ),
        story(
            "remote-connect-empty",
            "Connect: no hosts yet",
            CONNECT,
            || modal(forms::modal("connect-empty")),
        ),
        story(
            "remote-connect-loading",
            "Connect: listing projects",
            CONNECT,
            || modal(forms::modal("connect-loading")),
        ),
        story(
            "remote-connect-projects",
            "Connect: pick a project",
            CONNECT,
            || modal(forms::modal("connect-projects")),
        ),
        story(
            "remote-connect-error",
            "Connect: host unreachable",
            CONNECT,
            || modal(forms::modal("connect-error")),
        ),
        story("remote-notice-connected", "Connected", PAGE, || {
            notice(Link::Connected)
        }),
        story("remote-notice-ssh", "Reading over ssh", PAGE, || {
            notice(Link::OverSsh)
        }),
        story("remote-notice-reconnecting", "Reconnecting", PAGE, || {
            notice(Link::Reconnecting)
        }),
        story("remote-notice-disconnected", "Disconnected", PAGE, || {
            notice(Link::Disconnected)
        }),
        story("remote-notice-older-pom", "Host pom older", PAGE, || {
            notice(Link::OlderPom)
        }),
        story(
            "remote-notice-older-pom-manual",
            "Host pom older, update by hand",
            PAGE,
            || notice(Link::OlderPomManual),
        ),
        story(
            "remote-notice-updating",
            "Updating the host's pom",
            PAGE,
            || notice(Link::Updating),
        ),
        story("remote-notice-newer-pom", "Host pom newer", PAGE, || {
            notice(Link::NewerPom)
        }),
        story(
            "remote-notice-restarted",
            "Server restarted",
            (560.0, 90.0),
            restarted,
        ),
    ]
}
