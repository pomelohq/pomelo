//! The Environment and Secrets tabs over a scratch project: a service's resolved env with where each value
//! comes from, the session's secrets, and the tabs with nothing to show or no config to read.

use std::sync::Arc;

use workspace::Item;

use crate::fixtures::Scratch;
use crate::page::TabStory;
use crate::{Story, StoryDef};

const PROJECT_YML: &str = "session: myproject\npresets:\n  rails:\n    env:\n      RAILS_ENV: development\n      RAILS_LOG_TO_STDOUT: \"1\"\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    preset: [rails]\n    databases:\n      main: \"api_{{branch.safe}}\"\n    env:\n      DATABASE_URL: \"postgresql://{{shared.postgres.url}}/{{db.main}}\"\n      STRIPE_KEY: \"{{secret.STRIPE_KEY}}\"\n    services:\n      server:\n        cmd: bin/rails server\n";

const NO_SERVICES_YML: &str = "session: myproject\nrepos:\n  api: {}\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Project {
    Configured,
    NoServices,
    Unreadable,
}

fn context(
    project: Project,
    secrets: &[(&str, &str)],
) -> anyhow::Result<(environment_ui::EnvironmentContext, Scratch)> {
    let scratch = Scratch::new("env")?;
    let dir = scratch.path().to_path_buf();
    let yaml = match project {
        Project::NoServices => NO_SERVICES_YML,
        _ => PROJECT_YML,
    };
    std::fs::write(dir.join("pom.yml"), yaml)?;
    let config = Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
    let state = pom_paths::StateDir::new(dir.join("state"));
    let store = pom_secrets::SecretStore::new(state.clone(), "myproject");
    for (name, value) in secrets {
        store
            .set(name, value)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    let runner = Arc::new(pom_services::ServiceRunner::new(
        pom_services::RunnerOptions {
            project_root: dir.clone(),
            session: "myproject".into(),
            state: state.clone(),
            holders: pom_ptyhost::SocketDir::new(dir.join("s")),
            binary: "/nonexistent".into(),
            docker: "/nonexistent".into(),
        },
    ));
    let readable = project != Project::Unreadable;
    Ok((
        environment_ui::EnvironmentContext {
            state,
            runner,
            config: Arc::new(move || readable.then(|| config.clone())),
            workspaces: vec![("main".into(), true), ("feat-login".into(), false)],
            branch: "feat-login".into(),
        },
        scratch,
    ))
}

const SECRETS: &[(&str, &str)] = &[
    ("STRIPE_KEY", "sk_test_123"),
    ("SMTP_PASSWORD", "mailpit-dev"),
    ("SENTRY_DSN", "https://example.invalid/1"),
];

fn line(key: &str, value: &str, source: &str, secret: bool) -> pom_services::EnvLine {
    pom_services::EnvLine {
        connection: false,
        key: key.into(),
        value: value.into(),
        source: source.into(),
        secret,
    }
}

fn env(project: Project) -> anyhow::Result<Box<dyn Story>> {
    let (context, scratch) = context(project, SECRETS)?;
    Ok(Box::new(TabStory::keeping(
        environment_ui::EnvItem::new(context),
        scratch,
    )))
}

/// The server's env as a running workspace resolves it, one secret shown in the clear.
fn env_resolved() -> anyhow::Result<Box<dyn Story>> {
    let (context, scratch) = context(Project::Configured, SECRETS)?;
    let mut item = environment_ui::EnvItem::new(context);
    item.show(
        pom_services::ServiceExplain {
            repo: "api".into(),
            alias: "api".into(),
            service: "server".into(),
            cmd: "bin/rails server".into(),
            databases: [("main".to_string(), "myproject_api_feat-login".to_string())]
                .into_iter()
                .collect(),
            env: vec![
                line(
                    "DATABASE_URL",
                    "postgres://postgres@localhost:5432/myproject_api_feat-login",
                    "own",
                    false,
                ),
                line("RAILS_ENV", "development", "preset:rails", false),
                line("RAILS_LOG_TO_STDOUT", "1", "preset:rails", false),
                line("STRIPE_KEY", "sk_test_123", "own", true),
                line("SMTP_PASSWORD", "mailpit-dev", "own", true),
                line("PORT", "3000", "resolved", false),
            ],
        },
        Some(3000),
        &["SMTP_PASSWORD"],
    );
    Ok(Box::new(TabStory::keeping(item, scratch)))
}

fn secrets(
    listed: &'static [(&'static str, &'static str)],
    adding: bool,
) -> anyhow::Result<Box<dyn Story>> {
    let (context, scratch) = context(Project::Configured, listed)?;
    let mut item = environment_ui::SecretsItem::new(context);
    if adding {
        item.type_new("POSTHOG_KEY", "phc_example1");
        item.show_states(("SMTP_PASSWORD", "mailpit-dev"), "SENTRY_DSN");
    }
    item.set_focused(true);
    Ok(Box::new(TabStory::keeping(item, scratch)))
}

const TAB: (f32, f32) = (820.0, 280.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Environment and secrets",
        title,
        size: TAB,
        build,
    };
    vec![
        story("environment-resolved", "Env of a service", env_resolved),
        story("environment-from-config", "Env from pom.yml", || {
            env(Project::Configured)
        }),
        story("environment-no-services", "No services", || {
            env(Project::NoServices)
        }),
        story("environment-unreadable", "pom.yml unreadable", || {
            env(Project::Unreadable)
        }),
        story("secrets", "Secrets", || secrets(SECRETS, false)),
        story("secrets-adding", "Adding one", || secrets(SECRETS, true)),
        story("secrets-empty", "No secrets", || secrets(&[], false)),
    ]
}
