mod env_item;
mod secrets_item;

use std::sync::Arc;

use pom_config::Config;
use pom_paths::StateDir;
use pom_services::ServiceRunner;
use ui::{div, Node, Rect};

pub use env_item::EnvItem;
pub use secrets_item::SecretsItem;

/// What the tabs read: where secrets are stored, the project's runner (ports) and config, and its workspaces
/// as (branch, is main).
#[derive(Clone)]
pub struct EnvironmentContext {
    pub state: StateDir,
    pub runner: Arc<ServiceRunner>,
    pub config: Arc<dyn Fn() -> Option<Arc<Config>> + Send + Sync>,
    pub workspaces: Vec<(String, bool)>,
    pub branch: String,
}

impl EnvironmentContext {
    fn session(&self) -> &str {
        self.runner.session()
    }
}

fn hit_at(hits: &[(Rect, u64)], x: f32, y: f32) -> Option<u64> {
    hits.iter()
        .rev()
        .find(|(rect, _)| x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h)
        .map(|(_, id)| *id)
}

fn icon_button(id: u64, kind: ui::IconKind, color: ui::Rgba, hot: bool) -> Node {
    let mut button = div()
        .w_px(22.0)
        .h_px(20.0)
        .rounded(4.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(ui::icon(kind).size(12.0).color(color));
    if hot {
        button = button.bg(ui::theme().ghost_element_hover);
    }
    button.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace::Item;

    pub(crate) struct Project {
        pub context: EnvironmentContext,
        _dir: tempfile::TempDir,
    }

    pub(crate) fn project() -> Project {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().to_path_buf();
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\npresets:\n  rails:\n    env:\n      RAILS_ENV: development\nenvironments:\n  staging:\n    api.web: https://staging.example.com\nrepos:\n  api:\n    preset: [rails]\n    databases:\n      main: \"api_{{branch.safe}}\"\n    env:\n      TOKEN: \"{{secret.TOKEN}}\"\n    services:\n      web:\n        cmd: rails s\n",
        )
        .expect("pom.yml");
        let config = Arc::new(Config::load(&root.join("pom.yml")).expect("config"));
        let state = StateDir::new(root.join("state"));
        let runner = Arc::new(ServiceRunner::new(pom_services::RunnerOptions {
            project_root: root.clone(),
            session: "demo".into(),
            state: state.clone(),
            holders: pom_ptyhost::SocketDir::new(root.join("s")),
            binary: "/nonexistent".into(),
            docker: "/nonexistent".into(),
        }));
        Project {
            context: EnvironmentContext {
                state,
                runner,
                config: Arc::new(move || Some(config.clone())),
                workspaces: vec![("main".into(), true), ("feat-a".into(), false)],
                branch: "main".into(),
            },
            _dir: dir,
        }
    }

    #[test]
    fn secrets_are_added_listed_and_deleted_after_confirming() {
        let project = project();
        let mut secrets = SecretsItem::new(project.context.clone());
        assert!(secrets.names().is_empty());
        secrets.type_new("TOKEN", "t0ps3cret");
        secrets.add();
        assert_eq!(secrets.names(), ["TOKEN"]);
        let body = ui::Rect::new(0.0, 0.0, 600.0, 400.0, ui::Rgba::TRANSPARENT);
        let painted = secrets.paint_body(body, true).expect("painted");
        let texts: String = painted.texts.iter().map(|text| text.text.clone()).collect();
        assert!(texts.contains("{{secret.TOKEN}}"), "{texts}");
        assert!(!texts.contains("t0ps3cret"), "values stay hidden");

        secrets.type_new("bad name", "x");
        secrets.add();
        assert_eq!(secrets.names(), ["TOKEN"]);

        let delete = 100 + 2;
        let press = |secrets: &mut SecretsItem, id: u64| {
            let body = ui::Rect::new(0.0, 0.0, 600.0, 400.0, ui::Rgba::TRANSPARENT);
            let painted = secrets.paint_body(body, true).expect("painted");
            let (rect, _) = painted
                .hits
                .iter()
                .find(|(_, hit)| *hit == id)
                .copied()
                .expect("button");
            secrets.pointer_down(
                rect.x + 1.0,
                rect.y + 1.0,
                1,
                terminal::Modifiers::default(),
            );
        };
        press(&mut secrets, delete);
        assert_eq!(secrets.names(), ["TOKEN"], "the first press only asks");
        press(&mut secrets, delete);
        assert!(secrets.names().is_empty());
    }

    #[test]
    fn the_environment_tab_explains_the_picked_service() {
        let project = project();
        pom_secrets::SecretStore::new(project.context.state.clone(), "demo")
            .set("TOKEN", "abc")
            .expect("secret");
        let mut env = EnvItem::new(project.context.clone());
        let explained = env.explained().cloned().expect("explained");
        assert_eq!(
            (explained.repo.as_str(), explained.service.as_str()),
            ("api", "web")
        );
        assert_eq!(
            explained.databases.get("main").map(String::as_str),
            Some("demo_api_main")
        );
        let token = explained
            .env
            .iter()
            .find(|line| line.key == "TOKEN")
            .expect("TOKEN");
        assert!(token.secret);
        let body = ui::Rect::new(0.0, 0.0, 800.0, 500.0, ui::Rgba::TRANSPARENT);
        let painted = env.paint_body(body, true).expect("painted");
        let texts: String = painted
            .texts
            .iter()
            .map(|text| text.text.clone())
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            texts.contains("RAILS_ENV") && texts.contains("rails"),
            "{texts}"
        );
        assert!(!texts.contains("abc"), "secret values are masked: {texts}");
        let (rect, _) = painted
            .hits
            .iter()
            .find(|(_, id)| *id == 3)
            .copied()
            .expect("branch picker");
        env.pointer_down(
            rect.x + 1.0,
            rect.y + 1.0,
            1,
            terminal::Modifiers::default(),
        );
        assert_eq!(
            env.explained()
                .and_then(|explain| explain.databases.get("main").cloned())
                .as_deref(),
            Some("demo_api_feat-a")
        );
    }
}
